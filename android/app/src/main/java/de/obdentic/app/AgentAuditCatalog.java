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
        if (!unchanged(expected, now)) throw new IOException("Protokollordner wurde von außen verändert");
    }

    /** After the app's own change (create, append, delete) the new state is the expected one. */
    public static synchronized void remember(Context context, Uri tree) throws IOException {
        JSONObject next = snapshot(context.getContentResolver(), tree);
        if (!context.getSharedPreferences("agent-audit-catalog", Context.MODE_PRIVATE).edit().putString(tree.toString(), next.toString()).commit())
            throw new IOException("Protokollkatalog konnte nicht gespeichert werden");
        expected = next;
        expectedTree = tree.toString();
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
