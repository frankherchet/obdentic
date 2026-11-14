package de.obdentic.app;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;

/** REQ-036 / #297: smart analysis priority state machine on a supplied monotonic clock. Pure logic, no vehicle, no BLE, no threads. */
final class SmartAnalysisAcceptance {
    private static final String REGEN = "dpf.field_regeneration_request_status", COOLANT = "engine.coolant_temperature",
        SPEED = "vehicle.speed", RPM = "engine.rpm", PEDAL = "vehicle.accelerator_pedal_e", LOAD = "engine.load";
    private static final long T = 1_000_000L, SEC = 1000L, MIN = 60_000L;

    static void run() {
        semantics();
        rotation();
        dpf();
        cooling();
        turbo();
        rail();
        egr();
        priorityAndResume();
        missingAndStale();
        sensorGaps();
        availability();
        remaining();
        deterministic();
    }

    private static void semantics() {
        require(Arrays.equals(SmartAnalysis.triggerSemantics(), new String[] {REGEN, COOLANT, SPEED, RPM, PEDAL, LOAD}), "REQ-036 trigger semantics are the six permanent trigger signals: " + Arrays.toString(SmartAnalysis.triggerSemantics()));
        SmartAnalysis.triggerSemantics()[0] = "changed";
        require(REGEN.equals(SmartAnalysis.triggerSemantics()[0]), "REQ-036 trigger semantics cannot be altered by a caller");
    }

    private static void rotation() {
        SmartAnalysis a = new SmartAnalysis(T);
        require("dpf".equals(a.update(T, r())), "REQ-036 rotation starts at dpf");
        require(a.reason().startsWith("rotation:"), "REQ-036 rotation reason: " + a.reason());
        for (String key : SmartAnalysis.triggerSemantics()) require(a.reason().contains(key), "REQ-036 explicit unavailable report names " + key + ": " + a.reason());
        require("dpf".equals(a.update(T + 3 * MIN - 1, r())), "REQ-036 rotation holds a component for three minutes");
        String[] expected = {"cooling", "turbo", "rail", "egr", "dpf", "cooling"};
        for (int i = 0; i < expected.length; i++) require(expected[i].equals(a.update(T + (i + 1) * 3 * MIN, r())), "REQ-036 rotation step " + (i + 1) + " is " + expected[i]);
        SmartAnalysis gap = new SmartAnalysis(T);
        require("rail".equals(gap.update(T + 3 * 3 * MIN + 5, r())), "REQ-036 a long update gap advances several rotation slots");
        require(gap.remainingMs(T) == 3 * MIN - 5, "REQ-036 a backwards clock value is clamped to the last update");
    }

    private static void dpf() {
        SmartAnalysis a = new SmartAnalysis(T);
        require("dpf".equals(a.update(T, r(REGEN, 1.0))), "REQ-036 nonzero DPF request shows dpf");
        require(a.reason().startsWith("dpf:") && a.reason().contains("not proof"), "REQ-036 DPF reason is a request, not proof of regeneration: " + a.reason());
        require("dpf".equals(a.update(T + SEC, r(REGEN, 2.0, PEDAL, 95.0))), "REQ-036 DPF wins over turbo");
        require("dpf".equals(a.update(T + 3 * MIN, r(REGEN, -1.0))), "REQ-036 any nonzero request counts, also negative");
        a.update(T + 3 * MIN + SEC, r(REGEN, 0.0));
        require(!a.reason().startsWith("dpf:"), "REQ-036 request zero ends the dpf trigger: " + a.reason());
        a.update(T + 3 * MIN + 2 * SEC, r(REGEN, 1.0));
        a.update(T + 3 * MIN + 3 * SEC, r());
        require(!a.reason().startsWith("dpf:"), "REQ-036 an unavailable request is not a request: " + a.reason());
    }

    private static void cooling() {
        SmartAnalysis hot = new SmartAnalysis(T);
        hot.update(T, r(COOLANT, 90.0));
        hot.update(T + SEC, r(COOLANT, 91.0));
        require(!kind(hot).equals("cooling"), "REQ-036 first reading above 80 C is no observed upward crossing");

        SmartAnalysis a = new SmartAnalysis(T);
        a.update(T, r(COOLANT, 79.9));
        require(!kind(a).equals("cooling"), "REQ-036 79.9 C does not trigger");
        require("cooling".equals(a.update(T + SEC, r(COOLANT, 80.0))), "REQ-036 crossing upward 80 C shows cooling");
        require(a.reason().startsWith("cooling:"), "REQ-036 cooling reason: " + a.reason());
        a.update(T + 2 * SEC, r(COOLANT, 70.0));
        require(kind(a).equals("cooling"), "REQ-036 cooling is held although coolant fell again");
        a.update(T + SEC + 10 * MIN - 1, r(COOLANT, 70.0));
        require(kind(a).equals("cooling"), "REQ-036 cooling holds ten minutes (just before)");
        a.update(T + SEC + 10 * MIN, r(COOLANT, 70.0));
        require(kind(a).equals("rotation"), "REQ-036 cooling hold ends after ten minutes");

        SmartAnalysis once = new SmartAnalysis(T);
        once.update(T, r(COOLANT, 79.0));
        once.update(T + SEC, r(COOLANT, 85.0));
        once.update(T + SEC + 10 * MIN, r(COOLANT, 86.0));
        require(kind(once).equals("rotation"), "REQ-036 cooling is one-shot while coolant stays above 80 C");
        once.update(T + 11 * MIN, r(COOLANT, 79.0));
        once.update(T + 11 * MIN + SEC, r(COOLANT, 80.0));
        require(kind(once).equals("cooling"), "REQ-036 cooling re-arms after coolant was below 80 C");

        SmartAnalysis gap = new SmartAnalysis(T);
        gap.update(T, r(COOLANT, 79.0));
        gap.update(T + SEC, r());
        gap.update(T + 2 * SEC, r(COOLANT, 85.0));
        require(!kind(gap).equals("cooling"), "REQ-036 a coolant gap is not a proven crossing");
    }

    private static void turbo() {
        SmartAnalysis a = new SmartAnalysis(T);
        a.update(T, r(PEDAL, 80.0));
        require(!kind(a).equals("turbo"), "REQ-036 pedal at exactly 80 % does not trigger");
        require("turbo".equals(a.update(T + SEC, r(PEDAL, 81.0))), "REQ-036 pedal above 80 % shows turbo");
        require(a.reason().startsWith("turbo:"), "REQ-036 turbo reason: " + a.reason());
        a.update(T + 5 * SEC, r(PEDAL, 50.0));
        a.update(T + 24_999, r(PEDAL, 50.0));
        require(kind(a).equals("turbo"), "REQ-036 turbo holds until 20 s after release (just before)");
        a.update(T + 25 * SEC, r(PEDAL, 50.0));
        require(kind(a).equals("rotation"), "REQ-036 turbo ends 20 s after release");

        a.update(T + 24 * SEC, r(PEDAL, 50.0));
        require(kind(a).equals("rotation"), "REQ-036 a backwards clock step does not revive an expired turbo window");

        SmartAnalysis again = new SmartAnalysis(T);
        again.update(T, r(PEDAL, 90.0));
        again.update(T + 5 * SEC, r(PEDAL, 10.0));
        again.update(T + 10 * SEC, r(PEDAL, 90.0));
        again.update(T + 12 * SEC, r(PEDAL, 10.0));
        again.update(T + 31_999, r(PEDAL, 10.0));
        require(kind(again).equals("turbo"), "REQ-036 a renewed press restarts the 20 s release window");
        again.update(T + 32 * SEC, r(PEDAL, 10.0));
        require(kind(again).equals("rotation"), "REQ-036 renewed release window ends 20 s after the last release");

        SmartAnalysis lost = new SmartAnalysis(T);
        lost.update(T, r(PEDAL, 90.0));
        lost.update(T + SEC, r());
        require(kind(lost).equals("rotation"), "REQ-036 a lost pedal reading is unknown, not a release: no turbo, no release window: " + lost.reason());
        lost.update(T + 5 * SEC, r(PEDAL, 10.0));
        require(kind(lost).equals("rotation"), "REQ-036 a fresh low pedal after a gap starts no release window (the press was never seen released)");
        lost.update(T + 6 * SEC, r(PEDAL, 90.0));
        require(kind(lost).equals("turbo"), "REQ-036 turbo re-arms on a fresh pedal above 80 %");

        SmartAnalysis known = new SmartAnalysis(T);
        known.update(T, r(PEDAL, 90.0));
        known.update(T + 5 * SEC, r(PEDAL, 10.0));
        known.update(T + 10 * SEC, r());
        require(kind(known).equals("turbo"), "REQ-036 an already started release window survives a pedal gap");
        known.update(T + 25 * SEC - 1, r());
        require(kind(known).equals("turbo"), "REQ-036 the known release window still ends only 20 s after the observed release");
        known.update(T + 25 * SEC, r());
        require(kind(known).equals("rotation"), "REQ-036 the known release window ends on time despite the gap");
    }

    private static void rail() {
        SmartAnalysis a = new SmartAnalysis(T);
        require("rail".equals(a.update(T, r(SPEED, 0.0, COOLANT, 85.0, RPM, 800.0))), "REQ-036 standstill, warm, idle rpm shows rail");
        require(a.reason().startsWith("rail:"), "REQ-036 rail reason: " + a.reason());
        a.update(T + SEC, r(SPEED, 10.0, COOLANT, 85.0, RPM, 800.0));
        a.update(T + 3 * MIN - 1, r(SPEED, 10.0, COOLANT, 85.0, RPM, 800.0));
        require(kind(a).equals("rail"), "REQ-036 rail holds three minutes after the condition ended");
        a.update(T + 3 * MIN, r(SPEED, 10.0, COOLANT, 85.0, RPM, 800.0));
        require(kind(a).equals("rotation"), "REQ-036 rail hold ends after three minutes");

        SmartAnalysis once = new SmartAnalysis(T);
        once.update(T, r(SPEED, 0.0, COOLANT, 85.0, RPM, 800.0));
        once.update(T + 3 * MIN, r(SPEED, 0.0, COOLANT, 85.0, RPM, 800.0));
        require(kind(once).equals("rotation"), "REQ-036 rail is one-shot while the condition stays true");
        once.update(T + 3 * MIN + SEC, r(SPEED, 5.0, COOLANT, 85.0, RPM, 800.0));
        once.update(T + 3 * MIN + 2 * SEC, r(SPEED, 0.0, COOLANT, 85.0, RPM, 800.0));
        require(kind(once).equals("rail"), "REQ-036 rail re-arms after the condition was false");

        require(railAt(0.0, 85.0, 500.0), "REQ-036 rpm 500 is inside the rail window");
        require(railAt(0.0, 80.0, 1100.0), "REQ-036 rpm 1100 and coolant 80 C are inside the rail window");
        require(!railAt(0.0, 85.0, 499.0), "REQ-036 rpm 499 is outside");
        require(!railAt(0.0, 85.0, 1101.0), "REQ-036 rpm 1101 is outside");
        require(!railAt(0.0, 79.9, 800.0), "REQ-036 coolant below 80 C is outside");
        require(!railAt(1.0, 85.0, 800.0), "REQ-036 moving vehicle is outside");
        SmartAnalysis noSpeed = new SmartAnalysis(T);
        noSpeed.update(T, r(COOLANT, 85.0, RPM, 800.0));
        require(!kind(noSpeed).equals("rail"), "REQ-036 a missing speed is not standstill");
        require(noSpeed.reason().contains(SPEED), "REQ-036 the missing speed is reported: " + noSpeed.reason());
    }

    private static void egr() {
        SmartAnalysis a = new SmartAnalysis(T);
        for (long t = 0; t <= 300 * SEC; t += 10 * SEC) {
            a.update(T + t, r(SPEED, 60.0, LOAD, 30.0));
            String expected = t >= 30 * SEC && t < 210 * SEC ? "egr" : "rotation";
            require(kind(a).equals(expected), "REQ-036 steady EGR: 30 s trigger, 3 min hold, one-shot; at +" + t / SEC + " s got " + a.reason());
        }
        require(a.reason().startsWith("rotation:") && !a.reason().contains(SPEED), "REQ-036 reason lists only unavailable signals: " + a.reason());

        SmartAnalysis cut = new SmartAnalysis(T);
        cut.update(T, r(SPEED, 60.0, LOAD, 30.0));
        cut.update(T + 20 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        cut.update(T + 25 * SEC, r(SPEED, 60.0, LOAD, 70.0));
        cut.update(T + 26 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        cut.update(T + 55_999, r(SPEED, 60.0, LOAD, 30.0));
        require(kind(cut).equals("rotation"), "REQ-036 leaving the steady band restarts the 30 s");
        cut.update(T + 56 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        require(kind(cut).equals("egr"), "REQ-036 EGR after 30 s of steady driving");

        SmartAnalysis rearm = new SmartAnalysis(T);
        rearm.update(T, r(SPEED, 60.0, LOAD, 30.0));
        rearm.update(T + 30 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        rearm.update(T + 40 * SEC, r(SPEED, 90.0, LOAD, 30.0));
        rearm.update(T + 50 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        rearm.update(T + 80 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        rearm.update(T + 30 * SEC + 3 * MIN, r(SPEED, 60.0, LOAD, 30.0));
        require(kind(rearm).equals("egr"), "REQ-036 EGR re-arms after the condition was false");
        rearm.update(T + 80 * SEC + 3 * MIN - 1, r(SPEED, 60.0, LOAD, 30.0));
        require(kind(rearm).equals("egr"), "REQ-036 re-armed EGR holds a fresh 3 min from its new trigger");
        rearm.update(T + 80 * SEC + 3 * MIN, r(SPEED, 60.0, LOAD, 30.0));
        require(kind(rearm).equals("rotation"), "REQ-036 re-armed EGR hold ends 3 min after the new trigger");

        require(egrAfter30s(50.0, 15.0) && egrAfter30s(80.0, 60.0), "REQ-036 band edges 50-80 km/h and 15-60 % load are inside");
        require(!egrAfter30s(49.9, 30.0) && !egrAfter30s(80.1, 30.0), "REQ-036 speed outside 50-80 km/h");
        require(!egrAfter30s(60.0, 14.9) && !egrAfter30s(60.0, 60.1), "REQ-036 load outside 15-60 %");
    }

    private static void priorityAndResume() {
        SmartAnalysis a = new SmartAnalysis(T);
        a.update(T, r(COOLANT, 79.0));
        require("cooling".equals(a.update(T + SEC, r(COOLANT, 85.0))), "REQ-036 cooling shown");
        require("cooling".equals(a.update(T + 2 * SEC, r(COOLANT, 85.0, PEDAL, 90.0))), "REQ-036 cooling outranks turbo");
        require("dpf".equals(a.update(T + 3 * SEC, r(COOLANT, 85.0, PEDAL, 90.0, REGEN, 2.0))), "REQ-036 dpf outranks cooling and turbo");
        require("cooling".equals(a.update(T + 4 * SEC, r(COOLANT, 85.0, PEDAL, 10.0))), "REQ-036 cooling resumes while its window is active");
        require(a.reason().startsWith("cooling:"), "REQ-036 resumed cooling is a trigger, not rotation: " + a.reason());

        SmartAnalysis b = new SmartAnalysis(T);
        b.update(T, r(SPEED, 60.0, LOAD, 30.0));
        require("egr".equals(b.update(T + 30 * SEC, r(SPEED, 60.0, LOAD, 30.0))), "REQ-036 egr shown after steady driving");
        require("turbo".equals(b.update(T + 40 * SEC, r(SPEED, 60.0, LOAD, 30.0, PEDAL, 90.0))), "REQ-036 turbo preempts egr");
        require("turbo".equals(b.update(T + 50 * SEC, r(SPEED, 60.0, LOAD, 30.0, PEDAL, 0.0))), "REQ-036 turbo holds after release");
        b.update(T + 69_999, r(SPEED, 60.0, LOAD, 30.0, PEDAL, 0.0));
        require(kind(b).equals("turbo"), "REQ-036 turbo still holds just before 20 s");
        b.update(T + 70 * SEC, r(SPEED, 60.0, LOAD, 30.0, PEDAL, 0.0));
        require(kind(b).equals("egr"), "REQ-036 egr resumes after the turbo window while its own window is active");
        b.update(T + 80 * SEC, r(SPEED, 0.0, COOLANT, 85.0, RPM, 800.0));
        require(kind(b).equals("rail"), "REQ-036 rail outranks the still active egr window");
        b.update(T + 209_999, r(SPEED, 0.0, COOLANT, 85.0, RPM, 800.0));
        require(kind(b).equals("rail"), "REQ-036 rail holds");
        b.update(T + 80 * SEC + 3 * MIN - 1, r(SPEED, 20.0));
        require(kind(b).equals("rail"), "REQ-036 rail window runs its three minutes");
        require("rail".equals(b.update(T + 80 * SEC + 3 * MIN, r(SPEED, 20.0))), "REQ-036 rotation continues from the component shown last");
        require(kind(b).equals("rotation"), "REQ-036 egr window already ended, nothing resumes");
        require("rail".equals(b.update(T + 80 * SEC + 6 * MIN - 1, r())), "REQ-036 last component stays for a full rotation slot");
        require("egr".equals(b.update(T + 80 * SEC + 6 * MIN, r())), "REQ-036 rotation then advances in the fixed order");

        SmartAnalysis tie = new SmartAnalysis(T);
        tie.update(T, r(COOLANT, 79.0, SPEED, 0.0, RPM, 800.0, PEDAL, 10.0));
        require("cooling".equals(tie.update(T + SEC, r(COOLANT, 85.0, SPEED, 0.0, RPM, 800.0, PEDAL, 10.0))), "REQ-036 simultaneous cooling+rail picks cooling");
        require("cooling".equals(tie.update(T + 2 * SEC, r(COOLANT, 85.0, SPEED, 0.0, RPM, 800.0, PEDAL, 95.0))), "REQ-036 cooling outranks simultaneous turbo and rail");
        tie.update(T + 3 * SEC, r(COOLANT, 85.0, SPEED, 0.0, RPM, 800.0, PEDAL, 95.0, REGEN, 1.0));
        require(tie.reason().startsWith("dpf:"), "REQ-036 dpf outranks all four others: " + tie.reason());
    }

    private static void missingAndStale() {
        SmartAnalysis a = new SmartAnalysis(T);
        a.update(T, r(SPEED, 60.0, LOAD, 30.0));
        a.update(T + 20 * SEC, r());
        a.update(T + 30 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        a.update(T + 59_999, r(SPEED, 60.0, LOAD, 30.0));
        require(kind(a).equals("rotation"), "REQ-036 an update without readings restarts the EGR steady time");
        a.update(T + 60 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        require(kind(a).equals("egr"), "REQ-036 EGR after a fresh 30 s of readings");

        SmartAnalysis nan = new SmartAnalysis(T);
        nan.update(T, r(PEDAL, Double.NaN, REGEN, Double.POSITIVE_INFINITY));
        require(kind(nan).equals("rotation"), "REQ-036 NaN or infinite readings are unavailable, not triggers");
        require(nan.reason().contains(PEDAL) && nan.reason().contains(REGEN), "REQ-036 non-finite readings are reported unavailable: " + nan.reason());
        require("dpf".equals(new SmartAnalysis(T).update(T, null)), "REQ-036 null readings mean everything unavailable");
    }

    private static void deterministic() {
        List<String> first = script(), second = script();
        require(first.equals(second), "REQ-036 identical input gives identical output: " + first + " vs " + second);
        require(first.contains("dpf") && first.contains("cooling") && first.contains("turbo") && first.contains("rail") && first.contains("egr"), "REQ-036 the script exercised all components: " + first);
    }

    private static List<String> script() {
        SmartAnalysis a = new SmartAnalysis(T);
        List<String> out = new ArrayList<>();
        out.add(a.update(T, r(COOLANT, 70.0)));
        out.add(a.update(T + SEC, r(COOLANT, 82.0, PEDAL, 90.0)));
        out.add(a.update(T + 2 * SEC, r(COOLANT, 82.0, PEDAL, 90.0, REGEN, 1.0)));
        out.add(a.update(T + 5 * MIN, r(COOLANT, 82.0, PEDAL, 10.0)));
        out.add(a.update(T + 11 * MIN, r(COOLANT, 82.0, PEDAL, 10.0, SPEED, 0.0, RPM, 900.0)));
        out.add(a.update(T + 15 * MIN, r(SPEED, 60.0, LOAD, 30.0)));
        out.add(a.update(T + 15 * MIN + 30 * SEC, r(SPEED, 60.0, LOAD, 30.0)));
        out.add(a.update(T + 20 * MIN, r()));
        out.add(a.update(T + 25 * MIN, r(PEDAL, 95.0)));
        return out;
    }

    private static void sensorGaps() {
        SmartAnalysis rail = new SmartAnalysis(T);
        rail.update(T, railReadings());
        rail.update(T + 3 * MIN + SEC, r());
        rail.update(T + 3 * MIN + 2 * SEC, r(SPEED, 0.0));
        rail.update(T + 3 * MIN + 3 * SEC, railReadings());
        require(kind(rail).equals("rotation"), "REQ-036 a stale gap does not re-arm the one-shot rail trigger: " + rail.reason());
        rail.update(T + 3 * MIN + 4 * SEC, r(SPEED, 5.0));
        rail.update(T + 3 * MIN + 5 * SEC, railReadings());
        require(kind(rail).equals("rotation"), "REQ-036 a partly known false condition (speed only) does not re-arm rail: " + rail.reason());
        rail.update(T + 3 * MIN + 6 * SEC, r(SPEED, 5.0, COOLANT, 85.0, RPM, 800.0));
        rail.update(T + 3 * MIN + 7 * SEC, railReadings());
        require(kind(rail).equals("rail"), "REQ-036 a fully known false condition re-arms rail: " + rail.reason());

        SmartAnalysis egr = new SmartAnalysis(T);
        egr.update(T, r(SPEED, 60.0, LOAD, 30.0));
        egr.update(T + 30 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        require(kind(egr).equals("egr"), "REQ-036 egr fired for the gap case");
        egr.update(T + 215 * SEC, r());
        egr.update(T + 220 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        egr.update(T + 250 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        require(kind(egr).equals("rotation"), "REQ-036 a stale gap resets the steady time but does not re-arm a fired egr trigger: " + egr.reason());
        egr.update(T + 251 * SEC, r(SPEED, 90.0, LOAD, 30.0));
        egr.update(T + 252 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        egr.update(T + 282 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        require(kind(egr).equals("egr"), "REQ-036 only a fresh out-of-range reading re-arms egr: " + egr.reason());

        SmartAnalysis partial = new SmartAnalysis(T);
        partial.update(T, r(SPEED, 60.0));
        partial.update(T + 30 * SEC, r(SPEED, 60.0));
        require(kind(partial).equals("rotation"), "REQ-036 an unavailable load is never read as steady");
    }

    private static void availability() {
        SmartAnalysis none = new SmartAnalysis(T);
        none.setAvailable(ids());
        require("".equals(none.update(T, r(REGEN, 1.0))), "REQ-036 no available component returns the empty string");
        require(none.reason().startsWith("unavailable:"), "REQ-036 explicit reason when nothing is available: " + none.reason());
        require(none.remainingMs(T) == 0, "REQ-036 no remaining time without an available component");
        none.setAvailable(null);
        require("".equals(none.update(T + SEC, r())), "REQ-036 a null set means none available");
        none.setAvailable(ids("rail"));
        require("rail".equals(none.update(T + 2 * SEC, r(REGEN, 1.0))), "REQ-036 the unavailable dpf trigger is ignored, only rail remains");
        require(kind(none).equals("rotation") && none.remainingMs(T + 2 * SEC) == 3 * MIN, "REQ-036 rotation slot restarts when components return: " + none.reason());

        SmartAnalysis one = new SmartAnalysis(T);
        one.setAvailable(ids("rail"));
        require("rail".equals(one.update(T, r())), "REQ-036 a single available component is always shown");
        require(one.remainingMs(T) == 3 * MIN && one.remainingMs(T + SEC) == 3 * MIN - SEC, "REQ-036 rotation remaining time counts down");
        require("rail".equals(one.update(T + 3 * MIN, r())) && one.remainingMs(T + 3 * MIN) == 3 * MIN, "REQ-036 a single component keeps its slot restarting");
        require("rail".equals(one.update(T + 7 * MIN, r(PEDAL, 90.0))) && kind(one).equals("rotation"), "REQ-036 an unavailable turbo trigger never shows turbo");

        SmartAnalysis skip = new SmartAnalysis(T);
        skip.setAvailable(ids("cooling", "egr"));
        require("cooling".equals(skip.update(T, r())), "REQ-036 rotation skips the unavailable dpf at the start");
        require("cooling".equals(skip.update(T + 3 * MIN - 1, r())), "REQ-036 slot holds three minutes");
        require("egr".equals(skip.update(T + 3 * MIN, r())), "REQ-036 rotation skips unavailable turbo and rail");
        require("cooling".equals(skip.update(T + 6 * MIN, r())), "REQ-036 rotation wraps over unavailable ids");
        require("egr".equals(skip.update(T + 9 * MIN, r())), "REQ-036 rotation keeps alternating");

        SmartAnalysis lower = new SmartAnalysis(T);
        lower.setAvailable(ids("rail", "egr"));
        lower.update(T, r(COOLANT, 79.0, SPEED, 60.0, LOAD, 30.0));
        lower.update(T + SEC, r(COOLANT, 85.0, SPEED, 60.0, LOAD, 30.0));
        lower.update(T + 30 * SEC, r(COOLANT, 85.0, SPEED, 60.0, LOAD, 30.0));
        require("egr".equals(lower.update(T + 31 * SEC, r(COOLANT, 85.0, SPEED, 60.0, LOAD, 30.0))) && kind(lower).equals("egr"), "REQ-036 an active but unavailable higher trigger does not shadow an available lower one: " + lower.reason());
        require(lower.remainingMs(T + 31 * SEC) == 3 * MIN - SEC, "REQ-036 egr remaining counts from its trigger");

        SmartAnalysis back = new SmartAnalysis(T);
        back.update(T, r(COOLANT, 79.0));
        back.update(T + SEC, r(COOLANT, 85.0));
        back.setAvailable(ids("dpf", "turbo", "rail", "egr"));
        require("turbo".equals(back.update(T + 2 * SEC, r(COOLANT, 85.0))) && kind(back).equals("rotation"), "REQ-036 an unavailable cooling window is skipped: " + back.reason());
        back.setAvailable(ids("dpf", "cooling", "turbo", "rail", "egr"));
        require("cooling".equals(back.update(T + 5 * MIN, r(COOLANT, 85.0))) && kind(back).equals("cooling"), "REQ-036 a returning component resumes while its window still runs: " + back.reason());
        require(back.remainingMs(T + 5 * MIN) == 5 * MIN + SEC, "REQ-036 the window aged while the component was unavailable");

        SmartAnalysis aged = new SmartAnalysis(T);
        aged.update(T, r(COOLANT, 79.0));
        aged.update(T + SEC, r(COOLANT, 85.0));
        aged.setAvailable(ids("dpf", "turbo", "rail", "egr"));
        aged.update(T + 2 * SEC, r(COOLANT, 85.0));
        aged.update(T + 11 * MIN, r(COOLANT, 85.0));
        aged.setAvailable(ids("dpf", "cooling", "turbo", "rail", "egr"));
        require("dpf".equals(aged.update(T + 11 * MIN + SEC, r(COOLANT, 85.0))) && kind(aged).equals("rotation"), "REQ-036 an expired window does not resume when the component returns: " + aged.reason());

        SmartAnalysis change = new SmartAnalysis(T);
        change.setAvailable(ids("dpf", "turbo"));
        require("dpf".equals(change.update(T, r())), "REQ-036 rotation among two available ids starts at dpf");
        require("turbo".equals(change.update(T + 3 * MIN, r())), "REQ-036 rotation skips cooling");
        change.setAvailable(ids("dpf", "cooling", "turbo", "rail", "egr"));
        require("turbo".equals(change.update(T + 3 * MIN + SEC, r())) && change.remainingMs(T + 3 * MIN + SEC) == 3 * MIN - SEC, "REQ-036 a set change does not restart the running slot");
        require("rail".equals(change.update(T + 6 * MIN, r())), "REQ-036 returned components are part of the next rotation step");
        change.setAvailable(ids("dpf", "cooling"));
        require("dpf".equals(change.update(T + 6 * MIN + SEC, r())) && change.remainingMs(T + 6 * MIN + SEC) == 3 * MIN, "REQ-036 losing the shown component moves on and restarts the slot");
    }

    private static void remaining() {
        SmartAnalysis fresh = new SmartAnalysis(T);
        require(fresh.remainingMs(T) == 3 * MIN, "REQ-036 fresh rotation remaining is one slot");

        SmartAnalysis dpf = new SmartAnalysis(T);
        dpf.update(T, r(REGEN, 1.0));
        require(dpf.remainingMs(T + 5 * MIN) == -1, "REQ-036 dpf has no bounded remaining time");

        SmartAnalysis cooling = new SmartAnalysis(T);
        cooling.update(T, r(COOLANT, 79.0));
        cooling.update(T + SEC, r(COOLANT, 85.0));
        require(cooling.remainingMs(T + SEC) == 10 * MIN && cooling.remainingMs(T + 2 * SEC) == 10 * MIN - SEC, "REQ-036 cooling remaining");
        require(cooling.remainingMs(T + 11 * MIN) == 0, "REQ-036 remaining never goes negative");
        require(cooling.remainingMs(T) == 10 * MIN, "REQ-036 an earlier clock value is clamped to the last update");

        SmartAnalysis turbo = new SmartAnalysis(T);
        turbo.update(T, r(PEDAL, 90.0));
        require(turbo.remainingMs(T + 5 * SEC) == 20 * SEC, "REQ-036 held pedal reports the release window");
        turbo.update(T + 5 * SEC, r(PEDAL, 10.0));
        require(turbo.remainingMs(T + 10 * SEC) == 15 * SEC && turbo.remainingMs(T + 60 * SEC) == 0, "REQ-036 turbo release remaining");

        SmartAnalysis rail = new SmartAnalysis(T);
        rail.update(T, railReadings());
        require(rail.remainingMs(T + SEC) == 3 * MIN - SEC, "REQ-036 rail remaining");

        SmartAnalysis egr = new SmartAnalysis(T);
        egr.update(T, r(SPEED, 60.0, LOAD, 30.0));
        egr.update(T + 30 * SEC, r(SPEED, 60.0, LOAD, 30.0));
        require(egr.remainingMs(T + 30 * SEC) == 3 * MIN && egr.remainingMs(T + 31 * SEC) == 3 * MIN - SEC, "REQ-036 egr remaining");
    }

    private static Map<String, Double> railReadings() { return r(SPEED, 0.0, COOLANT, 85.0, RPM, 800.0); }

    private static java.util.Set<String> ids(String... values) { return new HashSet<>(Arrays.asList(values)); }

    private static String kind(SmartAnalysis a) { String reason = a.reason(); return reason.substring(0, reason.indexOf(':')); }

    private static boolean railAt(double speed, double coolant, double rpm) {
        SmartAnalysis a = new SmartAnalysis(T);
        a.update(T, r(SPEED, speed, COOLANT, coolant, RPM, rpm));
        return kind(a).equals("rail");
    }

    private static boolean egrAfter30s(double speed, double load) {
        SmartAnalysis a = new SmartAnalysis(T);
        a.update(T, r(SPEED, speed, LOAD, load));
        a.update(T + 30 * SEC, r(SPEED, speed, LOAD, load));
        return kind(a).equals("egr");
    }

    private static Map<String, Double> r(Object... keyValues) {
        Map<String, Double> map = new HashMap<>();
        for (int i = 0; i < keyValues.length; i += 2) map.put((String) keyValues[i], (Double) keyValues[i + 1]);
        return map;
    }

    private static void require(boolean value, String message) { if (!value) throw new AssertionError(message); }
}
