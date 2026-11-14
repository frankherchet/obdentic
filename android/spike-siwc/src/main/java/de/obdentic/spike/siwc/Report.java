package de.obdentic.spike.siwc;

import android.util.Log;
import java.util.ArrayList;
import java.util.List;
import java.util.regex.Pattern;

/** Sanitized result lines. Callers pass only statuses, codes, lengths, counts, durations. */
final class Report {
    private static final List<String> LINES = new ArrayList<>();
    // Defensive scrub: long opaque runs (tokens, ids) and anything containing '@' never get logged.
    private static final Pattern OPAQUE = Pattern.compile("[A-Za-z0-9_+/=.-]{32,}");
    private static final Pattern AT = Pattern.compile("\\S*@\\S*");
    private static Runnable listener;

    private Report() {}

    static void line(String check, String result, String detail) {
        String d = detail == null ? "" : detail;
        d = AT.matcher(OPAQUE.matcher(d).replaceAll("<redacted>")).replaceAll("<redacted>");
        String text = check + "=" + result + " " + d;
        Log.i("SiwcSpike", text);
        Runnable l;
        synchronized (Report.class) {
            LINES.add(text);
            l = listener;
        }
        if (l != null) l.run();
    }

    static synchronized String all() {
        return String.join("\n", LINES);
    }

    static synchronized void setListener(Runnable r) {
        listener = r;
    }
}
