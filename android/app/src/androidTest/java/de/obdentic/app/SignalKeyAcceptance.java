package de.obdentic.app;

import android.content.Context;
import android.os.SystemClock;
import org.json.*;
import java.io.File;
import java.util.*;

/** #256: a Knowledge signal and a standard PID with the same semantic are two selectable signals with their own status and value. Fake adapter, no BLE. */
final class SignalKeyAcceptance {
    private static final String ADAPTER = "AA:BB:CC:00:00:56", PID = "engine.oil_temperature", KEY = "engine.oil_temperature@vag.ecu_target.7e0";

    private static final class Link implements ObdService.Link {
        final ObdService.Link delegate; final List<String> commands = Collections.synchronizedList(new ArrayList<>()); volatile String target = "";
        Link(ObdService.Link delegate) { this.delegate = delegate; }
        public boolean isOpen() { return delegate.isOpen(); }
        public void close() { delegate.close(); }
        public String exchange(String command, long timeoutMs) throws Exception {
            String compact = command.trim().replace(" ", "").toUpperCase(Locale.ROOT);
            if (compact.startsWith("ATSH")) target = compact.substring(4);
            commands.add(target + ":" + compact);
            if (compact.equals("0140")) return "7E8 06 41 40 FF FF FF EF\r>"; // like the demo, but without PID 5C (engine oil temperature)
            if (compact.startsWith("22") && compact.length() == 6) {
                if (!target.equals("7E0")) return "NO DATA\r>";
                int did = Integer.parseInt(compact.substring(2), 16);
                if (did == 0xF187) return "7E8 06 62 F1 87 31 32 33\r>";
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
            service.vehicles = new VehicleStore(new File(context.getCacheDir(), "signalkey-" + System.nanoTime() + ".json"));
            service.connector = address -> { link[0] = new Link(service.simulatedLink()); return link[0]; };
            service.setAutoConnect(false); service.disconnect(); service.connect(ADAPTER);
            waitFor(() -> service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "ready");
            service.runJob("inventory"); waitFor(() -> !service.snapshot().optBoolean("busy") && service.snapshot().optJSONObject("vehicle").optBoolean("engine_target_validated"), "engine target validated");
            service.runJob("ecu.discover"); waitFor(() -> !service.snapshot().optBoolean("busy") && service.snapshot().optInt("udsCandidates") > 0, "engine identified");

            JSONObject pid = null, did = null;
            JSONArray catalog = service.snapshot().getJSONArray("signals");
            for (int i = 0; i < catalog.length(); i++) { JSONObject s = catalog.getJSONObject(i);
                if (s.optString("meaning", s.optString("semantic")).equals(PID) && !s.has("definition_id")) pid = s;
                if (s.optString("definition_id").equals("vag.ea189.oil.oil_temperature")) did = s; }
            require(pid != null && did != null, "#256 both oil temperature signals are in the catalog");
            require(PID.equals(pid.optString("semantic")), "#256 the standard PID keeps its plain semantic as id: " + pid);
            require(KEY.equals(did.optString("semantic")) && PID.equals(did.optString("meaning")), "#256 the Knowledge signal gets its own id and keeps its meaning: " + did);
            require("unsupported".equals(pid.optString("status")), "#256 the PID is unsupported on this ECU: " + pid);
            require(!did.optString("status").equals("unsupported"), "#256 the PID's status is not applied to the Knowledge signal: " + did);

            service.setSignals(new JSONArray().put(PID).put(KEY));
            waitFor(() -> hasValue(service.snapshot().optJSONArray("values"), KEY), "#256 the Knowledge signal delivers a value");
            JSONObject row = row(service.snapshot().optJSONArray("values"), KEY);
            require(KEY.equals(row.optString("semantic")) && Double.isFinite(row.optDouble("value", Double.NaN)), "#256 value row is keyed by the signal id: " + row);
            require(link[0].commands.contains("7E0:2211BE"), "#256 the DID is read from the engine target: " + link[0].commands);
            require(row(service.snapshot().optJSONArray("values"), PID) == null, "#256 the unsupported PID is not polled and has no row");
        } finally {
            service.disconnect(); service.connector = savedConnector; service.vehicles = savedStore; service.permissionOverride = savedPermission; service.bluetoothOverride = savedBluetooth;
            service.setDemo(); waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
            service.setAutoConnect(true);
            Set<String> created = names(recordings); created.removeAll(before);
            for (String n : created) new File(recordings, n).delete();
        }
    }

    private static JSONObject row(JSONArray values, String id) { for (int i = 0; values != null && i < values.length(); i++) if (id.equals(values.optJSONObject(i).optString("semantic"))) return values.optJSONObject(i); return null; }
    private static boolean hasValue(JSONArray values, String id) { JSONObject r = row(values, id); return r != null && r.has("value"); }
    private static Set<String> names(File dir) { String[] list = dir.list(); return list == null ? new HashSet<>() : new HashSet<>(Arrays.asList(list)); }
    private interface Condition { boolean ready() throws Exception; }
    private static void waitFor(Condition condition, String name) throws Exception {
        long until = SystemClock.elapsedRealtime() + 30000;
        while (SystemClock.elapsedRealtime() < until) { if (condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("Timeout: " + name);
    }
    private static void require(boolean condition, String message) { if (!condition) throw new AssertionError(message); }
}
