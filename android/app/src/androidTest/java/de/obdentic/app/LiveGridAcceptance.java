package de.obdentic.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.pm.ActivityInfo;
import android.content.res.Configuration;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.ScrollView;
import android.widget.TextView;
import java.util.ArrayList;
import org.json.JSONArray;

/** #239: compact live tile grid. */
final class LiveGridAcceptance {
    interface OnMain { <T> T call(java.util.concurrent.Callable<T> c) throws Exception; }

    /**
     * #239 is a portrait-phone criterion ("8 tiles on the first portrait screen"); in landscape the 3-column grid
     * legitimately has fewer rows above the fold. A phone lying on its side must not decide the result, so the
     * Activity is pinned to portrait for the check only (its own requested orientation: no system setting, no lock)
     * and released afterwards. Both changes recreate the Activity; returns the one that is current afterwards.
     */
    static Activity run(Instrumentation inst, OnMain main, ObdService service) throws Exception {
        // The launched Activity may have been replaced since (configuration change, recreation); a replaced Activity no longer
        // polls, so its tiles would never follow the selection. Always work on the one in front.
        Activity portrait = front(main, 15000);
        boolean pinned = !isPortrait(main, portrait);
        if (pinned) {
            final Activity old = portrait;
            inst.runOnMainSync(() -> old.setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_PORTRAIT));
            portrait = awaitOther(main, old, 10000);
            require(portrait != null && isPortrait(main, portrait), "#239 the Activity can be shown in portrait");
        }
        Activity checked = verify(main, service);
        if (!pinned) return checked;
        final Activity pinnedActivity = checked;
        inst.runOnMainSync(() -> pinnedActivity.setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED));
        Activity released = awaitOther(main, pinnedActivity, 5000); // stays the same when the phone itself is upright
        return released != null ? released : pinnedActivity;
    }

    /** The resumed, laid-out MainActivity once it has been the same instance for 500 ms; fails after {@code ms}. */
    static Activity front(OnMain main, long ms) throws Exception {
        long end = System.currentTimeMillis() + ms, since = 0;
        MainActivity candidate = null;
        while (System.currentTimeMillis() < end) {
            MainActivity current = MainActivity.resumed;
            if (current != null && main.call(() -> current.getWindow().getDecorView().getWidth() > 0 && current.getWindow().getDecorView().isLaidOut())) {
                if (current != candidate) { candidate = current; since = System.currentTimeMillis(); }
                else if (System.currentTimeMillis() - since >= 500) return current;
            } else candidate = null;
            Thread.sleep(100);
        }
        throw new AssertionError("#239 a laid-out MainActivity is in front (resumed=" + MainActivity.resumed + ")");
    }

    private static boolean isPortrait(OnMain main, Activity activity) throws Exception {
        return main.call(() -> activity.getResources().getConfiguration().orientation == Configuration.ORIENTATION_PORTRAIT);
    }

    /** The resumed MainActivity once it is another instance than {@code old}, or null after {@code ms}. */
    private static Activity awaitOther(OnMain main, Activity old, long ms) throws Exception {
        long end = System.currentTimeMillis() + ms;
        while (System.currentTimeMillis() < end) {
            MainActivity now = MainActivity.resumed;
            if (now != null && now != old && main.call(() -> now.getWindow().getDecorView().getWidth() > 0)) return now;
            Thread.sleep(100);
        }
        return null;
    }

    /** Returns the Activity on which the 8 selected tiles were verified. */
    private static Activity verify(OnMain main, ObdService service) throws Exception {
        require("".equals(MainActivity.staleText(10, false)) && "vor 11 s".equals(MainActivity.staleText(11, false)) && "".equals(MainActivity.staleText(99, true)), "#239 stale helper: only > 10 s and not replay shows 'vor N s'");
        JSONArray catalog = service.snapshot().getJSONArray("signals"), pick = new JSONArray();
        for (int i = 0; i < 8; i++) pick.put(catalog.getJSONObject(i).getString("semantic"));
        service.setSignals(pick);
        long end = System.currentTimeMillis() + 20000;
        String last = "";
        Activity shown = null;
        while (System.currentTimeMillis() < end) {
            MainActivity current = MainActivity.resumed; // re-read every round: the Activity may be recreated while the check runs
            if (current == null) last = "no MainActivity is resumed";
            else {
                shown = current;
                last = main.call(() -> check(service, current));
                if (last.isEmpty()) break;
            }
            Thread.sleep(300);
        }
        require(last.isEmpty() && shown != null, "#239 " + last);
        final Activity verified = shown;
        String first = main.call(() -> ranges(verified.getWindow().getDecorView()));
        Thread.sleep(2500);
        String later = main.call(() -> ranges(verified.getWindow().getDecorView()));
        require(!first.isEmpty() && later.contains("min ") && later.contains("max "), "#239 min/max text present: " + later);
        return verified;
    }

    /**
     * Empty when the Live tab of {@code activity} shows its 8 selected tiles and all 8 are in the scroll viewport with name and
     * formatted value. Runs on the main thread. Brings the Live tab to its top first, as a user opening it would see it.
     */
    private static String check(ObdService service, MainActivity activity) {
        View root = activity.getWindow().getDecorView();
        if (service.snapshot().optJSONArray("selected") == null || service.snapshot().optJSONArray("selected").length() != 8) return "service selection is not the 8 picked signals yet";
        Button live = findButton(root, "Live");
        ScrollView scroll = find(root);
        if (live == null || scroll == null) return "no Live tab button or scroll view";
        if (scroll.getScrollY() != 0 || !isLiveTab(root)) { live.performClick(); return "Live tab not yet at its first screen"; }
        ArrayList<View> tiles = new ArrayList<>();
        collect(root, tiles);
        if (tiles.size() != 8) return "Live tab shows " + tiles.size() + " tiles for the 8 selected signals";
        for (View tile : tiles) if (!tile.isLaidOut() || tile.getHeight() <= 0) return "tiles not laid out yet";
        int[] s = new int[2], t = new int[2];
        scroll.getLocationOnScreen(s);
        int inView = 0;
        for (View tile : tiles) {
            tile.getLocationOnScreen(t);
            if (t[1] >= s[1] && t[1] < s[1] + scroll.getHeight()) inView++;
            else continue;
            ViewGroup g = (ViewGroup) tile;
            String name = ((TextView) g.getChildAt(0)).getText().toString(), value = ((TextView) g.getChildAt(1)).getText().toString();
            if (name.isEmpty() || name.matches("^[a-z]+(\\.[a-z0-9_]+)+$")) return "tile name missing or semantic id: '" + name + "'";
            if (!value.matches(".*[0-9].*") && !value.equals("Nicht verfügbar")) return "tile value not yet formatted: '" + value + "'";
        }
        if (inView < 8) {
            float density = activity.getResources().getDisplayMetrics().density;
            return "only " + inView + " of " + tiles.size() + " tiles on the first screen (viewport " + Math.round(scroll.getWidth() / density) + "x" + Math.round(scroll.getHeight() / density) + " dp, first tile y=" + firstTop(tiles) + ", viewport top " + s[1] + ")";
        }
        FormatAcceptance.noSemanticIds(root);
        return "";
    }

    private static int firstTop(ArrayList<View> tiles) { int[] t = new int[2]; tiles.get(0).getLocationOnScreen(t); return t[1]; }

    /** The Live tab is showing when its "Messwerte auswählen" button is on screen. */
    private static boolean isLiveTab(View root) { return findButton(root, "Messwerte auswählen") != null; }

    private static Button findButton(View v, String text) {
        if (v instanceof Button && text.contentEquals(((Button) v).getText())) return (Button) v;
        if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) { Button f = findButton(g.getChildAt(i), text); if (f != null) return f; } }
        return null;
    }

    private static String ranges(View root) {
        StringBuilder b = new StringBuilder();
        ArrayList<View> tiles = new ArrayList<>();
        collect(root, tiles);
        for (View tile : tiles) b.append(((TextView) ((ViewGroup) tile).getChildAt(3)).getText()).append('|');
        return b.toString().replace("|", "").isEmpty() ? "" : b.toString();
    }

    private static void collect(View v, ArrayList<View> out) {
        if ("live-tile".equals(v.getTag())) out.add(v);
        if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) collect(g.getChildAt(i), out); }
    }

    private static ScrollView find(View v) {
        if (v instanceof ScrollView) return (ScrollView) v;
        if (v instanceof ViewGroup) { ViewGroup g = (ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) { ScrollView f = find(g.getChildAt(i)); if (f != null) return f; } }
        return null;
    }

    private static void require(boolean value, String message) { if (!value) throw new AssertionError(message); }
}
