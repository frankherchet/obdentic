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

    /** Fails closed when the folder differs from what the app left; the first sight of a folder is adopted. */
    public static synchronized void verify(Context context, Uri tree) throws IOException {
        JSONObject now = snapshot(context.getContentResolver(), tree);
        if (!tree.toString().equals(expectedTree) || expected == null) { expected = now; expectedTree = tree.toString(); return; }
        if (!unchanged(expected, now)) throw new IOException("Protokollordner wurde von außen verändert");
    }

    /** After the app's own change (create, append, delete) the new state is the expected one. */
    public static synchronized void remember(Context context, Uri tree) throws IOException {
        expected = snapshot(context.getContentResolver(), tree);
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
                    files.put(c.getString(1), new JSONObject().put("id", c.getString(0))
                        .put("size", c.isNull(2) ? -1 : c.getLong(2)).put("modified", c.isNull(3) ? -1 : c.getLong(3)));
                }
            }
        } catch (JSONException | SecurityException | IllegalArgumentException | UnsupportedOperationException e) {
            throw new IOException("Protokollordner nicht verfügbar");
        }
        return files;
    }
}
