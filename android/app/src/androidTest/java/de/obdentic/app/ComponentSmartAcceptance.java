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

    /** Answers like the demo adapter, except for the steered trigger inputs and the engine's DIDs; every command is logged with its target. */
    private static final class Link implements ObdService.Link {
        final ObdService.Link delegate; final List<String> commands = Collections.synchronizedList(new ArrayList<>());
        volatile String target = ""; volatile boolean open = true, pedalMissing, failActivation; volatile int regen; volatile String pedal = "20";
        Link(ObdService.Link delegate) { this.delegate = delegate; }
        public boolean isOpen() { return open && delegate.isOpen(); }
        public void close() { open = false; delegate.close(); }
        void drop() { open = false; }
        public String exchange(String command, long timeoutMs) throws Exception {
            String compact = command.trim().replace(" ", "").toUpperCase(Locale.ROOT);
            if (!open) throw new IOException("simulated link lost");
            if (compact.startsWith("ATSH")) target = compact.substring(4);
            commands.add(target + ":" + compact);
            if (compact.equals("014A")) return pedalMissing ? "NO DATA\r>" : "7E8 03 41 4A " + pedal + "\r>";
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
            presetsAndBlockers();
            service.permissionOverride = Boolean.TRUE; service.bluetoothOverride = Boolean.TRUE;
            service.vehicles = new VehicleStore(new File(context.getCacheDir(), "component-smart-" + System.nanoTime() + ".json"));
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
            replay(service, link, dir, before, chain);
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

    // ---- REQ-036 ---------------------------------------------------------------------------------------------------------------------

    /** Switch of the continuous recording off: the analysis starts and ends its own recording. Returns the chain of files it wrote. */
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
        waitFor(() -> smart(service).optJSONArray("missing").length() == 0 && smart(service).optJSONArray("triggers").length() == 6, "all six triggers have fresh readings");
        require(smart(service).optJSONArray("triggers").length() == 6, "REQ-036.2 six trigger rows are shown");
        waitFor(() -> "rail".equals(smart(service).optString("component")), "rail window (standstill, warm, idle rpm)");

        // REQ-036.2: every set holds all triggers; the user's own selection is not part of it and is not touched
        JSONArray selected = service.snapshot().getJSONArray("selected");
        for (String t : new String[] {"engine.rpm", "vehicle.speed", "engine.coolant_temperature", PEDAL, "engine.load"}) require(contains(selected, t), "REQ-036.2 trigger " + t + " is selected: " + selected);
        require(containsMeaning(service, selected, REGEN), "REQ-036.2 the Knowledge trigger is selected by its key: " + selected);
        require(selected.length() <= 32 && !contains(selected, USER), "REQ-036.1 at most 32 signals, the user's selection is replaced for the run: " + selected);
        JSONObject view = smart(service);
        require(!view.optString("reason").isEmpty() && view.optLong("remaining_ms", -2) != -2 && view.optString("recording").equals(owned), "REQ-036.5 the view shows set, reason, remaining time and recording: " + view);
        // the selection belongs to the analysis while it runs
        long base = seq(service); service.setSignals(new JSONArray().put("engine.rpm")); String refused = awaitMessage(service, base);
        require(refused.contains("Smart-Analyse") && sameSelection(service, selected), "REQ-036.1 the selection is locked while the analysis runs: " + refused);
        base = seq(service); service.applyComponentPreset("egr"); refused = awaitMessage(service, base);
        require(refused.contains("Smart-Analyse"), "REQ-035.3 no component preset (and no session) while the analysis runs: " + refused);
        base = seq(service); service.applySignalPreset("component:dpf"); refused = awaitMessage(service, base);
        require(refused.contains("Smart-Analyse"), "a stored preset is refused while the analysis runs: " + refused);

        // priority: dpf request interrupts rail, resumes rail when it ends; pressing the pedal interrupts rail again (rotation of the file inside that very write)
        l.regen = 1; waitFor(() -> "dpf".equals(smart(service).optString("component")), "dpf request shows dpf");
        JSONObject change = lastChange(service);
        require("interruption".equals(change.optString("kind")) && "rail".equals(change.optString("previous_set")) && "dpf".equals(change.optString("set")) && change.optString("reason").contains("dpf"), "REQ-036.4 dpf interrupts rail: " + change);
        l.regen = 0; waitFor(() -> "rail".equals(smart(service).optString("component")), "rail resumes");
        change = lastChange(service);
        require("resumption".equals(change.optString("kind")) && "dpf".equals(change.optString("previous_set")), "REQ-036.4 the valid lower window resumes: " + change);
        service.rotateAfterSmart = "change"; l.pedal = "FF";
        waitFor(() -> "turbo".equals(smart(service).optString("component")), "pedal shows turbo");
        change = lastChange(service);
        require("interruption".equals(change.optString("kind")) && "rail".equals(change.optString("previous_set")), "REQ-036.4 turbo interrupts rail: " + change);
        require(service.rotateAfterSmart.isEmpty(), "the forced rotation happened inside the record of the change");
        waitFor(() -> !service.snapshot().optString("autoRecording").equals(owned), "the recording rotated while the analysis runs");
        require(smart(service).optBoolean("active"), "REQ-036.5 the analysis continues across the file rotation");

        // a trigger reading that is missing is shown as such and never as zero (REQ-036.2)
        l.pedalMissing = true;
        waitFor(() -> contains(smart(service).optJSONArray("missing"), PEDAL), "missing pedal reported");
        JSONObject row = triggerRow(service, PEDAL);
        require(!"fresh".equals(row.optString("state")) && !row.has("value") && smart(service).optJSONObject("trigger_ages").optLong(PEDAL, 0) < 0, "REQ-036.2 the missing reading carries no value and no age: " + row);
        require(smart(service).optString("reason").contains(PEDAL) && contains(smart(service).optJSONArray("missing"), PEDAL) && !contains(smart(service).optJSONArray("unavailable"), PEDAL) && smart(service).has("reasons"), "REQ-036.2 the trigger problem is in missing/triggers, the set's own reasons stay in reasons: " + smart(service));
        l.pedalMissing = false; l.pedal = "20";
        waitFor(() -> !contains(smart(service).optJSONArray("missing"), PEDAL), "pedal reading returns");

        // stop: the stop event is the last event of its file, even when the recording rotates in that very write; nothing the analysis chose was stored
        service.rotateAfterSmart = "stop"; service.stopSmart();
        waitFor(() -> !smart(service).optBoolean("active") && service.snapshot().optString("autoRecording").isEmpty(), "analysis stopped and own recording closed");
        require(service.snapshot().optString("message").startsWith("Smart-Analyse beendet") && smart(service).optString("reason").length() > 0, "REQ-036.5 the end is visible with its reason: " + service.snapshot().optString("message"));
        require(sameSelection(service, user) && sameSelection(service.vehicles.selection(vin), user), "REQ-036.1 the user's selection (shown and stored) is back: " + service.snapshot().getJSONArray("selected"));
        require(count(l.commands, "1003") == sessions && count(l.commands, "1001") == returns && count(l.commands, "3E80") == testers, "REQ-036.1 the analysis never touches the session: " + l.commands);
        require(smart(service).optJSONArray("changes").length() > 0, "REQ-036.5 the latest changes stay visible after the stop");

        List<String> chain = new ArrayList<>(names(dir)); chain.removeAll(filesBefore); chain.removeIf(n -> !n.endsWith("-auto.jsonl")); Collections.sort(chain);
        require(chain.size() == 3 && chain.get(0).equals(owned), "own recording rotated twice (turbo change, stop): " + chain);
        List<List<JSONObject>> files = new ArrayList<>(); for (String n : chain) files.add(read(new File(dir, n)));
        for (List<JSONObject> events : files) {
            require("session".equals(events.get(0).optString("type")) && "session_end".equals(events.get(events.size() - 1).optString("type")), "every file has its header and end marker");
            for (JSONObject e : events) require(!"uds.enable".equals(e.optString("action")), "REQ-036.1 no extended session in the file");
        }
        JSONObject headerComponent = files.get(0).get(0).optJSONObject("component");
        require(headerComponent == null || headerComponent.length() == 0, "REQ-035.4 a preset replaced by a manual selection is not claimed by the next header: " + headerComponent);
        // per-transition order across the files
        List<String> sequence = new ArrayList<>(); long lastOffset;
        for (int f = 0; f < files.size(); f++) { lastOffset = -1;
            for (JSONObject e : files.get(f)) { JSONObject data = smartData(e); if (data == null) continue;
                require(e.has("at") && e.optLong("offset_us", -1) >= lastOffset, "REQ-036.5 wall time and monotonic offset on every change, ascending: " + e); lastOffset = e.optLong("offset_us");
                String kind = data.optString("kind"); JSONObject readings = data.optJSONObject("readings");
                for (java.util.Iterator<String> names = readings == null ? Collections.<String>emptyIterator() : readings.keys(); names.hasNext(); ) { String name = names.next(); JSONObject r = readings.optJSONObject(name);
                    require(r == null || !r.has("value") || "fresh".equals(r.optString("state")), "REQ-036.2 only a fresh reading carries a value, a missing or stale one never shows zero: " + r); }
                if (!kind.equals("availability") && !kind.equals("resume")) sequence.add(kind + ":" + data.optString("set")); } }
        int dpfAt = sequence.indexOf("interruption:dpf"), turboAt = sequence.indexOf("interruption:turbo");
        require(sequence.get(0).startsWith("start:") && (sequence.contains("trigger:rail") || sequence.get(0).equals("start:rail")) && dpfAt > 0 && turboAt > dpfAt, "REQ-036.3 start, then the rail trigger, before the interruption: " + sequence);
        require(sequence.subList(dpfAt, turboAt + 1).equals(Arrays.asList("interruption:dpf", "resumption:rail", "interruption:turbo")), "REQ-036.4 transitions in order: " + sequence);
        List<String> tail = new ArrayList<>(sequence.subList(turboAt + 1, sequence.size())); tail.removeAll(Collections.singleton("resumption:rail")); // the missing pedal may end the turbo window before the stop
        require(tail.equals(Arrays.asList("stop:")), "REQ-036.4 the stop ends the sequence: " + sequence);
        JSONObject dpfEvent = null; for (JSONObject e : files.get(0)) { JSONObject d = smartData(e); if (d != null && "interruption".equals(d.optString("kind")) && "dpf".equals(d.optString("set"))) dpfEvent = d; }
        JSONObject reading = dpfEvent.getJSONObject("readings").getJSONObject(REGEN);
        require(dpfEvent.getLong("reading_at") > 0 && "fresh".equals(reading.optString("state")) && reading.optDouble("value") == 1.0 && reading.getLong("age_ms") >= 0 && !dpfEvent.getString("reason").isEmpty(), "REQ-036.5 the change records the reading behind it with its time stamp: " + dpfEvent);
        require("change".equals(dpfEvent.optString("change")), "the change class of an interruption is 'change': " + dpfEvent);
        int reads = 0; for (JSONObject e : files.get(0)) if ("read".equals(e.optString("action")) && e.optString("argument").startsWith(REGEN)) reads++;
        require(reads >= 3, "REQ-036.5 the raw read events stay in the recording: " + reads);
        // P2: the rotation inside the record of the interruption continues with that very set; the stop is last and nothing resumes after it
        JSONObject first = firstSmart(files.get(1));
        require(first != null && "resume".equals(first.optString("change")) && "turbo".equals(first.optString("set")) && first.getJSONObject("readings").length() == 6, "REQ-036.5 the continuation file starts with the set it continues: " + first);
        List<JSONObject> second = files.get(1); JSONObject lastSmart = null; for (JSONObject e : second) if (smartData(e) != null) lastSmart = smartData(e);
        require("stop".equals(lastSmart.optString("change")) && "rotated".equals(second.get(second.size() - 1).optString("reason")), "REQ-036.5 the stop is the last change of its file: " + lastSmart);
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
        waitFor(() -> contains(smart(service).optJSONArray("missing"), REGEN) && smart(service).optJSONArray("triggers").length() == 6, "unavailable Knowledge trigger reported");
        JSONObject regen = triggerRow(service, REGEN);
        require("unavailable".equals(regen.optString("state")) && !regen.has("value") && smart(service).optJSONObject("trigger_ages").optLong(REGEN, 0) < 0 && contains(smart(service).optJSONArray("missing"), REGEN) && !contains(smart(service).optJSONArray("unavailable"), REGEN),
            "REQ-036.2 a trigger without a vehicle signal is unavailable, never zero: " + regen);
        service.runJob("dtc.scan");
        waitFor(() -> !smart(service).optBoolean("active") && !service.snapshot().optBoolean("busy"), "a job ends the analysis");
        require(smart(service).optString("reason").contains("Auftrag") && sameSelection(service, user) && running.equals(service.snapshot().optString("autoRecording")), "REQ-036.1 a job ends it, the selection is back and the recording goes on: " + smart(service).optString("reason"));

        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started again");
        l.drop(); // the link is lost: the analysis ends, the recording stays partial
        waitFor(() -> "error".equals(service.snapshot().optString("phase")) && !smart(service).optBoolean("active"), "lost link ends the analysis");
        require(sameSelection(service, user) && sameSelection(service.vehicles.selection(vin), user), "REQ-036.1 the user's selection survives a lost link");
        endsWithStop(new File(dir, running), "error");

        reconnect(service, link, false); l = link[0];
        String next = service.snapshot().optString("autoRecording");
        service.startSmart(); waitFor(() -> smart(service).optBoolean("active"), "smart analysis started on the new connection");
        require(count(l.commands, "1003") == 0 && count(l.commands, "1001") == 0, "REQ-036.1 neither connecting nor starting the analysis opens a session");
        service.disconnect(); waitFor(() -> "disconnected".equals(service.snapshot().optString("phase")) && !smart(service).optBoolean("active") && service.snapshot().optString("autoRecording").isEmpty(), "disconnect ends the analysis");
        waitFor(() -> sameSelection(service, user), "selection restored after the disconnect");
        endsWithStop(new File(dir, next), "disconnected");
        require(sameSelection(service.vehicles.selection(vin), user) && count(l.commands, "1003") == 0, "REQ-036.1 the stored selection is the user's, no session was ever opened");
    }

    private static void endsWithStop(File file, String reason) throws Exception {
        waitFor(() -> { List<JSONObject> e = read(file); return "session_end".equals(e.get(e.size() - 1).optString("type")); }, "recording closed " + file.getName());
        List<JSONObject> events = read(file); JSONObject end = events.get(events.size() - 1);
        require(reason.equals(end.optString("reason")), "the recording ends with " + reason + ": " + end);
        JSONObject before = events.get(events.size() - 2), data = smartData(before);
        require(data != null && "stop".equals(data.optString("change")) && !data.optString("reason").isEmpty() && before.optLong("offset_us") <= end.optLong("offset_us"), "REQ-036.5 the stop event comes right before the end marker: " + before);
    }

    // ---- replay ----------------------------------------------------------------------------------------------------------------------

    private static void replay(ObdService service, Link[] link, File dir, Set<String> before, List<String> chain) throws Exception {
        int traffic = link[0].commands.size();
        // the preset: the last component.apply of the first connection (the refused activation of dpf)
        List<String> all = new ArrayList<>(names(dir)); all.removeAll(before); Collections.sort(all); JSONObject lastApply = null;
        for (String n : all) for (JSONObject e : read(new File(dir, n))) if ("component.apply".equals(e.optString("action")) && "dpf".equals(e.getJSONObject("result").getJSONObject("data").optString("id"))) lastApply = e.getJSONObject("result").getJSONObject("data");
        require(lastApply != null, "REQ-035.4 the apply was recorded");
        String applyFile = null; for (String n : all) for (JSONObject e : read(new File(dir, n))) if ("component.apply".equals(e.optString("action")) && "dpf".equals(e.getJSONObject("result").getJSONObject("data").optString("id"))) applyFile = n;
        open(service, applyFile);
        JSONObject component = service.snapshot().getJSONObject("component");
        require(component.optBoolean("replay") && lastApply.getString("id").equals(component.optString("id")) && component.getJSONObject("uds").optBoolean("requested") && !component.getJSONObject("uds").optString("error").isEmpty()
            && "uds_unavailable".equals(entry(component, SESSION_SIGNAL).optString("status")), "REQ-035.4 replay shows the preset, its statuses and the failed activation without traffic: " + component);
        // the analysis: first file of the own recording, its continuation and the stop file
        open(service, chain.get(0));
        JSONObject view = smart(service); List<JSONObject> events = read(new File(dir, chain.get(0))); JSONObject lastData = null; int count = 0;
        for (JSONObject e : events) if (smartData(e) != null) { lastData = smartData(e); count++; }
        require(view.optBoolean("replay") && !view.optBoolean("active") && view.optJSONArray("changes").length() == Math.min(count, 20) && lastData.optString("set").equals(view.optString("component"))
            && "turbo".equals(view.optString("component")) && service.snapshot().getJSONObject("component").length() == 0, "REQ-036.5 replay shows the recorded changes and claims no stale preset: " + view);
        require(sameSelection(service, lastData.getJSONArray("selected")), "REQ-036.5 replay selects the recorded set");
        JSONArray changes = view.getJSONArray("changes"); JSONObject lastShown = changes.getJSONObject(changes.length() - 1);
        require("interruption".equals(lastShown.optString("kind")) && "rail".equals(lastShown.optString("previous_set")) && lastShown.optLong("at") > 0, "REQ-036.5 the history names the class and time of each change: " + lastShown);
        open(service, chain.get(1));
        view = smart(service); changes = view.getJSONArray("changes");
        require("resume".equals(changes.getJSONObject(0).optString("change")) && "stop".equals(changes.getJSONObject(changes.length() - 1).optString("change")) && sameSelection(service, new JSONArray().put(USER)), "REQ-036.5 the continuation file replays resume ... stop and the user's selection: " + changes);
        open(service, chain.get(2));
        require(sameSelection(service, new JSONArray().put(USER)) && smart(service).optJSONArray("changes").length() == 0 && !smart(service).optBoolean("active"), "REQ-036.1 replay of the file after the stop shows the restored selection and no analysis: " + service.snapshot().getJSONArray("selected"));
        require(link[0].commands.size() == traffic, "replay sent nothing to an adapter: " + link[0].commands.subList(traffic, link[0].commands.size()));
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
    private static JSONObject lastChange(ObdService service) {
        JSONArray changes = smart(service).optJSONArray("changes");
        for (int i = changes.length() - 1; i >= 0; i--) if (!changes.optJSONObject(i).optString("kind").equals("availability")) return changes.optJSONObject(i);
        return new JSONObject();
    }
    private static JSONObject triggerRow(ObdService service, String semantic) {
        JSONArray rows = smart(service).optJSONArray("triggers");
        for (int i = 0; i < rows.length(); i++) if (semantic.equals(rows.optJSONObject(i).optString("semantic"))) return rows.optJSONObject(i);
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
        if (!"smart.set".equals(event.optString("action"))) return null;
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
