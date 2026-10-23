package de.obdentic.app;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import java.io.File;
import java.io.FileNotFoundException;

/** Grants one explicitly shared recording, never general app storage access. */
public final class RecordingProvider extends ContentProvider {
    @Override public boolean onCreate() { return true; }

    private File recording(Uri uri) throws FileNotFoundException {
        String name = uri.getLastPathSegment();
        if (uri.getPathSegments().size() != 2 || !"recordings".equals(uri.getPathSegments().get(0))
                || name == null || !name.matches("[A-Za-z0-9._-]+") || !name.endsWith(".jsonl")) {
            throw new FileNotFoundException("Ungültige Aufnahme");
        }
        File directory = new File(getContext().getFilesDir(), "recordings");
        File file = new File(directory, name);
        try {
            if (!file.getCanonicalFile().getParentFile().equals(directory.getCanonicalFile()) || !file.isFile()) {
                throw new FileNotFoundException("Aufnahme fehlt");
            }
        } catch (java.io.IOException e) { throw new FileNotFoundException(e.getMessage()); }
        return file;
    }

    @Override public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (!"r".equals(mode)) throw new FileNotFoundException("Nur lesender Zugriff");
        return ParcelFileDescriptor.open(recording(uri), ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override public String getType(Uri uri) { return "application/x-ndjson"; }

    @Override public Cursor query(Uri uri, String[] projection, String selection, String[] args, String sort) {
        String[] columns = projection == null
            ? new String[]{OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE} : projection;
        MatrixCursor cursor = new MatrixCursor(columns);
        try {
            File file = recording(uri);
            Object[] row = new Object[columns.length];
            for (int i = 0; i < columns.length; i++) {
                if (OpenableColumns.DISPLAY_NAME.equals(columns[i])) row[i] = file.getName();
                else if (OpenableColumns.SIZE.equals(columns[i])) row[i] = file.length();
            }
            cursor.addRow(row);
        } catch (FileNotFoundException ignored) { /* Deleted between listing and sharing. */ }
        return cursor;
    }

    @Override public Uri insert(Uri uri, ContentValues values) { throw new UnsupportedOperationException(); }
    @Override public int delete(Uri uri, String selection, String[] args) { throw new UnsupportedOperationException(); }
    @Override public int update(Uri uri, ContentValues values, String selection, String[] args) { throw new UnsupportedOperationException(); }
}
