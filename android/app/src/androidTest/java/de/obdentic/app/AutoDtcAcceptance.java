package de.obdentic.app;

import android.content.Context;
import android.os.SystemClock;
import org.json.*;
import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.*;

/** REQ-034 through the real service and core with a fake adapter: automatic stored + pending fault-code scan after inventory, setting, recording, replay, failure. No BLE. */
final class AutoDtcAcceptance {
    private static final String ADAPTER = "AA:BB:CC:00:00:34";

    static final class Link implements ObdService.Link {
        final ObdService.Link delegate; final List<String> commands = Collections.synchronizedList(new ArrayList<>()); final boolean failing;
        Link(ObdService.Link delegate, boolean failing) { this.delegate = delegate; this.failing = failing; }
        public boolean isOpen() { return delegate.isOpen(); }
        public void close() { delegate.close(); }
        public String exchange(String command, long timeoutMs) throws Exception {
            String compact = command.trim().replace(" ", "").toUpperCase(Locale.ROOT);
            commands.add(compact);
            if (compact.equals("03")) return failing ? "NO DATA\r>" : "7E8 03 43 01 0C\r>";
            if (compact.equals("07")) return failing ? "7E8 03 7F 07 11\r>" : "7E8 03 47 02 01\r>";
            return delegate.exchange(command, timeoutMs);
        }
    }

    static void run(ObdService service, Context context) throws Exception {
        File recordings = new File(context.getFilesDir(), "recordings"); Set<String> before = names(recordings);
        VehicleStore savedStore = service.vehicles; ObdService.Connector savedConnector = service.connector;
        Boolean savedPermission = service.permissionOverride, savedBluetooth = service.bluetoothOverride;
        boolean savedSetting = ObdService.autoDtcEnabled(context);
        Link[] link = new Link[1]; boolean[] failing = {false};
        try {
            service.permissionOverride = Boolean.TRUE; service.bluetoothOverride = Boolean.TRUE;
            service.vehicles = new VehicleStore(new File(context.getCacheDir(), "autodtc-" + System.nanoTime() + ".json"));
            service.connector = address -> { link[0] = new Link(service.simulatedLink(), failing[0]); return link[0]; };
            service.setAutoConnect(false); service.disconnect(); service.setAutoDtc(true);
            ObdService.autoScan = true;

            // REQ-034.1/034.2: after inventory (and the automatic survey) stored and pending codes are read once
            service.connect(ADAPTER);
            waitFor(() -> service.snapshot().optJSONObject("diagnostics") != null && service.snapshot().optJSONObject("diagnostics").has("pending") && !service.snapshot().optBoolean("busy"), "automatic scan result");
            JSONObject data = service.snapshot().getJSONObject("diagnostics");
            require("P010C".equals(data.getJSONArray("observations").getJSONObject(0).getJSONArray("codes").getString(0)), "REQ-034.2 stored code: " + data);
            require("P0201".equals(data.getJSONObject("pending").getJSONArray("observations").getJSONObject(0).getJSONArray("codes").getString(0)), "REQ-034.2 pending code: " + data);
            List<String> commands = new ArrayList<>(link[0].commands);
            require(count(commands, "03") == 1 && count(commands, "07") == 1, "REQ-034.1 exactly one Mode 03 and one Mode 07 request: " + commands);
            int lastSurvey = -1; for (int i = 0; i < commands.size(); i++) if (commands.get(i).startsWith("22")) lastSurvey = i;
            require(lastSurvey < commands.indexOf("03"), "REQ-034.1 scan runs after the automatic ECU identification and availability jobs");
            for (String c : commands) require(!c.equals("04") && !c.startsWith("14"), "REQ-034.2 no clearing service: " + c);

            // REQ-034.3: TX/RX in the running automatic recording; replay shows the same codes without traffic
            String own = service.snapshot().optString("autoRecording");
            String content = new String(Files.readAllBytes(new File(recordings, own).toPath()), StandardCharsets.UTF_8);
            require(content.contains("\"action\":\"dtc.scan\"") && content.contains("7E8 03 47 02 01") && !content.contains("\"type\":\"session_end\""), "REQ-034.3 scan is in the running automatic recording");
            service.disconnect(); waitFor(() -> service.snapshot().optString("phase").equals("disconnected") && service.snapshot().optString("autoRecording").isEmpty(), "disconnected");
            int sent = link[0].commands.size();
            service.openRecording(own);
            waitFor(() -> service.snapshot().optString("mode").equals("replay") && service.snapshot().optJSONObject("diagnostics") != null && service.snapshot().optJSONObject("diagnostics").has("pending") && !service.snapshot().optBoolean("busy"), "replay shows the scan");
            data = service.snapshot().getJSONObject("diagnostics");
            require("P0201".equals(data.getJSONObject("pending").getJSONArray("observations").getJSONObject(0).getJSONArray("codes").getString(0)) && "P010C".equals(data.getJSONArray("observations").getJSONObject(0).getJSONArray("codes").getString(0)) && link[0].commands.size() == sent, "REQ-034.3 replay shows stored and pending codes and sends nothing");

            // REQ-034.1: setting off, no scan on a (re)connection
            service.setAutoDtc(false);
            service.connect(ADAPTER);
            waitFor(() -> service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "ready with setting off");
            SystemClock.sleep(1500);
            require(count(link[0].commands, "03") == 0 && count(link[0].commands, "07") == 0, "REQ-034.1 no scan with the setting off: " + link[0].commands);

            // REQ-034.1/034.3: a reconnect scans again; a failed scan stays an explicit result and ends nothing
            service.disconnect(); waitFor(() -> service.snapshot().optString("phase").equals("disconnected"), "disconnected");
            service.setAutoDtc(true); failing[0] = true;
            service.connect(ADAPTER);
            waitFor(() -> service.snapshot().optString("phase").equals("ready") && link[0] != null && count(link[0].commands, "07") == 1 && !service.snapshot().optBoolean("busy"), "failed scan finished");
            JSONObject result = service.snapshot();
            require(result.optString("phase").equals("ready") && result.optString("mode").equals("live"), "REQ-034.3 a failed scan keeps the connection");
            JSONObject failed = result.optJSONObject("diagnostics");
            require(failed != null && failed.toString().contains("NO DATA") && failed.getJSONObject("pending").getJSONArray("errors").length() > 0, "REQ-034.2 NO DATA and the negative response stay explicit: " + failed);
            service.disconnect();
            waitFor(() -> service.snapshot().optString("phase").equals("disconnected"), "disconnected");

            // REQ-034.1: demo never scans
            service.setDemo(); waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
            SystemClock.sleep(1500);
            JSONObject demo = service.snapshot().optJSONObject("diagnostics");
            require(demo == null || demo.length() == 0, "REQ-034.1 no scan in demo: " + demo);
        } finally {
            ObdService.autoScan = false; failing[0] = false;
            service.disconnect(); service.connector = savedConnector; service.vehicles = savedStore; service.permissionOverride = savedPermission; service.bluetoothOverride = savedBluetooth;
            service.setAutoDtc(savedSetting);
            service.setDemo(); waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
            service.setAutoConnect(true);
            Set<String> created = names(recordings); created.removeAll(before);
            for (String n : created) new File(recordings, n).delete();
        }
    }

    private static int count(List<String> commands, String wire) { int n = 0; synchronized (commands) { for (String c : commands) if (c.equals(wire)) n++; } return n; }
    private static Set<String> names(File dir) { String[] list = dir.list(); return list == null ? new HashSet<>() : new HashSet<>(Arrays.asList(list)); }
    private interface Condition { boolean ready() throws Exception; }
    private static void waitFor(Condition condition, String name) throws Exception {
        long until = SystemClock.elapsedRealtime() + 60000;
        while (SystemClock.elapsedRealtime() < until) { if (condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("Timeout: " + name);
    }
    private static void require(boolean condition, String message) { if (!condition) throw new AssertionError(message); }
}
