package de.obdentic.app;

import android.content.Context;
import android.content.UriPermission;
import android.net.Uri;
import android.provider.DocumentsContract;
import java.io.IOException;

/** User-selected shared internal storage for agent audits; never falls back to another directory. */
public final class AgentAuditStorage {
    private static final String PREFS = "agent-audit", FOLDER = "folder";
    private static final String INTERNAL_PROVIDER = "com.android.externalstorage.documents";
    private static volatile String testProvider = "";

    /** Acceptance seam: the test APK's own file-backed document provider counts as a local provider. Never set in the app. */
    public static void allowTestProvider(String authority) { testProvider = authority == null ? "" : authority; }
    private AgentAuditStorage() {}

    public static String folder(Context context) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(FOLDER, "");
    }

    /** Only the primary internal-storage tree is supported, not removable storage or cloud providers. */
    public static boolean isLocalTree(Uri tree) {
        if (tree == null || !"content".equals(tree.getScheme())
                || !(INTERNAL_PROVIDER.equals(tree.getAuthority()) || (!testProvider.isEmpty() && testProvider.equals(tree.getAuthority())))) return false;
        try {
            String id = DocumentsContract.getTreeDocumentId(tree);
            return id.startsWith("primary:") && DocumentsContract.isTreeUri(tree);
        } catch (IllegalArgumentException | UnsupportedOperationException e) { return false; }
    }

    public static void select(Context context, Uri tree) throws IOException {
        if (!isLocalTree(tree)) throw new IOException("Nur ein Ordner im internen Gerätespeicher ist erlaubt");
        requireGrant(context, tree);
        if (!context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(FOLDER, tree.toString()).commit())
            throw new IOException("Protokollordner konnte nicht gespeichert werden");
    }

    /** Restore the chosen grant before every use; permission loss is a hard failure. */
    public static Uri selected(Context context) throws IOException {
        String saved = folder(context);
        if (saved.isEmpty()) throw new IOException("Kein Protokollordner gewählt");
        Uri tree = Uri.parse(saved);
        if (!isLocalTree(tree)) throw new IOException("Protokollordner ist nicht lokal");
        requireGrant(context, tree);
        return tree;
    }

    private static void requireGrant(Context context, Uri tree) throws IOException {
        for (UriPermission grant : context.getContentResolver().getPersistedUriPermissions())
            if (tree.equals(grant.getUri()) && grant.isReadPermission() && grant.isWritePermission()) return;
        throw new IOException("Berechtigung für Protokollordner fehlt");
    }
}
