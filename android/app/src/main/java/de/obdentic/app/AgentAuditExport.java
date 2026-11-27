package de.obdentic.app;

import android.content.Context;
import android.database.Cursor;
import android.net.Uri;
import android.provider.DocumentsContract;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

/**
 * REQ-045.24/.25: export of one explicitly selected session after a warning and a redaction preview. The source audit is only read.
 * Nothing is created before the user confirms; the redacted copy gets its own file name, never an audit name.
 */
public final class AgentAuditExport {
    public static final String WARNING = "Der Export kann Fahrzeugdaten und Gesprächsinhalte enthalten. Erkannt werden nur Werte, die die App kennt.";

    private AgentAuditExport() {}

    /** The redacted text and every match, for the confirmation screen; creates nothing. */
    public static JSONObject preview(Context context, String sessionId) throws IOException {
        Uri tree = AgentAuditStorage.selected(context);
        String wanted = "obdentic-agent-" + sessionId + ".jsonl";
        try {
            Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
            try (Cursor c = context.getContentResolver().query(children, new String[]{DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME}, null, null, null)) {
                if (c == null) throw new IOException("Protokollordner nicht lesbar");
                while (c.moveToNext()) {
                    if (!wanted.equals(c.getString(1))) continue;
                    Uri file = DocumentsContract.buildDocumentUriUsingTree(tree, c.getString(0));
                    String text = read(context, file);
                    return redactJsonl(text, knownValues(context)).put("session_id", sessionId).put("warning", WARNING);
                }
            }
        } catch (JSONException | SecurityException | IllegalArgumentException e) {
            throw new IOException("Export nicht möglich");
        }
        throw new IOException("Sitzung nicht gefunden");
    }

    /** Null and no file unless confirmed; otherwise the new redacted copy. */
    public static Uri confirm(Context context, JSONObject preview, boolean confirmed) throws IOException {
        if (!confirmed) return null;
        Uri tree = AgentAuditStorage.selected(context);
        try {
            Uri parent = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
            Uri copy = DocumentsContract.createDocument(context.getContentResolver(), parent, AgentAuditSession.MIME_TYPE,
                "obdentic-export-" + preview.getString("session_id") + ".jsonl");
            if (copy == null) throw new IOException("Export konnte nicht angelegt werden");
            try (OutputStream out = context.getContentResolver().openOutputStream(copy, "w")) {
                if (out == null) throw new IOException("Export konnte nicht geschrieben werden");
                out.write(preview.getString("text").getBytes(StandardCharsets.UTF_8));
            }
            AgentAuditCatalog.remember(context, tree);
            return copy;
        } catch (JSONException | SecurityException | IllegalArgumentException e) {
            throw new IOException("Export nicht möglich");
        }
    }

    private static JSONObject knownValues(Context context) throws IOException, JSONException {
        JSONObject known = new JSONObject();
        for (String category : new String[]{"vin", "ecu", "odometer", "address", "account"}) known.put(category, new JSONArray());
        VehicleStore store = new VehicleStore(new java.io.File(context.getFilesDir(), "vehicles.json"));
        if (store.unreadable()) throw new IOException("Fahrzeugspeicher nicht lesbar");
        JSONArray vehicles = store.snapshot().getJSONArray("vehicles");
        for (int i = 0; i < vehicles.length(); i++) {
            JSONObject vehicle = vehicles.getJSONObject(i);
            add(known.getJSONArray("vin"), vehicle.optString("vin"));
            JSONObject odometer = vehicle.optJSONObject("odometer");
            if (odometer != null && odometer.has("km")) add(known.getJSONArray("odometer"), String.valueOf(odometer.get("km")));
            JSONObject decrease = vehicle.optJSONObject("odometer_decrease");
            if (decrease != null && decrease.has("previous_km")) add(known.getJSONArray("odometer"), String.valueOf(decrease.get("previous_km")));
            JSONObject discovery = vehicle.optJSONObject("ecu_discovery");
            JSONArray targets = discovery == null ? null : discovery.optJSONArray("targets");
            for (int j = 0; targets != null && j < targets.length(); j++) {
                JSONObject ecu = targets.optJSONObject(j);
                if (ecu == null) continue;
                for (String key : new String[]{"request_id", "response_id", "target_id"}) add(known.getJSONArray("ecu"), ecu.optString(key));
                JSONArray identity = ecu.optJSONArray("identity");
                for (int k = 0; identity != null && k < identity.length(); k++) {
                    JSONObject item = identity.optJSONObject(k);
                    if (item == null || item.isNull("value")) continue;
                    JSONArray bytes = item.optJSONArray("value");
                    if (bytes == null) add(known.getJSONArray("ecu"), item.optString("value"));
                    else {
                        StringBuilder ascii = new StringBuilder(), hex = new StringBuilder();
                        boolean printable = bytes.length() > 0;
                        for (int n = 0; n < bytes.length(); n++) {
                            int b = bytes.optInt(n, -1); if (b < 0 || b > 255) { printable = false; break; }
                            if (n > 0) hex.append(' ');
                            hex.append(String.format(java.util.Locale.ROOT, "%02X", b));
                            if (b < 32 || b > 126) printable = false;
                            ascii.append((char) b);
                        }
                        add(known.getJSONArray("ecu"), hex.toString());
                        if (printable) add(known.getJSONArray("ecu"), ascii.toString());
                    }
                }
            }
        }
        add(known.getJSONArray("address"), context.getSharedPreferences("adapter", Context.MODE_PRIVATE).getString("known_address", ""));
        for (String value : ChatGptAccount.get(context).auditRedactionIdentity()) add(known.getJSONArray("account"), value);
        return known;
    }

    private static void add(JSONArray values, String value) {
        if (value == null || value.isEmpty()) return;
        for (int i = 0; i < values.length(); i++) if (value.equals(values.optString(i))) return;
        values.put(value);
    }

    private static JSONObject redactJsonl(String text, JSONObject known) throws JSONException {
        JSONArray categories = new JSONArray();
        for (String category : new String[]{"vin", "ecu", "odometer", "address", "account"}) categories.put(category);
        JSONArray matches = new JSONArray();
        StringBuilder output = new StringBuilder();
        String[] lines = text.split("\n", -1);
        for (int i = 0; i < lines.length; i++) {
            if (i == lines.length - 1 && lines[i].isEmpty()) break;
            Object item;
            try { item = new org.json.JSONTokener(lines[i]).nextValue(); }
            catch (JSONException torn) {
                JSONObject masked = AgentAuditPrivacy.redact(lines[i], known, categories);
                appendMatches(matches, masked.getJSONArray("matches"), i + 1);
                output.append(masked.getString("text"));
                if (i < lines.length - 1) output.append('\n');
                continue;
            }
            String wrapped = new JSONArray().put(redactValue(item, known, categories, matches, i + 1)).toString();
            output.append(wrapped, 1, wrapped.length() - 1);
            if (i < lines.length - 1) output.append('\n');
        }
        return new JSONObject().put("text", output.toString()).put("matches", matches).put("categories", categories);
    }

    private static Object redactValue(Object item, JSONObject known, JSONArray categories, JSONArray matches, int line) throws JSONException {
        if (item instanceof JSONObject) {
            JSONObject source = (JSONObject) item, result = new JSONObject();
            java.util.Iterator<String> keys = source.keys();
            while (keys.hasNext()) {
                String key = keys.next();
                JSONObject maskedKey = AgentAuditPrivacy.redact(key, known, categories);
                appendMatches(matches, maskedKey.getJSONArray("matches"), line);
                String redactedKey = maskedKey.getString("text");
                if (result.has(redactedKey)) throw new JSONException("redacted keys collide");
                result.put(redactedKey, redactValue(source.get(key), known, categories, matches, line));
            }
            return result;
        }
        if (item instanceof JSONArray) {
            JSONArray source = (JSONArray) item, result = new JSONArray();
            for (int i = 0; i < source.length(); i++) result.put(redactValue(source.get(i), known, categories, matches, line));
            return result;
        }
        if (item instanceof String || item instanceof Number) {
            JSONObject masked = AgentAuditPrivacy.redact(String.valueOf(item), known, categories);
            JSONArray found = masked.getJSONArray("matches");
            appendMatches(matches, found, line);
            return found.length() == 0 ? item : masked.getString("text");
        }
        return item;
    }

    private static void appendMatches(JSONArray all, JSONArray found, int line) throws JSONException {
        for (int i = 0; i < found.length(); i++) all.put(found.getJSONObject(i).put("line", line));
    }

    private static String read(Context context, Uri file) throws IOException {
        try (InputStream in = context.getContentResolver().openInputStream(file)) {
            if (in == null) throw new IOException("Sitzung nicht lesbar");
            ByteArrayOutputStream all = new ByteArrayOutputStream();
            byte[] buffer = new byte[8192];
            for (int n; (n = in.read(buffer)) != -1; ) all.write(buffer, 0, n);
            return all.toString("UTF-8");
        }
    }
}
