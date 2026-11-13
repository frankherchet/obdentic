package de.obdentic.app;

import android.content.Context;
import android.os.SystemClock;
import org.json.JSONObject;
import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.*;

/** REQ-020.8 / REQ-020.7 through the real service with a fake adapter: with the automatic ECU identification on, a cut link and the automatic reconnect still continue the named recording, and the identification lands in it. */
final class ResumeRecordingAcceptance {
    private static final String ADAPTER = "AA:BB:CC:00:00:40";

    private static final class Cut implements ObdService.Link {
        final ObdService.Link inner; volatile boolean up = true;
        Cut(ObdService.Link inner) { this.inner = inner; }
        public boolean isOpen() { return up && inner.isOpen(); }
        public void close() { inner.close(); }
        public String exchange(String command, long timeoutMs) throws Exception { if (!up) throw new java.io.IOException("Verbindung unterbrochen"); return inner.exchange(command, timeoutMs); }
    }

    static void run(ObdService service, Context context) throws Exception {
        File recordings = new File(context.getFilesDir(), "recordings"); Set<String> before = names(recordings);
        VehicleStore savedStore = service.vehicles; ObdService.Connector savedConnector = service.connector;
        Boolean savedPermission = service.permissionOverride, savedBluetooth = service.bluetoothOverride;
        Cut[] link = new Cut[1];
        try {
            service.permissionOverride = Boolean.TRUE; service.bluetoothOverride = Boolean.TRUE;
            service.vehicles = new VehicleStore(new File(context.getCacheDir(), "resume-" + System.nanoTime() + ".json"));
            service.connector = address -> { link[0] = new Cut(new AvailabilityAcceptance.Link(service.simulatedLink())); return link[0]; };
            service.setAutoConnect(false); service.disconnect();
            ObdService.autoSurvey = true; service.connect(ADAPTER);
            waitFor(() -> service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy") && service.snapshot().optJSONObject("availability").optJSONArray("entries") != null, "first connection with its automatic survey done");

            service.startRecording("engine-drive"); waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "named recording before the loss");
            String first = service.snapshot().getString("recording"); Set<String> beforeLoss = names(recordings);
            service.setAutoConnect(true); link[0].up = false;
            waitFor(() -> service.snapshot().optString("recording").isEmpty(), "the loss ends the named recording");
            waitFor(() -> !service.snapshot().optString("recording").isEmpty() && !service.snapshot().optBoolean("busy"), "REQ-020.8 the named recording continues after the automatic reconnection although ECU identification runs");
            String second = service.snapshot().getString("recording");
            SystemClock.sleep(1500);
            require(second.equals(service.snapshot().optString("recording")), "REQ-020.8 no automatic job of the reconnection ends the continued recording: " + service.snapshot().optString("recording"));
            String content = new String(Files.readAllBytes(new File(recordings, second).toPath()), StandardCharsets.UTF_8);
            require(!second.equals(first) && new JSONObject(content.split("\n")[0]).optString("profile").equals("engine-drive"), "REQ-020.8 the continued recording is a new file with the same profile: " + second);
            require(!service.snapshot().optString("message").contains("Zuerst laufenden Auftrag beenden"), "REQ-020.8 the resume is not rejected as busy");
            Set<String> created = names(recordings); created.removeAll(beforeLoss); String auto = "";
            for (String n : created) if (n.endsWith("-auto.jsonl")) auto += new String(Files.readAllBytes(new File(recordings, n).toPath()), StandardCharsets.UTF_8);
            require(auto.contains("\"action\":\"ecu.discover\""), "REQ-020.7 the automatic ECU identification of the reconnection ran, in the automatic recording, before the resume: " + created);
        } finally {
            ObdService.autoSurvey = false;
            service.setAutoConnect(false); service.disconnect(); service.connector = savedConnector; service.vehicles = savedStore; service.permissionOverride = savedPermission; service.bluetoothOverride = savedBluetooth;
            service.setDemo(); waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
            service.setAutoConnect(true);
            Set<String> created = names(recordings); created.removeAll(before);
            for (String n : created) new File(recordings, n).delete();
        }
    }

    private static Set<String> names(File dir) { String[] list = dir.list(); return list == null ? new HashSet<>() : new HashSet<>(Arrays.asList(list)); }
    private interface Condition { boolean ready() throws Exception; }
    private static void waitFor(Condition condition, String name) throws Exception {
        long until = SystemClock.elapsedRealtime() + 60000;
        while (SystemClock.elapsedRealtime() < until) { if (condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("Timeout: " + name);
    }
    private static void require(boolean condition, String message) { if (!condition) throw new AssertionError(message); }
}
