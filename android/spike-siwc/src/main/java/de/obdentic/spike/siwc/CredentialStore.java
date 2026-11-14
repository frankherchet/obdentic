package de.obdentic.spike.siwc;

import android.content.Context;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.GeneralSecurityException;
import java.security.KeyStore;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;
import org.json.JSONException;
import org.json.JSONObject;

/** Credential record encrypted with an AndroidKeyStore AES-256-GCM key; file = IV || ciphertext. */
final class CredentialStore {
    private static final String ALIAS = "siwc-spike";
    private static final String TRANSFORM = "AES/GCM/NoPadding";
    private static final int IV_LEN = 12;

    private CredentialStore() {}

    private static SecretKey key() throws GeneralSecurityException, IOException {
        KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
        ks.load(null);
        if (!ks.containsAlias(ALIAS)) {
            KeyGenerator kg = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
            kg.init(new KeyGenParameterSpec.Builder(
                    ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256)
                    .build());
            kg.generateKey();
        }
        return (SecretKey) ks.getKey(ALIAS, null);
    }

    private static File dir(Context ctx) {
        File d = new File(ctx.getFilesDir(), "siwc");
        if (!d.isDirectory()) d.mkdirs();
        return d;
    }

    static void save(Context ctx, JSONObject record) throws GeneralSecurityException, IOException {
        Cipher c = Cipher.getInstance(TRANSFORM);
        c.init(Cipher.ENCRYPT_MODE, key());
        byte[] iv = c.getIV();
        byte[] ct = c.doFinal(record.toString().getBytes(StandardCharsets.UTF_8));
        File dir = dir(ctx);
        File tmp = new File(dir, "credential.bin.tmp");
        try (FileOutputStream out = new FileOutputStream(tmp)) {
            out.write(iv);
            out.write(ct);
            out.getFD().sync();
        }
        File dst = new File(dir, "credential.bin");
        if (!tmp.renameTo(dst)) throw new IOException("rename_failed");
    }

    /** Null when no file exists; throws when the file cannot be decrypted or parsed. */
    static JSONObject load(Context ctx) throws GeneralSecurityException, IOException, JSONException {
        File f = new File(dir(ctx), "credential.bin");
        if (!f.isFile()) return null;
        byte[] all = Files.readAllBytes(f.toPath());
        if (all.length <= IV_LEN) throw new GeneralSecurityException("short_file");
        Cipher c = Cipher.getInstance(TRANSFORM);
        c.init(Cipher.DECRYPT_MODE, key(), new GCMParameterSpec(128, all, 0, IV_LEN));
        byte[] pt = c.doFinal(all, IV_LEN, all.length - IV_LEN);
        return new JSONObject(new String(pt, StandardCharsets.UTF_8));
    }
}
