package de.obdentic.spike378;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.provider.DocumentsContract;
import android.util.Log;

/** Throwaway spike #378: the user picks the audit tree with the system picker. */
public class PickActivity extends Activity {
  @Override protected void onCreate(Bundle b) {
    super.onCreate(b);
    Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
    i.putExtra(Intent.EXTRA_LOCAL_ONLY, true);
    i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION | Intent.FLAG_GRANT_PREFIX_URI_PERMISSION);
    i.putExtra(DocumentsContract.EXTRA_INITIAL_URI, DocumentsContract.buildDocumentUri(
        "com.android.externalstorage.documents", "primary:Documents"));
    startActivityForResult(i, 1);
  }

  @Override protected void onActivityResult(int req, int res, Intent d) {
    Uri u = d == null ? null : d.getData();
    Log.i("Spike378", "PICK result=" + res + " uri=" + u + " flags=" + (d == null ? -1 : d.getFlags()));
    if (res == RESULT_OK && u != null) {
      int f = d.getFlags() & (Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
      try {
        getContentResolver().takePersistableUriPermission(u, f);
        getSharedPreferences("s", 0).edit().putString("tree", u.toString()).commit();
        Log.i("Spike378", "PICK persisted grantedFlags=" + f);
      } catch (Throwable t) { Log.i("Spike378", "PICK persist failed " + t); }
    }
    finish();
  }
}
