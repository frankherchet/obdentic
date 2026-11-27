package de.obdentic.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.os.Bundle;
import android.os.SystemClock;

/** Focused, entirely synthetic REQ-046.23 acceptance. */
public final class SharedDiagnosisRunner extends Instrumentation {
    @Override public void onCreate(Bundle args) { super.onCreate(args); start(); }
    @Override public void onStart() {
        Bundle result = new Bundle();
        try {
            ObdService.autoScan = false; ObdService.autoSurvey = false;
            startActivitySync(new Intent(getTargetContext(), MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            long end = SystemClock.elapsedRealtime() + 60000;
            while (ObdService.current == null && SystemClock.elapsedRealtime() < end) SystemClock.sleep(100);
            if (ObdService.current == null) throw new AssertionError("service unavailable");
            AgentAuditAcceptance.runShared(this);
            result.putString("stream", "REQ-046.23 synthetic Android acceptance passed\n");
            finish(Activity.RESULT_OK, result);
        } catch (Throwable error) {
            result.putString("stream", "FAILED: " + error + "\n" + android.util.Log.getStackTraceString(error));
            finish(Activity.RESULT_CANCELED, result);
        }
    }
}
