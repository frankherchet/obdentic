package de.obdentic.app;

import android.content.Context;
import android.content.SharedPreferences;
import android.os.SystemClock;
import org.json.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.*;

/** REQ-028 through the real service with a fake adapter link (answers like the demo, but in live mode). No BLE traffic happens. */
final class AutoRecordingAcceptance {
    private static final String ADAPTER = "AA:BB:CC:00:00:09";

    static void run(ObdService service, Context context) throws Exception {
        SharedPreferences prefs = context.getSharedPreferences("adapter", Context.MODE_PRIVATE);
        File dir = new File(context.getFilesDir(), "recordings");
        Set<String> before = names(dir);
        VehicleStore savedStore = service.vehicles; ObdService.Connector savedConnector = service.connector;
        boolean[] reachable = {true};
        try {
            prefs.edit().clear().commit();
            require(ObdService.autoRecordEnabled(context), "REQ-028.5 the continuous recording is on by default");
            service.vehicles = new VehicleStore(new File(context.getCacheDir(), "autorec-vehicles-" + System.nanoTime() + ".json"));
            service.connector = address -> { if (!reachable[0]) throw new IOException("Adapter nicht erreichbar"); return service.simulatedLink(); };
            service.setAutoConnect(false); service.disconnect();

            // REQ-028.1 / .2: the file starts before the first adapter byte and ends with the disconnect
            service.connect(ADAPTER); ready(service);
            String first = service.snapshot().optString("autoRecording");
            require(first.endsWith("-live-auto.jsonl"), "REQ-028.1 a recording runs without any user action: " + first);
            require(service.snapshot().optString("recording").isEmpty(), "REQ-028.1 it is not a named recording");
            File firstFile = new File(dir, first);
            List<String> lines = lines(firstFile);
            JSONObject header = new JSONObject(lines.get(0));
            require(header.optString("schema").equals("OBDENTIC-MOBILE") && header.optInt("version") == 1 && header.optString("profile").equals("session-auto"), "REQ-028.1 OBDENTIC-MOBILE v1 header of the automatic profile: " + lines.get(0));
            int firstTx = -1, initialize = -1, catalog = -1, inventory = -1;
            for (int i = 1; i < lines.size(); i++) { JSONObject e = new JSONObject(lines.get(i));
                if (firstTx < 0 && e.optString("type").equals("wire") && e.optString("direction").equals("TX")) firstTx = i;
                if (e.optString("type").equals("action")) { String a = e.optString("action"); if (a.equals("initialize") && initialize < 0) initialize = i; if (a.equals("catalog") && catalog < 0) catalog = i; if (a.equals("inventory") && inventory < 0) inventory = i; } }
            require(firstTx > 0 && initialize > firstTx && catalog > initialize && inventory > catalog, "REQ-028.1 the handshake is in the file: tx=" + firstTx + " init=" + initialize + " catalog=" + catalog + " inventory=" + inventory);
            service.disconnect(); waitFor(() -> service.snapshot().optString("autoRecording").isEmpty(), "recording closed on disconnect");
            JSONObject end = new JSONObject(lastLine(firstFile));
            require(end.optString("type").equals("session_end") && end.optString("reason").equals("disconnected"), "REQ-028.2 session_end on disconnect: " + end);

            // REQ-028.1: an attempt that fails still leaves a file with its error marker
            reachable[0] = false; Set<String> beforeFail = names(dir);
            service.connect(ADAPTER); waitFor(() -> service.snapshot().optString("phase").equals("error"), "failed attempt");
            waitFor(() -> service.snapshot().optString("autoRecording").isEmpty(), "failed attempt closes its recording");
            Set<String> failedFiles = names(dir); failedFiles.removeAll(beforeFail);
            require(failedFiles.size() == 1, "REQ-028.1 a failed attempt leaves one file: " + failedFiles);
            String failedName = failedFiles.iterator().next();
            require(failedName.endsWith("-auto.jsonl") && new JSONObject(lastLine(new File(dir, failedName))).optString("reason").equals("error"), "REQ-028.1 the failed attempt ends with an error marker");
            reachable[0] = true; service.disconnect();

            // REQ-028.4: a named recording replaces the automatic one, which resumes afterwards
            service.connect(ADAPTER); ready(service);
            String beforeNamed = service.snapshot().optString("autoRecording");
            service.startRecording("engine-drive");
            waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "named recording");
            String named = service.snapshot().optString("recording");
            require(!named.endsWith("-auto.jsonl") && service.snapshot().optString("autoRecording").isEmpty(), "REQ-028.4 one file is written at a time");
            require(new JSONObject(lastLine(new File(dir, beforeNamed))).optString("reason").equals("stopped"), "REQ-028.4 the automatic recording is closed with a marker");
            service.stopRecording();
            waitFor(() -> service.snapshot().optString("recording").isEmpty() && !service.snapshot().optString("autoRecording").isEmpty(), "automatic recording resumes");
            String resumed = service.snapshot().optString("autoRecording");
            require(!resumed.equals(beforeNamed), "REQ-028.4 the automatic recording continues in a new file");
            require(new JSONObject(lastLine(new File(dir, named))).optString("type").equals("session_end") && new JSONObject(lines(new File(dir, named)).get(0)).optString("profile").equals("engine-drive"), "REQ-028.4 the named recording is complete and unchanged in kind");

            // REQ-028.3: rotation by size and by age, without a gap
            service.rotateBytes = 6000; Set<String> beforeRotate = names(dir);
            waitFor(() -> { Set<String> now = names(dir); now.removeAll(beforeRotate); return now.size() >= 3; }, "rotation by size");
            service.rotateBytes = 10L * 1024 * 1024;
            service.disconnect(); waitFor(() -> service.snapshot().optString("autoRecording").isEmpty(), "closed after rotation");
            List<String> chain = new ArrayList<>(names(dir)); chain.removeAll(beforeRotate); chain.removeIf(n -> !n.endsWith("-auto.jsonl")); Collections.sort(chain);
            require(chain.size() >= 3, "REQ-028.3 the session rotated into several files: " + chain);
            for (int i = 0; i < chain.size(); i++) { List<String> part = lines(new File(dir, chain.get(i)));
                require(new JSONObject(part.get(0)).optString("profile").equals("session-auto"), "REQ-028.3 each file starts with its own header");
                String reason = new JSONObject(part.get(part.size() - 1)).optString("reason");
                require(reason.equals(i < chain.size() - 1 ? "rotated" : "disconnected"), "REQ-028.3 rotated files end with reason rotated: " + chain.get(i) + " " + reason);
                if (i > 0) { List<String> previous = lines(new File(dir, chain.get(i - 1)));
                    long gap = new JSONObject(part.get(1)).optLong("at") - new JSONObject(previous.get(previous.size() - 2)).optLong("at");
                    require(gap >= 0 && gap < 2000, "REQ-028.3 no gap between rotated files: " + gap + " ms"); } }
            service.connect(ADAPTER); ready(service); service.rotateMs = 1500; String timed = service.snapshot().optString("autoRecording");
            waitFor(() -> !service.snapshot().optString("autoRecording").equals(timed), "rotation by age"); service.rotateMs = 30L * 60 * 1000;
            require(new JSONObject(lastLine(new File(dir, timed))).optString("reason").equals("rotated"), "REQ-028.3 rotation by age");

            // REQ-028.5: the switch persists and closes the running recording; off means no recording
            String running = service.snapshot().optString("autoRecording");
            service.setAutoRecord(false); waitFor(() -> service.snapshot().optString("autoRecording").isEmpty(), "switch off closes the recording");
            require(new JSONObject(lastLine(new File(dir, running))).optString("reason").equals("stopped"), "REQ-028.5 switching off ends the recording with a marker");
            require(!ObdService.autoRecordEnabled(context) && !context.getSharedPreferences("adapter", Context.MODE_PRIVATE).getBoolean("auto_recording", true), "REQ-028.5 the setting persists");
            Set<String> beforeOff = names(dir); service.disconnect(); service.connect(ADAPTER); ready(service);
            Set<String> offFiles = names(dir); offFiles.removeAll(beforeOff); offFiles.removeIf(n -> !n.endsWith("-auto.jsonl")); offFiles.remove(running);
            require(offFiles.isEmpty() && service.snapshot().optString("autoRecording").isEmpty(), "REQ-028.5 no recording while off: " + offFiles);
            service.disconnect(); service.setAutoRecord(true);

            // REQ-028.8: an unwritable folder never stops the session
            require(dir.setWritable(false), "test setup: make the folder read-only");
            try { service.connect(ADAPTER); ready(service);
                require(service.snapshot().optString("autoRecording").isEmpty() && service.snapshot().optString("phase").equals("ready"), "REQ-028.8 the live session runs without a recording");
                service.runJob("dtc.scan"); waitFor(() -> service.snapshot().optJSONObject("diagnostics").length() > 0, "job still works"); }
            finally { dir.setWritable(true); }
            service.disconnect();

            // REQ-028.6: nothing was removed by the app so far; REQ-028.7: explicit delete leaves named files and the running one
            for (String n : chain) require(new File(dir, n).isFile(), "REQ-028.6 the app never deletes recordings itself: " + n);
            require(new File(dir, named).isFile(), "named recording present");
            service.connect(ADAPTER); ready(service); String live = service.snapshot().optString("autoRecording");
            service.deleteAutoRecordings();
            waitFor(() -> service.snapshot().optString("message").contains("automatische Aufnahmen gelöscht"), "delete message");
            for (String n : chain) require(!new File(dir, n).exists(), "REQ-028.7 completed automatic recordings are deleted: " + n);
            require(new File(dir, named).isFile() && new File(dir, live).isFile(), "REQ-028.7 named and running recordings stay");

            // REQ-028.10: replay of an automatic file restores vehicle and support from the recorded handshake
            service.disconnect(); waitFor(() -> service.snapshot().optString("autoRecording").isEmpty(), "closed before replay");
            service.openRecording(live);
            waitFor(() -> service.snapshot().optString("mode").equals("replay") && !service.snapshot().optJSONObject("vehicle").optString("vin").isEmpty() && !service.snapshot().optBoolean("busy"), "replay of the automatic file");
            JSONArray signals = service.snapshot().getJSONArray("signals"); int supported = 0;
            for (int i = 0; i < signals.length(); i++) if ("supported".equals(signals.getJSONObject(i).optString("status"))) supported++;
            require(supported > 0, "REQ-028.10 signal support is re-derived from the recorded handshake");
        } finally {
            dir.setWritable(true); service.disconnect(); service.connector = savedConnector; service.vehicles = savedStore; service.rotateBytes = 10L * 1024 * 1024; service.rotateMs = 30L * 60 * 1000;
            prefs.edit().clear().commit();
            service.setDemo(); // the runner continues in demo mode
            waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
            Set<String> created = names(dir); created.removeAll(before);
            for (String n : created) new File(dir, n).delete();
        }
    }

    private static Set<String> names(File dir) { String[] list = dir.list(); return list == null ? new HashSet<>() : new HashSet<>(Arrays.asList(list)); }
    private static List<String> lines(File file) throws IOException { return Arrays.asList(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8).trim().split("\n")); }
    private static String lastLine(File file) throws IOException { List<String> l = lines(file); return l.get(l.size() - 1); }
    private static void ready(ObdService service) throws Exception {
        waitFor(() -> service.snapshot().optString("phase").equals("ready") && service.snapshot().optString("mode").equals("live") && !service.snapshot().optBoolean("busy"), "live connection ready");
    }
    private interface Condition { boolean ready() throws Exception; }
    private static void waitFor(Condition condition, String name) throws Exception {
        long until = SystemClock.elapsedRealtime() + 30000;
        while (SystemClock.elapsedRealtime() < until) { if (condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("Timeout: " + name);
    }
    private static void require(boolean condition, String message) { if (!condition) throw new AssertionError(message); }
}
