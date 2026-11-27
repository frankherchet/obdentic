package de.obdentic.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.os.Bundle;
import android.widget.Button;

/**
 * The agent audit checks on their own ({@code am instrument -w de.obdentic.app.debug.test/de.obdentic.app.AgentAuditAcceptanceRunner});
 * the same checks run inside the full AcceptanceRunner. Synthetic client and test document provider: no model, no vehicle, no manual folder choice.
 */
public final class AgentAuditAcceptanceRunner extends Instrumentation {
    @Override public void onCreate(Bundle arguments) { super.onCreate(arguments); start(); }

    @Override public void onStart() {
        Bundle result = new Bundle();
        ObdService.autoScan = false;
        ObdService.autoSurvey = false;
        android.content.SharedPreferences prefs = getTargetContext().getSharedPreferences("adapter", 0);
        boolean automatic = prefs.getBoolean("auto", true);
        prefs.edit().putBoolean("auto", false).commit();
        try {
            AgentAuditAcceptance.run(this);
            Activity activity = startActivitySync(new Intent(getTargetContext(), MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            runOnMainSync(() -> UiNav.openAdapter(activity));
            waitForIdleSync();
            runOnMainSync(() -> {
                Button account = UiNav.button(activity.getWindow().getDecorView(), "Konto öffnen");
                if (account == null) throw new AssertionError("account navigation missing");
                account.performClick();
            });
            waitForIdleSync();
            runOnMainSync(() -> {
                // REQ-045.23: selecting a local audit folder is an explicit user action.
                if (UiNav.button(activity.getWindow().getDecorView(), "Protokollordner auswählen") == null)
                    throw new AssertionError("audit folder selection missing");
            });
            result.putString("stream", "AGENT AUDIT ACCEPTANCE PASSED\n");
            finish(Activity.RESULT_OK, result);
        } catch (Throwable error) {
            result.putString("stream", "FAILED: " + error + "\n" + android.util.Log.getStackTraceString(error));
            finish(Activity.RESULT_CANCELED, result);
        } finally { prefs.edit().putBoolean("auto", automatic).commit(); }
    }
}
