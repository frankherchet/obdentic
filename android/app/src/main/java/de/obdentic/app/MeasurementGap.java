package de.obdentic.app;

import org.json.JSONObject;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * REQ-037: when live polling has produced no usable value for a while. Pure decisions on caller-supplied monotonic
 * milliseconds, so the rules run without an adapter and without waiting. It states what was observed (reads returned no
 * finite value) and nothing about why: no ignition, adapter or vehicle state is inferred here.
 */
public final class MeasurementGap {
    /** The shown headline; the cause stays unknown. */
    public static final String TEXT = "Keine aktuellen Messwerte";
    public static final long START_AFTER_MS = 10_000L, PROBE_EVERY_MS = 5_000L;
    public enum Change { NONE, START, RECOVERED, STOPPED }

    private final Set<String> succeeded = new HashSet<>(); // signals that returned a finite value in this session
    private List<String> eligible = Collections.emptyList(); // the selected signals that are actually read, in selection order
    private boolean active;
    private long windowStart = -1, lastAttemptEnd = -1, lastProbe = -1;
    private final Set<String> failed = new HashSet<>(); // signals attempted without a finite value since the window was last restarted
    private String lastProbed = "";

    public synchronized boolean active() { return active; }

    /** Fresh initialization: nothing carries over from an earlier session. */
    public synchronized void reset() { succeeded.clear(); eligible = Collections.emptyList(); active = false; lastProbe = -1; lastProbed = ""; restart(); }

    /** The failed-read timer starts over: no finite-free stretch of attempted reads is known any more. */
    private void restart() { windowStart = lastAttemptEnd = -1; failed.clear(); }

    /** The longest quiet time between attempts that still counts as continuous reading, from the longest poll period among the read signals. */
    public static long pauseLimitMs(long longestPeriodMs) { return longestPeriodMs + 2_000L; }

    /** A usable measurement: an ok read with a finite number, no error and not marked unavailable. */
    public static boolean finite(JSONObject result) {
        if (result == null || !result.optBoolean("ok")) return false;
        JSONObject data = result.optJSONObject("data");
        return data != null && !data.isNull("value") && Double.isFinite(data.optDouble("value", Double.NaN)) && data.isNull("error") && !"unavailable".equals(data.optString("status"));
    }

    private List<String> candidates() {
        List<String> out = new ArrayList<>();
        for (String key : eligible) if (succeeded.contains(key)) out.add(key);
        return out;
    }

    /**
     * The signals that are currently read (selected and supported). A change of selection breaks a pending window; an open gap
     * with nothing left to probe ends without recovery (STOPPED), because no measurement can ever answer it.
     */
    public synchronized Change select(List<String> signals) {
        if (signals.equals(eligible)) return Change.NONE;
        eligible = new ArrayList<>(signals); restart();
        if (active && candidates().isEmpty()) { active = false; return Change.STOPPED; }
        return Change.NONE;
    }

    /** Reads are not being attempted (a job runs): a pending window starts over; an open gap stays open. */
    public synchronized void interrupt() { restart(); }

    /**
     * One attempted read of a selected signal. A gap starts only when the failure window has lasted START_AFTER_MS and every
     * selected signal that ever returned a finite value has itself been attempted without one since the window started: a slow
     * signal that was simply not due yet keeps the window open without waiting on unsupported or never-successful signals.
     * @param maxPauseMs the longest quiet time between attempts that still counts as continuous reading
     */
    public synchronized Change observe(String key, boolean finite, long startedAt, long endedAt, long maxPauseMs) {
        if (finite) {
            succeeded.add(key); restart(); lastAttemptEnd = endedAt;
            if (!active) return Change.NONE;
            active = false; return Change.RECOVERED;
        }
        if (lastAttemptEnd >= 0 && startedAt - lastAttemptEnd > maxPauseMs) restart(); // reads were not being attempted in between
        lastAttemptEnd = endedAt;
        if (active) return Change.NONE;
        if (windowStart < 0) { if (eligible.contains(key) && succeeded.contains(key)) { windowStart = startedAt; failed.add(key); } return Change.NONE; }
        failed.add(key);
        List<String> known = candidates();
        if (endedAt - windowStart < START_AFTER_MS || known.isEmpty() || !failed.containsAll(known)) return Change.NONE;
        active = true; lastProbe = endedAt; lastProbed = ""; return Change.START;
    }

    /** Ends an open gap without recovery (the observation stopped); false when none was open. Previously successful signals are kept. */
    public synchronized boolean stop() { boolean was = active; active = false; restart(); return was; }

    /** During a gap: the one previously successful, selected signal to read now (at most once per PROBE_EVERY_MS), or null. */
    public synchronized String nextProbe(long now) {
        if (!active || (lastProbe >= 0 && now - lastProbe < PROBE_EVERY_MS)) return null;
        List<String> list = candidates(); if (list.isEmpty()) return null;
        String key = list.get((Math.max(-1, list.indexOf(lastProbed)) + 1) % list.size());
        lastProbe = now; lastProbed = key; return key;
    }

    /** Milliseconds until the next probe is due: 0 when due now, -1 when there is nothing to probe. */
    public synchronized long probeWait(long now) {
        if (!active || candidates().isEmpty()) return -1;
        return lastProbe < 0 ? 0 : Math.max(0, lastProbe + PROBE_EVERY_MS - now);
    }
}
