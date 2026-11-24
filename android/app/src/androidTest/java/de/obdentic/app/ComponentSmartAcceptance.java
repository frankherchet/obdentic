package de.obdentic.app;

import android.content.Context;
import android.os.SystemClock;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;

/**
 * REQ-035.1-.4 and REQ-036.1-.5 at the phone seam: the real service, the real Rust core, a simulated adapter whose answers the test
 * steers (synthetic data only, no BLE, no vehicle). Component presets resolve against the identified engine's Knowledge, the one explicit
 * apply may open the existing extended session once, and the smart analysis switches sets by priority while recording each change.
 */
final class ComponentSmartAcceptance {
    private static final String ADAPTER = "AA:BB:CC:00:00:97", REGEN = "dpf.field_regeneration_request_status", PEDAL = "vehicle.accelerator_pedal_e",
        SESSION_SIGNAL = "dpf.surface_temperature", USER = "engine.maf";
    private static final Set<String> STATUSES = new HashSet<>(Arrays.asList("selected", "unsupported", "missing", "no_ecu", "ambiguous", "rejected", "not_answered",
        "uds_unavailable", "uds_ended", "pending_uds", "session_excluded"));
    /** The ledger contexts of REQ-039.2: the five goal groups' conditions and the generic baseline. */
    private static final Set<String> GOAL_CONTEXTS = new HashSet<>(Arrays.asList("baseline", "cooling.warm_up", "cooling.warm_driving", "dpf.warm_up", "dpf.warm_driving", "dpf.raw_request", "turbo", "rail.warm_up", "rail", "egr"));

    /** Answers like the demo adapter, except for the steered trigger inputs and the engine's DIDs; every command is logged with its target. */
    private static final class Link implements ObdService.Link {
        final ObdService.Link delegate; final List<String> commands = Collections.synchronizedList(new ArrayList<>());
        volatile String target = ""; volatile boolean open = true, pedalMissing, pedalRejected, failActivation; volatile int regen; volatile String pedal = "20";
        Link(ObdService.Link delegate) { this.delegate = delegate; }
        public boolean isOpen() { return open && delegate.isOpen(); }
        public void close() { open = false; delegate.close(); }
        void drop() { open = false; }
        public String exchange(String command, long timeoutMs) throws Exception {
            String compact = command.trim().replace(" ", "").toUpperCase(Locale.ROOT);
            if (!open) throw new IOException("simulated link lost");
            if (compact.startsWith("ATSH")) target = compact.substring(4);
            commands.add(target + ":" + compact);
            if (compact.equals("014A")) return pedalMissing ? "NO DATA\r>" : pedalRejected ? "7E8 03 7F 01 12\r>" : "7E8 03 41 4A " + pedal + "\r>";
            if (compact.equals("1003")) return !target.equals("7E0") ? "NO DATA\r>" : failActivation ? "7E8 03 7F 10 22\r>" : "7E8 06 50 03 00 32 01 F4\r>";
            if (compact.equals("1001")) return target.equals("7E0") ? "7E8 02 50 01\r>" : "NO DATA\r>";
            if (compact.equals("3E80")) return "NO DATA\r>";
            if (compact.startsWith("22") && compact.length() == 6) {
                if (!target.equals("7E0")) return "NO DATA\r>";
                int did = Integer.parseInt(compact.substring(2), 16);
                if (did == 0xF187) return "7E8 06 62 F1 87 31 32 33\r>";
                if (did == 0x115B) return String.format(Locale.ROOT, "7E8 04 62 11 5B %02X\r>", regen);
                int length = did == 0x1156 || did == 0x1164 || did == 0x115E ? 4
                    : did == 0x11C1 || did == 0x115C ? 1 : 2;
                StringBuilder response = new StringBuilder(String.format(Locale.ROOT, "7E8 %02X 62 %02X %02X", 3 + length, did >> 8, did & 0xff));
                for (int i = 0; i < length; i++) response.append(i == 0 && did == 0x115B ? String.format(Locale.ROOT, " %02X", regen) : " 00");
                return response.append("\r>").toString();
            }
            return delegate.exchange(command, timeoutMs);
        }
    }

    static void run(ObdService service, Context context) throws Exception {
        File dir = new File(context.getFilesDir(), "recordings"); Set<String> before = names(dir);
        VehicleStore savedStore = service.vehicles; ObdService.Connector savedConnector = service.connector;
        Boolean savedPermission = service.permissionOverride, savedBluetooth = service.bluetoothOverride;
        boolean savedAutoRecord = ObdService.autoRecordEnabled(context);
        Link[] link = new Link[1];
        try {
            presetsAndBlockers(); coverageUiText(); coverageSnapshotCost(context);
            service.permissionOverride = Boolean.TRUE; service.bluetoothOverride = Boolean.TRUE;
            storeFile = new File(context.getCacheDir(), "component-smart-" + System.nanoTime() + ".json");
            service.vehicles = new VehicleStore(storeFile);
            service.connector = address -> { link[0] = new Link(service.simulatedLink()); return link[0]; };
            service.setAutoConnect(false); service.setAutoRecord(true);
            service.disconnect(); connect(service); discover(service);
            presets(service, link);
            reconnect(service, link, true);
            String vin = service.snapshot().getJSONObject("vehicles").optString("selected");
            require(!vin.isEmpty(), "the synthetic vehicle is a known record");
            JSONArray user = new JSONArray().put(USER);
            apply(service, "cooling");
            require("cooling".equals(service.snapshot().getJSONObject("component").optString("id")), "cooling preset applied");
            // REQ-035.4: while the selection is still the preset's own, a recording header carries the provenance
            service.startRecording("live-selection"); waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "named recording");
            File namedFile = new File(dir, service.snapshot().getString("recording"));
            service.stopRecording(); waitFor(() -> service.snapshot().optString("recording").isEmpty(), "named recording stopped");
            waitFor(() -> { List<JSONObject> e = read(namedFile); return "session_end".equals(e.get(e.size() - 1).optString("type")); }, "named recording closed");
            JSONObject provenance = read(namedFile).get(0).optJSONObject("component");
            require(provenance != null && "cooling".equals(provenance.optString("id")) && provenance.getJSONArray("entries").length() >= 5, "REQ-035.4 the header carries the applied preset with its entry statuses: " + provenance);
            // a manual selection replaces the preset's: its status is cleared and never claimed again
            service.setSignals(user); waitFor(() -> sameSelection(service, user), "user selection");
            require(service.snapshot().getJSONObject("component").length() == 0, "a manual selection clears the stale preset status: " + service.snapshot().getJSONObject("component"));
            List<String> chain = smartWithOwnRecording(service, link[0], dir, vin, user);
            reconnect(service, link, false);
            continuingAndEnding(service, link, dir, vin, user);
            coverageRace(service, link, dir); // REQ-038.9 (#344 review): a rotation right before a raw read never misplaces its fact's evidence
            coverageNegativeResponse(service, link, dir); // REQ-038.9 (#344 review): a standard negative response is a rejection with its NRC; facts never copy error text
            coverageLedger(service, link, vin); // REQ-038.6/.8/.10 (#340): the vehicle ledger is live, stored, restored and never overwritten when unreadable or unwritable
            replay(service, link, dir, before, chain);
            coverage(service, link, dir, chain); // REQ-038.9 (#339): the same three files, as facts and as the rebuilt whole run
            coverageDiscovery(service, link, dir, chain); // REQ-038.9 (#344 review): parts are found anywhere in the folder; untrusted records invalidate
            require(service.snapshot().optJSONObject("coverage") != null, "REQ-038.9 replay coverage exists before leaving replay");
            require(service.snapshot().isNull("coverageLive") && service.snapshot().isNull("coverageStored"), "REQ-038.10 replay shows no run or stored vehicle coverage of the live session");
            connect(service);
            require(service.snapshot().isNull("coverage"), "REQ-038.9 a live connection cannot inherit replay coverage");
            open(service, chain.get(0));
            require(service.snapshot().optJSONObject("coverage") != null, "REQ-038.9 reopening derives coverage again");
            service.setDemo();
            waitFor(() -> "demo".equals(service.snapshot().optString("mode")) && "ready".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"), "demo after replay");
            require(service.snapshot().isNull("coverage"), "REQ-038.9 demo cannot inherit replay coverage");
            require(service.snapshot().isNull("coverageLive") && service.snapshot().isNull("coverageStored") && MainActivity.coverageKind(service.snapshot()).isEmpty(), "REQ-038.10 demo shows no run or stored vehicle coverage");
            // REQ-038.6 / REQ-013.9: deleting the record deletes its ledger; recordings stay
            require(service.vehicles.hasCoverageEntry(vin), "test setup: the record carries its ledger before it is deleted");
            // (#340 review) the record is deleted while a run credits it: nothing of it is shown or written afterwards
            service.setAutoConnect(false); reconnect(service, link, false);
            service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started before the vehicle is deleted");
            waitFor(() -> { JSONObject live = liveCoverage(service); return live != null && live.optBoolean("vehicle_attached") && entryCount(live.optJSONObject("run")) > 0; }, "the run credits the vehicle before it is deleted");
            Set<String> recordingsBefore = names(dir);
            service.deleteVehicle(vin);
            waitFor(() -> !service.vehicles.hasCoverageEntry(vin) && service.vehicles.coverageLedger(vin) == null, "REQ-038.6 the vehicle ledger is deleted with its record");
            waitFor(() -> { JSONObject live = liveCoverage(service); return live != null && !live.optBoolean("vehicle_attached"); }, "REQ-038.6 the run no longer credits the deleted vehicle");
            require(!MainActivity.coverageText(service.snapshot()).contains("Fahrzeug gesamt"), "REQ-038.6 no vehicle totals for a deleted vehicle: " + MainActivity.coverageText(service.snapshot()));
            SystemClock.sleep(1500); stopSmart(service);
            waitFor(() -> !service.snapshot().optBoolean("busy"), "idle after the run");
            require(!service.snapshot().optString("message").contains("Fahrzeug-Abdeckung nicht gespeichert") && !service.snapshot().toString().contains("Fahrzeug nicht gefunden"), "REQ-038.6 no false write failure: " + service.snapshot().optString("message"));
            require(!service.vehicles.hasCoverageEntry(vin) && service.vehicles.coverageLedger(vin) == null, "REQ-038.6 no ledger is written for the deleted vehicle");
            require(names(dir).equals(recordingsBefore), "REQ-038.6 deleting the record leaves the recordings unchanged");
        } finally {
            service.rotateAfterSmart = ""; service.rotateMs = 30L * 60 * 1000;
            service.disconnect(); service.connector = savedConnector; service.vehicles = savedStore; service.permissionOverride = savedPermission; service.bluetoothOverride = savedBluetooth;
            service.setAutoRecord(savedAutoRecord);
            service.setDemo(); waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
            service.setAutoConnect(true);
            Set<String> created = names(dir); created.removeAll(before);
            for (String n : created) new File(dir, n).delete();
        }
    }

    // ---- pure checks -----------------------------------------------------------------------------------------------------------------

    private static void presetsAndBlockers() throws Exception {
        JSONArray presets = ComponentPresets.list(); List<String> ids = new ArrayList<>();
        for (int i = 0; i < presets.length(); i++) ids.add(presets.getJSONObject(i).getString("id"));
        require(ids.equals(Arrays.asList("dpf", "egr", "cooling", "turbo", "rail")), "REQ-035.1 exactly the five built-in presets: " + ids);
        JSONObject live = new JSONObject().put("mode", "live").put("phase", "ready");
        require(ObdService.smartBlocker(live) == null, "REQ-036.1 a ready live session may start the analysis");
        require(ObdService.smartBlocker(new JSONObject().put("mode", "demo").put("phase", "ready")) != null, "REQ-036.1 refused in demo");
        require(ObdService.smartBlocker(new JSONObject().put("mode", "replay").put("phase", "replay")) != null, "REQ-036.1 refused in replay");
        require(ObdService.smartBlocker(new JSONObject().put("mode", "live").put("phase", "disconnected")) != null, "REQ-036.1 refused without a ready link");
        require(ObdService.smartBlocker(copy(live).put("recording", "x.jsonl")) != null, "REQ-036.1 refused while a named recording runs");
        require(ObdService.smartBlocker(copy(live).put("busy", true)) != null, "REQ-036.1 refused while a job runs");
        require(ObdService.smartBlocker(copy(live).put("uds", new JSONObject().put("active", true))) != null, "REQ-036.1 refused while the extended session is active");
        require(ObdService.smartBlocker(copy(live).put("uds", new JSONObject().put("state", "indeterminate"))) != null, "REQ-036.1 refused while the session state is unclear");
    }

    // ---- REQ-035 ---------------------------------------------------------------------------------------------------------------------

    private static void presets(ObdService service, Link[] link) throws Exception {
        Link l = link[0];
        // a preset without any session entry: selects, reports, sends no 10 03 (also not by connecting or discovering)
        String message = apply(service, "cooling");
        require(message.contains("angewendet") && message.contains("verfügbar"), "REQ-035.2 the message names the availability: " + message);
        JSONObject cooling = service.snapshot().getJSONObject("component");
        entriesAreAccountedFor(service.snapshot(), cooling, "cooling");
        require(count(l.commands, "1003") == 0, "REQ-035.3 neither connecting, discovery nor a preset without session entries sends 10 03: " + l.commands);
        require(!cooling.getJSONObject("uds").optBoolean("requested"), "REQ-035.3 no activation was requested for cooling");

        // REQ-027.7: a stored preset merely named like a component preset is an ordinary stored preset
        long base = seq(service);
        service.saveSignalPreset("component:dpf", new JSONArray().put("engine.rpm")); awaitMessage(service, base);
        base = seq(service); service.applySignalPreset("component:dpf"); String stored = awaitMessage(service, base);
        require(stored.startsWith("Vorlage angewendet"), "REQ-035.3 a saved preset named component:dpf is applied as a stored preset: " + stored);
        require(sameSelection(service, new JSONArray().put("engine.rpm")), "REQ-035.3 the stored selection was applied: " + service.snapshot().getJSONArray("selected"));
        require(count(l.commands, "1003") == 0, "REQ-035.3 a name collision never opens the extended session: " + l.commands);
        require(service.snapshot().getJSONObject("component").length() == 0, "REQ-035.4 a stored preset replaces the selection, so the component status is cleared (never claimed by headers or replay)");

        // the explicit apply opens the existing session exactly once
        int sessions = count(l.commands, "1003");
        message = apply(service, "dpf");
        require(count(l.commands, "1003") == sessions + 1, "REQ-035.3 exactly one 10 03 for the explicit apply: " + l.commands);
        require(service.snapshot().getJSONObject("uds").optBoolean("active"), "REQ-035.3 the existing session is active: " + message);
        JSONObject dpf = service.snapshot().getJSONObject("component");
        entriesAreAccountedFor(service.snapshot(), dpf, "dpf");
        require(dpf.getJSONObject("uds").optBoolean("requested") && dpf.getJSONObject("uds").optBoolean("active"), "REQ-035.4 the status records the preset-requested activation: " + dpf.getJSONObject("uds"));
        JSONObject surface = entry(dpf, SESSION_SIGNAL);
        require(surface != null && "selected".equals(surface.optString("status")) && surface.optBoolean("session") && surface.optString("key").contains("@"), "REQ-035.2 the session entry resolves to its unique key once active: " + surface);
        require(contains(service.snapshot().getJSONArray("selected"), surface.getString("key")) && contains(service.snapshot().getJSONArray("selected"), "engine.rpm"), "REQ-035.3 session and standard entries are selected together");

        // the analysis refuses to start while the session is active and sends nothing
        int udsSent = count(l.commands, "1003"), returned = count(l.commands, "1001");
        base = seq(service); service.startSmart(); message = awaitMessage(service, base);
        require(message.contains("Smart-Analyse nicht gestartet") && !service.snapshot().getJSONObject("smart").optBoolean("active"), "REQ-036.1 refused with a visible reason while the session runs: " + message);
        require(count(l.commands, "1003") == udsSent && count(l.commands, "1001") == returned, "REQ-036.1 the refusal sends no session service");

        // the session ends (another job): its entries become unavailable with the reason, the Mode 01 selection keeps polling
        long rpmAt = updatedAt(service, "engine.rpm");
        service.runJob("dtc.scan");
        waitFor(() -> !service.snapshot().optBoolean("busy") && !service.snapshot().getJSONObject("uds").optBoolean("active"), "session left by the job");
        JSONObject ended = entry(service.snapshot().getJSONObject("component"), SESSION_SIGNAL);
        require(ended != null && "uds_ended".equals(ended.optString("status")) && !ended.optString("detail").isEmpty(), "REQ-035.4 the entry is unavailable with the reason once the session ended: " + ended);
        require(!contains(service.snapshot().getJSONArray("selected"), surface.getString("key")) && contains(service.snapshot().getJSONArray("selected"), "engine.rpm"), "REQ-035.4 the session signal leaves the selection, the OBD2 selection stays");
        waitFor(() -> updatedAt(service, "engine.rpm") > rpmAt, "REQ-035.4 the OBD2 selection keeps polling after the session ended");
        require(count(l.commands, "1003") == udsSent, "REQ-035.3 the session is never re-armed or retried after it ended");

        // activation refused by the ECU: default entries stay selected, session entries are unavailable with the code, one attempt only
        l.failActivation = true; sessions = count(l.commands, "1003");
        message = apply(service, "dpf");
        SystemClock.sleep(1500);
        require(count(l.commands, "1003") == sessions + 1, "REQ-035.3 a refused activation is attempted once and never retried: " + l.commands);
        require(!service.snapshot().getJSONObject("uds").optBoolean("active"), "REQ-035.4 a refused activation stays in the default session");
        JSONObject refused = service.snapshot().getJSONObject("component"); JSONObject failedEntry = entry(refused, SESSION_SIGNAL);
        require(refused.getJSONObject("uds").optBoolean("requested") && !refused.getJSONObject("uds").optString("error").isEmpty(), "REQ-035.4 the status shows the failed activation: " + refused.getJSONObject("uds"));
        require(failedEntry != null && "uds_unavailable".equals(failedEntry.optString("status")) && !failedEntry.optString("detail").isEmpty() && !failedEntry.has("key"), "REQ-035.4 the session entry is unavailable with the reason and has no key: " + failedEntry);
        require(contains(service.snapshot().getJSONArray("selected"), "engine.rpm") && message.contains("UDS"), "REQ-035.4 the default-session entries are still selected and the message names the UDS failure: " + message);
        entriesAreAccountedFor(service.snapshot(), refused, "dpf");
        l.failActivation = false;

        // refused while a named recording runs, and the analysis too
        service.startRecording("live-selection"); waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "named recording");
        sessions = count(l.commands, "1003");
        base = seq(service); service.applyComponentPreset("egr"); message = awaitMessage(service, base);
        require(message.equals("Messwertauswahl während Aufnahme gesperrt") && "dpf".equals(service.snapshot().getJSONObject("component").optString("id")), "REQ-035.3 refused while a recording runs: " + message);
        base = seq(service); service.startSmart(); message = awaitMessage(service, base);
        require(message.contains("Smart-Analyse nicht gestartet") && message.contains("benannte"), "REQ-036.1 refused while a named recording runs: " + message);
        require(count(l.commands, "1003") == sessions, "REQ-035.3 a refused apply sends nothing");
        service.stopRecording(); waitFor(() -> service.snapshot().optString("recording").isEmpty(), "named recording stopped");
    }

    private static void entriesAreAccountedFor(JSONObject snapshot, JSONObject component, String id) throws Exception {
        require(id.equals(component.optString("id")) && !component.optString("label").isEmpty(), "REQ-035.1 the status names the preset: " + component.optString("id"));
        JSONArray entries = component.getJSONArray("entries"), selected = component.getJSONArray("selected"), unavailable = component.getJSONArray("unavailable");
        require(entries.length() == component.getInt("total") && entries.length() >= 5 && entries.length() <= 12, "REQ-035.1 5 to 12 entries: " + entries.length());
        require(selected.length() == component.getInt("available") && selected.length() + unavailable.length() == entries.length(), "REQ-035.2 every entry is selected or unavailable, none dropped");
        for (int i = 0; i < entries.length(); i++) {
            JSONObject e = entries.getJSONObject(i); String status = e.optString("status");
            require(STATUSES.contains(status), "REQ-035.2 every entry shows its own known status: " + e);
            require(!e.has("value"), "REQ-035.2 an entry never carries a value: " + e);
            boolean listed = contains(unavailable, e.getString("semantic")), picked = contains(snapshot.getJSONArray("selected"), e.optString("key"));
            if (status.equals("selected")) require(!listed && picked && e.optString("key").length() > 0, "REQ-035.2 a selected entry is in the selection: " + e);
            else require(listed, "REQ-035.2 an unavailable entry is listed with its reason: " + e);
        }
    }

    // ---- REQ-039 (the planner on the live session; REQ-036.1 end conditions) ----------------------------------------------------------

    /** Switch of the continuous recording off: the planner starts and ends its own recording. Returns the chain of files it wrote. */
    private static List<String> smartWithOwnRecording(ObdService service, Link l, File dir, String vin, JSONArray user) throws Exception {
        service.setAutoRecord(false);
        waitFor(() -> service.snapshot().optString("autoRecording").isEmpty() && !service.snapshot().optBoolean("autoRecord"), "continuous recording off");
        int sessions = count(l.commands, "1003"), returns = count(l.commands, "1001"), testers = count(l.commands, "3E80");
        Set<String> filesBefore = names(dir);
        service.startSmart();
        waitFor(() -> smart(service).optBoolean("active"), "smart analysis started");
        String owned = service.snapshot().optString("autoRecording");
        require(owned.endsWith("-auto.jsonl") && !service.snapshot().optBoolean("autoRecord"), "REQ-036.1 with the switch off the analysis writes its own recording: " + owned);
        waitFor(() -> service.snapshot().optString("message").contains("eigene Aufzeichnung"), "start message names the own recording");
        waitFor(() -> "serving".equals(goalPhase(service, "rail")) && "serving".equals(goalPhase(service, "baseline")), "REQ-039.2 the rail goal (standstill, warm, idle rpm) and the baseline are served");

        // REQ-039.4/.5: the monitored triggers are in the selection; the user's own selection is not part of it and is not touched
        JSONArray selected = service.snapshot().getJSONArray("selected");
        for (String t : new String[] {"engine.rpm", "vehicle.speed", "engine.coolant_temperature", PEDAL, "engine.load"}) require(contains(selected, t), "REQ-039.4 trigger " + t + " is selected: " + selected);
        require(containsMeaning(service, selected, REGEN), "REQ-039.4 the Knowledge trigger is selected by its key: " + selected);
        require(selected.length() <= 32 && !contains(selected, USER), "REQ-039.5 at most 32 signals, the user's selection is replaced for the run: " + selected);
        JSONObject view = smart(service);
        require(view.optString("recording").equals(owned) && view.optJSONArray("goals").length() > 0 && view.optInt("selected_count") == selected.length(), "REQ-039.8 the view shows the goals and the recording: " + view);
        String summary = MainActivity.smartSummaryText(service.snapshot());
        require(summary.contains(MainActivity.goalLabel("rail")) && summary.contains("bis die Bedingung endet") && !summary.contains("Komponente"), "REQ-039.8 the card names the goal with its reason and remaining time: " + summary);
        // the selection belongs to the planner while it runs
        long base = seq(service); service.setSignals(new JSONArray().put("engine.rpm")); String refused = awaitMessage(service, base);
        require(refused.contains("Smart-Analyse") && sameSelection(service, selected), "REQ-036.1 the selection is locked while the analysis runs: " + refused);
        base = seq(service); service.applyComponentPreset("egr"); refused = awaitMessage(service, base);
        require(refused.contains("Smart-Analyse"), "REQ-035.3 no component preset (and no session) while the analysis runs: " + refused);
        base = seq(service); service.applySignalPreset("component:dpf"); refused = awaitMessage(service, base);
        require(refused.contains("Smart-Analyse"), "a stored preset is refused while the analysis runs: " + refused);

        // REQ-039.4: a nonzero raw request starts the dpf goal at once and ends with the first fresh 0
        l.regen = 1; waitFor(() -> isActive(goalPhase(service, "dpf.raw_request")), "REQ-039.4 a raw request shows the dpf goal");
        JSONObject change = lastChangeOf(service, "dpf.raw_request");
        require("started".equals(change.optString("kind")) && change.optLong("at") > 0, "REQ-039.8 the goal start is shown with its time: " + change);
        require(MainActivity.smartSummaryText(service.snapshot()).contains(MainActivity.REGEN_REQUEST_LABEL), "REQ-039.4 the request is shown as a raw request status");
        l.regen = 0; waitFor(() -> !isActive(goalPhase(service, "dpf.raw_request")), "REQ-039.12 the dpf goal ends with a fresh reading of 0");
        change = lastChangeOf(service, "dpf.raw_request");
        require("expired".equals(change.optString("kind")) && "condition_ended".equals(change.optString("reason")), "REQ-039.12 the goal ends because its condition ended: " + change);
        // REQ-039.12: pressing the pedal starts the turbo goal (rotation of the file inside that very write)
        service.rotateAfterSmart = "started"; l.pedal = "FF";
        waitFor(() -> isActive(goalPhase(service, "turbo")), "REQ-039.2 the pedal starts the turbo goal");
        require(service.rotateAfterSmart.isEmpty(), "the forced rotation happened inside the record of the change");
        waitFor(() -> !service.snapshot().optString("autoRecording").equals(owned), "the recording rotated while the analysis runs");
        require(smart(service).optBoolean("active"), "REQ-036.5 the analysis continues across the file rotation");

        // REQ-039.6: a trigger reading that is missing makes its condition unknown: the goal ends with that reason, never as a release
        l.pedalMissing = true;
        waitFor(() -> !isActive(goalPhase(service, "turbo")), "REQ-039.6 an unanswered pedal reading ends the turbo goal");
        change = lastChangeOf(service, "turbo");
        require("expired".equals(change.optString("kind")) && !"condition_ended".equals(change.optString("reason")) && !change.optString("reason").isEmpty(), "REQ-039.6 an unknown condition is no release: " + change);
        l.pedalMissing = false; l.pedal = "20";

        // stop: the stop event is the last event of its file, even when the recording rotates in that very write; nothing the planner chose was stored
        service.rotateAfterSmart = "stopped"; service.stopSmart();
        waitFor(() -> !smart(service).optBoolean("active") && service.snapshot().optString("autoRecording").isEmpty(), "analysis stopped and own recording closed");
        require(service.snapshot().optString("message").startsWith("Smart-Analyse beendet") && smart(service).optString("reason").length() > 0, "REQ-039.1 the end is visible with its reason: " + service.snapshot().optString("message"));
        require(sameSelection(service, user) && sameSelection(service.vehicles.selection(vin), user), "REQ-036.1 the user's selection (shown and stored) is back: " + service.snapshot().getJSONArray("selected"));
        require(count(l.commands, "1003") == sessions && count(l.commands, "1001") == returns && count(l.commands, "3E80") == testers, "REQ-036.1 the analysis never touches the session: " + l.commands);
        require(smart(service).optJSONArray("changes").length() > 0, "REQ-039.8 the latest changes stay visible after the stop");

        List<String> chain = new ArrayList<>(names(dir)); chain.removeAll(filesBefore); chain.removeIf(n -> !n.endsWith("-auto.jsonl")); Collections.sort(chain);
        require(chain.size() == 3 && chain.get(0).equals(owned), "own recording rotated twice (turbo start, stop): " + chain);
        List<List<JSONObject>> files = new ArrayList<>(); for (String n : chain) files.add(read(new File(dir, n)));
        for (List<JSONObject> events : files) {
            require("session".equals(events.get(0).optString("type")) && "session_end".equals(events.get(events.size() - 1).optString("type")), "every file has its header and end marker");
            for (JSONObject e : events) require(!"uds.enable".equals(e.optString("action")), "REQ-036.1 no extended session in the file");
        }
        JSONObject headerComponent = files.get(0).get(0).optJSONObject("component");
        require(headerComponent == null || headerComponent.length() == 0, "REQ-035.4 a preset replaced by a manual selection is not claimed by the next header: " + headerComponent);
        // REQ-039.8: every plan change with wallclock time, monotonic offset, goal, reason, the condition readings and the definition versions, in order across the files
        List<String> sequence = new ArrayList<>(); long lastOffset;
        for (int f = 0; f < files.size(); f++) { lastOffset = -1;
            for (JSONObject e : files.get(f)) { JSONObject data = smartData(e); if (data == null) continue;
                require(e.has("at") && e.optLong("offset_us", -1) >= lastOffset, "REQ-039.8 wall time and monotonic offset on every change, ascending: " + e); lastOffset = e.optLong("offset_us");
                String kind = data.optString("kind");
                require(data.optInt("goal_definition", -1) == 2 && !data.optString("knowledge_revision").isEmpty() && data.has("inputs") && data.has("reason") && data.has("goal"), "REQ-039.8 every change names the definition versions, the inputs, the goal and the reason: " + data);
                JSONArray inputs = data.getJSONArray("inputs");
                for (int i = 0; i < inputs.length(); i++) { JSONObject input = inputs.getJSONObject(i);
                    require(input.isNull("value") != input.isNull("reason"), "REQ-039.6 an unknown condition reading carries its reason and never a value, zero included: " + input); }
                if (!kind.equals("selection") && !kind.equals("resume") && !kind.equals("skipped")) sequence.add(kind + ":" + (data.isNull("goal") ? "" : data.optString("goal"))); } }
        int railAt = sequence.indexOf("started:rail"), dpfAt = sequence.indexOf("started:dpf.raw_request"), dpfEnd = sequence.indexOf("expired:dpf.raw_request"), turboAt = sequence.indexOf("started:turbo"), turboEnd = sequence.indexOf("expired:turbo");
        require(railAt >= 0 && railAt < dpfAt && dpfAt < dpfEnd && dpfEnd < turboAt && turboAt < turboEnd, "REQ-039.8 start, the raw request, its end, the pedal and its unknown end in order: " + sequence);
        require(sequence.get(sequence.size() - 1).equals("stopped:"), "REQ-039.1 the stop ends the sequence: " + sequence);
        JSONObject dpfEvent = null; for (JSONObject e : files.get(0)) { JSONObject d = smartData(e); if (d != null && "started".equals(d.optString("kind")) && "dpf.raw_request".equals(d.optString("goal"))) dpfEvent = d; }
        JSONArray dpfInputs = dpfEvent.getJSONArray("inputs"); boolean regenInput = false;
        for (int i = 0; i < dpfInputs.length(); i++) regenInput |= dpfInputs.getJSONObject(i).optString("signal").startsWith(REGEN) && dpfInputs.getJSONObject(i).optDouble("value") == 1.0;
        require(regenInput && dpfEvent.getLong("at_us") > 0, "REQ-039.8 the change records the condition reading behind it: " + dpfEvent);
        int reads = 0; for (JSONObject e : files.get(0)) if ("read".equals(e.optString("action")) && e.optString("argument").startsWith(REGEN)) reads++;
        require(reads >= 3, "REQ-039.8 the raw read events stay in the recording: " + reads);
        // the rotation inside the record of the turbo start continues with the goals and the selection of that very step; the stop is last and nothing resumes after it
        JSONObject first = firstSmart(files.get(1));
        require(first != null && "resume".equals(first.optString("kind")) && first.getJSONArray("goals").length() > 0 && first.getJSONArray("selected").length() > 0, "REQ-039.8 the continuation file starts with the plan it continues: " + first);
        List<JSONObject> second = files.get(1); JSONObject lastSmart = null; for (JSONObject e : second) if (smartData(e) != null) lastSmart = smartData(e);
        require("stopped".equals(lastSmart.optString("kind")) && lastSmart.isNull("goal") && "rotated".equals(second.get(second.size() - 1).optString("reason")), "REQ-039.8 the stop is the last change of its file: " + lastSmart);
        require(firstSmart(files.get(2)) == null && "stopped".equals(files.get(2).get(files.get(2).size() - 1).optString("reason")), "REQ-036.5 a rotation inside the stop never resumes the analysis");
        require(sameSelection(files.get(2).get(0).optJSONArray("selected"), new JSONArray().put(USER)), "REQ-036.1 the file that a rotation inside the stop write opens starts with the user's restored selection: " + files.get(2).get(0).optJSONArray("selected"));
        service.setAutoRecord(true);
        return chain;
    }

    /** Continuous recording on: the analysis joins the running file; a job, a lost link and a disconnect each end it with a stop event before the end marker. */
    private static void continuingAndEnding(ObdService service, Link[] link, File dir, String vin, JSONArray user) throws Exception {
        Link l = link[0];
        String running = service.snapshot().optString("autoRecording"); require(!running.isEmpty() && service.snapshot().optBoolean("autoRecord"), "continuous recording runs");
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started in the running recording");
        require(running.equals(service.snapshot().optString("autoRecording")) && !service.snapshot().optString("message").contains("eigene Aufzeichnung"), "REQ-036.1 the analysis continues the running recording");
        waitFor(() -> "not_reachable".equals(goalPhase(service, "dpf.raw_request")), "REQ-039.7 a goal whose condition input is unavailable is reported as not reachable");
        JSONObject regen = goalRow(service, "dpf.raw_request");
        require("condition_input_unavailable".equals(regen.optString("reason")) && MainActivity.smartSummaryText(service.snapshot()).contains("nicht erreichbar"), "REQ-039.7 the reason is shown, never zero or a healthy state: " + regen);
        service.runJob("dtc.scan");
        waitFor(() -> !smart(service).optBoolean("active") && !service.snapshot().optBoolean("busy"), "a job ends the analysis");
        require(smart(service).optString("reason").contains("Auftrag") && sameSelection(service, user) && running.equals(service.snapshot().optString("autoRecording")), "REQ-036.1 a job ends it, the selection is back and the recording goes on: " + smart(service).optString("reason"));

        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started again");
        l.drop(); // the link is lost: the analysis ends, the recording stays partial
        waitFor(() -> "error".equals(service.snapshot().optString("phase")) && !smart(service).optBoolean("active"), "lost link ends the analysis");
        require(sameSelection(service, user) && sameSelection(service.vehicles.selection(vin), user), "REQ-036.1 the user's selection survives a lost link");
        endsWithStop(new File(dir, running), "error", "error");

        reconnect(service, link, false); l = link[0];
        String next = service.snapshot().optString("autoRecording");
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started on the new connection");
        require(count(l.commands, "1003") == 0 && count(l.commands, "1001") == 0, "REQ-036.1 neither connecting nor starting the analysis opens a session");
        service.disconnect(); waitFor(() -> "disconnected".equals(service.snapshot().optString("phase")) && !smart(service).optBoolean("active") && service.snapshot().optString("autoRecording").isEmpty(), "disconnect ends the analysis");
        waitFor(() -> sameSelection(service, user), "selection restored after the disconnect");
        endsWithStop(new File(dir, next), "disconnected", "disconnect");
        require(sameSelection(service.vehicles.selection(vin), user) && count(l.commands, "1003") == 0, "REQ-036.1 the stored selection is the user's, no session was ever opened");
        // every other way out of the live session ends a running analysis: opening a recording and switching to the demo (a restart of the app keeps nothing of it)
        reconnect(service, link, false); l = link[0];
        String viaReplay = service.snapshot().optString("autoRecording");
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started before a replay is opened");
        open(service, running);
        require(!smart(service).optBoolean("active") && !service.snapshot().optBoolean("busy"), "REQ-036.1 opening a recording ends the analysis: " + smart(service));
        endsWithStop(new File(dir, viaReplay), "disconnected", "disconnect");
        reconnect(service, link, false); l = link[0];
        String viaDemo = service.snapshot().optString("autoRecording");
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started before the demo");
        service.setDemo();
        waitFor(() -> "demo".equals(service.snapshot().optString("mode")) && "ready".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"), "demo");
        require(!smart(service).optBoolean("active") && !MainActivity.smartSummaryText(service.snapshot()).startsWith("Aktiv"), "REQ-036.1 the demo cannot run the analysis: " + smart(service));
        endsWithStop(new File(dir, viaDemo), "disconnected", "disconnect");
        long refusedBase = seq(service); service.startSmart(); String refusedInDemo = awaitMessage(service, refusedBase);
        require(refusedInDemo.contains("Smart-Analyse nicht gestartet") && refusedInDemo.contains("Demo") && !smart(service).optBoolean("active"), "REQ-036.1 a start in the demo is refused with its reason: " + refusedInDemo);
        reconnect(service, link, false);
    }

    private static void endsWithStop(File file, String reason, String token) throws Exception {
        waitFor(() -> { List<JSONObject> e = read(file); return "session_end".equals(e.get(e.size() - 1).optString("type")); }, "recording closed " + file.getName());
        List<JSONObject> events = read(file); JSONObject end = events.get(events.size() - 1);
        require(reason.equals(end.optString("reason")), "the recording ends with " + reason + ": " + end);
        int at = events.size() - 2; // REQ-038.9: the run's own end record (coverage.boundary run_end) may follow the stop event; nothing else may
        if ("coverage.boundary".equals(events.get(at).optString("action")) && "run_end".equals(events.get(at).getJSONObject("result").getJSONObject("data").optString("class"))) at--;
        JSONObject before = events.get(at), data = smartData(before);
        require(data != null && "stopped".equals(data.optString("kind")) && data.isNull("goal") && token.equals(data.optString("reason")) && before.optLong("offset_us") <= end.optLong("offset_us"), "REQ-039.8 the stop event with its reason comes right before the end marker (or the run's end record): " + before);
    }

    // ---- replay ----------------------------------------------------------------------------------------------------------------------

    private static void replay(ObdService service, Link[] link, File dir, Set<String> before, List<String> chain) throws Exception {
        // the preset: the last component.apply of the first connection (the refused activation of dpf)
        List<String> all = new ArrayList<>(names(dir)); all.removeAll(before); Collections.sort(all); JSONObject lastApply = null;
        for (String n : all) for (JSONObject e : read(new File(dir, n))) if ("component.apply".equals(e.optString("action")) && "dpf".equals(e.getJSONObject("result").getJSONObject("data").optString("id"))) lastApply = e.getJSONObject("result").getJSONObject("data");
        require(lastApply != null, "REQ-035.4 the apply was recorded");
        String applyFile = null; for (String n : all) for (JSONObject e : read(new File(dir, n))) if ("component.apply".equals(e.optString("action")) && "dpf".equals(e.getJSONObject("result").getJSONObject("data").optString("id"))) applyFile = n;
        open(service, applyFile);
        SystemClock.sleep(1500); int traffic = link[0].commands.size(); // the live link just closed: a polling read still in flight at that moment is not replay traffic; everything after it is checked
        JSONObject component = service.snapshot().getJSONObject("component");
        require(component.optBoolean("replay") && lastApply.getString("id").equals(component.optString("id")) && component.getJSONObject("uds").optBoolean("requested") && !component.getJSONObject("uds").optString("error").isEmpty()
            && "uds_unavailable".equals(entry(component, SESSION_SIGNAL).optString("status")), "REQ-035.4 replay shows the preset, its statuses and the failed activation without traffic: " + component);
        // the analysis: first file of the own recording, its continuation and the stop file
        open(service, chain.get(0));
        JSONObject view = smart(service); List<JSONObject> events = read(new File(dir, chain.get(0))); JSONObject lastSelection = null; int count = 0;
        for (JSONObject e : events) { JSONObject d = smartData(e); if (d == null) continue; if ("selection".equals(d.optString("kind"))) lastSelection = d; else if (!"skipped".equals(d.optString("kind")) && !"resume".equals(d.optString("kind"))) count++; }
        require(view.optBoolean("replay") && !view.optBoolean("active") && view.optJSONArray("changes").length() == Math.min(count, 20) && isActive(goalPhase(service, "turbo")) && "serving".equals(goalPhase(service, "baseline"))
            && service.snapshot().getJSONObject("component").length() == 0, "REQ-039.8 replay shows the recorded changes and goals and claims no stale preset: " + view);
        require(lastSelection != null && sameSelection(service, lastSelection.getJSONArray("selected")), "REQ-039.8 replay selects the recorded plan");
        JSONArray changes = view.getJSONArray("changes"); JSONObject lastShown = changes.getJSONObject(changes.length() - 1);
        require("started".equals(lastShown.optString("kind")) && "turbo".equals(lastShown.optString("goal")) && lastShown.optLong("at") > 0, "REQ-039.8 the history names goal, class and time of each change: " + lastShown);
        require(MainActivity.smartSummaryText(service.snapshot()).startsWith("Aus der Aufnahme") && MainActivity.smartSummaryText(service.snapshot()).contains(MainActivity.goalLabel("turbo")), "REQ-039.8 the card says it shows a recording: " + MainActivity.smartSummaryText(service.snapshot()));
        open(service, chain.get(1));
        view = smart(service); changes = view.getJSONArray("changes");
        require("stopped".equals(changes.getJSONObject(changes.length() - 1).optString("kind")) && "user".equals(view.optString("reason")) && view.getJSONArray("goals").length() == 0 && sameSelection(service, new JSONArray().put(USER)), "REQ-039.8 the continuation file replays ... stop and the user's selection: " + changes);
        open(service, chain.get(2));
        require(sameSelection(service, new JSONArray().put(USER)) && smart(service).optJSONArray("changes").length() == 0 && !smart(service).optBoolean("active"), "REQ-036.1 replay of the file after the stop shows the restored selection and no analysis: " + service.snapshot().getJSONArray("selected"));
        // REQ-039.8: legacy set changes are skipped visibly, without sending an adapter command.
        String legacy = "synthetic-legacy-smart.jsonl";
        JSONObject header = read(new File(dir, chain.get(2))).get(0);
        JSONObject oldSet = new JSONObject().put("type", "action").put("action", "smart.set").put("result", new JSONObject().put("ok", true));
        JSONObject end = new JSONObject().put("type", "session_end").put("reason", "stopped");
        java.nio.file.Files.write(new File(dir, legacy).toPath(), (header.toString()+"\n"+oldSet+"\n"+end+"\n").getBytes(StandardCharsets.UTF_8));
        open(service, legacy);
        require(MainActivity.smartSummaryText(service.snapshot()).contains("Ältere Smart-Analyse in dieser Aufnahme nicht angezeigt"), "REQ-039.8 legacy analysis skip is visible in replay");
        open(service, chain.get(2));
        require(!MainActivity.smartSummaryText(service.snapshot()).contains("Ältere Smart-Analyse"), "REQ-039.8 the legacy note does not leak to another replay");
        require(link[0].commands.size() == traffic, "replay sent nothing to an adapter: " + link[0].commands.subList(traffic, link[0].commands.size()));
    }

    /**
     * REQ-038.9 (#339): the run of the smart analysis that rotated twice wrote its coverage facts into all three files, as one run in three parts with
     * verified joins and no value; replay opened on any part rebuilds the whole run from the set of parts, and with the middle part deleted it says partial
     * with the missing part number. Replay writes nothing and sends nothing.
     */
    private static void coverage(ObdService service, Link[] link, File dir, List<String> chain) throws Exception {
        int traffic = link[0].commands.size();
        List<List<JSONObject>> files = new ArrayList<>(); for (String n : chain) files.add(read(new File(dir, n)));
        String run = null; int[] counts = new int[files.size()]; int facts = 0;
        Set<String> contexts = new HashSet<>();
        for (int f = 0; f < files.size(); f++) {
            Set<Long> reads = new HashSet<>(); for (JSONObject e : files.get(f)) if ("read".equals(e.optString("action"))) reads.add(e.optLong("offset_us"));
            List<JSONObject> own = new ArrayList<>(); for (JSONObject e : files.get(f)) if (e.optString("action").startsWith("coverage.")) own.add(e);
            counts[f] = own.size(); require(own.size() >= 2, "REQ-038.9 file " + f + " holds coverage records: " + own.size());
            JSONObject first = own.get(0), firstData = first.getJSONObject("result").getJSONObject("data");
            require("coverage.run".equals(first.optString("action")) && firstData.optInt("part") == f + 1 && (f == 0 ? "start" : "resume").equals(firstData.optString("action")), "REQ-038.9 file " + f + " begins with its part marker: " + first);
            if (f > 0) require(firstData.optInt("previous_part_records", -1) == counts[f - 1], "REQ-038.9 the resume states the records of the previous part (" + counts[f - 1] + "): " + firstData);
            long lastOffset = -1; boolean ended = false;
            for (JSONObject e : own) {
                JSONObject result = e.getJSONObject("result"), data = result.getJSONObject("data");
                require(result.optBoolean("ok") && e.has("at") && data.has("run_offset_us") && data.optLong("run_offset_us") >= lastOffset, "REQ-038.9 wall time, ok and an ascending run clock on every record: " + e); lastOffset = data.optLong("run_offset_us");
                if (run == null) run = data.getString("run"); require(run.equals(data.getString("run")), "REQ-038.9 one run id in all parts");
                require(!data.has("value") && !data.has("response") && !data.has("vin"), "REQ-038.9 facts carry classes and references, never a value: " + e);
                if ("coverage.fact".equals(e.optString("action"))) {
                    facts++; JSONObject evidence = data.getJSONObject("evidence");
                    contexts.add(data.optString("context"));
                    require(GOAL_CONTEXTS.contains(data.optString("context")) && data.optInt("goal_definition") == 2 && !data.optString("knowledge_revision").isEmpty() && !data.optString("outcome").isEmpty()
                        && data.has("session") && data.has("identity_confirmed") && data.has("implausible") && data.has("static_identity"), "REQ-038.9 the fact has every field of the ledger: " + e);
                    if (!evidence.optBoolean("in_previous_file")) require(reads.contains(evidence.getLong("offset_us")), "REQ-038.9 the evidence reference names the raw read record in the same file: " + e);
                }
                if ("coverage.boundary".equals(e.optString("action")) && "run_end".equals(data.optString("class"))) { require(f == files.size() - 1 && !ended, "REQ-038.9 run_end only once, in the last part"); ended = true; }
            }
            require(ended == (f == files.size() - 1), "REQ-038.9 only the last part ends the run: file " + f);
        }
        require(facts > 0, "REQ-038.9 the reads of the run left facts");
        require(contexts.contains("baseline") && contexts.size() > 1, "REQ-039.2 a read counts toward every goal context whose condition holds and always toward the baseline: " + contexts);
        for (int f : new int[] {1, 0, 2}) {
            open(service, chain.get(f)); JSONObject set = service.snapshot().optJSONObject("coverage");
            require(set != null && set.getJSONArray("runs").length() == 1, "REQ-038.9 replay of part " + (f + 1) + " derives the run: " + set);
            JSONObject derived = set.getJSONArray("runs").getJSONObject(0);
            require(derived.optBoolean("run_complete") && !derived.optBoolean("partial") && "exact".equals(derived.optString("figures")) && derived.getJSONArray("parts").length() == 3
                && derived.getJSONArray("entries").length() > 0 && derived.getJSONArray("joins").getJSONObject(0).optBoolean("verified") && derived.getJSONArray("joins").getJSONObject(1).optBoolean("verified"),
                "REQ-038.9 replay of any part rebuilds the whole run, complete with exact figures: " + derived);
        }
        require(new File(dir, chain.get(1)).delete(), "the middle part can be deleted (REQ-028.7)");
        for (int f : new int[] {0, 2}) {
            open(service, chain.get(f)); JSONObject derived = service.snapshot().getJSONObject("coverage").getJSONArray("runs").getJSONObject(0);
            require(derived.optBoolean("partial") && !derived.optBoolean("run_complete") && "supplied_parts_only".equals(derived.optString("figures")) && derived.getJSONArray("missing_parts").length() == 1
                && derived.getJSONArray("missing_parts").optInt(0) == 2 && derived.getJSONArray("parts").length() == 2, "REQ-038.9 without the middle part the run is partial and names part 2: " + derived);
        }
        require(link[0].commands.size() == traffic, "REQ-038.9 replay sent nothing to an adapter: " + link[0].commands.subList(traffic, link[0].commands.size()));
    }

    // ---- REQ-038.9, review of #344 ---------------------------------------------------------------------------------------------------

    /**
     * A rotation by another thread right before a raw read is written: the raw record and the file serial are taken under one lock, so the fact of
     * that read is not flagged as living in the previous file, and every flag of every fact names the file that really holds the raw read. Live,
     * simulated adapter, no session; the seam only rotates the open automatic file.
     */
    private static void coverageRace(ObdService service, Link[] link, File dir) throws Exception {
        Set<String> known = names(dir);
        reconnect(service, link, false);
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started for the rotation race");
        waitFor(() -> smart(service).optJSONArray("goals").length() > 0, "goals before the race");
        String owned = service.snapshot().optString("autoRecording"); require(!owned.isEmpty(), "the run records into an automatic file");
        service.rotateBeforeRead = true;
        waitFor(() -> !service.rotateBeforeRead && !owned.equals(service.snapshot().optString("autoRecording")), "the file rotated right before a raw read record");
        Thread.sleep(1500); // a few more reads land in the new file
        service.disconnect(); waitFor(() -> "disconnected".equals(service.snapshot().optString("phase")) && service.snapshot().optString("autoRecording").isEmpty(), "disconnected after the race");
        List<String> made = new ArrayList<>(names(dir)); made.removeAll(known); made.removeIf(n -> !n.endsWith("-auto.jsonl")); Collections.sort(made);
        require(made.size() == 2 && made.get(0).equals(owned), "the seam rotated exactly once: " + made);
        List<List<JSONObject>> files = new ArrayList<>(); for (String n : made) files.add(read(new File(dir, n)));
        JSONObject raced = null; for (JSONObject e : files.get(1)) if ("read".equals(e.optString("action"))) { raced = e; break; }
        require(raced != null, "the raw read that followed the rotation is the first read of the new file");
        boolean found = false;
        for (int f = 0; f < files.size(); f++) for (JSONObject e : files.get(f)) {
            if (!"coverage.fact".equals(e.optString("action"))) continue;
            JSONObject evidence = e.getJSONObject("result").getJSONObject("data").getJSONObject("evidence"); boolean previous = evidence.optBoolean("in_previous_file");
            require(holdsRead(files.get(f), evidence) == !previous && (!previous || (f > 0 && holdsRead(files.get(f - 1), evidence))),
                "REQ-038.9 the evidence flag names the file that holds the raw read (file " + f + ", previous " + previous + "): " + e);
            if (f == 1 && evidence.optLong("at") == raced.optLong("at") && evidence.optLong("offset_us") == raced.optLong("offset_us")) { found = true; require(!previous, "REQ-038.9 the fact of the raced read is in the same file as its raw read: " + e); }
        }
        require(found, "REQ-038.9 the raced raw read has its fact in the new file");
    }
    private static boolean holdsRead(List<JSONObject> events, JSONObject evidence) {
        for (JSONObject e : events) if ("read".equals(e.optString("action")) && e.optLong("at") == evidence.optLong("at") && e.optLong("offset_us") == evidence.optLong("offset_us")) return true;
        return false;
    }

    private static final Set<String> CLOSED_REASONS = new HashSet<>(Arrays.asList("timeout", "no_data", "session_state", "malformed_response", "read_failed", "no_usable_answer", "undecodable_value", "classification_failed"));

    /** All records of the automatic files made since {@code known}, in file order. */
    private static List<JSONObject> autoEvents(File dir, Set<String> known) throws Exception {
        List<String> made = new ArrayList<>(names(dir)); made.removeAll(known); made.removeIf(n -> !n.endsWith("-auto.jsonl")); Collections.sort(made);
        List<JSONObject> events = new ArrayList<>(); for (String n : made) events.addAll(read(new File(dir, n)));
        return events;
    }

    /**
     * REQ-038.9 (#339, review of #344): a standard negative response to a Mode 01 read (7F 01 12) is a rejection with its code in the fact, not a failed
     * read, because the core passes it on as structure; and no fact of the run copies the wording of an error or of a wire line - a reason, when there is
     * one, comes from the closed set. Live, simulated adapter, no session; the pedal input answers 7F 01 12 for a while.
     */
    private static void coverageNegativeResponse(ObdService service, Link[] link, File dir) throws Exception {
        Set<String> known = names(dir);
        reconnect(service, link, false);
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started for the negative response");
        waitFor(() -> smart(service).optJSONArray("goals").length() > 0, "goals before the negative response");
        link[0].pedalRejected = true;
        waitFor(() -> { for (JSONObject e : autoEvents(dir, known)) if (rejectedPedal(e)) return true; return false; }, "the rejected pedal read left its fact");
        link[0].pedalRejected = false;
        waitFor(() -> { for (JSONObject e : autoEvents(dir, known)) if ("read".equals(e.optString("action")) && e.getJSONObject("result").optBoolean("ok") && PEDAL.equals(e.optString("argument"))) return true; return false; }, "the pedal answers again");
        service.disconnect(); waitFor(() -> "disconnected".equals(service.snapshot().optString("phase")) && service.snapshot().optString("autoRecording").isEmpty(), "disconnected after the negative response");
        List<JSONObject> events = autoEvents(dir, known); int rejected = 0;
        for (JSONObject e : events) {
            if (!"coverage.fact".equals(e.optString("action"))) continue;
            JSONObject data = e.getJSONObject("result").getJSONObject("data"); String text = e.toString();
            require(!text.contains("OBD-II") && !text.contains("NO DATA") && !text.contains("ELM327") && !data.has("error") && !data.has("negative_response"), "REQ-038.9 a fact never copies the text of an error or a wire line: " + e);
            if (data.has("reason")) require(CLOSED_REASONS.contains(data.optString("reason")), "REQ-038.9 a reason comes from the closed set: " + e);
            if (!rejectedPedal(e)) continue;
            rejected++;
            require(data.optInt("response_code", -1) == 0x12 && !data.has("reason"), "REQ-038.9 the negative response is a rejection with its code, not an error: " + e);
            JSONObject evidence = data.getJSONObject("evidence"); boolean raw = false;
            for (JSONObject r : events) if ("read".equals(r.optString("action")) && r.optLong("at") == evidence.optLong("at") && r.optLong("offset_us") == evidence.optLong("offset_us")) {
                JSONObject negative = r.getJSONObject("result").optJSONObject("negative_response"); raw = negative != null && negative.optInt("service") == 1 && negative.optInt("nrc") == 0x12;
            }
            require(raw, "REQ-038.9 the evidence reference names the raw read that carries the structured negative response: " + e);
        }
        require(rejected > 0, "REQ-038.9 the rejected reads left facts");
    }
    // ---- #340 (REQ-038.6/.8/.10) --------------------------------------------------------------------------------------------------
    private static File storeFile;
    private static JSONObject liveCoverage(ObdService service) { return service.snapshot().optJSONObject("coverageLive"); }
    private static int entryCount(JSONObject summary) throws Exception { return summary == null || summary.optJSONArray("entries") == null ? 0 : summary.getJSONArray("entries").length(); }
    private static int samples(JSONObject ledger) throws Exception { int n = 0; JSONArray entries = ledger.getJSONArray("entries"); for (int i = 0; i < entries.length(); i++) n += entries.getJSONObject(i).optInt("qualifying_samples"); return n; }
    private static void stopSmart(ObdService service) throws Exception { service.stopSmart(); waitFor(() -> !smart(service).optBoolean("active"), "smart analysis stopped"); }
    /**
     * REQ-038.6/.8/.10 through the service: the run and vehicle ledgers are live on the Live tab, persisted with the vehicle record, kept across an app
     * restart and a second run, never overwritten when unreadable or unwritable, and removed with the record. Simulated adapter, no vehicle traffic.
     */
    private static void coverageLedger(ObdService service, Link[] link, String vin) throws Exception {
        reconnect(service, link, false);
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started for the ledger");
        waitFor(() -> { JSONObject live = liveCoverage(service); return live != null && entryCount(live.optJSONObject("run")) > 0; }, "REQ-038.10 the run ledger fills while the run is on");
        JSONObject live = liveCoverage(service);
        require(live.optBoolean("vehicle_attached") && !live.optBoolean("session_only") && live.optString("problem").isEmpty(), "REQ-038.1/.10 the confirmed vehicle's ledger is attached: " + live);
        String text = MainActivity.coverageText(service.snapshot());
        require(text.contains("Dieser Lauf · Technische Basisabdeckung") && text.contains("Fahrzeug gesamt · Technische Basisabdeckung") && text.contains("abgedeckt") && text.contains("Messzeit") && text.contains("längstes Segment"),
            "REQ-038.10 the Live card names the baseline label, counts, measured time and longest segment: " + text);
        stopSmart(service);
        waitFor(() -> service.vehicles.coverageLedger(vin) != null, "REQ-038.6 the vehicle ledger is stored when the run ends");
        int first = samples(service.vehicles.coverageLedger(vin));
        require(first > 0 && entryCount(service.snapshot().optJSONObject("vehicles")) == 0 && !service.snapshot().toString().contains("coverage_ledger"), "REQ-038.6 the ledger is stored with the record and never rides in the service state: " + first);
        VehicleStore reopened = new VehicleStore(storeFile);
        require(reopened.coverageLedger(vin) != null && samples(reopened.coverageLedger(vin)) == first, "REQ-038.6 the ledger survives an app restart");
        waitFor(() -> { JSONObject stored = service.snapshot().optJSONObject("coverageStored"); return stored != null && stored.optJSONObject("vehicle") != null; }, "REQ-038.10 the stored ledger is shown while no run is on");
        require(MainActivity.coverageText(service.snapshot()).contains("Fahrzeug gesamt (gespeichert)"), "REQ-038.10 stored coverage is labeled as stored");

        // a later run adds to the same ledger
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "second run");
        waitFor(() -> { JSONObject l = liveCoverage(service); return l != null && entryCount(l.optJSONObject("vehicle")) > 0; }, "second run ledger");
        SystemClock.sleep(1500); stopSmart(service);
        waitFor(() -> { try { return samples(service.vehicles.coverageLedger(vin)) > first; } catch (Exception e) { return false; } }, "REQ-038.6 the second run adds to the vehicle ledger");

        // REQ-038.8: an unreadable ledger is shown as unavailable, the run goes on, the stored data stays as it is
        JSONObject good = service.vehicles.coverageLedger(vin);
        JSONObject unreadable = new JSONObject().put("v", 99).put("entries", new JSONArray());
        require(service.vehicles.setCoverageLedger(vin, unreadable) == null, "test setup: an unreadable ledger on the record");
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "run with an unreadable ledger");
        waitFor(() -> { JSONObject l = liveCoverage(service); return l != null && !l.optString("problem").isEmpty() && entryCount(l.optJSONObject("run")) > 0; }, "REQ-038.8 the problem is shown and the run ledger still counts");
        live = liveCoverage(service);
        require(!live.optBoolean("vehicle_attached") && live.optString("problem").contains("nicht lesbar"), "REQ-038.8 no vehicle ledger is attached: " + live);
        text = MainActivity.coverageText(service.snapshot());
        require(text.contains("Fahrzeug-Abdeckung nicht verfügbar") && !text.contains("Fahrzeug gesamt ·"), "REQ-038.8 unavailable, never zero: " + text);
        require(smart(service).optBoolean("active") && !service.snapshot().optString("autoRecording").isEmpty(), "REQ-038.8 measurement and recording are not stopped by it");
        stopSmart(service);
        require(service.vehicles.coverageLedger(vin).optInt("v") == 99, "REQ-038.8 an unreadable ledger is never overwritten by the run");
        service.vehicles.setCoverageLedger(vin, good);

        // REQ-038.8: an unwritable store shows once that the ledger was not stored; the run was not stopped; the stored file is unchanged
        File folder = storeFile.getParentFile(); String before = new String(java.nio.file.Files.readAllBytes(storeFile.toPath()), StandardCharsets.UTF_8);
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "run before the write failure");
        waitFor(() -> { JSONObject l = liveCoverage(service); return l != null && entryCount(l.optJSONObject("run")) > 0; }, "ledger before the write failure");
        require(folder.setWritable(false, false), "test setup: the store folder becomes read-only");
        try {
            stopSmart(service);
            waitFor(() -> service.snapshot().optString("message").contains("Fahrzeug-Abdeckung nicht gespeichert"), "REQ-038.8 the failed write is reported");
        } finally { folder.setWritable(true, false); }
        require(before.equals(new String(java.nio.file.Files.readAllBytes(storeFile.toPath()), StandardCharsets.UTF_8)), "REQ-038.8 a failed write leaves the stored ledger as it was");
        coverageLostLink(service, link, vin);
        coverageStoreReset(service, link, vin);
        coverageEntryPoints(service, link, vin);
    }
    // ---- every entry point of the persisted vehicle ledger (#340, maintainer decision): connect, select, unconfirmed identity, run end by stop/job/disconnect, 30 s save, inventory switch, delete, reset, unreadable store ----
    private static final String OTHER_VIN = "WVW00000000005555";
    private static String storedText(ObdService service) { return MainActivity.coverageKind(service.snapshot()).equals("stored") ? MainActivity.coverageText(service.snapshot()) : ""; }
    private static void selectVehicle(ObdService service, String vin) throws Exception {
        long base = seq(service); service.selectVehicle(vin); awaitMessage(service, base);
    }
    private static String addVehicle(VehicleStore store, String vin) throws Exception {
        return store.observe(new JSONObject().put("vin", vin).put("responders", new JSONArray()).put("signals", new JSONArray()), 1L);
    }
    private static void startRun(ObdService service, String why) throws Exception {
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), why);
        waitFor(() -> { JSONObject live = liveCoverage(service); return live != null && entryCount(live.optJSONObject("run")) > 0; }, why + ": the run ledger fills");
    }
    private static void ledgerGrew(ObdService service, String vin, int base, String name) throws Exception {
        waitFor(() -> { try { return samples(service.vehicles.coverageLedger(vin)) > base; } catch (Exception e) { return false; } }, name);
    }
    private static void coverageEntryPoints(ObdService service, Link[] link, String vin) throws Exception {
        // connect: the connection itself shows the selected vehicle's stored ledger, no run needed
        reconnect(service, link, false);
        waitFor(() -> storedText(service).contains("Fahrzeug gesamt (gespeichert)"), "REQ-038.10 connecting shows the stored ledger of the selected vehicle: " + MainActivity.coverageText(service.snapshot()));
        // select vehicle: a vehicle without a ledger never shows another one's; selecting back shows it again
        require(addVehicle(service.vehicles, OTHER_VIN).equals("switched"), "test setup: a second vehicle record");
        selectVehicle(service, OTHER_VIN);
        waitFor(() -> MainActivity.coverageKind(service.snapshot()).isEmpty(), "REQ-038.10 selecting a vehicle without a ledger shows no stored card: " + MainActivity.coverageText(service.snapshot()));
        // unconfirmed identity: the connected vehicle is not the selected one, so the run is credited to no vehicle
        int base = samples(service.vehicles.coverageLedger(vin));
        startRun(service, "run with an unconfirmed identity");
        JSONObject live = liveCoverage(service);
        require(live.optBoolean("session_only") && !live.optBoolean("vehicle_attached") && live.optString("problem").isEmpty(), "REQ-038.6 an unconfirmed run is session only: " + live);
        require(MainActivity.coverageText(service.snapshot()).contains("Nur dieser Lauf · Fahrzeug nicht bestätigt"), "REQ-038.10 the card names the unconfirmed identity: " + MainActivity.coverageText(service.snapshot()));
        SystemClock.sleep(1500); stopSmart(service);
        require(!service.vehicles.hasCoverageEntry(OTHER_VIN) && samples(service.vehicles.coverageLedger(vin)) == base, "REQ-038.6 an unconfirmed run writes no vehicle ledger");
        // select vehicle during a run: the run keeps crediting the vehicle it started with
        selectVehicle(service, vin);
        waitFor(() -> storedText(service).contains("Fahrzeug gesamt (gespeichert)"), "REQ-038.10 selecting the vehicle with a ledger shows it again");
        startRun(service, "run before the selection changes");
        selectVehicle(service, OTHER_VIN);
        require(liveCoverage(service) != null && liveCoverage(service).optBoolean("vehicle_attached"), "REQ-038.6 a changed selection does not detach the running run: " + liveCoverage(service));
        SystemClock.sleep(1500); stopSmart(service);
        ledgerGrew(service, vin, base, "REQ-038.6 the run stores into the vehicle it credited, not into the newly selected one");
        require(!service.vehicles.hasCoverageEntry(OTHER_VIN), "REQ-038.6 the newly selected vehicle gets no ledger from that run");
        waitFor(() -> MainActivity.coverageKind(service.snapshot()).isEmpty(), "REQ-038.10 after the run the stored card follows the selection (none for a vehicle without a ledger)");
        // delete vehicle without a run: the selected vehicle is gone, no card
        service.deleteVehicle(OTHER_VIN);
        waitFor(() -> service.vehicles.name(OTHER_VIN).isEmpty() && service.vehicles.selected().isEmpty() && MainActivity.coverageKind(service.snapshot()).isEmpty(), "REQ-038.6/.10 deleting the selected vehicle clears the stored card");
        selectVehicle(service, vin);
        // inventory that switches the vehicle: the stored card follows without a new connection
        require(addVehicle(service.vehicles, OTHER_VIN).equals("switched"), "test setup: the second record again");
        selectVehicle(service, OTHER_VIN);
        waitFor(() -> MainActivity.coverageKind(service.snapshot()).isEmpty(), "test setup: no stored card for the second vehicle");
        service.runJob("inventory");
        waitFor(() -> vin.equals(service.vehicles.selected()) && storedText(service).contains("Fahrzeug gesamt (gespeichert)"), "REQ-038.10 an inventory that switches the vehicle refreshes the stored card: " + MainActivity.coverageText(service.snapshot()));
        waitFor(() -> !service.snapshot().optBoolean("busy"), "idle after the inventory");
        service.deleteVehicle(OTHER_VIN); waitFor(() -> service.vehicles.name(OTHER_VIN).isEmpty(), "second record deleted");
        selectVehicle(service, vin);
        // run end by a job, and by a disconnect
        base = samples(service.vehicles.coverageLedger(vin));
        startRun(service, "run before a job ends it");
        SystemClock.sleep(1500); service.runJob("inventory");
        waitFor(() -> !smart(service).optBoolean("active"), "the job ends the run");
        ledgerGrew(service, vin, base, "REQ-038.6 a run ended by a job stores its ledger");
        waitFor(() -> !service.snapshot().optBoolean("busy"), "idle after the job");
        base = samples(service.vehicles.coverageLedger(vin));
        startRun(service, "run before the disconnect");
        SystemClock.sleep(1500); service.disconnect();
        waitFor(() -> !smart(service).optBoolean("active") && "disconnected".equals(service.snapshot().optString("phase")), "the disconnect ends the run");
        ledgerGrew(service, vin, base, "REQ-038.6 a run ended by a disconnect stores its ledger");
        // periodic save: the ledger is stored while the run is still on (a killed app loses at most 30 s)
        reconnect(service, link, false);
        base = samples(service.vehicles.coverageLedger(vin));
        startRun(service, "run for the periodic save");
        ledgerGrew(service, vin, base, "REQ-038.6 the vehicle ledger is stored about every 30 s while the run is on");
        require(smart(service).optBoolean("active"), "REQ-038.6 the periodic save did not end the run");
        stopSmart(service);
        coverageUnreadableStore(service, link, vin);
        coverageDeleteAndResetInRun(service, link);
    }
    /** REQ-038.8 (#340 review): an unreadable vehicle store is reported as the reason on connect, on refresh and for a run, never as a missing identity and never as an empty ledger. */
    private static void coverageUnreadableStore(ObdService service, Link[] link, String vin) throws Exception {
        File broken = new File(storeFile.getParentFile(), "unreadable-" + System.nanoTime() + ".json");
        VehicleStore good = service.vehicles; String reason = "Fahrzeug-Abdeckung nicht verfügbar · " + VehicleStore.UNREADABLE;
        try {
            // refresh: the card of a readable store turns into the unavailable text when the store becomes unreadable
            reconnect(service, link, false);
            waitFor(() -> storedText(service).contains("Fahrzeug gesamt (gespeichert)"), "test setup: the stored card before the store breaks");
            java.nio.file.Files.write(broken.toPath(), "not json".getBytes(StandardCharsets.UTF_8));
            service.vehicles = new VehicleStore(broken); require(service.vehicles.unreadable(), "test setup: the store is unreadable");
            selectVehicle(service, vin);
            waitFor(() -> storedText(service).contains(reason) && !storedText(service).contains("Fahrzeug gesamt"), "REQ-038.8 a refresh with an unreadable store says why the coverage is unavailable: " + MainActivity.coverageText(service.snapshot()));
            // connect: the same on a fresh connection
            reconnect(service, link, false);
            waitFor(() -> storedText(service).contains(reason), "REQ-038.8 connecting with an unreadable store says why the coverage is unavailable: " + MainActivity.coverageText(service.snapshot()));
            // run start and run end
            startRun(service, "run with an unreadable store");
            JSONObject live = liveCoverage(service);
            require(VehicleStore.UNREADABLE.equals(live.optString("problem")) && !live.optBoolean("vehicle_attached"), "REQ-038.8 the run names the unreadable store as its problem: " + live);
            String text = MainActivity.coverageText(service.snapshot());
            require(text.contains(reason) && !text.contains("nicht bestätigt") && !text.contains("Fahrzeug gesamt"), "REQ-038.8 the run card says the store is unreadable, not that the identity is unconfirmed: " + text);
            stopSmart(service);
            waitFor(() -> storedText(service).contains(reason), "REQ-038.8 after the run the card still says why the coverage is unavailable: " + MainActivity.coverageText(service.snapshot()));
            require("not json".equals(new String(java.nio.file.Files.readAllBytes(broken.toPath()), StandardCharsets.UTF_8)), "REQ-038.8 the unreadable store is never overwritten by a run");
        } finally { service.vehicles = good; broken.delete(); }
        reconnect(service, link, false);
    }
    /** REQ-038.6 (#340 review): deleting the record (no run) and resetting the store (run on) leave no card, no ledger and no write for the gone record. */
    private static void coverageDeleteAndResetInRun(ObdService service, Link[] link) throws Exception {
        File temporary = new File(storeFile.getParentFile(), "reset-run-" + System.nanoTime() + ".json");
        VehicleStore good = service.vehicles;
        try {
            service.vehicles = new VehicleStore(temporary);
            reconnect(service, link, false);
            String vin = service.vehicles.selected(); require(!vin.isEmpty(), "test setup: the connection created a record in the temporary store");
            startRun(service, "run in the temporary store"); SystemClock.sleep(1500); stopSmart(service);
            waitFor(() -> service.vehicles.coverageLedger(vin) != null && storedText(service).contains("Fahrzeug gesamt (gespeichert)"), "test setup: a stored ledger and card");
            service.deleteVehicle(vin);
            waitFor(() -> !service.vehicles.hasCoverageEntry(vin) && MainActivity.coverageKind(service.snapshot()).isEmpty(), "REQ-038.6/.10 deleting the record without a run clears its stored card: " + MainActivity.coverageText(service.snapshot()));
            reconnect(service, link, false);
            startRun(service, "run before the store is reset");
            require(liveCoverage(service).optBoolean("vehicle_attached"), "test setup: the run credits the record");
            SystemClock.sleep(1500);
            service.resetVehicleStore();
            waitFor(() -> { JSONObject l = liveCoverage(service); return l != null && !l.optBoolean("vehicle_attached"); }, "REQ-038.6 a reset of the store detaches the running run");
            require(!MainActivity.coverageText(service.snapshot()).contains("Fahrzeug gesamt"), "REQ-038.6 no vehicle totals after the reset: " + MainActivity.coverageText(service.snapshot()));
            stopSmart(service); waitFor(() -> !service.snapshot().optBoolean("busy"), "idle after the run");
            require(!service.vehicles.hasCoverageEntry(vin) && service.vehicles.coverageLedger(vin) == null && !service.snapshot().optString("message").contains("Fahrzeug-Abdeckung nicht gespeichert"), "REQ-038.6 nothing is written for the reset record: " + service.snapshot().optString("message"));
            require(MainActivity.coverageKind(service.snapshot()).isEmpty(), "REQ-038.10 no stored card after the reset: " + MainActivity.coverageText(service.snapshot()));
        } finally { service.vehicles = good; temporary.delete(); }
        reconnect(service, link, false);
    }
    /** REQ-038.8/.10 (#340 review): after a confirmed reset of an unreadable vehicle store the Live tab no longer says the coverage is unavailable and the stored card is cleared. */
    private static void coverageStoreReset(ObdService service, Link[] link, String vin) throws Exception {
        require(MainActivity.coverageKind(service.snapshot()).equals("stored"), "test setup: a stored card is shown before the store becomes unreadable");
        File broken = new File(storeFile.getParentFile(), "unreadable-" + System.nanoTime() + ".json");
        VehicleStore good = service.vehicles;
        try {
            java.nio.file.Files.write(broken.toPath(), "not json".getBytes(StandardCharsets.UTF_8));
            service.vehicles = new VehicleStore(broken);
            require(service.vehicles.unreadable(), "test setup: the store is unreadable");
            service.resetVehicleStore();
            waitFor(() -> service.snapshot().optString("message").contains("Fahrzeugspeicher zurückgesetzt"), "REQ-038.8 the reset is confirmed");
            waitFor(() -> MainActivity.coverageKind(service.snapshot()).isEmpty(), "REQ-038.8/.10 the stored card is cleared after the reset: " + service.snapshot().optJSONObject("coverageStored"));
            require(!MainActivity.coverageText(service.snapshot()).contains("nicht verfügbar"), "REQ-038.8 no 'nicht verfügbar' text after the reset: " + MainActivity.coverageText(service.snapshot()));
        } finally { service.vehicles = good; broken.delete(); }
        reconnect(service, link, false);
    }
    /** REQ-038.6 (#340 review): a lost link stores what the run credited before the core goes away, and a failed write on that path is part of the error message. */
    private static void coverageLostLink(ObdService service, Link[] link, String vin) throws Exception {
        service.setAutoConnect(false);
        reconnect(service, link, false);
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started before the link is lost");
        waitFor(() -> { JSONObject live = liveCoverage(service); return live != null && entryCount(live.optJSONObject("run")) > 0; }, "the run ledger fills before the link is lost");
        SystemClock.sleep(1500);
        int before = samples(service.vehicles.coverageLedger(vin));
        link[0].drop();
        waitFor(() -> "error".equals(service.snapshot().optString("phase")) && !smart(service).optBoolean("active"), "the lost link ends the analysis");
        waitFor(() -> { try { return samples(service.vehicles.coverageLedger(vin)) > before; } catch (Exception e) { return false; } }, "REQ-038.6 a lost link still stores what the run credited");
        require(!service.snapshot().optString("message").contains("Fahrzeug-Abdeckung nicht gespeichert"), "REQ-038.6 a stored ledger is not reported as lost: " + service.snapshot().optString("message"));
        waitFor(() -> liveCoverage(service) == null && MainActivity.coverageText(service.snapshot()).contains("Fahrzeug gesamt (gespeichert)"), "REQ-038.10 after a lost link the run card is gone and the stored ledger is shown: " + MainActivity.coverageText(service.snapshot()));

        reconnect(service, link, false);
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started before the write fails");
        waitFor(() -> { JSONObject live = liveCoverage(service); return live != null && entryCount(live.optJSONObject("run")) > 0; }, "the run ledger fills before the write fails");
        SystemClock.sleep(1500);
        File folder = storeFile.getParentFile(); String stored = new String(java.nio.file.Files.readAllBytes(storeFile.toPath()), StandardCharsets.UTF_8);
        require(folder.setWritable(false, false), "test setup: the store folder becomes read-only");
        try {
            link[0].drop();
            waitFor(() -> "error".equals(service.snapshot().optString("phase")) && !smart(service).optBoolean("active"), "the lost link ends the analysis without a writable store");
            String message = service.snapshot().optString("message");
            require(message.contains("Fahrzeug-Abdeckung nicht gespeichert") && message.contains(VehicleStore.WRITE_FAILED), "REQ-038.8 the failed write on a lost link is part of the error message: " + message);
        } finally { folder.setWritable(true, false); }
        require(stored.equals(new String(java.nio.file.Files.readAllBytes(storeFile.toPath()), StandardCharsets.UTF_8)), "REQ-038.8 the failed write on a lost link leaves the stored ledger as it was");
        reconnect(service, link, false);
    }
    /** REQ-038.6 (#340 review): the service state and the selected VIN never copy a vehicle's stored ledger; its size does not matter. */
    private static void coverageSnapshotCost(Context context) throws Exception {
        File file = new File(context.getCacheDir(), "snapshot-cost-" + System.nanoTime() + ".json");
        try {
            VehicleStore store = new VehicleStore(file); String vin = "WVW00000000004444";
            store.observe(new JSONObject().put("vin", vin).put("responders", new JSONArray()).put("signals", new JSONArray()), 1L);
            JSONArray entries = new JSONArray(); String pad = "x".repeat(120);
            for (int i = 0; i < 20000; i++) entries.put(new JSONObject().put("context", "baseline").put("qualifying_samples", 1).put("pad", pad + i));
            require(store.setCoverageLedger(vin, new JSONObject().put("v", 1).put("entries", entries)) == null, "test setup: a large ledger on the record");
            long start = SystemClock.elapsedRealtime();
            for (int i = 0; i < 200; i++) require(store.selected().equals(vin) && store.snapshot().getJSONArray("vehicles").length() == 1, "the selected VIN and the snapshot are readable");
            long took = SystemClock.elapsedRealtime() - start;
            JSONObject snapshot = store.snapshot();
            require(!snapshot.toString().contains("coverage_ledger") && snapshot.getJSONArray("vehicles").getJSONObject(0).optString("vin").equals(vin) && snapshot.getJSONArray("vehicles").getJSONObject(0).has("first_connected"),
                "REQ-038.6 the snapshot keeps the record without its ledger");
            require(took < 1500, "REQ-038.6 200 snapshots with a 3 MB stored ledger take " + took + " ms");
            require(samples(store.coverageLedger(vin)) == 20000, "REQ-038.6 the ledger itself is intact");
        } finally { file.delete(); }
    }
    /** REQ-038.8/.10: the card text of unavailable, unreliable and partial coverage; pure, no service. */
    private static void coverageUiText() throws Exception {
        JSONObject run = new JSONObject("{\"contexts\":[{\"context\":\"baseline\",\"covered\":1,\"partial\":2,\"not_covered\":3}],\"entries\":[{\"context\":\"baseline\",\"valid_time_ms\":65000,\"longest_segment_ms\":20000,\"latest\":{\"state\":\"rejected\"}}]}");
        JSONObject live = new JSONObject().put("run", run).put("session_only", true).put("vehicle_attached", false).put("problem", "");
        String text = MainActivity.coverageText(new JSONObject().put("mode", "live").put("coverageLive", live));
        require(text.contains("Nur dieser Lauf · Fahrzeug nicht bestätigt") && text.contains("1 abgedeckt · 2 teilweise · 3 nicht abgedeckt") && text.contains("Messzeit 1 min 5 s · längstes Segment 20 s") && text.contains("1 abgelehnt"), "REQ-038.10 counts, time, segment and latest states: " + text);
        text = MainActivity.coverageText(new JSONObject().put("mode", "live").put("coverageLive", new JSONObject().put("invalid", true).put("run", run)));
        require(text.contains("nicht verlässlich") && !text.contains("abgedeckt"), "REQ-038.8 an unreliable run shows no figures: " + text);
        text = MainActivity.coverageText(new JSONObject().put("mode", "live").put("coverageLive", new JSONObject().put("run", run).put("problem", "Fahrzeugspeicher nicht lesbar")));
        require(text.contains("Fahrzeug-Abdeckung nicht verfügbar · Fahrzeugspeicher nicht lesbar") && !text.contains("Fahrzeug gesamt"), "REQ-038.8 a problem is named: " + text);
        JSONObject partial = new JSONObject("{\"coverage_invalid\":false,\"partial\":true,\"partial_reasons\":[\"missing_parts\",\"no_run_end\"],\"contexts\":[],\"entries\":[]}");
        text = MainActivity.coverageText(new JSONObject().put("mode", "replay").put("coverage", new JSONObject().put("runs", new JSONArray().put(partial))));
        require(text.contains("nur die gelieferten Teile, nicht der ganze Lauf") && text.contains("Teile fehlen") && !text.contains("Ganzer Lauf"), "REQ-038.9/.10 a partial replay never reads as the whole run: " + text);
        JSONObject broken = new JSONObject("{\"coverage_invalid\":true,\"issues\":[\"record after run_end\"]}");
        text = MainActivity.coverageText(new JSONObject().put("mode", "replay").put("coverage", new JSONObject().put("runs", new JSONArray().put(broken))));
        require(text.contains("nicht verlässlich") && text.contains("record after run_end") && !text.contains("abgedeckt"), "REQ-038.9 an invalid replay shows its reason and no figures: " + text);
        require(MainActivity.coverageKind(new JSONObject().put("mode", "live")).isEmpty(), "no card without coverage");
    }
    private static boolean rejectedPedal(JSONObject e) throws Exception {
        if (!"coverage.fact".equals(e.optString("action"))) return false;
        JSONObject data = e.getJSONObject("result").getJSONObject("data");
        return PEDAL.equals(data.optString("signal_key")) && "rejected".equals(data.optString("outcome"));
    }

    /** Synthetic recordings of the shape the service writes: header, filler wire lines and coverage records, one file per part. Never any vehicle data. */
    private static final class Synth {
        private static final long AT = 1_700_000_000_000L; private static long clock = 5_000_000;
        final List<String> lines = new ArrayList<>(); final String revision; int coverage;
        Synth(String revision) throws Exception { this.revision = revision; lines.add(new JSONObject().put("type", "session").put("schema", "OBDENTIC-MOBILE").put("version", 1).put("mode", "live").toString()); }
        Synth filler(int n) throws Exception { for (int i = 0; i < n; i++) { clock += 1000; lines.add(new JSONObject().put("type", "wire").put("direction", "TX").put("text", "010C").put("at", AT + clock / 1000).put("offset_us", clock).toString()); } return this; }
        JSONObject envelope(String action, JSONObject data) throws Exception {
            clock += 1_000_000; data.put("run_offset_us", clock);
            return new JSONObject().put("type", "action").put("action", action).put("argument", "").put("at", AT + clock / 1000).put("offset_us", clock).put("result", new JSONObject().put("ok", true).put("data", data));
        }
        Synth add(JSONObject envelope) { lines.add(envelope.toString()); coverage++; return this; }
        Synth start(String run) throws Exception { return add(envelope("coverage.run", new JSONObject().put("run", run).put("action", "start").put("part", 1).put("goal_definition", 1).put("knowledge_revision", revision))); }
        Synth resume(String run, long part, int previous) throws Exception { return add(envelope("coverage.run", new JSONObject().put("run", run).put("action", "resume").put("part", part).put("previous_part_records", previous).put("goal_definition", 1).put("knowledge_revision", revision))); }
        JSONObject factEnvelope(String run) throws Exception {
            JSONObject data = new JSONObject().put("run", run).put("signal_key", "engine.rpm").put("context", "baseline").put("ecu_target", JSONObject.NULL).put("session", "default").put("implausible", false)
                .put("static_identity", false).put("identity_confirmed", false).put("goal_definition", 1).put("knowledge_revision", revision).put("outcome", "decoded").put("value_kind", "numeric").put("finite", true);
            data.put("evidence", new JSONObject().put("at", AT + clock / 1000).put("offset_us", clock));
            return envelope("coverage.fact", data);
        }
        Synth fact(String run) throws Exception { return add(factEnvelope(run)); }
        JSONObject boundaryEnvelope(String run, String kind) throws Exception { return envelope("coverage.boundary", new JSONObject().put("run", run).put("class", kind)); }
        Synth end(String run) throws Exception { return add(boundaryEnvelope(run, "run_end")); }
        File write(File dir, String name, String reason) throws Exception {
            File file = new File(dir, name);
            try (java.io.Writer out = new java.io.OutputStreamWriter(new java.io.FileOutputStream(file), StandardCharsets.UTF_8)) {
                for (String line : lines) out.write(line + "\n");
                if (reason != null) out.write(new JSONObject().put("type", "session_end").put("reason", reason).put("at", AT + clock / 1000).put("offset_us", clock + 1).toString() + "\n");
            }
            return file;
        }
    }
    private static JSONObject runOf(JSONObject set, String run) {
        JSONArray runs = set == null ? null : set.optJSONArray("runs");
        for (int i = 0; runs != null && i < runs.length(); i++) if (run.equals(runs.optJSONObject(i).optString("run"))) return runs.optJSONObject(i);
        return null;
    }
    private static void requireWhole(JSONObject set, String run, int parts, String why) throws Exception {
        JSONObject r = runOf(set, run);
        require(r != null && !r.optBoolean("coverage_invalid") && r.optBoolean("run_complete") && !r.optBoolean("partial") && "exact".equals(r.optString("figures")) && r.getJSONArray("parts").length() == parts, why + ": " + r + " in " + set);
    }

    /**
     * REQ-038.9, review of #344: the parts of a run are found by the run ids named anywhere in the folder's automatic recordings, within the
     * recording limits: a part whose first coverage record comes after the first 50 lines, a part that begins with another, ended run, and the
     * parts of that other run (found through the first). A coverage record with a missing or corrupt envelope type or an absurd part number
     * invalidates its run instead of being skipped. Synthetic files only; replay sends nothing.
     */
    private static void coverageDiscovery(ObdService service, Link[] link, File dir, List<String> chain) throws Exception {
        int traffic = link[0].commands.size();
        String revision = null; for (JSONObject e : read(new File(dir, chain.get(0)))) if ("coverage.run".equals(e.optString("action"))) { revision = e.getJSONObject("result").getJSONObject("data").getString("knowledge_revision"); break; }
        require(revision != null, "the recorded run carries the Knowledge revision of this build");
        // a part whose run starts after line 50 of its file, three parts
        String delayed = "synthetic-delayed";
        Synth d1 = new Synth(revision).filler(60).start(delayed).fact(delayed).fact(delayed); d1.write(dir, "20990101-000001-001-live-auto.jsonl", "rotated");
        Synth d2 = new Synth(revision).filler(60).resume(delayed, 2, d1.coverage).fact(delayed).fact(delayed); d2.write(dir, "20990101-000001-002-live-auto.jsonl", "rotated");
        Synth d3 = new Synth(revision).filler(60).resume(delayed, 3, d2.coverage).fact(delayed).fact(delayed).end(delayed); d3.write(dir, "20990101-000001-003-live-auto.jsonl", "disconnected");
        for (int f = 1; f <= 3; f++) {
            open(service, "20990101-000001-00" + f + "-live-auto.jsonl"); JSONObject set = service.snapshot().optJSONObject("coverage");
            require(set != null && set.getJSONArray("runs").length() == 1, "REQ-038.9 only the run of the opened file is derived (part " + f + "): " + set);
            requireWhole(set, delayed, 3, "REQ-038.9 parts whose run record is far behind the header are found (opened part " + f + ")");
        }
        // a part that begins with another, ended run; that run's earlier part is found through it
        String earlier = "synthetic-earlier", later = "synthetic-later";
        Synth a = new Synth(revision).start(earlier).fact(earlier); a.write(dir, "20990101-000002-001-live-auto.jsonl", "rotated");
        Synth b = new Synth(revision).resume(earlier, 2, a.coverage).fact(earlier).end(earlier).filler(3).start(later).fact(later); b.write(dir, "20990101-000002-002-live-auto.jsonl", "rotated");
        Synth c = new Synth(revision).resume(later, 2, b.coverage).fact(later).end(later); c.write(dir, "20990101-000002-003-live-auto.jsonl", "disconnected");
        for (int f : new int[] {3, 2, 1}) {
            open(service, "20990101-000002-00" + f + "-live-auto.jsonl"); JSONObject set = service.snapshot().optJSONObject("coverage");
            require(set != null && set.getJSONArray("runs").length() == 2, "REQ-038.9 the two successive runs are derived (opened file " + f + "): " + set);
            requireWhole(set, later, 2, "REQ-038.9 a run that starts after another run in the same file is found (opened file " + f + ")");
            requireWhole(set, earlier, 2, "REQ-038.9 the earlier run, whose first part holds nothing of the opened run, is whole too (opened file " + f + ")");
        }
        // a coverage record without envelope type, in the opened file and in a part found by its run id
        String broken = "synthetic-corrupt";
        Synth e1 = new Synth(revision).start(broken).fact(broken); e1.write(dir, "20990101-000003-001-live-auto.jsonl", "rotated");
        Synth e2 = new Synth(revision).resume(broken, 2, e1.coverage).fact(broken);
        JSONObject untyped = e2.boundaryEnvelope(broken, "gap"); untyped.remove("type");
        e2.add(untyped).fact(broken).end(broken); e2.write(dir, "20990101-000003-002-live-auto.jsonl", "disconnected");
        for (int f = 1; f <= 2; f++) {
            open(service, "20990101-000003-00" + f + "-live-auto.jsonl"); JSONObject r = runOf(service.snapshot().optJSONObject("coverage"), broken);
            require(r != null && r.optBoolean("coverage_invalid") && !r.has("entries") && r.getJSONArray("issues").toString().contains("not an action"), "REQ-038.9 a coverage record without type invalidates the run (opened file " + f + "): " + r);
        }
        // an absurd part number is invalid, never expanded into a list of missing parts
        String huge = "synthetic-huge";
        Synth h1 = new Synth(revision).start(huge).fact(huge); h1.write(dir, "20990101-000004-001-live-auto.jsonl", "rotated");
        new Synth(revision).resume(huge, Long.MAX_VALUE, h1.coverage).fact(huge).end(huge).write(dir, "20990101-000004-002-live-auto.jsonl", "disconnected");
        for (int f = 1; f <= 2; f++) {
            open(service, "20990101-000004-00" + f + "-live-auto.jsonl"); JSONObject r = runOf(service.snapshot().optJSONObject("coverage"), huge);
            require(r != null && r.optBoolean("coverage_invalid") && !r.has("missing_parts") && r.getJSONArray("issues").toString().contains("exceeds the limit"), "REQ-038.9 a part number beyond the limit invalidates the run (opened file " + f + "): " + r);
        }
        require(link[0].commands.size() == traffic, "REQ-038.9 discovery and replay sent nothing to an adapter");
    }
    private static void open(ObdService service, String name) throws Exception {
        service.openRecording(name);
        waitFor(() -> "replay".equals(service.snapshot().optString("mode")) && name.equals(service.snapshot().optString("replayFile")) && !service.snapshot().optBoolean("busy"), "replay " + name);
        require("replay".equals(service.snapshot().optString("phase")), "replay is a separate mode");
    }

    // ---- helpers ---------------------------------------------------------------------------------------------------------------------

    private static void connect(ObdService service) throws Exception {
        service.connect(ADAPTER); waitFor(() -> service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "ready");
    }
    private static void discover(ObdService service) throws Exception {
        service.runJob("inventory"); waitFor(() -> !service.snapshot().optBoolean("busy") && service.snapshot().optJSONObject("vehicle").optBoolean("engine_target_validated"), "engine target validated");
        service.runJob("ecu.discover");
        waitFor(() -> !service.snapshot().optBoolean("busy") && service.snapshot().optInt("udsCandidates") > 0 && service.snapshot().optJSONArray("sessionCandidates").length() > 0, "engine identified, session candidates listed");
    }
    /** A fresh connection (and, when asked, a fresh identification); the new session starts in the default session. */
    private static void reconnect(ObdService service, Link[] link, boolean identify) throws Exception {
        service.disconnect(); waitFor(() -> service.snapshot().optString("phase").equals("disconnected") && !service.snapshot().optBoolean("busy"), "disconnected");
        connect(service); if (identify) discover(service);
        waitFor(() -> !service.snapshot().optString("autoRecording").isEmpty() || !service.snapshot().optBoolean("autoRecord"), "recording follows the link");
    }
    private static String apply(ObdService service, String id) throws Exception {
        long base = seq(service); service.applyComponentPreset(id); waitFor(() -> seq(service) > base && !service.snapshot().optBoolean("busy"), "preset " + id + " applied");
        return service.snapshot().optString("message");
    }
    private static String awaitMessage(ObdService service, long base) throws Exception {
        waitFor(() -> seq(service) > base, "service message"); return service.snapshot().optString("message");
    }
    private static long seq(ObdService service) { return service.snapshot().optLong("messageSeq"); }
    private static JSONObject smart(ObdService service) { return service.snapshot().optJSONObject("smart"); }
    private static JSONObject goalRow(ObdService service, String goal) {
        JSONArray goals = smart(service).optJSONArray("goals");
        for (int i = 0; goals != null && i < goals.length(); i++) if (goal.equals(goals.optJSONObject(i).optString("goal"))) return goals.optJSONObject(i);
        return new JSONObject();
    }
    private static String goalPhase(ObdService service, String goal) { return goalRow(service, goal).optString("phase"); }
    private static boolean isActive(String phase) { return phase.equals("serving") || phase.equals("interrupted") || phase.equals("awaiting_retry") || phase.equals("complete"); }
    /** The latest shown plan change of the goal. */
    private static JSONObject lastChangeOf(ObdService service, String goal) {
        JSONArray changes = smart(service).optJSONArray("changes");
        for (int i = changes.length() - 1; i >= 0; i--) if (goal.equals(changes.optJSONObject(i).optString("goal"))) return changes.optJSONObject(i);
        return new JSONObject();
    }
    private static JSONObject entry(JSONObject component, String semantic) {
        JSONArray entries = component.optJSONArray("entries");
        for (int i = 0; entries != null && i < entries.length(); i++) if (semantic.equals(entries.optJSONObject(i).optString("semantic"))) return entries.optJSONObject(i);
        return null;
    }
    private static boolean containsMeaning(ObdService service, JSONArray keys, String meaning) {
        JSONArray catalog = service.snapshot().optJSONArray("signals");
        for (int i = 0; i < catalog.length(); i++) { JSONObject s = catalog.optJSONObject(i); if (meaning.equals(s.optString("meaning")) && contains(keys, s.optString("semantic"))) return true; }
        return false;
    }
    private static long updatedAt(ObdService service, String semantic) {
        JSONArray values = service.snapshot().optJSONArray("values");
        for (int i = 0; values != null && i < values.length(); i++) if (semantic.equals(values.optJSONObject(i).optString("semantic"))) return values.optJSONObject(i).optLong("updatedAt");
        return 0;
    }
    private static boolean sameSelection(ObdService service, JSONArray expected) { return sameSelection(service.snapshot().optJSONArray("selected"), expected); }
    private static boolean sameSelection(JSONArray actual, JSONArray expected) {
        if (actual == null || actual.length() != expected.length()) return false;
        for (int i = 0; i < expected.length(); i++) if (!contains(actual, expected.optString(i))) return false;
        return true;
    }
    private static boolean contains(JSONArray array, String value) { for (int i = 0; array != null && i < array.length(); i++) if (value.equals(array.optString(i))) return true; return false; }
    private static int count(List<String> commands, String command) { int n = 0; synchronized (commands) { for (String c : commands) if (c.endsWith(":" + command)) n++; } return n; }
    private static JSONObject copy(JSONObject o) throws Exception { return new JSONObject(o.toString()); }
    private static JSONObject smartData(JSONObject event) {
        if (!"smart.plan".equals(event.optString("action"))) return null;
        JSONObject result = event.optJSONObject("result"); return result == null ? null : result.optJSONObject("data");
    }
    private static JSONObject firstSmart(List<JSONObject> events) { for (JSONObject e : events) if (smartData(e) != null) return smartData(e); return null; }
    private static List<JSONObject> read(File file) throws Exception {
        List<JSONObject> events = new ArrayList<>();
        try (BufferedReader in = new BufferedReader(new InputStreamReader(new FileInputStream(file), StandardCharsets.UTF_8))) { String line; while ((line = in.readLine()) != null) events.add(new JSONObject(line)); }
        return events;
    }
    private static Set<String> names(File dir) { String[] list = dir.list(); return list == null ? new HashSet<>() : new HashSet<>(Arrays.asList(list)); }
    private interface Condition { boolean ready() throws Exception; }
    private static void waitFor(Condition condition, String name) throws Exception {
        long until = SystemClock.elapsedRealtime() + 40000;
        while (SystemClock.elapsedRealtime() < until) { if (condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("Timeout: " + name);
    }
    private static void require(boolean condition, String message) { if (!condition) throw new AssertionError(message); }
}
