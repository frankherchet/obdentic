package de.obdentic.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.os.Bundle;
import android.os.SystemClock;

/**
 * The ChatGPT account checks of the full phone acceptance (AcceptanceRunner) on their own, for a quick run:
 * {@code am instrument -w de.obdentic.app.debug.test/de.obdentic.app.AccountAcceptanceRunner}. Fake authorization server and browser; no vehicle,
 * no adapter and no real account. Not phone evidence (REQ-044.8).
 */
public final class AccountAcceptanceRunner extends Instrumentation {
    @Override public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        start();
    }

    @Override public void onStart() {
        Bundle result = new Bundle();
        ObdService.autoScan = false;
        ObdService.autoSurvey = false;
        android.content.SharedPreferences adapterPrefs = getTargetContext().getSharedPreferences("adapter", android.content.Context.MODE_PRIVATE);
        java.util.Map<String, ?> userAdapter = new java.util.HashMap<>(adapterPrefs.getAll());
        adapterPrefs.edit().putBoolean("auto", false).commit(); // the user's known adapter is never connected by this run
        try {
            Activity activity = startActivitySync(new Intent(getTargetContext(), MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            long until = SystemClock.elapsedRealtime() + 30000;
            while (ObdService.current == null && SystemClock.elapsedRealtime() < until) SystemClock.sleep(100);
            ObdService service = ObdService.current;
            if (service == null) throw new AssertionError("service creation");
            runOnMainSync(service::setDemo);
            until = SystemClock.elapsedRealtime() + 30000;
            while (!"ready".equals(service.snapshot().optString("phase")) && SystemClock.elapsedRealtime() < until) SystemClock.sleep(100);
            StringBuilder report = new StringBuilder();
            AccountAcceptance.run(getTargetContext());
            report.append("PASS: account engine\n");
            AccountTransportAcceptance.run();
            report.append("PASS: account HTTPS transport\n");
            AccountUiAcceptance.run(this, (MainActivity) activity, service, getTargetContext());
            report.append("PASS: account section and sign-in service\n");
            result.putString("stream", report + "\nACCOUNT ACCEPTANCE PASSED (simulated; not phone evidence)\n");
            restore(adapterPrefs, userAdapter);
            finish(Activity.RESULT_OK, result);
        } catch (Throwable error) {
            result.putString("stream", "FAILED: " + error + "\n" + android.util.Log.getStackTraceString(error));
            restore(adapterPrefs, userAdapter);
            finish(Activity.RESULT_CANCELED, result);
        }
    }

    private static void restore(android.content.SharedPreferences prefs, java.util.Map<String, ?> saved) {
        android.content.SharedPreferences.Editor editor = prefs.edit().clear();
        if (saved.get("known_address") instanceof String) editor.putString("known_address", (String) saved.get("known_address"));
        if (saved.get("auto") instanceof Boolean) editor.putBoolean("auto", (Boolean) saved.get("auto"));
        editor.commit();
    }
}
