package de.obdentic.app;

import java.util.List;
import java.util.Map;

/** #231: pure scheduling decisions for live polling; reads run back-to-back, each signal at its own period. */
public final class PollSchedule {
    public static final long DEFAULT_PERIOD_MS = 500, MIN_PERIOD_MS = 250;
    private PollSchedule() {}

    /** Period for a signal: the profile interval when one is defined, else the live default, never below the floor. */
    public static long period(Long profileInterval) { return Math.max(MIN_PERIOD_MS, profileInterval == null ? DEFAULT_PERIOD_MS : profileInterval); }

    /** The signal that has been due longest, or null when none is due at {@code now}. */
    public static String next(List<String> eligible, Map<String, Long> due, long now) {
        String best = null; long earliest = Long.MAX_VALUE;
        for (String candidate : eligible) { long at = due.getOrDefault(candidate, 0L); if (at <= now && at < earliest) { best = candidate; earliest = at; } }
        return best;
    }

    /** Milliseconds until the next signal is due: 0 when one is due now, -1 when there is nothing to poll. */
    public static long waitMs(List<String> eligible, Map<String, Long> due, long now) {
        long wait = -1;
        for (String candidate : eligible) { long left = Math.max(0, due.getOrDefault(candidate, 0L) - now); if (wait < 0 || left < wait) wait = left; }
        return wait;
    }
}
