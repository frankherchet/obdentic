package de.obdentic.app;

import android.app.Activity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ScrollView;
import android.widget.TextView;
import java.util.ArrayList;
import org.json.JSONArray;

/** #239: compact live tile grid. */
final class LiveGridAcceptance {
    interface OnMain { <T> T call(java.util.concurrent.Callable<T> c) throws Exception; }

    static void run(OnMain main, ObdService service, Activity activity) throws Exception {
        require("".equals(MainActivity.staleText(10, false)) && "vor 11 s".equals(MainActivity.staleText(11, false)) && "".equals(MainActivity.staleText(99, true)), "#239 stale helper: only > 10 s and not replay shows 'vor N s'");
        JSONArray catalog = service.snapshot().getJSONArray("signals"), pick = new JSONArray();
        for (int i = 0; i < 8; i++) pick.put(catalog.getJSONObject(i).getString("semantic"));
        service.setSignals(pick);
        long end = System.currentTimeMillis() + 15000;
        String last = "";
        while (System.currentTimeMillis() < end) {
            last = main.call(() -> check(activity.getWindow().getDecorView(), false));
            if (last.isEmpty()) break;
            Thread.sleep(300);
        }
        require(last.isEmpty(), "#239 " + last);
        String first = main.call(() -> ranges(activity.getWindow().getDecorView()));
        Thread.sleep(2500);
        String later = main.call(() -> ranges(activity.getWindow().getDecorView()));
        require(!first.isEmpty() && later.contains("min ") && later.contains("max "), "#239 min/max text present: " + later);
    }

    /** Empty when 8 tiles are visible in the scroll viewport with name and formatted value. */
    private static String check(View root, boolean unused) {
        ArrayList<View> tiles = new ArrayList<>();
        collect(root, tiles);
        ScrollView scroll = find(root);
        if (scroll == null) return "no scroll view";
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
        if (inView < 8) return "only " + inView + " of " + tiles.size() + " tiles on the first screen";
        FormatAcceptance.noSemanticIds(root);
        return "";
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
