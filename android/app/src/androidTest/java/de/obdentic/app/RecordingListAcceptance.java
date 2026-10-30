package de.obdentic.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.TextView;
import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;

/** REQ-028.13: thousands of recordings must not freeze the recordings tab. Writes old-dated dummy files and removes them again. */
final class RecordingListAcceptance {
    static void run(Instrumentation inst, ObdService service) throws Exception {
        File dir = new File(inst.getTargetContext().getFilesDir(), "recordings");
        List<File> dummies = new ArrayList<>();
        try {
            byte[] line = "{}\n".getBytes(StandardCharsets.UTF_8);
            for (int i = 0; i < 3000; i++) { File f = new File(dir, String.format("19990101-000000-%03d-%d-live-auto.jsonl", i % 1000, i)); Files.write(f.toPath(), line); dummies.add(f); }
            Activity a = MainActivity.resumed; require(a != null, "Activity in front");
            inst.runOnMainSync(() -> { Button b = find(a.getWindow().getDecorView(), "Live"); if (b != null) b.performClick(); });
            long start = SystemClock.elapsedRealtime();
            inst.runOnMainSync(() -> { Button b = find(a.getWindow().getDecorView(), "Aufnahmen"); require(b != null, "tab button"); b.performClick(); });
            long took = SystemClock.elapsedRealtime() - start;
            int[] cards = {0}; boolean[] note = {false};
            inst.runOnMainSync(() -> { cards[0] = count(a.getWindow().getDecorView(), "Als Replay öffnen"); note[0] = text(a.getWindow().getDecorView(), "ältere Aufnahmen nicht angezeigt"); });
            require(took < 1500, "REQ-028.13 the recordings tab opens fast with 3000 extra recordings: " + took + " ms");
            require(cards[0] <= 30 && cards[0] > 0, "REQ-028.13 at most the newest 30 recordings are listed: " + cards[0]);
            require(note[0], "REQ-028.13 the number of older recordings is shown");
            long snap = SystemClock.elapsedRealtime(); for (int i = 0; i < 10; i++) service.snapshot();
            require(SystemClock.elapsedRealtime() - snap < 1000, "REQ-028.13 ten state snapshots stay fast: " + (SystemClock.elapsedRealtime() - snap) + " ms");
            // REQ-033.1: the debug tab is one full-height window that follows the newest line
            inst.runOnMainSync(() -> { Button b = find(a.getWindow().getDecorView(), "Debug"); require(b != null, "debug tab"); b.performClick(); });
            SystemClock.sleep(1500);
            boolean[] bottom = {false}; String[] why = {""};
            inst.runOnMainSync(() -> { android.widget.ScrollView sv = scrollView(a.getWindow().getDecorView()); View child = sv.getChildAt(0);
                TextView log = monospace(sv); bottom[0] = log != null && log.getHeight() >= sv.getHeight() - 400 && sv.getScrollY() + sv.getHeight() >= child.getHeight() - 4;
                why[0] = "log=" + (log == null ? "none" : log.getHeight()) + " view=" + sv.getHeight() + " y=" + sv.getScrollY() + " content=" + child.getHeight(); });
            require(bottom[0], "REQ-033.1 the debug window fills the screen and shows the newest line: " + why[0]);
            inst.runOnMainSync(() -> { Button b = find(a.getWindow().getDecorView(), "Live"); if (b != null) b.performClick(); });
        } finally { for (File f : dummies) f.delete(); }
    }
    private static Button find(View v, String t) {
        if (v instanceof Button && t.contentEquals(((Button) v).getText())) return (Button) v;
        if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) { Button f = find(g.getChildAt(i), t); if (f != null) return f; } }
        return null;
    }
    private static android.widget.ScrollView scrollView(View v) {
        if (v instanceof android.widget.ScrollView) return (android.widget.ScrollView) v;
        if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) { android.widget.ScrollView f = scrollView(g.getChildAt(i)); if (f != null) return f; } }
        return null;
    }
    private static TextView monospace(View v) {
        if (v instanceof TextView && !(v instanceof Button) && ((TextView) v).getTypeface() == android.graphics.Typeface.MONOSPACE) return (TextView) v;
        if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) { TextView f = monospace(g.getChildAt(i)); if (f != null) return f; } }
        return null;
    }
    private static int count(View v, String t) {
        int n = v instanceof Button && t.contentEquals(((Button) v).getText()) ? 1 : 0;
        if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) n += count(g.getChildAt(i), t); }
        return n;
    }
    private static boolean text(View v, String part) {
        if (v instanceof TextView && ((TextView) v).getText().toString().contains(part)) return true;
        if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) if (text(g.getChildAt(i), part)) return true; }
        return false;
    }
    private static void require(boolean c, String m) { if (!c) throw new AssertionError(m); }
}
