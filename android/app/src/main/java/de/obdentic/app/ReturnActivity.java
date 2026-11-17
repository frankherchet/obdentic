package de.obdentic.app;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;

/**
 * REQ-042.6: target of the link on the sign-in callback page. It takes nothing from the intent that opened it (no data, no extras):
 * it only brings the running app to the front. The sign-in result comes from the attempt state, whichever way the user returns.
 */
public final class ReturnActivity extends Activity {
    @Override protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        startActivity(new Intent(this, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_REORDER_TO_FRONT));
        finish();
    }
}
