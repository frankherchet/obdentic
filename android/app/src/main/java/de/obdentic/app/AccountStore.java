package de.obdentic.app;

import android.content.Context;
import android.content.SharedPreferences;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.security.GeneralSecurityException;
import java.security.KeyStore;
import java.util.UUID;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;
import org.json.JSONException;
import org.json.JSONObject;

/**
 * REQ-043.1: the credential record as one app-private file, AES-256-GCM encrypted with a key that never leaves the Android Keystore
 * (fresh IV per write, atomic replace), plus the two values that outlive a sign-out: the random host id and the issued client id.
 * App backup and device transfer exclude all of it (manifest and data extraction rules).
 */
class AccountStore {
    private static final String TRANSFORM = "AES/GCM/NoPadding";
    private static final int IV_LEN = 12, FORMAT = 1;
    private static final String RECORD = "credential.bin", TEMP = "credential.bin.tmp";

    /** The decrypted record. Tokens and account identity live here and nowhere else. */
    static final class Record {
        final String clientId, accessToken, refreshToken, idToken, scope, email, subject;
        final long expiresAtMs, earliestRefreshMs;
        final boolean rotationPending;

        Record(String clientId, String accessToken, String refreshToken, String idToken, String scope, String email, String subject,
               long expiresAtMs, long earliestRefreshMs, boolean rotationPending) {
            this.clientId = clientId;
            this.accessToken = accessToken;
            this.refreshToken = refreshToken;
            this.idToken = idToken;
            this.scope = scope;
            this.email = email;
            this.subject = subject;
            this.expiresAtMs = expiresAtMs;
            this.earliestRefreshMs = earliestRefreshMs;
            this.rotationPending = rotationPending;
        }

        /** REQ-042.9: the permission to use the ChatGPT plan is part of the granted scope. */
        boolean hasPlan() { return hasPlan(scope); }

        static boolean hasPlan(String scope) {
            if (scope == null) return false;
            for (String s : scope.split("\\s+")) if (s.equals(ChatGptAccount.PLAN_SCOPE)) return true;
            return false;
        }

        Record refreshed(String access, String refresh, String newScope, long expiresAt, long earliest) {
            return new Record(clientId, access, refresh, idToken, newScope, email, subject, expiresAt, earliest, false);
        }

        Record pendingRotation() {
            return new Record(clientId, accessToken, refreshToken, idToken, scope, email, subject, expiresAtMs, earliestRefreshMs, true);
        }

        JSONObject toJson() throws JSONException {
            return new JSONObject().put("v", FORMAT).put("client_id", clientId).put("access_token", accessToken).put("refresh_token", refreshToken)
                .put("id_token", idToken).put("scope", scope).put("email", email).put("subject", subject)
                .put("expires_at", expiresAtMs).put("earliest_refresh", earliestRefreshMs).put("rotation_pending", rotationPending);
        }

        static Record fromJson(JSONObject o) throws JSONException {
            if (o.getInt("v") != FORMAT) throw new JSONException("version");
            String access = o.getString("access_token"), refresh = o.getString("refresh_token");
            if (access.isEmpty() || refresh.isEmpty()) throw new JSONException("empty");
            return new Record(o.getString("client_id"), access, refresh, o.getString("id_token"), o.getString("scope"), o.getString("email"),
                o.getString("subject"), o.getLong("expires_at"), o.getLong("earliest_refresh"), o.optBoolean("rotation_pending", false));
        }
    }

    private final File dir;
    private final String alias;
    private final SharedPreferences prefs, ui;

    /** {@code name} separates independent stores: "account" in the app, a unique name in the acceptance run. */
    AccountStore(Context context, String name) {
        dir = new File(context.getFilesDir(), name);
        alias = "obdentic-" + name;
        prefs = context.getSharedPreferences(name, Context.MODE_PRIVATE);
        ui = context.getSharedPreferences(name + "-ui", Context.MODE_PRIVATE);
    }

    File directory() { return dir; }

    // ---------------------------------------------------------------- values kept after sign-out (REQ-043.12)

    /** A random id created once and reused for every sign-in (REQ-042.2). */
    synchronized String hostId() {
        String id = prefs.getString("host_id", null);
        if (id == null) {
            id = "urn:uuid:" + UUID.randomUUID();
            prefs.edit().putString("host_id", id).commit();
        }
        return id;
    }

    /** The client id issued by the first registration, or null. */
    synchronized String clientId() { return prefs.getString("client_id", null); }

    synchronized void saveClientId(String clientId) { prefs.edit().putString("client_id", clientId).commit(); }

    synchronized void deleteClientId() { prefs.edit().remove("client_id").commit(); }

    /** The one-time dialog of REQ-042.11: "" never due, "due" until confirmed, "done" afterwards. Kept outside the account values. */
    synchronized String firstUse() { return ui.getString("first_use", ""); }

    synchronized void setFirstUse(String value) { ui.edit().putString("first_use", value).commit(); }

    // ---------------------------------------------------------------- the encrypted record

    private SecretKey key() throws GeneralSecurityException, IOException {
        KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
        ks.load(null);
        if (!ks.containsAlias(alias)) {
            KeyGenerator kg = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
            kg.init(new KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).setKeySize(256).build());
            kg.generateKey();
        }
        SecretKey key = (SecretKey) ks.getKey(alias, null);
        if (key == null) throw new GeneralSecurityException("key_missing");
        return key;
    }

    /** Writes the record: temporary file, sync, atomic rename. A failure leaves the previous record (or none) and no temporary file. */
    synchronized void save(Record record) throws GeneralSecurityException, IOException, JSONException {
        Cipher c = Cipher.getInstance(TRANSFORM);
        c.init(Cipher.ENCRYPT_MODE, key()); // the Keystore picks a fresh random IV for every encryption
        byte[] iv = c.getIV();
        byte[] ct = c.doFinal(record.toJson().toString().getBytes(StandardCharsets.UTF_8));
        if (!dir.isDirectory() && !dir.mkdirs()) throw new IOException("no_directory");
        File tmp = new File(dir, TEMP);
        try {
            try (FileOutputStream out = new FileOutputStream(tmp)) {
                out.write(FORMAT);
                out.write(iv);
                out.write(ct);
                out.getFD().sync();
            }
            Files.move(tmp.toPath(), new File(dir, RECORD).toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING);
        } catch (IOException | RuntimeException e) {
            tmp.delete(); // a failed write leaves no temporary file with ciphertext behind
            throw e;
        }
    }

    /** Null when no record exists; throws when the file cannot be decrypted or parsed (REQ-043.3). */
    synchronized Record load() throws GeneralSecurityException, IOException, JSONException {
        File f = new File(dir, RECORD);
        if (!f.isFile()) return null;
        byte[] all = Files.readAllBytes(f.toPath());
        if (all.length <= 1 + IV_LEN || all[0] != FORMAT) throw new GeneralSecurityException("format");
        Cipher c = Cipher.getInstance(TRANSFORM);
        c.init(Cipher.DECRYPT_MODE, key(), new GCMParameterSpec(128, all, 1, IV_LEN));
        return Record.fromJson(new JSONObject(new String(c.doFinal(all, 1 + IV_LEN, all.length - 1 - IV_LEN), StandardCharsets.UTF_8)));
    }

    /** Deletes the record file and the Keystore key; host id and client id stay (REQ-043.12). */
    synchronized void deleteRecord() {
        File[] files = dir.listFiles();
        if (files != null) for (File f : files) f.delete();
        dir.delete();
        try {
            KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
            ks.load(null);
            if (ks.containsAlias(alias)) ks.deleteEntry(alias);
        } catch (GeneralSecurityException | IOException | RuntimeException ignored) {
            // an unusable key store has nothing left to delete
        }
    }

    /** Test cleanup: everything of this store. */
    synchronized void deleteAll() {
        deleteRecord();
        prefs.edit().clear().commit();
        ui.edit().clear().commit();
    }
}
