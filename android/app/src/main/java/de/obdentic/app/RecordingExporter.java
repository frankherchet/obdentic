package de.obdentic.app;

import android.content.ClipData;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.provider.DocumentsContract;
import java.io.*;
import java.util.*;

/** REQ-018: copies completed internal recordings into a user-chosen folder or hands them to the share sheet. Never modifies the originals. */
public final class RecordingExporter {
    public static final String MIME = "application/x-ndjson", AUTHORITY = "de.obdentic.app.files";
    private static final String PREFS = "export", FOLDER = "folder", AUTO = "auto";

    /** Export destination; the folder picker's tree URI in the app, a plain directory in tests. */
    public interface Target {
        /** Existing file names with sizes; throws {@link Unavailable} when the folder is gone or the permission was revoked. */
        Map<String, Long> list() throws IOException;
        OutputStream create(String name) throws IOException;
        /** Removes a file created by {@link #create}; used only for incomplete copies. */
        void delete(String name) throws IOException;
    }
    public static final class Unavailable extends IOException { public Unavailable(String message) { super(message); } }

    public static final class Result {
        public int copied, skipped, failed, conflicts;
        public boolean unavailable;
        public String message() {
            return unavailable ? UNAVAILABLE : "Export: " + copied + " kopiert, " + skipped + " übersprungen, " + failed + " fehlgeschlagen"
                + (conflicts > 0 ? " · " + conflicts + " Datei(en) im Ordner mit abweichender Größe nicht überschrieben" : "");
        }
    }
    public static final String UNAVAILABLE = "Export-Ordner nicht verfügbar · bitte neu wählen";

    /** REQ-018.2/.6/.7: copies each file unless the target already has its name and size; a different size under the same name is a failure, never an overwrite. */
    public static Result export(List<File> files, Target target) {
        Result result = new Result();
        Map<String, Long> present;
        try { present = target.list(); } catch (Unavailable e) { result.unavailable = true; return result; }
        catch (IOException e) { result.failed = files.size(); return result; }
        for (File file : files) {
            String name = file.getName(); Long existing = present.get(name);
            if (existing != null) {
                if (existing == file.length()) result.skipped++; else { result.failed++; result.conflicts++; }
                continue;
            }
            try { copy(file, target, name); result.copied++; }
            catch (Unavailable e) { discard(target, name); result.unavailable = true; return result; }
            catch (IOException | RuntimeException e) { discard(target, name); result.failed++; }
        }
        return result;
    }
    private static void copy(File file, Target target, String name) throws IOException {
        long size = file.length(), total = 0;
        try (InputStream input = new FileInputStream(file); OutputStream output = target.create(name)) {
            byte[] buffer = new byte[8192]; int count;
            while ((count = input.read(buffer)) != -1) { output.write(buffer, 0, count); total += count; }
            output.flush();
        }
        if (total != size) throw new IOException("Datei während des Exports verändert");
    }
    private static void discard(Target target, String name) { try { target.delete(name); } catch (IOException | RuntimeException ignored) { /* nothing more to clean */ } }

    /** Names of completed recordings: every listed file except the one still being written (REQ-018.3). */
    public static List<String> completed(org.json.JSONArray records, String active) {
        List<String> names = new ArrayList<>();
        for (int i = 0; i < records.length(); i++) { String name = records.optJSONObject(i).optString("name"); if (!name.equals(active)) names.add(name); }
        return names;
    }

    public static Uri uri(String name) { return new Uri.Builder().scheme("content").authority(AUTHORITY).appendPath("recordings").appendPath(name).build(); }

    /** REQ-018.5: one share intent for all given recordings, read access only through the existing provider. */
    public static Intent share(List<String> names) {
        ArrayList<Uri> uris = new ArrayList<>(); ClipData clip = null;
        for (String name : names) {
            Uri uri = uri(name); uris.add(uri);
            if (clip == null) clip = ClipData.newRawUri(name, uri); else clip.addItem(new ClipData.Item(uri));
        }
        Intent intent = new Intent(Intent.ACTION_SEND_MULTIPLE).setType(MIME).putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        if (clip != null) intent.setClipData(clip);
        return intent;
    }

    // REQ-018.1/.4: app-private settings. commit() so the value is on disk before the process can be killed.
    public static String folder(Context context) { return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(FOLDER, ""); }
    public static void setFolder(Context context, String uri) { context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(FOLDER, uri).commit(); }
    public static boolean auto(Context context) { return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(AUTO, false); }
    public static void setAuto(Context context, boolean on) { context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(AUTO, on).commit(); }

    /** Target for the persisted folder-picker tree; {@link Unavailable} when the permission is gone. */
    public static Target tree(Context context, String stored) throws Unavailable {
        Uri tree = Uri.parse(stored); boolean granted = false;
        for (android.content.UriPermission p : context.getContentResolver().getPersistedUriPermissions())
            if (p.getUri().equals(tree) && p.isReadPermission() && p.isWritePermission()) granted = true;
        if (!granted) throw new Unavailable("Berechtigung widerrufen");
        return new TreeTarget(context.getContentResolver(), tree);
    }

    private static final class TreeTarget implements Target {
        private final ContentResolver resolver; private final Uri tree, parent; private final Map<String, Uri> created = new HashMap<>();
        TreeTarget(ContentResolver resolver, Uri tree) {
            this.resolver = resolver; this.tree = tree; parent = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
        }
        public Map<String, Long> list() throws IOException {
            Map<String, Long> names = new HashMap<>();
            try {
                try (Cursor root = resolver.query(parent, new String[]{DocumentsContract.Document.COLUMN_DOCUMENT_ID}, null, null, null)) {
                    if (root == null || !root.moveToFirst()) throw new Unavailable("Ordner fehlt");
                }
                Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
                try (Cursor cursor = resolver.query(children, new String[]{DocumentsContract.Document.COLUMN_DISPLAY_NAME, DocumentsContract.Document.COLUMN_SIZE}, null, null, null)) {
                    if (cursor == null) throw new Unavailable("Ordner nicht lesbar");
                    while (cursor.moveToNext()) names.put(cursor.getString(0), cursor.isNull(1) ? -1L : cursor.getLong(1));
                }
            } catch (SecurityException | IllegalArgumentException | UnsupportedOperationException e) { throw new Unavailable(String.valueOf(e.getMessage())); }
            return names;
        }
        public OutputStream create(String name) throws IOException {
            try {
                Uri document = DocumentsContract.createDocument(resolver, parent, MIME, name);
                if (document == null) throw new IOException("Datei konnte nicht angelegt werden");
                created.put(name, document);
                OutputStream output = resolver.openOutputStream(document, "w");
                if (output == null) throw new IOException("Dateizugriff nicht verfügbar");
                return output;
            } catch (SecurityException | FileNotFoundException e) { throw new Unavailable(String.valueOf(e.getMessage())); }
        }
        public void delete(String name) throws IOException {
            Uri document = created.remove(name);
            if (document != null) try { DocumentsContract.deleteDocument(resolver, document); } catch (FileNotFoundException ignored) { /* already gone */ }
        }
    }
}
