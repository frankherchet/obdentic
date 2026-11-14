package de.obdentic.app;

import android.content.Context;
import android.os.SystemClock;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

/**
 * REQ-037.1-.3 at the phone seam: the real service, the real Rust core and a simulated adapter whose answers the test steers
 * (synthetic data only, no BLE, no vehicle). Part one decides the gap rules on a supplied clock (exact 10 s / 5 s boundaries, no waiting).
 * Part two runs the real service in real time against the fake link: values, unsupported and never-successful signals, a sustained silence,
 * the probe cadence seen on the wire and in the recording, recovery, unknown trigger values, the recording across a forced rotation and
 * a stop, and offline replay without any adapter traffic. The fake link is told nothing about ignition: it only answers or stays silent.
 */
final class MeasurementGapAcceptance {
    private static final String ADAPTER = "AA:BB:CC:00:00:37", TEXT = "Keine aktuellen Messwerte";
    private static final String RPM = "engine.rpm", COOLANT = "engine.coolant_temperature", IAT = "engine.intake_air_temperature", MAP = "engine.intake_manifold_pressure";
    private static final String RPM_PID = "010C", COOLANT_PID = "0105", IAT_PID = "010F", MAP_PID = "010B";
    private static final Set<String> SUPPORT_PIDS = new HashSet<>(Arrays.asList("00", "20", "40", "60", "80", "A0"));

    /** What the Live tab shows: its visible texts after a short wait for the refresh. */
    interface Ui { String text(String tab) throws Exception; }

    // ---- part one: the rules on a supplied clock -------------------------------------------------------------------------------------

    static void rules() throws Exception {
        MeasurementGap.Change none = MeasurementGap.Change.NONE;
        // REQ-037.1: unsupported, unselected and never-successful signals alone cannot start a gap, however long they stay silent
        MeasurementGap g = new MeasurementGap(); g.select(Arrays.asList(RPM, IAT));
        for (long t = 0; t < 120_000; t += 500) require(g.observe(IAT, false, t, t + 100, 1000) == none && g.observe(RPM, false, t + 200, t + 300, 1000) == none, "REQ-037.1 signals that never produced a value cannot start a gap (t=" + t + ")");
        require(!g.active() && g.nextProbe(200_000) == null && g.probeWait(200_000) == -1, "REQ-037.1/.2 no gap, nothing to probe");
        // a signal that produced a value that is not selected any more cannot start it either
        g = new MeasurementGap(); g.select(Arrays.asList(RPM, IAT));
        require(g.observe(RPM, true, 0, 100, 1000) == none, "a finite value alone is no event");
        g.select(Arrays.asList(IAT));
        for (long t = 500; t < 60_000; t += 500) require(g.observe(IAT, false, t, t + 100, 1000) == none, "REQ-037.1 a previously successful signal that is no longer selected cannot start a gap");
        require(!g.active(), "no gap without a selected, previously successful signal");

        // REQ-037.1: exactly 10 s of attempted reads without a finite value
        g = new MeasurementGap(); g.select(Arrays.asList(RPM, COOLANT, IAT));
        require(g.observe(RPM, true, 0, 100, 1000) == none, "first value");
        require(g.observe(RPM, false, 500, 600, 1000) == none, "the first miss opens the window");
        for (long t = 1000; t <= 9500; t += 500) require(g.observe(RPM, false, t, t + 100, 1000) == none, "no gap before 10 s (t=" + t + ")");
        require(g.observe(RPM, false, 9600, 10_499, 1000) == none && !g.active(), "9.999 s of misses is not yet a gap");
        require(g.observe(RPM, false, 10_499, 10_500, 1000) == MeasurementGap.Change.START && g.active(), "10.000 s of misses starts the gap");
        require(g.observe(RPM, false, 10_600, 10_700, 1000) == none && g.observe(IAT, false, 10_800, 10_900, 1000) == none, "misses during a gap change nothing");
        // REQ-037.2: one probe per 5 s of a signal that returned a value, never a never-successful one
        require(g.nextProbe(10_500 + 4999) == null, "no probe before 5 s");
        require(RPM.equals(g.nextProbe(15_500)), "the previously successful signal is probed after 5 s");
        require(g.nextProbe(15_501) == null && g.nextProbe(20_499) == null, "at most one probe per 5 s");
        require(g.probeWait(15_500) == 5000 && g.probeWait(17_500) == 3000, "the time to the next probe is reported");
        require(RPM.equals(g.nextProbe(20_500)), "next probe 5 s later");
        // a miss of another signal does not recover; the first finite value of any selected signal does, once
        require(g.observe(IAT, false, 20_600, 20_700, 1000) == none && g.active(), "a never-successful signal's miss changes nothing");
        require(g.observe(IAT, true, 20_800, 20_900, 1000) == MeasurementGap.Change.RECOVERED && !g.active(), "the first finite value of any selected signal ends the gap");
        require(g.observe(RPM, true, 21_000, 21_100, 1000) == none && g.nextProbe(40_000) == null && g.probeWait(40_000) == -1, "recovery is reported once and probing ends");

        // an individual error next to successful measurements never starts a gap
        g = new MeasurementGap(); g.select(Arrays.asList(RPM, COOLANT)); g.observe(COOLANT, true, 0, 100, 1000);
        for (long t = 200; t < 120_000; t += 500) { g.observe(RPM, false, t, t + 50, 1000); require(g.observe(COOLANT, true, t + 100, t + 150, 1000) == none, "success"); }
        require(!g.active(), "REQ-037.1 errors of one signal alongside successful measurements do not start a gap");
        // a success in the middle of a window restarts the 10 s
        g = new MeasurementGap(); g.select(Arrays.asList(RPM, IAT)); g.observe(RPM, true, 0, 100, 1000);
        for (long t = 500; t < 9000; t += 500) g.observe(RPM, false, t, t + 100, 1000);
        require(g.observe(IAT, true, 9000, 9100, 1000) == none, "a success of a signal that was never successful also counts");
        for (long t = 9500; t < 19_400; t += 500) { require(g.observe(RPM, false, t, t + 100, 1000) == none, "the window restarted at the success (t=" + t + ")"); g.observe(IAT, false, t + 150, t + 160, 1000); }
        require(g.observe(RPM, false, 19_400, 19_499, 1000) == none && g.observe(RPM, false, 19_499, 19_500, 1000) == MeasurementGap.Change.START, "10 s after the new window opened at 9.5 s");
        // reads that are not attempted do not count as misses
        g = new MeasurementGap(); g.select(Arrays.asList(RPM)); g.observe(RPM, true, 0, 100, 1000); g.observe(RPM, false, 500, 600, 1000);
        require(g.observe(RPM, false, 3000, 3100, 1000) == none, "a 2.4 s pause without any read breaks the window");
        for (long t = 3500; t <= 12_500; t += 500) require(g.observe(RPM, false, t, t + 100, 1000) == none, "REQ-037.1 the window restarted after the pause, no gap at t=" + t);
        require(g.observe(RPM, false, 12_900, 13_000, 1000) == MeasurementGap.Change.START, "10 s after the reads resumed");
        // probes alternate between the signals that returned a value, in selection order
        g = new MeasurementGap(); g.select(Arrays.asList(RPM, COOLANT, IAT)); g.observe(RPM, true, 0, 10, 1000); g.observe(COOLANT, true, 20, 30, 1000);
        for (long t = 500; t < 10_000; t += 500) { g.observe(RPM, false, t, t + 10, 1000); g.observe(COOLANT, false, t + 20, t + 30, 1000); }
        require(g.observe(IAT, false, 10_000, 10_100, 1000) == none && g.observe(RPM, false, 10_100, 10_500, 1000) == MeasurementGap.Change.START, "gap once both previously successful signals failed for 10 s; the never-successful one does not count");
        require(RPM.equals(g.nextProbe(15_500)) && COOLANT.equals(g.nextProbe(20_500)) && RPM.equals(g.nextProbe(25_500)), "REQ-037.2 probes rotate over the previously successful signals only");
        // selection change, stop and a fresh session
        require(g.select(Arrays.asList(RPM, COOLANT, IAT)) == none && g.select(Arrays.asList(COOLANT)) == none && g.active(), "a selection that keeps a probe candidate keeps the gap");
        require(COOLANT.equals(g.nextProbe(40_000)), "after a selection change only selected signals are probed");
        require(g.select(Arrays.asList(IAT)) == MeasurementGap.Change.STOPPED && !g.active(), "a selection without any probe candidate ends the gap without claiming recovery");
        g.select(Arrays.asList(COOLANT));
        for (long t = 50_000; t <= 60_000; t += 500) require(g.observe(COOLANT, false, t, t + 10, 1000) == (t == 60_000 ? MeasurementGap.Change.START : none), "the signal stays known after the selection returns: gap 10 s after the first miss (t=" + t + ")");
        g.reset(); g.select(Arrays.asList(COOLANT));
        for (long t = 100_000; t < 160_000; t += 500) require(g.observe(COOLANT, false, t, t + 10, 1000) == none, "a fresh session forgets what was successful: nothing to compare with");
        g = new MeasurementGap(); g.select(Arrays.asList(RPM)); g.observe(RPM, true, 0, 10, 1000);
        for (long t = 500; t <= 10_500; t += 500) g.observe(RPM, false, t, t + 10, 1000);
        require(g.active() && g.stop() && !g.active() && !g.stop(), "stop ends an open gap once, without recovery");

        // REQ-037.1 slow signals: F is fast, K is slow (15 s period), both returned a value before. F failing for 10 s is not yet a gap while K was not due.
        long pause = MeasurementGap.pauseLimitMs(15_000);
        require(pause == 17_000 && MeasurementGap.pauseLimitMs(500) == 2_500 && MeasurementGap.pauseLimitMs(30_000) == 32_000, "the pause that still counts as continuous reading follows the longest period: " + pause);
        String F = RPM, K = COOLANT, U = IAT;
        g = new MeasurementGap(); g.select(Arrays.asList(F, K, U)); g.observe(F, true, 0, 100, pause); g.observe(K, true, 200, 300, pause);
        for (long t = 1000; t <= 24_500; t += 500) require(g.observe(F, false, t, t + 100, pause) == none && !g.active(), "REQ-037.1 a fast signal failing for more than 10 s is no gap while a previously successful slow signal has not been attempted (t=" + t + ")");
        require(g.observe(K, false, 25_000, 25_100, pause) == MeasurementGap.Change.START && g.active(), "REQ-037.1 the slow signal's failed attempt completes the evidence: gap (the never-successful signal U never had to be attempted)");
        String firstProbe = g.nextProbe(30_100);
        require(F.equals(firstProbe), "the first probe is a previously successful signal, in selection order: " + firstProbe);
        require(g.nextProbe(30_101) == null && g.nextProbe(35_099) == null, "at most one probe per 5 s");
        String secondProbe = g.nextProbe(35_100);
        require(K.equals(secondProbe), "the slow previously successful signal is probed next, 5 s later: " + secondProbe);
        require(g.observe(K, true, 36_000, 36_100, pause) == MeasurementGap.Change.RECOVERED && !g.active(), "REQ-037.1 a success of the slow signal ends the gap");
        // the slow signal succeeds on each of its attempts while the fast one keeps failing: never a gap
        g = new MeasurementGap(); g.select(Arrays.asList(F, K)); g.observe(F, true, 0, 100, pause); g.observe(K, true, 200, 300, pause);
        for (long t = 1000; t <= 90_000; t += 500) {
            require(g.observe(F, false, t, t + 100, pause) == none, "REQ-037.1 the slow signal still answers: no gap (t=" + t + ")");
            if (t % 15_000 == 0) require(g.observe(K, true, t + 200, t + 300, pause) == none, "the slow signal answers on its period");
        }
        require(!g.active(), "REQ-037.1 a slow signal that succeeds keeps the stream healthy although a fast one fails for 90 s");
        // the slow signal failed first, then the fast one keeps failing: the 10 s are counted from the first failed attempt
        g = new MeasurementGap(); g.select(Arrays.asList(F, K, U)); g.observe(F, true, 0, 100, pause); g.observe(K, true, 200, 300, pause);
        for (long t = 1000; t <= 11_000; t += 500) {
            if (t == 5000) require(g.observe(K, false, 5_050, 5_100, pause) == none, "the slow signal's miss alone is no gap");
            MeasurementGap.Change change = g.observe(F, false, t, t + 100, pause);
            require(change == (t == 11_000 ? MeasurementGap.Change.START : none), "gap exactly when 10 s of failed attempts exist and every successful signal failed in them (t=" + t + "): " + change);
        }
        // a pause longer than the slowest period means reads were not attempted: nothing is concluded from it
        g = new MeasurementGap(); g.select(Arrays.asList(F, K)); g.observe(F, true, 0, 100, pause); g.observe(K, true, 200, 300, pause);
        g.observe(F, false, 1000, 1100, pause); g.observe(K, false, 1200, 1300, pause);
        require(g.observe(F, false, 1300 + pause + 1, 1400 + pause + 1, pause) == none && !g.active(), "a pause longer than the longest period restarts the window");

        // what counts as a measurement: a finite number from an ok read; zero is a measurement, an error, NaN, a missing value and 'unavailable' are not
        require(MeasurementGap.finite(read(true, 0.0, null, null)) && MeasurementGap.finite(read(true, 800.0, null, null)), "a finite number, zero included, is a measurement");
        require(!MeasurementGap.finite(read(false, 800.0, null, null)) && !MeasurementGap.finite(read(true, null, null, null)) && !MeasurementGap.finite(new JSONObject("{\"ok\":true,\"data\":{\"value\":\"n/a\"}}"))
            && !MeasurementGap.finite(read(true, 800.0, "keine Antwort", null)) && !MeasurementGap.finite(read(true, 800.0, null, "unavailable")) && !MeasurementGap.finite(null), "no value, error, unavailable or not ok is no measurement");
    }

    private static JSONObject read(boolean ok, Double value, String error, String status) {
        try {
            JSONObject data = new JSONObject(); if (value != null) data.put("value", value); if (error != null) data.put("error", error); if (status != null) data.put("status", status);
            return new JSONObject().put("ok", ok).put("data", data);
        } catch (Exception e) { throw new AssertionError(e); }
    }

    // ---- part two: the real service in real time -------------------------------------------------------------------------------------

    /** Answers like the demo adapter except for the steered silence, support and the unsupported PID; every data read is logged with its time. */
    private static final class Link implements ObdService.Link {
        static final class Sent { final long mono, wall; final String cmd; final boolean answered; Sent(long mono, long wall, String cmd, boolean answered) { this.mono = mono; this.wall = wall; this.cmd = cmd; this.answered = answered; } }
        final ObdService.Link delegate; final List<Sent> data = Collections.synchronizedList(new ArrayList<>()); final List<String> commands = Collections.synchronizedList(new ArrayList<>());
        final Set<String> silent = ConcurrentHashMap.newKeySet(); volatile boolean silentAll; volatile int unsupportedPid = -1;
        Link(ObdService.Link delegate) { this.delegate = delegate; }
        public boolean isOpen() { return delegate.isOpen(); }
        public void close() { delegate.close(); }
        public String exchange(String command, long timeoutMs) throws Exception {
            String compact = command.trim().replace(" ", "").toUpperCase(Locale.ROOT);
            long mono = SystemClock.elapsedRealtime(), wall = System.currentTimeMillis();
            commands.add(compact);
            if (compact.equals("0100") && unsupportedPid >= 1 && unsupportedPid <= 8 * 4) { // support bitmap of PIDs 01-20 without that one PID
                int[] bytes = {0xFF, 0xFF, 0xFF, 0xFF}; bytes[(unsupportedPid - 1) / 8] &= ~(0x80 >> ((unsupportedPid - 1) % 8));
                return String.format(Locale.ROOT, "7E8 06 41 00 %02X %02X %02X %02X\r>", bytes[0], bytes[1], bytes[2], bytes[3]);
            }
            boolean read = compact.matches("01[0-9A-F]{2}") && !SUPPORT_PIDS.contains(compact.substring(2)) && !compact.equals("0101"); // 0101 belongs to inventory jobs, not to the polled selection
            if (!read) return delegate.exchange(command, timeoutMs);
            if (silentAll || silent.contains(compact.substring(2))) { data.add(new Sent(mono, wall, compact, false)); return "NO DATA\r>"; }
            String response = delegate.exchange(command, timeoutMs);
            data.add(new Sent(mono, wall, compact, response.contains(" 41 " + compact.substring(2))));
            return response;
        }
        List<Sent> since(long mono) { synchronized (data) { List<Sent> out = new ArrayList<>(); for (Sent s : data) if (s.mono > mono) out.add(s); return out; } }
        int count(String cmd, long from, long to) { int n = 0; for (Sent s : since(from)) if (s.cmd.equals(cmd) && s.mono <= to) n++; return n; }
        Sent lastAnswered() { synchronized (data) { Sent last = null; for (Sent s : data) if (s.answered) last = s; return last; } }
        int countAll(String cmd) { int n = 0; synchronized (data) { for (Sent s : data) if (s.cmd.equals(cmd)) n++; } return n; }
    }

    static void run(ObdService service, Context context, Ui ui) throws Exception {
        rules();
        File dir = new File(context.getFilesDir(), "recordings"); Set<String> before = names(dir);
        VehicleStore savedStore = service.vehicles; ObdService.Connector savedConnector = service.connector;
        Boolean savedPermission = service.permissionOverride, savedBluetooth = service.bluetoothOverride;
        boolean savedAutoRecord = ObdService.autoRecordEnabled(context);
        long savedRotateMs = service.rotateMs, savedRotateBytes = service.rotateBytes;
        List<Link> links = Collections.synchronizedList(new ArrayList<>());
        try {
            service.permissionOverride = Boolean.TRUE; service.bluetoothOverride = Boolean.TRUE;
            service.vehicles = new VehicleStore(new File(context.getCacheDir(), "measurement-gap-" + System.nanoTime() + ".json"));
            service.connector = address -> { Link l = new Link(service.simulatedLink()); l.unsupportedPid = 0x0B; l.silent.add("0F"); links.add(l); return l; };
            service.setAutoConnect(false); service.setAutoRecord(true); service.disconnect();
            service.connect(ADAPTER);
            waitFor(() -> "ready".equals(service.snapshot().optString("phase")) && "live".equals(service.snapshot().optString("mode")) && !service.snapshot().optBoolean("busy"), "ready live session", 30_000);
            waitFor(() -> !service.snapshot().optString("autoRecording").isEmpty(), "continuous recording", 30_000);
            Link link = links.get(0);
            require(!gap(service).optBoolean("active") && gap(service).has("probes") && MainActivity.gapText(service.snapshot()).isEmpty(), "REQ-037.1 a fresh ready session has no gap and shows no notice: " + gap(service));
            require("unsupported".equals(status(service, MAP)) && !"unsupported".equals(status(service, IAT)) && !"unsupported".equals(status(service, RPM)), "premise: the fake adapter does not advertise the intake manifold pressure, the others are advertised: MAP=" + status(service, MAP));

            // REQ-037.1: only unsupported and never-successful signals selected: reads are attempted, no gap, however long
            select(service, MAP, IAT);
            long from = SystemClock.elapsedRealtime(); boolean seen = false;
            while (SystemClock.elapsedRealtime() - from < 11_500) { seen |= gap(service).optBoolean("active"); SystemClock.sleep(250); }
            require(link.count(IAT_PID, from, Long.MAX_VALUE) >= 10, "premise: the never-successful signal is actually read: " + link.count(IAT_PID, from, Long.MAX_VALUE));
            require(link.countAll(MAP_PID) == 0, "an unsupported signal is not polled at all: " + link.commands);
            require(!seen && !gap(service).optBoolean("active") && MainActivity.gapText(service.snapshot()).isEmpty(), "REQ-037.1 unsupported and never-successful signals alone cannot start a gap after 11.5 s of attempted reads: " + gap(service));
            require(finiteValue(service, IAT) == null && "ready".equals(service.snapshot().optString("phase")), "an unanswered signal shows no value, the session stays ready");

            // REQ-037.1: selected values succeed, one signal is unsupported, one never answers: no gap while others succeed for longer than 10 s
            select(service, RPM, COOLANT, IAT, MAP);
            waitFor(() -> finiteValue(service, RPM) != null && finiteValue(service, COOLANT) != null, "values of the answering signals", 15_000);
            from = SystemClock.elapsedRealtime(); seen = false; long iatBefore = link.countAll(IAT_PID);
            while (SystemClock.elapsedRealtime() - from < 11_500) { seen |= gap(service).optBoolean("active"); SystemClock.sleep(250); }
            require(!seen && !gap(service).optBoolean("active") && MainActivity.gapText(service.snapshot()).isEmpty(), "REQ-037.1 errors of an individual signal alongside successful measurements do not start a gap: " + gap(service));
            require(link.countAll(IAT_PID) - iatBefore >= 10 && finiteValue(service, IAT) == null && finiteValue(service, RPM) != null && finiteValue(service, RPM) == 800.0, "the failing signal keeps being read, shows no value, the others keep their values: rpm=" + finiteValue(service, RPM));
            require(link.countAll(MAP_PID) == 0, "the unsupported signal is still not polled");
            require(!recordedGapEvents(dir, before).contains("start"), "no gap is recorded while measurements succeed: " + recordedGapEvents(dir, before));

            // REQ-037.1: sustained silence of every read: a gap after at least 10 s of attempted reads, with the neutral notice only
            link.silentAll = true;
            waitFor(() -> gap(service).optBoolean("active"), "gap after sustained silence", 25_000);
            long observed = SystemClock.elapsedRealtime(); Link.Sent last = link.lastAnswered();
            require(last != null && observed - last.mono >= 10_000 && observed - last.mono <= 14_000, "REQ-037.1 the gap starts after (and soon after) 10 s without a finite value: " + (observed - last.mono) + " ms");
            List<Link.Sent> attempted = new ArrayList<>(); for (Link.Sent s : link.since(last.mono)) if (s.mono <= observed) attempted.add(s);
            Set<String> attemptedPids = new HashSet<>(); for (Link.Sent s : attempted) attemptedPids.add(s.cmd);
            require(attempted.size() >= 10 && attemptedPids.containsAll(Arrays.asList(RPM_PID, COOLANT_PID, IAT_PID)) && !attemptedPids.contains(MAP_PID), "REQ-037.1 the full selection was attempted during the 10 s, the unsupported signal never: " + attempted.size() + " " + attemptedPids);
            JSONObject state = service.snapshot();
            require(MainActivity.gapText(state).equals(TEXT) && gap(service).optLong("since") > 0, "REQ-037.1 the notice reads exactly '" + TEXT + "': " + gap(service));
            String claims = (gap(service).toString() + MainActivity.gapText(state) + state.optString("message")).toLowerCase(Locale.ROOT);
            for (String forbidden : new String[] {"zündung", "zuendung", "ignition", "getrennt", "disconnect", "verbindung", "adapter", "fahrzeug"}) require(!claims.contains(forbidden), "REQ-037.1 no claim about ignition, adapter or vehicle ('" + forbidden + "'): " + claims);
            require("ready".equals(state.optString("phase")) && "live".equals(state.optString("mode")), "the session is neither disconnected nor failed: " + state.optString("phase"));
            for (String key : new String[] {RPM, COOLANT, IAT}) require(finiteValue(service, key) == null && !hasZero(service, key), "REQ-037.2 missing values stay unknown, never zero: " + key);

            // REQ-037.2: the probe cadence, seen at the adapter. A selection change and a job in the middle must not add reads.
            long gapSeen = observed; long selectionAt = -1, jobDone = -1; boolean selected = false, jobbed = false;
            while (SystemClock.elapsedRealtime() - gapSeen < 17_000) {
                long now = SystemClock.elapsedRealtime();
                if (!selected && now - gapSeen > 4000) { select(service, COOLANT, IAT, MAP); selectionAt = SystemClock.elapsedRealtime(); selected = true; }
                if (!jobbed && now - gapSeen > 9000) {
                    long jobStart = SystemClock.elapsedRealtime(); service.runJob("dtc.scan");
                    waitFor(() -> service.snapshot().optString("message").contains("Auftrag abgeschlossen: dtc.scan") && !service.snapshot().optBoolean("busy"), "job during a gap", 15_000);
                    jobDone = SystemClock.elapsedRealtime(); jobbed = true;
                    require(jobDone - jobStart < 8000, "REQ-037.2 a job stays responsive during a gap: " + (jobDone - jobStart) + " ms");
                }
                SystemClock.sleep(100);
            }
            require(selected && jobbed, "test steps ran");
            List<Link.Sent> probes = link.since(observed + 200);
            require(probes.size() >= 2 && probes.size() <= 4, "REQ-037.2 about one read per 5 s during 17 s instead of the full polling set: " + describe(probes));
            for (Link.Sent s : probes) require(!s.answered && (s.cmd.equals(RPM_PID) || s.cmd.equals(COOLANT_PID)), "REQ-037.2 only a signal that returned a value earlier is probed, not the never-successful or unsupported ones: " + describe(probes));
            for (int i = 1; i < probes.size(); i++) require(probes.get(i).mono - probes.get(i - 1).mono >= 4_700, "REQ-037.2 at most one probe every 5 s (also across the selection change and the job): " + describe(probes));
            for (Link.Sent s : probes) if (s.mono > selectionAt + 300) require(s.cmd.equals(COOLANT_PID), "REQ-037.2 after the selection changed only the still selected, previously successful signal is probed: " + describe(probes));
            require(gap(service).optInt("probes") >= 2, "the state counts the probes: " + gap(service));
            require(gap(service).optBoolean("active") && MainActivity.gapText(service.snapshot()).equals(TEXT), "a job and a selection change leave the gap visible (no measurement happened): " + gap(service));
            require(!link.commands.contains("3E80") && !link.commands.contains("1003"), "no session service was introduced by the gap: " + link.commands);

            // REQ-037.3: the open gap is carried into the file a forced rotation opens
            String first = service.snapshot().optString("autoRecording");
            service.rotateMs = 1200;
            waitFor(() -> !service.snapshot().optString("autoRecording").equals(first) && !service.snapshot().optString("autoRecording").isEmpty(), "forced rotation during the gap", 30_000);
            service.rotateMs = savedRotateMs;
            require(gap(service).optBoolean("active"), "the rotation does not touch the gap: " + gap(service));

            // REQ-037.2: recovery on the first finite result, normal polling restored promptly
            long restoreAt = SystemClock.elapsedRealtime(); link.silentAll = false;
            waitFor(() -> !gap(service).optBoolean("active"), "recovery", 10_000);
            long recoveredAt = SystemClock.elapsedRealtime(); Link.Sent firstAnswer = null;
            for (Link.Sent s : link.since(restoreAt - 1)) if (s.answered) { firstAnswer = s; break; }
            require(firstAnswer != null && recoveredAt - firstAnswer.mono <= 2_500, "REQ-037.2 the first finite result ends the gap at once: " + (firstAnswer == null ? "none" : (recoveredAt - firstAnswer.mono) + " ms"));
            require(MainActivity.gapText(service.snapshot()).isEmpty() && !gap(service).has("since"), "the notice disappears: " + gap(service));
            long recoverStart = SystemClock.elapsedRealtime(); int iat = link.countAll(IAT_PID), cool = link.countAll(COOLANT_PID);
            waitFor(() -> link.countAll(IAT_PID) >= iat + 2 && link.countAll(COOLANT_PID) >= cool + 4, "full polling restored", 6_000);
            SystemClock.sleep(1500);
            require(!gap(service).optBoolean("active") && finiteValue(service, COOLANT) != null && link.countAll(MAP_PID) == 0, "REQ-037.2 normal polling runs again and the gap does not return: " + gap(service));

            // REQ-037.2: unknown trigger values of the smart analysis stay unknown during a gap
            // the second gap lives in its own file: a rotation without an open gap carries nothing
            String afterRecovery = service.snapshot().optString("autoRecording"); service.rotateMs = 1200;
            waitFor(() -> !service.snapshot().optString("autoRecording").equals(afterRecovery) && !service.snapshot().optString("autoRecording").isEmpty(), "rotation after recovery", 30_000);
            service.rotateMs = savedRotateMs;
            service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started", 15_000);
            waitFor(() -> "fresh".equals(trigger(service, RPM).optString("state")) && "fresh".equals(trigger(service, COOLANT).optString("state")) && "fresh".equals(trigger(service, "vehicle.speed").optString("state")), "fresh trigger readings", 20_000);
            int changesBefore = nonAvailabilityChanges(service);
            String setBefore = smart(service).optString("component");
            link.silentAll = true;
            waitFor(() -> gap(service).optBoolean("active"), "gap while the analysis runs", 25_000);
            SystemClock.sleep(1500);
            for (String key : new String[] {RPM, COOLANT, "vehicle.speed", "vehicle.accelerator_pedal_e", "engine.load"}) {
                JSONObject row = trigger(service, key);
                require(!"fresh".equals(row.optString("state")) && !row.has("value"), "REQ-037.2 a missing or stale trigger reading is unknown, never zero: " + row);
            }
            require(nonAvailabilityChanges(service) == changesBefore && setBefore.equals(smart(service).optString("component")), "REQ-037.2 unknown triggers neither trigger, release nor re-arm a set: " + smart(service).optJSONArray("changes"));
            require(smart(service).optBoolean("active") && MainActivity.gapText(service.snapshot()).equals(TEXT), "the analysis keeps running with the notice shown");

            // REQ-037.3: the stop ends the open gap in the recording without claiming recovery
            service.disconnect();
            // disconnect() sets the phase at once; the worker then closes the recording and resets the gap: wait for all three, then judge one snapshot
            waitFor(() -> { JSONObject s = service.snapshot(); return "disconnected".equals(s.optString("phase")) && s.optString("autoRecording").isEmpty() && !s.optJSONObject("gap").optBoolean("active"); }, "disconnected, recording closed and gap cleared", 15_000);
            JSONObject disconnected = service.snapshot(); JSONObject disconnectedGap = disconnected.optJSONObject("gap");
            require("disconnected".equals(disconnected.optString("phase")) && disconnected.optString("autoRecording").isEmpty() && disconnectedGap != null && !disconnectedGap.optBoolean("active") && MainActivity.gapText(disconnected).isEmpty(), "REQ-037.1 a disconnect clears the gap, it is not carried into the next session: " + disconnected.optString("phase") + " " + disconnectedGap);

            // REQ-037.3: the recording
            List<String> chain = new ArrayList<>(names(dir)); chain.removeAll(before); chain.removeIf(n -> !n.endsWith("-auto.jsonl")); Collections.sort(chain);
            require(chain.size() >= 2, "the forced rotation produced at least two files: " + chain);
            List<List<JSONObject>> files = new ArrayList<>(); for (String n : chain) files.add(read(new File(dir, n)));
            String sequence = checkChain(files);
            require(sequence.matches("start:no_values(,resume:no_values)+,end:recovered,start:no_values(,resume:no_values)*,end:stopped"), "REQ-037.3 start, a resume in the rotated file, recovery, a second gap and its end by the stop, nothing else: " + sequence);
            List<JSONObject> gapEvents = new ArrayList<>(); for (List<JSONObject> f : files) for (JSONObject e : f) if (isGap(e)) gapEvents.add(e);
            JSONObject start = gapEvents.get(0), recovered = null;
            for (JSONObject e : gapEvents) if ("recovered".equals(data(e).optString("reason"))) recovered = e;
            require(start.optLong("at") - last.wall >= 10_000 && start.optLong("at") <= System.currentTimeMillis(), "REQ-037.3 the start is timestamped no earlier than 10 s after the last value: " + (start.optLong("at") - last.wall) + " ms");
            require(recovered.optLong("at") - start.optLong("at") >= 16_000, "REQ-037.3 the recorded recovery follows the 17 s silence: " + (recovered.optLong("at") - start.optLong("at")) + " ms");
            JSONObject stoppedEnd = gapEvents.get(gapEvents.size() - 1);
            require("end".equals(data(stoppedEnd).optString("change")) && "stopped".equals(data(stoppedEnd).optString("reason")), "REQ-037.3 the stop closes the gap with the observed outcome only: " + stoppedEnd);
            // the raw evidence of the first gap stays: the probes are in the recording, at most one per 5 s
            long startAt = start.optLong("at"), endAt = recovered.optLong("at"); List<Long> recordedProbes = new ArrayList<>(); int answeredTx = 0;
            for (List<JSONObject> f : files) for (JSONObject e : f) {
                if (e.optLong("at") > startAt && e.optLong("at") < endAt && "read".equals(e.optString("action")) && (RPM.equals(e.optString("argument")) || COOLANT.equals(e.optString("argument")) || IAT.equals(e.optString("argument")) || MAP.equals(e.optString("argument"))))
                    recordedProbes.add(e.optLong("at"));
                if ("wire".equals(e.optString("type")) && "TX".equals(e.optString("direction")) && e.optLong("at") < startAt) answeredTx++;
            }
            require(answeredTx > 10, "the raw request and response evidence of the working period is in the recording: " + answeredTx);
            require(recordedProbes.size() >= 2 && recordedProbes.size() <= (endAt - startAt) / 5000 + 2, "REQ-037.2 the recording shows about one read per 5 s during the gap, not the polling set: " + recordedProbes.size() + " in " + (endAt - startAt) + " ms");
            for (int i = 1; i < recordedProbes.size(); i++) require(recordedProbes.get(i) - recordedProbes.get(i - 1) >= 4_500, "REQ-037.2 recorded probes are at least 5 s apart: " + recordedProbes);
            boolean readsAfter = false; for (JSONObject e : files.get(files.size() - 1)) if ("read".equals(e.optString("action")) && e.optLong("at") > endAt) readsAfter = true;
            require(readsAfter, "normal reads follow the recorded recovery");
            // the unknown trigger readings of the analysis in the second gap carry no value unless fresh
            for (List<JSONObject> f : files) for (JSONObject e : f) {
                JSONObject smartData = "smart.set".equals(e.optString("action")) && e.optJSONObject("result") != null ? e.getJSONObject("result").optJSONObject("data") : null;
                JSONObject readings = smartData == null ? null : smartData.optJSONObject("readings");
                for (java.util.Iterator<String> names = readings == null ? Collections.<String>emptyIterator() : readings.keys(); names.hasNext(); ) { JSONObject r = readings.optJSONObject(names.next());
                    require(r == null || !r.has("value") || "fresh".equals(r.optString("state")), "REQ-037.2 a missing or stale trigger reading carries no value in the recording: " + r); }
            }

            // REQ-037.3: offline replay shows the gap without any adapter traffic; the raw evidence stays
            int traffic = 0; for (Link l : links) traffic += l.commands.size();
            String firstFile = null, resumedFile = null, stoppedFile = null;
            for (int i = 0; i < files.size(); i++) { String s = gapSequence(files.get(i));
                if (firstFile == null && s.startsWith("start:")) firstFile = chain.get(i);
                if (resumedFile == null && s.startsWith("resume:") && s.contains("end:recovered")) resumedFile = chain.get(i);
                if (s.contains("end:stopped")) stoppedFile = chain.get(i); }
            require(firstFile != null && resumedFile != null && stoppedFile != null, "the files holding start, recovery and stop were found: " + chain);
            open(service, firstFile);
            JSONObject replay = gap(service);
            require(replay.optBoolean("replay") && replay.optBoolean("active") && replay.optJSONArray("changes").length() >= 1 && "start".equals(replay.getJSONArray("changes").getJSONObject(0).optString("change"))
                && MainActivity.gapText(service.snapshot()).equals(TEXT), "REQ-037.3 replay of a file whose gap is still open shows it: " + replay);
            require(ui.text("Live").contains(TEXT), "REQ-037.3 the Live tab shows the notice in replay");
            open(service, resumedFile);
            replay = gap(service); JSONArray changes = replay.getJSONArray("changes");
            require(replay.optBoolean("replay") && !replay.optBoolean("active") && "recovered".equals(replay.optString("outcome")) && "resume".equals(changes.getJSONObject(0).optString("change")) && "end".equals(changes.getJSONObject(changes.length() - 1).optString("change"))
                && MainActivity.gapText(service.snapshot()).isEmpty(), "REQ-037.3 replay of the continuation file shows resume ... recovered and no open notice: " + replay);
            require(!ui.text("Live").contains(TEXT), "an ended gap shows no notice");
            open(service, stoppedFile);
            replay = gap(service);
            require(replay.optBoolean("replay") && !replay.optBoolean("active") && "stopped".equals(replay.optString("outcome")), "REQ-037.3 replay shows the stop as the observed outcome, not a recovery: " + replay);
            int traffic2 = 0; for (Link l : links) traffic2 += l.commands.size();
            require(traffic2 == traffic, "REQ-037.3 replay sends nothing to an adapter: " + (traffic2 - traffic) + " commands");
            legacyReplay(service, context);
            require(finalTraffic(links) == traffic, "an old recording opens without any adapter traffic");
        } finally {
            service.rotateMs = savedRotateMs; service.rotateBytes = savedRotateBytes;
            service.disconnect(); service.connector = savedConnector; service.vehicles = savedStore; service.permissionOverride = savedPermission; service.bluetoothOverride = savedBluetooth;
            service.setAutoRecord(savedAutoRecord);
            service.setDemo(); waitFor(() -> "demo".equals(service.snapshot().optString("mode")) && "ready".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"), "demo ready", 30_000);
            service.setAutoConnect(true);
            Set<String> created = names(dir); created.removeAll(before); for (String n : created) new File(dir, n).delete();
        }
    }

    /** REQ-037.3: a recording made before this feature (no gap events) opens as before and shows no gap. */
    private static void legacyReplay(ObdService service, Context context) throws Exception {
        String name = "legacy-no-gap-" + System.nanoTime() + ".jsonl";
        File dir = new File(context.getFilesDir(), "recordings"); dir.mkdirs(); File legacy = new File(dir, name);
        String[] lines = {
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"live\",\"profile\":\"session-auto\",\"selected\":[\"engine.rpm\"]}",
            "{\"type\":\"action\",\"action\":\"read\",\"argument\":\"engine.rpm\",\"result\":{\"ok\":true,\"data\":{\"semantic\":\"engine.rpm\",\"response\":[65,12,12,128],\"value\":99999}}}",
            "{\"type\":\"session_end\",\"reason\":\"disconnected\"}"};
        Files.write(legacy.toPath(), (String.join("\n", lines) + "\n").getBytes(StandardCharsets.UTF_8));
        try {
            open(service, name);
            JSONObject replay = gap(service);
            require(!replay.optBoolean("active") && MainActivity.gapText(service.snapshot()).isEmpty() && (replay.optJSONArray("changes") == null || replay.optJSONArray("changes").length() == 0) && !service.snapshot().optBoolean("replayPartial"), "REQ-037.3 an older recording opens and shows no gap: " + replay);
            JSONArray values = service.snapshot().optJSONArray("values"); Double rpm = null;
            for (int i = 0; values != null && i < values.length(); i++) if (RPM.equals(values.getJSONObject(i).optString("semantic"))) rpm = values.getJSONObject(i).optDouble("value");
            require(rpm != null && rpm == 800.0, "REQ-037.3 its raw response is still decoded from the recorded bytes: " + rpm);
        } finally { legacy.delete(); }
    }

    // ---- recording checks ------------------------------------------------------------------------------------------------------------

    private static boolean isGap(JSONObject e) { return "action".equals(e.optString("type")) && "measurement.gap".equals(e.optString("action")); }
    private static JSONObject data(JSONObject e) { JSONObject result = e.optJSONObject("result"), data = result == null ? null : result.optJSONObject("data"); return data == null ? new JSONObject() : data; }
    private static String gapSequence(List<JSONObject> events) {
        StringBuilder out = new StringBuilder(); for (JSONObject e : events) if (isGap(e)) out.append(out.length() == 0 ? "" : ",").append(data(e).optString("change")).append(':').append(data(e).optString("reason")); return out.toString();
    }
    /** Every file stands alone and the chain is coherent: the event shape is the shared contract, an open gap is carried by 'resume' as the first event after the header, the last file ends closed. Returns the change sequence. */
    private static String checkChain(List<List<JSONObject>> files) {
        boolean open = false; StringBuilder sequence = new StringBuilder();
        for (int f = 0; f < files.size(); f++) {
            List<JSONObject> events = files.get(f); boolean carried = open, evidence = false, firstGap = true; long lastAt = -1, lastOffset = -1;
            require("session".equals(events.get(0).optString("type")) && "session_end".equals(events.get(events.size() - 1).optString("type")), "file " + f + " has header and end marker");
            for (JSONObject e : events) {
                if ("wire".equals(e.optString("type")) || "read".equals(e.optString("action"))) evidence = true;
                if (!isGap(e)) continue;
                JSONObject result = e.optJSONObject("result"), data = result == null ? null : result.optJSONObject("data");
                require("".equals(e.optString("argument")) && result != null && result.optBoolean("ok") && data != null && data.length() == 2 && data.has("change") && data.has("reason"), "REQ-037.3 the event has the shared shape (empty argument, ok, exactly change and reason): " + e);
                require(e.optLong("at") > 0 && e.has("offset_us") && e.optLong("offset_us") >= lastOffset && e.optLong("at") >= lastAt, "REQ-037.3 timestamped with wall time and ascending offset: " + e);
                lastAt = e.optLong("at"); lastOffset = e.optLong("offset_us");
                String change = data.optString("change"), reason = data.optString("reason");
                if (change.equals("resume")) { require(f > 0 && carried && firstGap && !evidence && open && reason.equals("no_values"), "REQ-037.3 resume is the first event of the file after a rotation that left a gap open, and the gap stays open: file " + f + " " + e); open = true; }
                else if (change.equals("start")) { require(!open && reason.equals("no_values"), "start while no gap is open: " + e); open = true; }
                else if (change.equals("end")) { require(open && (reason.equals("recovered") || reason.equals("stopped")), "end closes an open gap with the observed outcome: " + e); open = false; }
                else throw new AssertionError("unknown change: " + e);
                firstGap = false; sequence.append(sequence.length() == 0 ? "" : ",").append(change).append(':').append(reason);
            }
            require(!carried || !firstGap, "REQ-037.3 a file following an open gap says so (resume) and is understandable on its own: file " + f);
            String reason = events.get(events.size() - 1).optString("reason");
            if (f < files.size() - 1) require(reason.equals("rotated") || !open, "an open gap ends a file only by rotation: " + reason);
        }
        require(!open, "REQ-037.3 the recording's end closes the gap");
        return sequence.toString();
    }
    private static List<String> recordedGapEvents(File dir, Set<String> before) throws Exception {
        List<String> out = new ArrayList<>(); Set<String> now = names(dir); now.removeAll(before); List<String> sorted = new ArrayList<>(now); Collections.sort(sorted);
        for (String n : sorted) for (JSONObject e : read(new File(dir, n))) if (isGap(e)) out.add(data(e).optString("change"));
        return out;
    }

    // ---- helpers ---------------------------------------------------------------------------------------------------------------------

    private static void select(ObdService service, String... keys) throws Exception {
        JSONArray array = new JSONArray(); for (String k : keys) array.put(k);
        service.setSignals(array);
        waitFor(() -> { JSONArray s = service.snapshot().optJSONArray("selected"); if (s == null || s.length() != keys.length) return false; for (String k : keys) if (!contains(s, k)) return false; return true; }, "selection " + Arrays.toString(keys), 15_000);
    }
    private static JSONObject gap(ObdService service) { JSONObject g = service.snapshot().optJSONObject("gap"); return g == null ? new JSONObject() : g; }
    private static JSONObject smart(ObdService service) { return service.snapshot().optJSONObject("smart"); }
    private static JSONObject trigger(ObdService service, String semantic) {
        JSONArray rows = smart(service).optJSONArray("triggers");
        for (int i = 0; rows != null && i < rows.length(); i++) if (semantic.equals(rows.optJSONObject(i).optString("semantic"))) return rows.optJSONObject(i);
        return new JSONObject();
    }
    private static int nonAvailabilityChanges(ObdService service) {
        JSONArray changes = smart(service).optJSONArray("changes"); int n = 0;
        for (int i = 0; changes != null && i < changes.length(); i++) if (!"availability".equals(changes.optJSONObject(i).optString("kind"))) n++;
        return n;
    }
    private static String status(ObdService service, String semantic) {
        JSONArray catalog = service.snapshot().optJSONArray("signals");
        for (int i = 0; catalog != null && i < catalog.length(); i++) if (semantic.equals(catalog.optJSONObject(i).optString("semantic"))) return catalog.optJSONObject(i).optString("status");
        return "";
    }
    private static Double finiteValue(ObdService service, String semantic) {
        JSONArray values = service.snapshot().optJSONArray("values");
        for (int i = 0; values != null && i < values.length(); i++) { JSONObject row = values.optJSONObject(i);
            if (semantic.equals(row.optString("semantic")) && row.has("value") && !row.has("error") && Double.isFinite(row.optDouble("value", Double.NaN))) return row.optDouble("value"); }
        return null;
    }
    private static boolean hasZero(ObdService service, String semantic) {
        JSONArray values = service.snapshot().optJSONArray("values");
        for (int i = 0; values != null && i < values.length(); i++) { JSONObject row = values.optJSONObject(i); if (semantic.equals(row.optString("semantic")) && row.has("value") && row.optDouble("value", 1) == 0.0) return true; }
        return false;
    }
    private static String describe(List<Link.Sent> sent) { StringBuilder s = new StringBuilder(); long base = sent.isEmpty() ? 0 : sent.get(0).mono; for (Link.Sent x : sent) s.append(x.cmd).append('@').append(x.mono - base).append(x.answered ? "+ " : "- "); return s.toString(); }
    private static int finalTraffic(List<Link> links) { int n = 0; for (Link l : links) n += l.commands.size(); return n; }
    private static void open(ObdService service, String name) throws Exception {
        service.openRecording(name);
        waitFor(() -> "replay".equals(service.snapshot().optString("mode")) && name.equals(service.snapshot().optString("replayFile")) && !service.snapshot().optBoolean("busy"), "replay " + name, 30_000);
    }
    private static boolean contains(JSONArray array, String value) { for (int i = 0; array != null && i < array.length(); i++) if (value.equals(array.optString(i))) return true; return false; }
    private static List<JSONObject> read(File file) throws Exception {
        List<JSONObject> events = new ArrayList<>();
        try (BufferedReader in = new BufferedReader(new InputStreamReader(new FileInputStream(file), StandardCharsets.UTF_8))) { String line; while ((line = in.readLine()) != null) events.add(new JSONObject(line)); }
        return events;
    }
    private static Set<String> names(File dir) { String[] list = dir.list(); return list == null ? new HashSet<>() : new HashSet<>(Arrays.asList(list)); }
    private interface Condition { boolean ready() throws Exception; }
    private static void waitFor(Condition condition, String name, long timeoutMs) throws Exception {
        long until = SystemClock.elapsedRealtime() + timeoutMs;
        while (SystemClock.elapsedRealtime() < until) { if (condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("Timeout: " + name);
    }
    private static void require(boolean condition, String message) { if (!condition) throw new AssertionError("REQ-037 " + message); }
}
