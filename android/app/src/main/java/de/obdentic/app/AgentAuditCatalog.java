package de.obdentic.app;

import android.content.ContentResolver;
import android.content.Context;
import android.database.Cursor;
import android.net.Uri;
import android.provider.DocumentsContract;
import java.io.IOException;
import java.util.Iterator;
import org.json.JSONException;
import org.json.JSONObject;

/**
 * REQ-045.26/.30: the guarantees hold for a folder nobody but OBDentic changes. The catalog remembers what the app left behind
 * (every file with its document id, size and modification time) and fails closed on any later difference: an unknown file, a
 * missing file, another size or another document identity. A change racing the check itself remains a residual risk.
 */
public final class AgentAuditCatalog {
    private static JSONObject expected;
    private static String expectedTree = "";

    private AgentAuditCatalog() {}

    /** Both catalogs hold the same files with the same fields. */
    public static boolean unchanged(JSONObject expectedCatalog, JSONObject actualCatalog) {
        if (expectedCatalog == null || actualCatalog == null || expectedCatalog.length() != actualCatalog.length()) return false;
        for (Iterator<String> names = expectedCatalog.keys(); names.hasNext(); ) {
            String name = names.next();
            JSONObject want = expectedCatalog.optJSONObject(name), have = actualCatalog.optJSONObject(name);
            if (want == null || have == null || want.length() != have.length()) return false;
            for (Iterator<String> fields = want.keys(); fields.hasNext(); ) {
                String field = fields.next();
                if (!have.has(field) || !String.valueOf(want.opt(field)).equals(String.valueOf(have.opt(field)))) return false;
            }
        }
        return true;
    }

    /** Restore the durable catalog before comparing the folder, including on the first operation in a new process. */
    public static synchronized void verify(Context context, Uri tree) throws IOException {
        JSONObject now = snapshot(context.getContentResolver(), tree);
        if (!tree.toString().equals(expectedTree) || expected == null) {
            String saved = context.getSharedPreferences("agent-audit-catalog", Context.MODE_PRIVATE).getString(tree.toString(), null);
            if (saved == null) {
                if (now.length() != 0) throw new IOException("Unbekannter Inhalt im Protokollordner");
                remember(context, tree);
            } else {
                try { expected = new JSONObject(saved); expectedTree = tree.toString(); }
                catch (JSONException e) { throw new IOException("Protokollkatalog nicht lesbar"); }
            }
        }
        String pending = context.getSharedPreferences("agent-audit-catalog", Context.MODE_PRIVATE).getString(tree + ":pending", null);
        if (pending != null) {
            try {
                JSONObject operation = new JSONObject(pending);
                String name = operation.getString("name"), kind = operation.getString("kind");
                JSONObject before = expected.optJSONObject(name), after = now.optJSONObject(name);
                JSONObject remainingExpected = new JSONObject(expected.toString()), remainingNow = new JSONObject(now.toString());
                remainingExpected.remove(name); remainingNow.remove(name);
                boolean rest = unchanged(remainingExpected, remainingNow);
                boolean accepted = false;
                if ("append".equals(kind) && before != null && after != null
                        && operation.getString("id").equals(before.optString("id"))
                        && operation.getString("id").equals(after.optString("id"))) {
                    long actual = after.optLong("size", -1);
                    accepted = actual == operation.getLong("after")
                        || (actual == operation.getLong("before") && unchanged(new JSONObject().put(name, before), new JSONObject().put(name, after)));
                } else if ("create".equals(kind) && before == null) {
                    accepted = after == null || after.optLong("size", -1) == 0;
                } else if ("delete".equals(kind) && before != null) {
                    accepted = after == null || unchanged(new JSONObject().put(name, before), new JSONObject().put(name, after));
                }
                if (!rest || !accepted) throw new IOException("Protokollordner wurde von außen verändert");
                remember(context, tree);
                return;
            } catch (JSONException e) { throw new IOException("Protokollkatalog nicht lesbar"); }
        }
        if (!unchanged(expected, now)) throw new IOException("Protokollordner wurde von außen verändert");
    }

    /** After the app's own change (create, append, delete) the new state is the expected one. */
    public static synchronized void remember(Context context, Uri tree) throws IOException {
        JSONObject next = snapshot(context.getContentResolver(), tree);
        if (!context.getSharedPreferences("agent-audit-catalog", Context.MODE_PRIVATE).edit().putString(tree.toString(), next.toString()).remove(tree + ":pending").commit())
            throw new IOException("Protokollkatalog konnte nicht gespeichert werden");
        expected = next;
        expectedTree = tree.toString();
    }

    /** Persist an append intent before touching the file; recovery accepts only the old or complete new size. */
    public static synchronized void prepareAppend(Context context, Uri tree, Uri document, long before, long after) throws IOException {
        verify(context, tree);
        String id = DocumentsContract.getDocumentId(document), name = find(id);
        JSONObject entry = expected.optJSONObject(name);
        if (entry == null || entry.optLong("size", -1) != before || after <= before) throw new IOException("Protokollgröße geändert");
        try { pending(context, tree, new JSONObject().put("kind", "append").put("name", name).put("id", id).put("before", before).put("after", after)); }
        catch (JSONException e) { throw new IOException("Protokollkatalog nicht schreibbar"); }
    }

    public static synchronized void prepareCreate(Context context, Uri tree, String name) throws IOException {
        verify(context, tree);
        if (expected.has(name)) throw new IOException("Datei existiert bereits");
        try { pending(context, tree, new JSONObject().put("kind", "create").put("name", name)); }
        catch (JSONException e) { throw new IOException("Protokollkatalog nicht schreibbar"); }
    }

    public static synchronized void prepareDelete(Context context, Uri tree, Uri document) throws IOException {
        verify(context, tree);
        String name = find(DocumentsContract.getDocumentId(document));
        try { pending(context, tree, new JSONObject().put("kind", "delete").put("name", name)); }
        catch (JSONException e) { throw new IOException("Protokollkatalog nicht schreibbar"); }
    }

    private static String find(String id) throws IOException {
        for (Iterator<String> names = expected.keys(); names.hasNext();) {
            String name = names.next();
            if (id.equals(expected.optJSONObject(name).optString("id"))) return name;
        }
        throw new IOException("Protokolldatei nicht im Katalog");
    }

    private static void pending(Context context, Uri tree, JSONObject operation) throws IOException {
        if (!context.getSharedPreferences("agent-audit-catalog", Context.MODE_PRIVATE).edit().putString(tree + ":pending", operation.toString()).commit())
            throw new IOException("Protokollkatalog konnte nicht gespeichert werden");
    }

    /** Explicit user confirmation adopts the current folder without expiring any existing file. */
    public static synchronized void adopt(Context context, Uri tree) throws IOException {
        JSONObject now = snapshot(context.getContentResolver(), tree);
        if (!context.getSharedPreferences("agent-audit-catalog", Context.MODE_PRIVATE).edit()
                .putString(tree.toString(), now.toString()).putString(tree + ":protected", now.names() == null ? "[]" : now.names().toString())
                .remove(tree + ":pending").commit()) throw new IOException("Protokollkatalog konnte nicht gespeichert werden");
        expected = now; expectedTree = tree.toString();
    }

    public static synchronized boolean protectedFile(Context context, Uri tree, String name) throws IOException {
        String saved = context.getSharedPreferences("agent-audit-catalog", Context.MODE_PRIVATE).getString(tree + ":protected", "[]");
        try {
            org.json.JSONArray names = new org.json.JSONArray(saved);
            for (int i = 0; i < names.length(); i++) if (name.equals(names.getString(i))) return true;
            return false;
        } catch (JSONException e) { throw new IOException("Protokollkatalog nicht lesbar"); }
    }

    private static JSONObject snapshot(ContentResolver resolver, Uri tree) throws IOException {
        JSONObject files = new JSONObject();
        try {
            Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
            try (Cursor c = resolver.query(children, new String[]{DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                    DocumentsContract.Document.COLUMN_DISPLAY_NAME, DocumentsContract.Document.COLUMN_SIZE,
                    DocumentsContract.Document.COLUMN_LAST_MODIFIED}, null, null, null)) {
                if (c == null) throw new IOException("Protokollordner nicht lesbar");
                while (c.moveToNext()) {
                    String name = c.getString(1);
                    if (name == null || c.isNull(0) || c.isNull(2) || c.isNull(3) || files.has(name) || c.getLong(2) < 0)
                        throw new IOException("Protokollinhalt nicht eindeutig");
                    files.put(name, new JSONObject().put("id", c.getString(0))
                        .put("size", c.getLong(2)).put("modified", c.getLong(3)));
                }
            }
        } catch (JSONException | SecurityException | IllegalArgumentException | UnsupportedOperationException e) {
            throw new IOException("Protokollordner nicht verfügbar");
        }
        return files;
    }
}
