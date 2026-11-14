package de.obdentic.app;

import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import java.util.Map;

/**
 * REQ-036 / #297: deterministic smart analysis. Picks which predefined component preset (dpf, cooling, turbo, rail, egr)
 * the default session should show, from permanent trigger signals alone. No threads, no wall clock: the caller supplies a
 * monotonic millisecond clock and the latest fresh readings keyed by semantic name. Readings that are missing or stale are
 * left out by the caller; they are never read as zero here and are reported as unavailable in {@link #reason()}.
 *
 * Priority DPF > cooling > turbo > rail > EGR. Edge/one-shot triggers (cooling crossing, rail, EGR) re-arm only after a
 * fresh reading proved their condition false; a gap in the readings is unknown and never re-arms. A lower-priority trigger
 * that was preempted resumes only while its own condition/window is still active. With no trigger active the available
 * component ids rotate every three minutes, continuing from the id shown last. Components without signals in the active
 * catalog ({@link #setAvailable}) are skipped by triggers and rotation, but their windows keep aging.
 */
public final class SmartAnalysis {
    static final String DPF = "dpf", COOLING = "cooling", TURBO = "turbo", RAIL = "rail", EGR = "egr";
    private static final String[] ORDER = {DPF, COOLING, TURBO, RAIL, EGR};
    private static final String REGEN = "dpf.field_regeneration_request_status", COOLANT = "engine.coolant_temperature",
        SPEED = "vehicle.speed", RPM = "engine.rpm", PEDAL = "vehicle.accelerator_pedal_e", LOAD = "engine.load";
    private static final String[] SEMANTICS = {REGEN, COOLANT, SPEED, RPM, PEDAL, LOAD};

    static final double COOLANT_C = 80, PEDAL_PERCENT = 80, RAIL_RPM_MIN = 500, RAIL_RPM_MAX = 1100,
        EGR_SPEED_MIN = 50, EGR_SPEED_MAX = 80, EGR_LOAD_MIN = 15, EGR_LOAD_MAX = 60;
    static final long COOLING_HOLD_MS = 10 * 60_000L, RAIL_HOLD_MS = 3 * 60_000L, TURBO_RELEASE_MS = 20_000L,
        EGR_STEADY_MS = 30_000L, EGR_HOLD_MS = 3 * 60_000L, ROTATION_MS = 3 * 60_000L;

    private long last;
    private boolean coolingArmed; // a fresh coolant reading below 80 C was the last coolant observation
    private long coolingUntil = Long.MIN_VALUE;
    private boolean railWasTrue;
    private long railUntil = Long.MIN_VALUE;
    private boolean turboHigh;
    private long turboUntil = Long.MIN_VALUE;
    private long egrSince = -1;
    private boolean egrFired;
    private long egrUntil = Long.MIN_VALUE;
    private boolean idle = true;
    private int rotation; // index into ORDER of the id shown last / in the current rotation slot
    private long slotStart;
    private String reason;
    private final boolean[] available = {true, true, true, true, true}; // all components until the caller says otherwise
    private static final int SHOWN_ROTATION = -1, SHOWN_NONE = -2;
    private int shown = SHOWN_ROTATION; // which source the last update returned: ORDER index of a trigger, or rotation, or none

    public SmartAnalysis(long nowMs) {
        last = nowMs;
        slotStart = nowMs;
        reason = "rotation: no trigger active, showing " + ORDER[0] + " (" + ROTATION_MS / 60_000 + " min per component)";
    }

    /** Semantic names the triggers read, in a fixed order; the caller must keep these signals permanently polled. */
    public static String[] triggerSemantics() { return SEMANTICS.clone(); }

    /**
     * Declares which component ids have at least one non-trigger signal in the active catalog; only the caller knows this and
     * must never invent a set. Unknown names are ignored, {@code null} means none. Until it is called every component counts as
     * available. Takes effect at the next {@link #update}: unavailable ids are neither picked by a trigger nor rotated to, while
     * their trigger windows keep aging, so an id that returns inside its window shows again.
     */
    public void setAvailable(Set<String> ids) {
        for (int i = 0; i < ORDER.length; i++) available[i] = ids != null && ids.contains(ORDER[i]);
    }

    /**
     * Component id to show now, or the empty string when no component is available. {@code readings} holds only fresh values by
     * semantic name; absent means unavailable, which is unknown: never zero, never a release or a proven false condition.
     */
    public String update(long nowMs, Map<String, Double> readings) {
        long now = Math.max(nowMs, last); // a monotonic clock never goes back; tolerate it instead of failing
        last = now;
        Double regen = value(readings, REGEN), coolant = value(readings, COOLANT), speed = value(readings, SPEED),
            rpm = value(readings, RPM), pedal = value(readings, PEDAL), load = value(readings, LOAD);

        if (coolant == null) coolingArmed = false; // continuity lost: a later hot reading is not a proven crossing
        else if (coolant < COOLANT_C) coolingArmed = true;
        else if (coolingArmed) { coolingArmed = false; coolingUntil = now + COOLING_HOLD_MS; }

        if (speed != null && coolant != null && rpm != null) { // edge memory changes only on a fully known condition; a gap neither triggers nor re-arms
            boolean rail = speed <= 0 && coolant >= COOLANT_C && rpm >= RAIL_RPM_MIN && rpm <= RAIL_RPM_MAX;
            if (rail && !railWasTrue) railUntil = now + RAIL_HOLD_MS;
            railWasTrue = rail;
        }

        boolean high = pedal != null && pedal > PEDAL_PERCENT;
        if (high) turboHigh = true;
        else if (pedal == null) turboHigh = false; // unknown is not a release: no release window starts, an already known one stays; a fresh press re-arms
        else if (turboHigh) { turboHigh = false; turboUntil = now + TURBO_RELEASE_MS; } // fresh reading at or below 80 %: release

        // unknown speed/load restarts the steady time (steadiness is unproven) but never re-arms a fired trigger; only a fresh out-of-range reading does
        boolean egrFalse = (speed != null && (speed < EGR_SPEED_MIN || speed > EGR_SPEED_MAX))
            || (load != null && (load < EGR_LOAD_MIN || load > EGR_LOAD_MAX));
        if (egrFalse) { egrSince = -1; egrFired = false; }
        else if (speed == null || load == null) egrSince = -1;
        else {
            if (egrSince < 0) egrSince = now;
            if (!egrFired && now - egrSince >= EGR_STEADY_MS) { egrFired = true; egrUntil = now + EGR_HOLD_MS; }
        }

        boolean[] active = {regen != null && regen != 0, now < coolingUntil, turboHigh || now < turboUntil, now < railUntil, now < egrUntil};
        int pick = -1, count = 0;
        for (int i = 0; i < ORDER.length; i++) if (available[i]) { count++; if (pick < 0 && active[i]) pick = i; }

        String why;
        if (pick >= 0) {
            idle = false; rotation = pick; shown = pick; why = triggerReason(pick);
        } else if (count == 0) {
            idle = false; shown = SHOWN_NONE; why = "unavailable: no component has signals in the active catalog";
        } else {
            if (!idle || !available[rotation]) { idle = true; slotStart = now; rotation = nextAvailable(rotation); } // continue on the id shown last, or the next one with signals
            long steps = (now - slotStart) / ROTATION_MS;
            if (steps > 0) {
                for (long s = steps % count; s > 0; s--) rotation = nextAvailable(rotation + 1);
                slotStart += steps * ROTATION_MS;
            }
            shown = SHOWN_ROTATION;
            why = "rotation: no trigger active, showing " + ORDER[rotation] + " (" + ROTATION_MS / 60_000 + " min per component, components without signals skipped)";
        }

        List<String> missing = new ArrayList<>();
        for (String key : SEMANTICS) if (value(readings, key) == null) missing.add(key);
        reason = missing.isEmpty() ? why : why + "; unavailable trigger signals: " + String.join(", ", missing);
        return shown == SHOWN_NONE ? "" : ORDER[rotation];
    }

    /**
     * Milliseconds until the result of the last {@link #update} is due to change by time alone, never negative: the rest of the
     * trigger hold or release window, or of the rotation slot. -1 while dpf is shown (it lasts as long as the request is nonzero);
     * 0 when no component is available. While the accelerator is held above 80 % the answer is the 20 s release window that
     * would start on release.
     */
    public long remainingMs(long nowMs) {
        long now = Math.max(nowMs, last);
        long until;
        switch (shown) {
            case SHOWN_NONE: return 0;
            case SHOWN_ROTATION: until = slotStart + ROTATION_MS; break;
            case 0: return -1;
            case 1: until = coolingUntil; break;
            case 2: if (turboHigh) return TURBO_RELEASE_MS; until = turboUntil; break;
            case 3: until = railUntil; break;
            default: until = egrUntil; break;
        }
        return Math.max(0, until - now);
    }

    private int nextAvailable(int from) {
        for (int k = 0; k < ORDER.length; k++) { int i = (from + k) % ORDER.length; if (available[i]) return i; }
        return rotation;
    }

    private static String triggerReason(int i) {
        switch (i) {
            case 0: return "dpf: regeneration request status is nonzero (a request only, not proof that a regeneration is running)";
            case 1: return "cooling: coolant crossed " + (int) COOLANT_C + " C upward, held " + COOLING_HOLD_MS / 60_000 + " min";
            case 2: return "turbo: accelerator pedal above " + (int) PEDAL_PERCENT + " % or released less than " + TURBO_RELEASE_MS / 1000 + " s ago";
            case 3: return "rail: standstill, coolant at least " + (int) COOLANT_C + " C, idle rpm " + (int) RAIL_RPM_MIN + "-" + (int) RAIL_RPM_MAX + ", held " + RAIL_HOLD_MS / 60_000 + " min";
            default: return "egr: steady " + (int) EGR_SPEED_MIN + "-" + (int) EGR_SPEED_MAX + " km/h at " + (int) EGR_LOAD_MIN + "-" + (int) EGR_LOAD_MAX + " % load for " + EGR_STEADY_MS / 1000 + " s, held " + EGR_HOLD_MS / 60_000 + " min";
        }
    }

    /** Human-readable explanation of the last {@link #update} result, including trigger signals that were unavailable. */
    public String reason() { return reason; }

    private static Double value(Map<String, Double> readings, String key) {
        Double v = readings == null ? null : readings.get(key);
        return v == null || v.isNaN() || v.isInfinite() ? null : v;
    }
}
