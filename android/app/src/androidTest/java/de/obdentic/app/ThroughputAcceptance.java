package de.obdentic.app;

import android.content.Context;
import android.os.SystemClock;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Arrays;
import java.util.HashSet;
import java.util.Set;

/**
 * REQ-022 through the real service, the real Rust core and the Diagnose card. The "live" link is the simulated adapter (answers like the demo,
 * single PIDs and 2 or 3 PIDs per request, 6 PIDs rejected); no BLE traffic happens. A real vehicle's multi-PID answers remain for #178.
 */
final class ThroughputAcceptance {
    /** What the Diagnose tab shows: its visible texts (after a short wait for the 500 ms refresh) and whether a button is enabled. */
    interface Ui { String text(String tab) throws Exception; Boolean enabled(String label, boolean expected) throws Exception; }
    private static final String ADDRESS = "AA:BB:CC:00:00:09", BUTTON = "Adapter-Leistung messen";

    static void run(ObdService service, Context context, Ui ui) throws Exception {
        File recordings = new File(context.getFilesDir(), "recordings");
        Set<String> before = names(recordings);
        VehicleStore savedStore = service.vehicles;
        try {
            // REQ-022.1: demo is unavailable with a reason, nothing is started or sent
            waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
            String demoReason = ObdService.throughputBlocker(service.snapshot());
            require(demoReason != null && demoReason.contains("Demo"), "REQ-022.1 demo: unavailable with a reason: " + demoReason);
            int wire = service.snapshot().optJSONArray("debug").length(); long seq = service.snapshot().optLong("messageSeq");
            service.runJob(ObdService.THROUGHPUT);
            require(service.snapshot().optLong("messageSeq") > seq && service.snapshot().optString("message").contains("Demo"), "REQ-022.1 demo: the refusal is shown: " + service.snapshot().optString("message"));
            require(service.snapshot().optJSONArray("debug").length() == wire && service.snapshot().optString("recording").isEmpty() && service.snapshot().optJSONObject("throughput").length() == 0, "REQ-022.1 demo: no job, no traffic, no recording");
            require(ui.text("Diagnose").contains(demoReason) && Boolean.FALSE.equals(ui.enabled(BUTTON, false)), "REQ-022.1 demo: Diagnose shows the reason and a disabled button");

            // live and ready: available, runs, recorded
            service.permissionOverride = Boolean.TRUE; service.bluetoothOverride = Boolean.TRUE;
            service.vehicles = new VehicleStore(new File(context.getCacheDir(), "throughput-vehicles-" + System.nanoTime() + ".json"));
            service.connector = address -> service.simulatedLink();
            service.disconnect();
            require(ObdService.throughputBlocker(service.snapshot()).contains("verbinden"), "REQ-022.1 not connected: unavailable with a reason: " + ObdService.throughputBlocker(service.snapshot()));
            service.connect(ADDRESS);
            waitFor(() -> service.snapshot().optString("phase").equals("ready") && service.snapshot().optString("mode").equals("live") && !service.snapshot().optBoolean("busy"), "live connection ready");
            require(ObdService.throughputBlocker(service.snapshot()) == null && Boolean.TRUE.equals(ui.enabled(BUTTON, true)), "REQ-022.1 live and ready: the button is enabled");
            service.runJob(ObdService.THROUGHPUT);
            waitFor(() -> !service.snapshot().optBoolean("busy") && service.snapshot().optJSONObject("throughput").length() > 0, "throughput job");
            JSONObject summary = service.snapshot().getJSONObject("throughput");

            // REQ-022.2: 6 advertised PIDs, 10 reads each, statistics
            JSONArray pids = summary.getJSONObject("phase1").getJSONArray("pids");
            require(pids.length() == 6, "REQ-022.2 six PIDs: " + summary);
            for (int i = 0; i < pids.length(); i++) require(pids.getJSONObject(i).getInt("n") == 10 && pids.getJSONObject(i).getInt("errors") == 0 && pids.getJSONObject(i).has("median_ms") && pids.getJSONObject(i).has("p90_ms"), "REQ-022.2 ten reads with median and p90 per PID: " + pids.getJSONObject(i));
            JSONObject overall = summary.getJSONObject("phase1").getJSONObject("overall");
            require(overall.getInt("n") == 60 && overall.getDouble("reads_per_s") > 0 && overall.has("median_ms") && overall.has("p90_ms"), "REQ-022.2 overall statistics with reads per second > 0: " + overall);
            // REQ-022.3 / .7: 2, 3, 6 PIDs; the simulated adapter answers 2 and 3 and rejects 6
            JSONArray multi = summary.getJSONArray("phase2");
            require(multi.length() == 3, "REQ-022.3 three multi-PID requests: " + multi);
            int[] sizes = {2, 3, 6}; String[] outcomes = {"full", "full", "rejected"};
            for (int i = 0; i < 3; i++) require(multi.getJSONObject(i).getJSONArray("pids").length() == sizes[i] && outcomes[i].equals(multi.getJSONObject(i).getString("outcome")) && multi.getJSONObject(i).has("elapsed_ms"), "REQ-022.3 request " + i + ": " + multi.getJSONObject(i));
            JSONObject first = multi.getJSONObject(0).getJSONArray("values").getJSONObject(0);
            require("engine.rpm".equals(first.getString("semantic")) && first.getDouble("value") == 800.0, "REQ-022.7 the multi-PID reply is decoded per PID with the catalog decoder: " + first);
            require(multi.getJSONObject(2).getJSONArray("values").getJSONObject(0).optString("status").equals("missing") && !multi.getJSONObject(2).getJSONArray("values").getJSONObject(0).has("value"), "REQ-022.7 a rejected request invents no value");
            // REQ-022.5: own recording, complete
            Set<String> added = names(recordings); added.removeAll(before);
            require(added.size() == 1, "REQ-022.5 exactly one new recording: " + added);
            String name = added.iterator().next(); File file = new File(recordings, name);
            String[] lines = new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8).trim().split("\n");
            require(new JSONObject(lines[0]).getString("profile").equals("adapter-throughput") && new JSONObject(lines[lines.length - 1]).getString("type").equals("session_end") && new JSONObject(lines[lines.length - 1]).getString("reason").equals("completed"), "REQ-022.5 own recording ends completed");
            int tx = 0; boolean action = false, multiTx = false;
            for (String line : lines) { JSONObject event = new JSONObject(line);
                if ("TX".equals(event.optString("direction"))) { tx++; multiTx |= event.optString("text").trim().equals("010C05"); }
                action |= "adapter.throughput".equals(event.optString("action")); }
            require(tx >= 63 && multiTx && action, "REQ-022.5 the recording holds every TX/RX and the result: tx=" + tx);
            require(service.snapshot().optString("recording").isEmpty() && ObdService.throughputBlocker(service.snapshot()) == null, "REQ-022.5 the recording is closed and the job can run again");

            // REQ-022.6: the summary card in Diagnose
            String card = ui.text("Diagnose");
            require(card.contains("ADAPTER-LEISTUNG") && card.contains("Gesamt:") && card.contains("Lesungen/s") && card.contains("Drehzahl") && card.contains("2 Werte: vollständig") && card.contains("6 Werte: abgelehnt"), "REQ-022.6 Diagnose shows the summary: " + card);

            // REQ-022.6: opening the recording re-derives the same summary offline
            JSONObject live = summary;
            service.openRecording(name);
            waitFor(() -> service.snapshot().optString("mode").equals("replay") && !service.snapshot().optBoolean("busy") && service.snapshot().optJSONObject("throughput").length() > 0, "replay of the throughput recording");
            JSONObject replayed = service.snapshot().getJSONObject("throughput");
            require(replayed.getJSONObject("phase1").toString().equals(live.getJSONObject("phase1").toString()) && replayed.getJSONArray("phase2").toString().equals(live.getJSONArray("phase2").toString()), "REQ-022.6 replay re-derives the same statistics and outcomes");
            String replayReason = ObdService.throughputBlocker(service.snapshot());
            require(replayReason != null && replayReason.contains("Wiedergabe") && ui.text("Diagnose").contains("Gesamt:") && Boolean.FALSE.equals(ui.enabled(BUTTON, false)), "REQ-022.1/.6 replay: card shown, button disabled with reason: " + replayReason);
        } finally {
            service.permissionOverride = null; service.bluetoothOverride = null; service.connector = null;
            service.setDemo(); waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy") && service.snapshot().optString("recording").isEmpty(), "demo restored after the throughput checks");
            service.vehicles = savedStore;
            for (String name : names(recordings)) if (!before.contains(name)) new File(recordings, name).delete();
        }
    }

    private static Set<String> names(File dir) { String[] list = dir.list(); return list == null ? new HashSet<>() : new HashSet<>(Arrays.asList(list)); }
    private interface Condition { boolean ready() throws Exception; }
    private static void waitFor(Condition condition, String name) throws Exception {
        long until = SystemClock.elapsedRealtime() + 30000;
        while (SystemClock.elapsedRealtime() < until) { if (condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("Timeout: " + name);
    }
    private static void require(boolean condition, String message) { if (!condition) throw new AssertionError(message); }
}
