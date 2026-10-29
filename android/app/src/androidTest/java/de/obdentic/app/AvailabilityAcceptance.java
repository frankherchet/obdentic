package de.obdentic.app;

import android.content.Context;
import android.os.SystemClock;
import org.json.*;
import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.*;

/** REQ-030 through the real service and core with a fake adapter: automatic start, closed reads, stored result, own recording, replay. No BLE. */
final class AvailabilityAcceptance {
    private static final String ADAPTER = "AA:BB:CC:00:00:30", OIL = "engine.oil_temperature@vag.ecu_target.7e0";

    static final class Link implements ObdService.Link {
        final ObdService.Link delegate; final List<String> commands = Collections.synchronizedList(new ArrayList<>()); volatile String target = "";
        Link(ObdService.Link delegate) { this.delegate = delegate; }
        public boolean isOpen() { return delegate.isOpen(); }
        public void close() { delegate.close(); }
        public String exchange(String command, long timeoutMs) throws Exception {
            String compact = command.trim().replace(" ", "").toUpperCase(Locale.ROOT);
            if (compact.startsWith("ATSH")) target = compact.substring(4);
            commands.add(target + ":" + compact);
            if (compact.equals("0140")) return "7E8 06 41 40 FF FF FF EF\r>"; // without PID 5C (engine oil temperature), like a real EA189
            if (compact.startsWith("22") && compact.length() == 6) {
                if (!target.equals("7E0")) return "NO DATA\r>";
                int did = Integer.parseInt(compact.substring(2), 16);
                if (did == 0xF187) return "7E8 06 62 F1 87 31 32 33\r>";
                if (did == 0x1153) return "7E8 03 7F 22 31\r>";
                if (did == 0x1105) return "NO DATA\r>";
                return String.format(Locale.ROOT, "7E8 05 62 %02X %02X 04 00\r>", did >> 8, did & 0xff);
            }
            return delegate.exchange(command, timeoutMs);
        }
    }

    static void run(ObdService service, Context context) throws Exception {
        File recordings = new File(context.getFilesDir(), "recordings"); Set<String> before = names(recordings);
        VehicleStore savedStore = service.vehicles; ObdService.Connector savedConnector = service.connector;
        Boolean savedPermission = service.permissionOverride, savedBluetooth = service.bluetoothOverride;
        Link[] link = new Link[1];
        try {
            service.permissionOverride = Boolean.TRUE; service.bluetoothOverride = Boolean.TRUE;
            service.vehicles = new VehicleStore(new File(context.getCacheDir(), "availability-" + System.nanoTime() + ".json"));
            service.connector = address -> { link[0] = new Link(service.simulatedLink()); return link[0]; };
            service.setAutoConnect(false); service.disconnect();

            // REQ-030.4: a new vehicle record is surveyed automatically after identification
            ObdService.autoSurvey = true; service.connect(ADAPTER);
            waitFor(() -> service.snapshot().optJSONObject("availability") != null && service.snapshot().optJSONObject("availability").optJSONArray("entries") != null, "automatic availability check for a new vehicle");
            waitFor(() -> !service.snapshot().optBoolean("busy"), "survey finished");
            JSONObject data = service.snapshot().getJSONObject("availability"); JSONArray entries = data.getJSONArray("entries");
            require(entries.length() > 5 && !data.optBoolean("truncated"), "REQ-030.2 the check covers the default-session signals: " + entries.length());
            require("answered".equals(status(entries, OIL)), "REQ-030.2 the oil temperature DID answers: " + status(entries, OIL));
            JSONObject rejected = entry(entries, "dpf.oil_ash_volume@vag.ecu_target.7e0");
            require(rejected != null && "rejected".equals(rejected.optString("status")) && rejected.optInt("nrc") == 0x31, "REQ-030.2 a negative response is classified with its code: " + rejected);
            require(entries.toString().indexOf("\"value\"") < 0, "REQ-030.2 no value is kept");
            List<String> services = new ArrayList<>(); for (String c : link[0].commands) { String wire = c.substring(c.indexOf(':') + 1); if (!wire.startsWith("AT") && !wire.startsWith("01") && !wire.startsWith("09") && !wire.startsWith("03")) services.add(wire); }
            require(!link[0].commands.toString().contains(":1003") && services.stream().allMatch(w -> w.startsWith("22")), "REQ-030.1 only 0x22 reads and no session change: " + services);

            // REQ-030.5: stored with the vehicle, own recording, automatic recording resumed
            JSONObject record = firstVehicle(service);
            require(record != null && record.optJSONObject("availability") != null && record.getJSONObject("availability").optString("knowledge_revision").length() >= 7, "REQ-030.5 result is stored in the vehicle record: " + record);
            Set<String> created = names(recordings); created.removeAll(before);
            String own = null; for (String n : created) if (!n.endsWith("-auto.jsonl") && new String(Files.readAllBytes(new File(recordings, n).toPath()), StandardCharsets.UTF_8).contains("\"profile\":\"knowledge-availability\"")) own = n;
            require(own != null, "REQ-030.5 the job wrote its own recording: " + created);
            List<String> lines = Files.readAllLines(new File(recordings, own).toPath());
            require(new JSONObject(lines.get(lines.size() - 1)).optString("type").equals("session_end"), "REQ-030.5 the recording is complete");
            waitFor(() -> !service.snapshot().optString("autoRecording").isEmpty(), "automatic recording resumed");

            // REQ-030.4: every inventory job surveys again
            long firstRun = data.optLong("started_at_ms"); String ownName = own;
            service.runJob("inventory");
            waitFor(() -> service.snapshot().optJSONObject("availability").optLong("started_at_ms") > firstRun && !service.snapshot().optBoolean("busy"), "inventory triggers a new survey");

            // REQ-030.5: replay restores the statuses without adapter traffic
            service.disconnect(); waitFor(() -> service.snapshot().optString("phase").equals("disconnected"), "disconnected");
            int sent = link[0].commands.size();
            service.openRecording(ownName);
            waitFor(() -> service.snapshot().optString("mode").equals("replay") && service.snapshot().optJSONObject("availability") != null && service.snapshot().optJSONObject("availability").optJSONArray("entries") != null && !service.snapshot().optBoolean("busy"), "replay restores the availability");
            require("answered".equals(status(service.snapshot().getJSONObject("availability").getJSONArray("entries"), OIL)) && link[0].commands.size() == sent, "REQ-030.5 replay shows the same statuses and sends nothing");

            // REQ-030.4: demo never surveys
            service.setDemo(); waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
            service.runJob("inventory"); waitFor(() -> service.snapshot().optString("message").contains("Auftrag abgeschlossen: inventory") && !service.snapshot().optBoolean("busy"), "demo inventory");
            SystemClock.sleep(1500);
            require(service.snapshot().optJSONObject("availability").length() == 0 && !service.snapshot().optBoolean("busy"), "REQ-030.4 no survey in demo: " + service.snapshot().optJSONObject("availability"));
        } finally {
            ObdService.autoSurvey = false;
            service.disconnect(); service.connector = savedConnector; service.vehicles = savedStore; service.permissionOverride = savedPermission; service.bluetoothOverride = savedBluetooth;
            service.setDemo(); waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
            service.setAutoConnect(true);
            Set<String> created = names(recordings); created.removeAll(before);
            for (String n : created) new File(recordings, n).delete();
        }
    }

    private static JSONObject firstVehicle(ObdService service) {
        JSONArray vehicles = service.snapshot().optJSONObject("vehicles").optJSONArray("vehicles");
        return vehicles == null || vehicles.length() == 0 ? null : vehicles.optJSONObject(0);
    }
    private static JSONObject entry(JSONArray entries, String key) { for (int i = 0; i < entries.length(); i++) if (key.equals(entries.optJSONObject(i).optString("key"))) return entries.optJSONObject(i); return null; }
    private static String status(JSONArray entries, String key) { JSONObject e = entry(entries, key); return e == null ? null : e.optString("status"); }
    private static Set<String> names(File dir) { String[] list = dir.list(); return list == null ? new HashSet<>() : new HashSet<>(Arrays.asList(list)); }
    private interface Condition { boolean ready() throws Exception; }
    private static void waitFor(Condition condition, String name) throws Exception {
        long until = SystemClock.elapsedRealtime() + 60000;
        while (SystemClock.elapsedRealtime() < until) { if (condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("Timeout: " + name);
    }
    private static void require(boolean condition, String message) { if (!condition) throw new AssertionError(message); }
}
