package de.obdentic.app;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.provider.DocumentsContract;

/**
 * Runs in the test APK, which owns the test provider's authority, and grants its tree to the app under test: the instrumentation itself runs
 * as the app and cannot grant a tree it does not own, and no human picks the folder.
 */
public final class AuditGrantReceiver extends BroadcastReceiver {
    @Override public void onReceive(Context context, Intent intent) {
        String pkg = intent.getStringExtra("package");
        try {
            context.grantUriPermission(pkg, DocumentsContract.buildTreeDocumentUri(AuditTestProvider.AUTHORITY, AuditTestProvider.ROOT_ID),
                Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
                    | Intent.FLAG_GRANT_PREFIX_URI_PERMISSION);
            setResultCode(1);
        } catch (RuntimeException e) {
            setResultCode(0);
        }
    }
}
