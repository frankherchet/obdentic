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
    public static JSONObject preview(Context context, String sessionId, JSONObject known, JSONArray categories) throws IOException {
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
                    return AgentAuditPrivacy.redact(text, known, categories).put("session_id", sessionId).put("warning", WARNING);
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
