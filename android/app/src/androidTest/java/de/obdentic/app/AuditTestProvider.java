package de.obdentic.app;

import android.database.Cursor;
import android.database.MatrixCursor;
import android.os.CancellationSignal;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract.Document;
import android.provider.DocumentsContract.Root;
import android.provider.DocumentsProvider;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;

/**
 * A file-backed document provider of the test APK that stands in for the system folder picker's local storage: the acceptance run
 * creates its tree and grants it to the app, so no manual folder choice is needed. Synthetic data only, in the test app's cache.
 */
public final class AuditTestProvider extends DocumentsProvider {
    public static final String AUTHORITY = "de.obdentic.app.debug.test.audit";
    public static final String ROOT_ID = "primary:audit";
    private File root;

    @Override public boolean onCreate() {
        root = new File(getContext().getCacheDir(), "audit-provider");
        return root.isDirectory() || root.mkdirs();
    }

    private File file(String documentId) throws FileNotFoundException {
        if (!documentId.equals(ROOT_ID) && !documentId.startsWith(ROOT_ID + "/")) throw new FileNotFoundException(documentId);
        File f = new File(root, documentId.substring(ROOT_ID.length()));
        if (!f.exists() && !documentId.equals(ROOT_ID)) throw new FileNotFoundException(documentId);
        return f;
    }

    private static String id(File f, File base) {
        String rel = f.getAbsolutePath().substring(base.getAbsolutePath().length());
        return ROOT_ID + rel;
    }

    private void row(MatrixCursor c, File f) {
        boolean dir = f.isDirectory();
        c.newRow().add(Document.COLUMN_DOCUMENT_ID, id(f, root)).add(Document.COLUMN_DISPLAY_NAME, dir && f.equals(root) ? "audit" : f.getName())
            .add(Document.COLUMN_MIME_TYPE, dir ? Document.MIME_TYPE_DIR : "application/x-ndjson").add(Document.COLUMN_SIZE, f.length())
            .add(Document.COLUMN_LAST_MODIFIED, f.lastModified())
            .add(Document.COLUMN_FLAGS, dir ? Document.FLAG_DIR_SUPPORTS_CREATE : Document.FLAG_SUPPORTS_WRITE | Document.FLAG_SUPPORTS_DELETE);
    }

    private static final String[] DOCUMENT = {Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE,
        Document.COLUMN_SIZE, Document.COLUMN_LAST_MODIFIED, Document.COLUMN_FLAGS};

    @Override public Cursor queryRoots(String[] projection) {
        return new MatrixCursor(projection != null ? projection : new String[]{Root.COLUMN_ROOT_ID});
    }

    @Override public Cursor queryDocument(String documentId, String[] projection) throws FileNotFoundException {
        MatrixCursor c = new MatrixCursor(projection != null ? projection : DOCUMENT);
        row(c, file(documentId));
        return c;
    }

    @Override public Cursor queryChildDocuments(String parentId, String[] projection, String sortOrder) throws FileNotFoundException {
        MatrixCursor c = new MatrixCursor(projection != null ? projection : DOCUMENT);
        File[] children = file(parentId).listFiles();
        if (children != null) for (File child : children) row(c, child);
        return c;
    }

    @Override public ParcelFileDescriptor openDocument(String documentId, String mode, CancellationSignal signal) throws FileNotFoundException {
        return ParcelFileDescriptor.open(file(documentId), ParcelFileDescriptor.parseMode(mode));
    }

    @Override public String createDocument(String parentId, String mimeType, String displayName) throws FileNotFoundException {
        File f = new File(file(parentId), displayName);
        try {
            if (!f.createNewFile()) throw new FileNotFoundException("exists " + displayName);
        } catch (IOException e) { throw new FileNotFoundException(e.getMessage()); }
        return id(f, root);
    }

    @Override public void deleteDocument(String documentId) throws FileNotFoundException {
        if (!file(documentId).delete()) throw new FileNotFoundException(documentId);
    }

    @Override public boolean isChildDocument(String parentDocumentId, String documentId) {
        return documentId.equals(parentDocumentId) || documentId.startsWith(parentDocumentId + "/");
    }
}
