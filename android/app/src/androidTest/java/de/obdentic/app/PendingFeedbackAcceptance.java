package de.obdentic.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.ProgressBar;
import android.widget.TextView;
import org.json.JSONObject;
import java.io.File;
import java.util.List;
import java.util.concurrent.Callable;
import java.util.concurrent.FutureTask;

/**
 * REQ-021 against the real Activity and service in demo mode. The pending entries that must stay pending are injected under the real
 * button's key with a target that never arrives (and, for the time limit, a short injected limit), so the checks do not race the worker.
 */
final class PendingFeedbackAcceptance {
    private static final PendingActions.Spec NEVER = new PendingActions.Spec("rec.start","Aufzeichnung starten","Aufzeichnung wird gestartet …",60000,(n,b)->false);
    private static final String START = "Gewählte Messwerte aufzeichnen", START_PENDING = "Aufzeichnung wird gestartet …", STOP = "Aufzeichnung stoppen", STOP_PENDING = "Aufzeichnung wird gestoppt …";

    /** Demo service ready and idle, the Activity in front. Returns the Activity that is current afterwards (REQ-021.6 recreates it). */
    static Activity run(Instrumentation inst, Activity activity, ObdService service) throws Exception {
        logic();
        PendingActions pending = service.pending;
        require(pending.signature().equals("[]") && service.snapshot().optString("recording").isEmpty(), "REQ-021 precondition: nothing pending, nothing recording");
        until(() -> MainActivity.resumed == activity, "Activity in front");
        service.setSignals(new org.json.JSONArray().put("engine.rpm")); until(() -> service.snapshot().optJSONArray("selected").length() == 1 && !service.snapshot().optBoolean("busy"), "a selection to record");
        click(inst, "Live"); // REQ-025.7: the recording button lives on the Live tab

        // REQ-021.1/.5: a pending button is disabled, shows the pending text and an indicator, and says so to accessibility services.
        pending.start(NEVER,"",service.snapshot(),SystemClock.elapsedRealtime());
        Button b = awaitButton(inst, START_PENDING);
        require(onMain(inst,() -> !b.isEnabled()), "REQ-021.1 pending button is disabled");
        require(onMain(inst,() -> ((ViewGroup)b.getParent()).getChildCount() == 2 && ((ViewGroup)b.getParent()).getChildAt(0) instanceof ProgressBar), "REQ-021.1 progress indicator next to the pending button");
        require(onMain(inst,() -> START_PENDING.contentEquals(b.getContentDescription())), "REQ-021.5 content description is the pending text");
        require(!onMain(inst,() -> findButton(activity.getWindow().getDecorView(),START) != null), "REQ-021.1 the idle label is replaced");

        // REQ-021.4: a tap that still reaches the listener starts nothing.
        inst.runOnMainSync(b::performClick);
        SystemClock.sleep(1200);
        require(service.snapshot().optString("recording").isEmpty() && pending.signature().equals("[rec.start]"), "REQ-021.4 a tap while pending starts no recording: "+service.snapshot().optString("recording"));

        // REQ-021.6: the pending state survives tab switches and Activity recreation.
        click(inst, "Aufnahmen"); click(inst, "Live");
        require(awaitButton(inst, START_PENDING) != null, "REQ-021.6 pending after a tab switch");
        inst.runOnMainSync(activity::recreate);
        until(() -> MainActivity.resumed != null && MainActivity.resumed != activity, "recreated Activity in front");
        Activity current = MainActivity.resumed;
        require(pending.isPending("rec.start"), "REQ-021.6 entry kept outside the Activity");
        Button again = awaitButton(inst, START_PENDING);
        require(onMain(inst,() -> !again.isEnabled() && START_PENDING.contentEquals(again.getContentDescription())), "REQ-021.6 recreated Activity renders the pending button");
        pending.resolve("rec.start");
        Button idle = awaitButton(inst, START);
        require(onMain(inst,() -> idle.isEnabled()), "REQ-021.2 button usable again after the pending state ended");

        // REQ-021.3: no result within the (injected, short) limit.
        pending.start(new PendingActions.Spec("rec.start","Aufzeichnung starten","Aufzeichnung wird gestartet …",1500,(n,b2)->false),"",service.snapshot(),SystemClock.elapsedRealtime());
        awaitButton(inst, START_PENDING);
        until(() -> onMain(inst,() -> hasText(current.getWindow().getDecorView(),"Keine Rückmeldung · Aufzeichnung starten")), "time limit message in the status line");
        require(pending.signature().equals("[]"), "REQ-021.3 pending cleared on expiry");
        Button usable = awaitButton(inst, START);
        require(onMain(inst,() -> usable.isEnabled()), "REQ-021.3 button usable again after expiry");
        require(((MainActivity)current).lastAnnouncement.equals("Keine Rückmeldung · Aufzeichnung starten"), "REQ-021.5 expiry announced: "+((MainActivity)current).lastAnnouncement);

        // REQ-021.1/.2/.5: a real start and stop through the button.
        inst.runOnMainSync(usable::performClick);
        require(((MainActivity)current).lastAnnouncement.equals(START_PENDING), "REQ-021.1/.5 the tap announces the pending text");
        until(() -> !service.snapshot().optString("recording").isEmpty(), "demo recording started");
        String name = service.snapshot().getString("recording");
        Button stop = awaitButton(inst, STOP);
        require(!pending.isPending("rec.start") && onMain(inst,() -> stop.isEnabled()), "REQ-021.2 start observed: pending ended, stop label shown and usable");
        require(!((MainActivity)current).lastAnnouncement.equals(START_PENDING), "REQ-021.5 final state announced: "+((MainActivity)current).lastAnnouncement);
        inst.runOnMainSync(stop::performClick);
        require(((MainActivity)current).lastAnnouncement.equals(STOP_PENDING), "REQ-021.1/.5 stop announces the pending text");
        until(() -> service.snapshot().optString("recording").isEmpty(), "demo recording stopped");
        Button restart = awaitButton(inst, START);
        require(!pending.isPending("rec.stop") && onMain(inst,() -> restart.isEnabled()), "REQ-021.2 stop observed: pending ended, start label shown and usable");

        // Leave nothing behind.
        service.deleteRecording(name);
        until(() -> !new File(new File(inst.getTargetContext().getFilesDir(),"recordings"),name).exists(), "acceptance recording deleted");
        service.setSignals(new org.json.JSONArray());
        until(() -> !service.snapshot().optBoolean("busy"), "idle");
        require(pending.signature().equals("[]"), "no pending entry left behind");
        return current;
    }

    /** The shared mechanism without UI: REQ-021.2/.3/.4. */
    private static void logic() {
        PendingActions p = new PendingActions(); JSONObject base = new JSONObject();
        PendingActions.Spec flag = new PendingActions.Spec("flag","Flag","Flag läuft …",1000,(n,b)->n.optBoolean("on"));
        require(p.start(flag,":a",base,0) && p.start(flag,":b",base,0), "REQ-021.1 independent ids");
        require(!p.start(flag,":a",base,10), "REQ-021.4 a second start of a pending id is refused");
        require(p.isPending("flag:a") && p.signature().equals("[flag:a, flag:b]"), "REQ-021.1 pending set");
        require(p.observe(base).isEmpty(), "REQ-021.2 nothing ends before the target state");
        try { p.observe(base.put("on",true)); } catch(org.json.JSONException e) { throw new AssertionError(e); }
        require(p.signature().equals("[]"), "REQ-021.2 target state ends the pending state");
        require(p.start(flag,":a",base,2000) && p.expire(2999).isEmpty(), "REQ-021.3 nothing expires before the limit");
        List<PendingActions.Entry> late = p.expire(3000);
        require(late.size() == 1 && late.get(0).name.equals("Flag") && p.signature().equals("[]"), "REQ-021.3 expiry after the injected limit ends the pending state");
        p.setNotice("Keine Rückmeldung · Flag",7);
        require(p.notice(7).equals("Keine Rückmeldung · Flag") && p.notice(8).isEmpty(), "REQ-021.3 the message stays until a later service message replaces it");
        require(p.start(flag,":a",base,3001), "REQ-021.3/.4 usable again after expiry");
    }

    private static void click(Instrumentation inst, String text) throws Exception {
        Activity a = MainActivity.resumed; require(a != null, "Activity in front");
        inst.runOnMainSync(() -> { Button button = findButton(a.getWindow().getDecorView(),text); require(button != null, "Button "+text); button.performClick(); });
    }
    private static Button awaitButton(Instrumentation inst, String text) throws Exception {
        Button[] found = new Button[1];
        until(() -> { MainActivity a = MainActivity.resumed; return a != null && onMain(inst,() -> (found[0] = findButton(a.getWindow().getDecorView(),text)) != null); }, "button "+text);
        return found[0];
    }
    private static Button findButton(View view, String text) {
        if(view instanceof Button && text.contentEquals(((Button)view).getText())) return (Button)view;
        if(view instanceof ViewGroup) { ViewGroup g=(ViewGroup)view; for(int i=0;i<g.getChildCount();i++) { Button f=findButton(g.getChildAt(i),text); if(f!=null) return f; } }
        return null;
    }
    private static boolean hasText(View view, String part) {
        if(view instanceof TextView && ((TextView)view).getText().toString().contains(part)) return true;
        if(view instanceof ViewGroup) { ViewGroup g=(ViewGroup)view; for(int i=0;i<g.getChildCount();i++) if(hasText(g.getChildAt(i),part)) return true; }
        return false;
    }
    private static boolean onMain(Instrumentation inst, Callable<Boolean> check) throws Exception {
        FutureTask<Boolean> task = new FutureTask<>(check); inst.runOnMainSync(task); return task.get();
    }
    private interface Condition { boolean ready() throws Exception; }
    private static void until(Condition condition, String name) throws Exception {
        long until = SystemClock.elapsedRealtime() + 20000;
        while(SystemClock.elapsedRealtime() < until) { if(condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("REQ-021 timeout: "+name);
    }
    private static void require(boolean condition, String message) { if(!condition) throw new AssertionError(message); }
}
