package de.obdentic.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.pm.ActivityInfo;
import android.os.SystemClock;
import android.text.Layout;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;
import org.json.JSONArray;
import org.json.JSONObject;
import java.util.concurrent.Callable;
import java.util.concurrent.FutureTask;

/**
 * The task-oriented UI: three work areas, adapter and vehicles as context targets, the Live start surface, Diagnose ordered by result,
 * no raw-traffic view anywhere. Real Activity and service in demo; no adapter traffic is started by looking around.
 */
final class UiStructureAcceptance {
    interface Shot { void shoot(Activity activity, String name); }

    /** Demo ready and idle, MainActivity in front. Leaves demo ready on Live. */
    static void run(Instrumentation inst, ObdService service, Shot shot) throws Exception {
        pureTexts();
        service.setSignals(new JSONArray()); until(() -> !service.snapshot().optBoolean("busy"), "idle demo");
        Activity a = front();

        // disconnected: one start surface; "Adapter verbinden" opens the adapter view without a scan, back returns to Live
        service.disconnect(); until(() -> "disconnected".equals(service.snapshot().optString("phase")), "disconnected");
        click(inst, a, "Live");
        require(onMain(inst, () -> button(a, "Adapter verbinden") != null && button(a, "Demo starten") != null), "REQ-055.3 start surface offers 'Adapter verbinden' and 'Demo starten'");
        require(onMain(inst, () -> button(a, "Auswahl aufzeichnen") == null && button(a, "Messwerte auswählen") == null && !hasText(a, "Smart-Analyse starten")), "REQ-055.3 no locked recording or selection actions without a connection");
        shot.shoot(a, "acceptance-start.png");
        long wire = service.wireEventCount();
        click(inst, a, "Adapter verbinden");
        require(onMain(inst, () -> title(a).equals("Adapter") && button(a, "Zurück") != null && button(a, "Adapter suchen") != null), "REQ-055.3 'Adapter verbinden' opens the adapter view");
        SystemClock.sleep(600);
        require(!"scanning".equals(service.snapshot().optString("phase")) && !service.pending.isPending("scan") && service.wireEventCount() == wire, "REQ-055.3/REQ-055.6 opening the adapter view starts no search and sends nothing");
        click(inst, a, "Zurück");
        require(onMain(inst, () -> title(a).equals("Live") && button(a, "Adapter verbinden") != null), "REQ-055.1 back returns to Live");
        inst.runOnMainSync(() -> UiNav.openAdapter(a)); inst.waitForIdleSync();
        shell(inst, "input keyevent 4"); // the system back key
        until(() -> onMain(inst, () -> title(a).equals("Live")) && MainActivity.resumed == a, "REQ-055.1 Android back from the adapter view returns to Live");

        // "Demo starten" stays on Live with the simulation named in the header
        click(inst, a, "Demo starten");
        until(() -> "ready".equals(service.snapshot().optString("phase")) && "demo".equals(service.snapshot().optString("mode")) && !service.snapshot().optBoolean("busy"), "demo from the start surface");
        until(() -> onMain(inst, () -> title(a).equals("Live") && modeText(a).equals("DEMO · SIMULIERT") && button(a, "Messwerte auswählen") != null), "REQ-055.2/REQ-055.3 Live with the simulation named in the header");

        // context targets remember the work area; a main tab switches directly
        click(inst, a, "Diagnose");
        inst.runOnMainSync(() -> UiNav.openAdapter(a));
        require(onMain(inst, () -> title(a).equals("Adapter") && nav(a).length() == 0), "REQ-055.1 the adapter view selects no main tab");
        click(inst, a, "Zurück");
        require(onMain(inst, () -> title(a).equals("Diagnose") && nav(a).equals("Diagnose")), "REQ-055.1 back returns to Diagnose");
        click(inst, a, "Aufnahmen");
        inst.runOnMainSync(() -> UiNav.openVehicles(a));
        require(onMain(inst, () -> title(a).equals("Fahrzeuge")), "REQ-055.1 the vehicle name opens the vehicle list in one tap");
        click(inst, a, "Live");
        require(onMain(inst, () -> title(a).equals("Live") && nav(a).equals("Live")), "REQ-055.1 a main tab switches directly from a context target");
        require(onMain(inst, () -> navLabels(a).equals("Live|Diagnose|Aufnahmen")), "REQ-055.1 the bottom navigation holds exactly Live, Diagnose, Aufnahmen: " + onMain(inst, () -> navLabels(a)));

        // an empty library offers the way to Live, no start button of its own
        require(service.snapshot().optString("recording").isEmpty() && service.snapshot().optString("autoRecording").isEmpty(), "no recording while the library is emptied");
        java.io.File dir = new java.io.File(inst.getTargetContext().getFilesDir(), "recordings"), hold = new java.io.File(inst.getTargetContext().getFilesDir(), "recordings-hold-" + System.nanoTime());
        require(dir.renameTo(hold) && dir.mkdirs(), "set the recordings aside");
        try {
            click(inst, a, "Aufnahmen");
            until(() -> onMain(inst, () -> hasText(a, "Noch keine Aufnahmen") && button(a, "Zu Live") != null && button(a, "Auswahl aufzeichnen") == null), "REQ-055.5 the empty library says 'Noch keine Aufnahmen' and offers 'Zu Live'");
            click(inst, a, "Zu Live");
            require(onMain(inst, () -> title(a).equals("Live")), "REQ-055.5 'Zu Live' opens Live");
        } finally { dir.delete(); require(hold.renameTo(dir), "recordings restored"); }

        // Diagnose: results before the special checks; opening, rotating and returning start nothing
        click(inst, a, "Diagnose");
        String order = onMain(inst, () -> allText(a));
        require(order.indexOf("FEHLERCODES") < order.indexOf("FAHRZEUG UND STEUERGERÄTE") && order.indexOf("FAHRZEUG UND STEUERGERÄTE") < order.indexOf("Weitere Prüfungen anzeigen")
            && !order.contains("ADAPTER-LEISTUNG"), "REQ-055.4 Diagnose starts with fault codes, then vehicle, the special checks closed: " + order);
        service.setSignals(new JSONArray()); until(() -> !service.snapshot().optBoolean("busy") && service.snapshot().optJSONArray("selected").length() == 0, "no polling: the demo start chose the default selection");
        SystemClock.sleep(1200); // a read in flight ends
        long before = service.wireEventCount(); String job = String.valueOf(service.snapshot().opt("jobResult"));
        inst.runOnMainSync(() -> UiNav.openMoreChecks(a));
        require(onMain(inst, () -> hasText(a, "ADAPTER-LEISTUNG") && hasText(a, "VERFÜGBARKEIT DER MESSWERTE") && hasText(a, "STEUERGERÄTE ERKENNEN") && button(a, "Weitere Prüfungen ausblenden") != null), "REQ-055.4 'Weitere Prüfungen' shows the special checks");
        inst.runOnMainSync(a::recreate);
        until(() -> MainActivity.resumed != null && MainActivity.resumed != a, "recreated Activity");
        Activity b = front();
        until(() -> onMain(inst, () -> title(b).equals("Diagnose") && button(b, "Weitere Prüfungen ausblenden") != null), "REQ-055.1 screen and open checks survive recreation");
        click(inst, b, "Live"); click(inst, b, "Diagnose");
        SystemClock.sleep(600);
        long after = service.wireEventCount();
        require(after == before && !service.snapshot().optBoolean("busy") && job.equals(String.valueOf(service.snapshot().opt("jobResult"))), "REQ-055.6 opening, recreating and returning start no job and send nothing: wire " + before + " -> " + after + ", busy " + service.snapshot().optBoolean("busy"));
        require(onMain(inst, () -> !hasText(b, "Debug") && !hasText(b, "TX ") && !hasText(b, "RX ") && !hasMonospace(b.getWindow().getDecorView())), "REQ-055.7 Diagnose shows no raw traffic or debug action");
        inst.runOnMainSync(() -> UiNav.closeMoreChecks(b));

        largeTextAndLandscape(inst, service, shot);
        click(inst, front(), "Live");
    }

    /** Header, tab and actions stay distinguishable with large text and in landscape; nothing in the context bar is cut off. */
    private static void largeTextAndLandscape(Instrumentation inst, ObdService service, Shot shot) throws Exception {
        String scale = shell(inst, "settings get system font_scale").trim();
        try {
            Activity old = front();
            shell(inst, "settings put system font_scale 1.3");
            until(() -> MainActivity.resumed != null && MainActivity.resumed != old && MainActivity.resumed.getResources().getConfiguration().fontScale > 1.2f, "large text applied");
            Activity big = front();
            click(inst, big, "Diagnose");
            checkHeader(inst, big, "large text");
            require(onMain(inst, () -> ((LinearLayout) UiNav.tagged(big, "context-adapter").getParent()).getOrientation() == LinearLayout.VERTICAL), "REQ-055.8 large text stacks the context bar instead of cutting it");
            shot.shoot(big, "acceptance-largefont.png");
            click(inst, big, "Live");
        } finally {
            Activity before = MainActivity.resumed;
            shell(inst, "settings put system font_scale " + (scale.isEmpty() || scale.equals("null") ? "1.0" : scale));
            until(() -> MainActivity.resumed != null && MainActivity.resumed != before, "text size restored");
        }
        Activity portrait = front();
        inst.runOnMainSync(() -> portrait.setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE));
        try {
            until(() -> MainActivity.resumed != null && MainActivity.resumed != portrait, "landscape");
            Activity wide = front();
            click(inst, wide, "Aufnahmen");
            checkHeader(inst, wide, "landscape");
            require(onMain(inst, () -> title(wide).equals("Aufnahmen")), "landscape keeps the work area");
        } finally {
            Activity wide = MainActivity.resumed;
            if (wide != null) inst.runOnMainSync(() -> wide.setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED));
            SystemClock.sleep(1500);
        }
    }

    private static void checkHeader(Instrumentation inst, Activity a, String where) throws Exception {
        require(onMain(inst, () -> {
            Button mode = (Button) UiNav.tagged(a, "context-adapter"), vehicle = (Button) UiNav.tagged(a, "context-vehicle");
            return complete(mode) && complete(vehicle) && String.valueOf(mode.getContentDescription()).startsWith("Verbindung: ")
                && String.valueOf(vehicle.getContentDescription()).startsWith("Gewähltes Fahrzeug: ") && mode.getHeight() >= dp(a, 48) && vehicle.getHeight() >= dp(a, 48)
                && nav(a).length() > 0 && navButtonsHigh(a);
        }), "REQ-055.8 " + where + ": mode, vehicle and selected tab are complete, labelled and at least 48 dp");
    }

    /** Pure texts: fault-code lines never call a timeout "no codes"; a failure points to a recording only when it exists. */
    private static void pureTexts() throws Exception {
        JSONObject empty = new JSONObject().put("observations", new JSONArray().put(new JSONObject().put("responder", "7E8").put("codes", new JSONArray()).put("no_dtcs", true)));
        JSONObject timeout = new JSONObject().put("observations", new JSONArray().put(new JSONObject().put("responder", "7E8").put("error", "Zeitüberschreitung")));
        JSONObject silent = new JSONObject().put("observations", new JSONArray());
        JSONObject codes = new JSONObject().put("observations", new JSONArray().put(new JSONObject().put("responder", "7E8").put("codes", new JSONArray().put("P0133"))));
        String none = "Keine gespeicherten Fehlercodes";
        require(lines(empty, none).equals("7E8 · " + none + "/text"), "REQ-055.4 an empty answer says no codes: " + lines(empty, none));
        require(!lines(timeout, none).contains(none) && lines(timeout, none).contains("Zeitüberschreitung/error"), "REQ-055.4 a timeout is an error, never 'no codes': " + lines(timeout, none));
        require(!lines(silent, none).contains(none) && lines(silent, none).contains("Keine Steuergeräte-Antworten"), "REQ-055.4 no answer is not 'no codes': " + lines(silent, none));
        require(lines(codes, none).equals("7E8 · P0133/text"), "REQ-055.4 a successful answer lists its codes: " + lines(codes, none));
        JSONObject failed = new JSONObject().put("ok", false).put("error", "NO DATA").put("action", "dtc.scan").put("recording", "x-live-auto.jsonl");
        require(MainActivity.failureRecordingNote(failed, new JSONObject().put("records", new JSONArray())).isEmpty(), "REQ-055.7 without the recording the failure claims no file");
        require(MainActivity.failureRecordingNote(new JSONObject().put("ok", false).put("error", "NO DATA"), new JSONObject()).isEmpty(), "REQ-055.7 a failure without a recording names none");
        require(MainActivity.failureRecordingNote(failed, new JSONObject().put("autoRecording", "x-live-auto.jsonl")).contains("x-live-auto.jsonl"), "REQ-055.7 a running recording is named for the offline evaluation");
        require(MainActivity.jobFailureTitle("dtc.scan").equals("FEHLERCODES NICHT GELESEN") && MainActivity.jobFailureScope("dtc.scan").contains("nicht als aktuell") && MainActivity.jobFailureScope("knowledge.availability").contains("gilt weiter"), "REQ-055.7 the failure names what was not determined and whether an earlier result still applies");
        require(MainActivity.modeText(new JSONObject().put("mode", "replay").put("replaySourceMode", "demo").put("replayPartial", true)).equals("REPLAY · DEMO · TEILAUFNAHME")
            && MainActivity.modeText(new JSONObject().put("mode", "live").put("phase", "idle")).equals("LIVE · GETRENNT")
            && MainActivity.modeText(new JSONObject().put("mode", "live").put("phase", "ready")).equals("LIVE · ADAPTER"), "REQ-055.2 the header names mode, source and partial recordings in words");
    }

    /** Replay: Live shows the replay source and decoded values, no raw traffic, and opening sent nothing. */
    static void replay(Instrumentation inst, ObdService service, long wireBefore) throws Exception {
        Activity a = toFront(inst);
        click(inst, a, "Live");
        until(() -> onMain(inst, () -> modeText(a).startsWith("REPLAY · ") && tiles(a.getWindow().getDecorView()) > 0), "REQ-055.2/REQ-055.3 Live shows the replay with its source and tiles");
        require(service.wireEventCount() == wireBefore, "REQ-055.6 opening a recording sends no adapter traffic");
        require(onMain(inst, () -> !hasText(a, "Debug") && !hasText(a, "TX ") && !hasText(a, "RX ") && !hasMonospace(a.getWindow().getDecorView()) && button(a, "Auswahl aufzeichnen") == null), "REQ-055.7 replay shows no raw traffic and no recording start");
    }

    /** Aufnahmen: whether the file's delete action, under its "Weitere Aktionen", is enabled. Closes the actions again. */
    static boolean deleteEnabled(Instrumentation inst, String filename) throws Exception {
        Activity a = toFront(inst);
        click(inst, a, "Aufnahmen");
        return onMain(inst, () -> {
            View more = described(a.getWindow().getDecorView(), "Weitere Aktionen: " + filename); require(more != null, "'Weitere Aktionen' of " + filename);
            more.performClick();
            View open = described(a.getWindow().getDecorView(), "Weitere Aktionen ausblenden: " + filename); require(open != null, "opened actions of " + filename);
            ViewGroup card = (ViewGroup) open.getParent().getParent();
            Button delete = UiNav.button(card, "Aufnahme löschen …"), export = UiNav.button(card, "Datei exportieren …"); require(delete != null && export != null, "file actions");
            boolean enabled = delete.isEnabled();
            open.performClick();
            return enabled;
        });
    }

    private static String lines(JSONObject dtcs, String none) { StringBuilder s = new StringBuilder(); for (String[] l : MainActivity.dtcLines(dtcs, none)) { if (s.length() > 0) s.append('\n'); s.append(l[0]).append('/').append(l[1]); } return s.toString(); }
    private static boolean complete(TextView t) { Layout l = t.getLayout(); if (l == null) return false; for (int i = 0; i < l.getLineCount(); i++) if (l.getEllipsisCount(i) > 0) return false; return t.getText().length() > 0; }
    private static boolean navButtonsHigh(Activity a) { for (Button b : navButtons(a)) if (b.getHeight() < dp(a, 48)) return false; return true; }
    private static int dp(Activity a, int n) { return Math.round(n * a.getResources().getDisplayMetrics().density) - 1; }
    private static String title(Activity a) { return String.valueOf(a.getTitle()); }
    private static String modeText(Activity a) { return ((Button) UiNav.tagged(a, "context-adapter")).getText().toString(); }
    private static java.util.List<Button> navButtons(Activity a) {
        java.util.List<Button> out = new java.util.ArrayList<>();
        View live = UiNav.button(a.getWindow().getDecorView(), "Live"); if (live == null) return out;
        ViewGroup bar = (ViewGroup) live.getParent(); for (int i = 0; i < bar.getChildCount(); i++) if (bar.getChildAt(i) instanceof Button) out.add((Button) bar.getChildAt(i));
        return out;
    }
    private static String navLabels(Activity a) { StringBuilder s = new StringBuilder(); for (Button b : navButtons(a)) { if (s.length() > 0) s.append('|'); s.append(b.getText()); } return s.toString(); }
    /** The selected main tab, "" on a context target. */
    private static String nav(Activity a) { for (Button b : navButtons(a)) if (b.isSelected()) return b.getText().toString(); return ""; }
    private static Button button(Activity a, String text) { return UiNav.button(a.getWindow().getDecorView(), text); }
    private static void click(Instrumentation inst, Activity a, String text) throws Exception { onMain(inst, () -> { Button b = button(a, text); require(b != null, "button '" + text + "'"); b.performClick(); return null; }); inst.waitForIdleSync(); }
    private static boolean hasText(Activity a, String part) { return allText(a).contains(part); }
    private static String allText(Activity a) { StringBuilder s = new StringBuilder(); collect(a.getWindow().getDecorView(), s); return s.toString(); }
    private static void collect(View v, StringBuilder out) {
        if (v.getVisibility() != View.VISIBLE) return;
        if (v instanceof TextView) out.append(((TextView) v).getText()).append('\n');
        if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) collect(g.getChildAt(i), out); }
    }
    private static boolean hasMonospace(View v) {
        if (v.getVisibility() != View.VISIBLE) return false;
        if (v instanceof TextView && !(v instanceof Button) && ((TextView) v).getTypeface() == android.graphics.Typeface.MONOSPACE && ((TextView) v).getText().length() > 40) return true; // tile values are short monospace numbers
        if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) if (hasMonospace(g.getChildAt(i))) return true; }
        return false;
    }
    private static int tiles(View v) { int n = "live-tile".equals(v.getTag()) ? 1 : 0; if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) n += tiles(g.getChildAt(i)); } return n; }
    private static View described(View v, String d) {
        if (v.getContentDescription() != null && d.contentEquals(v.getContentDescription())) return v;
        if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) { View f = described(g.getChildAt(i), d); if (f != null) return f; } }
        return null;
    }
    private static Activity front() throws Exception { return LiveGridAcceptance.front(new LiveGridAcceptance.OnMain() { public <T> T call(Callable<T> c) throws Exception { FutureTask<T> t = new FutureTask<>(c); new android.os.Handler(android.os.Looper.getMainLooper()).post(t); return t.get(); } }, 15000); }
    /** The runner sends the app to the background earlier (home key); a user opens it again before looking at the screen. */
    private static Activity toFront(Instrumentation inst) throws Exception {
        if (MainActivity.resumed == null) inst.getTargetContext().startActivity(new android.content.Intent(inst.getTargetContext(), MainActivity.class).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK | android.content.Intent.FLAG_ACTIVITY_REORDER_TO_FRONT));
        return front();
    }
    private static <T> T onMain(Instrumentation inst, Callable<T> c) throws Exception { FutureTask<T> t = new FutureTask<>(c); inst.runOnMainSync(t); return t.get(); }
    private static String shell(Instrumentation inst, String command) throws Exception {
        try (android.os.ParcelFileDescriptor fd = inst.getUiAutomation().executeShellCommand(command); java.io.InputStream in = new android.os.ParcelFileDescriptor.AutoCloseInputStream(fd)) {
            java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream(); byte[] buf = new byte[256]; int n; while ((n = in.read(buf)) > 0) out.write(buf, 0, n); return out.toString("UTF-8");
        }
    }
    private interface Condition { boolean ready() throws Exception; }
    private static void until(Condition c, String what) throws Exception { long end = SystemClock.elapsedRealtime() + 15000; while (SystemClock.elapsedRealtime() < end) { if (c.ready()) return; SystemClock.sleep(100); } throw new AssertionError("Timeout: " + what); }
    private static void require(boolean c, String m) { if (!c) throw new AssertionError(m); }
}
