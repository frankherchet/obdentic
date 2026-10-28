package de.obdentic.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.SystemClock;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;

/** Full live-vehicle acceptance. Reports contain only stage names and counts. */
public final class HardwareAcceptanceRunner extends Instrumentation {
    private static final long CONNECT_TIMEOUT_MS = 90_000;
    private static final int SCREEN_OFF_SECONDS = 300;
    private static final long STATUS_INTERVAL_MS = 30_000;

    private final StringBuilder report = new StringBuilder();
    private ObdService service;
    private String stage = "startup";
    private long nextStatusAt;
    private int checks;
    private boolean screenOff;

    @Override public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        nextStatusAt = SystemClock.elapsedRealtime() + STATUS_INTERVAL_MS;
        start();
    }

    @Override public void onStart() {
        Bundle result = new Bundle();
        try {
            stage = "startup";
            Intent launch = new Intent(getTargetContext(), MainActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            startActivitySync(launch); // Deliberately no demo extra: this is a normal live launch.
            waitFor(() -> ObdService.current != null, 30_000, "service startup");
            service = ObdService.current;

            stage = "live_scan";
            ensureDisconnectedForScan();
            long scanStarted = SystemClock.elapsedRealtime();
            runOnMainSync(service::scan);
            waitFor(() -> !"scanning".equals(service.snapshot().optString("phase")), 25_000, "BLE scan completion");
            JSONObject scanned = service.snapshot();
            require("idle".equals(scanned.optString("phase")), "scan did not complete cleanly");
            require(SystemClock.elapsedRealtime() - scanStarted >= 11_000, "scan ended before the full discovery window");
            JSONArray devices = scanned.optJSONArray("devices");
            require(devices != null && devices.length() == 1, "expected exactly one Carly scan candidate");
            String address = devices.getJSONObject(0).optString("address");
            require(!address.isEmpty(), "scan candidate has no connect address");
            passed("12-second BLE scan found exactly one Carly candidate");

            stage = "live_connect";
            runOnMainSync(() -> service.connect(address));
            waitFor(() -> {
                JSONObject state = service.snapshot();
                return "ready".equals(state.optString("phase"))
                    && "live".equals(state.optString("mode")) && !state.optBoolean("busy");
            }, CONNECT_TIMEOUT_MS, "live adapter connection");
            requireLiveValidatedTarget();
            passed("Carly connected in live mode with a validated engine target");

            stage = "live_rpm";
            requireFreshRpm();
            passed("Live engine RPM value received without an error; zero is accepted");

            stage = "recording_start";
            service.startRecording("engine-drive");
            waitFor(() -> !service.snapshot().optString("recording").isEmpty(), 30_000, "recording start");
            String filename = service.snapshot().optString("recording");
            File recording = new File(new File(getTargetContext().getFilesDir(), "recordings"), filename);
            waitFor(() -> recording.isFile() && recording.length() > 0, 30_000, "recording header write");

            stage = "vehicle_jobs";
            runJob("inventory");
            requireLiveValidatedTarget();
            runJob("dtc.scan");
            passed("Inventory and stored-DTC jobs completed");

            stage = "screen_off_recording";
            keepRecordingScreenOff(recording, filename);
            getUiAutomation().executeShellCommand("input keyevent 224").close();
            screenOff = false;
            service.stopRecording();
            waitFor(() -> service.snapshot().optString("recording").isEmpty(), 30_000, "recording stop boundary");
            waitFor(() -> !service.snapshot().optBoolean("busy"), 30_000, "recording worker idle");

            stage = "capture_validation";
            CaptureFacts capture = inspectCapture(recording);
            require(capture.hasInventory && capture.hasDtc,
                "capture is missing a requested vehicle job");
            require(capture.engineRpmPresent, "capture has no decodable engine RPM sample");
            File exportedCapture = copyCapture(recording);
            verifyReadOnlyExport(filename, recording.length());
            passed("Capture validated: header/end markers, TX=" + capture.tx + ", RX=" + capture.rx
                + ", reads=" + capture.reads + ", inventory/DTC evidence; read-only export opened");

            stage = "offline_replay";
            service.openRecording(filename);
            waitFor(() -> {
                JSONObject state = service.snapshot();
                return "replay".equals(state.optString("mode")) && !state.optBoolean("busy");
            }, 120_000, "offline replay");
            JSONObject replay = service.snapshot();
            require(!replay.optBoolean("replayPartial"), "recording replay is incomplete");
            require("stopped".equals(replay.optString("replayOutcome")), "recording replay lacks a stopped boundary");
            require(replay.optJSONArray("values") != null && replay.getJSONArray("values").length() > 0,
                "replay did not restore decoded values");
            requireReplayedRpm(capture.recordedRpm, replay);
            passed("Offline replay recomputed engine RPM from its recorded response");

            stage = "live_reconnect";
            runOnMainSync(service::disconnect);
            waitFor(() -> "disconnected".equals(service.snapshot().optString("phase")), 30_000, "replay disconnect");
            runOnMainSync(() -> service.connect(address));
            waitFor(() -> {
                JSONObject state = service.snapshot();
                return "ready".equals(state.optString("phase"))
                    && "live".equals(state.optString("mode")) && !state.optBoolean("busy");
            }, CONNECT_TIMEOUT_MS, "live reconnection");
            requireLiveValidatedTarget();
            requireFreshRpm();
            passed("Same scanned adapter reconnected live with a fresh engine RPM value");

            stage = "artifact_transfer";
            emitArtifact(exportedCapture);
            JSONObject finalState = service.snapshot();
            require("live".equals(finalState.optString("mode")) && "ready".equals(finalState.optString("phase")),
                "final service state is not live and ready");
            stage = "final_disconnect";
            runOnMainSync(service::disconnect);
            waitFor(() -> {
                JSONObject state = service.snapshot();
                return "disconnected".equals(state.optString("phase"))
                    && !state.optBoolean("busy") && state.optString("recording").isEmpty();
            }, 30_000, "final adapter disconnect");
            JSONObject disconnected = service.snapshot();
            require("live".equals(disconnected.optString("mode"))
                    && "disconnected".equals(disconnected.optString("phase")),
                "final service state is not safely disconnected");
            report.append("LIVE VEHICLE ACCEPTANCE PASSED\n")
                .append("Checks: ").append(checks)
                .append("; screen-off seconds: ")
                .append(SCREEN_OFF_SECONDS).append("\n")
                .append("Final state: mode=live, phase=disconnected; engine target validated, RPM available\n");
            result.putString("stream", report.toString());
            result.putString("mode", "live");
            result.putString("phase", "disconnected");
            result.putBoolean("engine_target_validated", true);
            result.putBoolean("rpm_available", true);
            finish(Activity.RESULT_OK, result);
        } catch (Throwable failure) {
            if (screenOff) {
                try { getUiAutomation().executeShellCommand("input keyevent 224").close(); } catch (Throwable ignored) { }
                screenOff = false;
            }
            cleanupAfterFailure();
            result.putString("stream", report + "FAILED at stage=" + stage + "; checks=" + checks + "\n");
            finish(Activity.RESULT_CANCELED, result);
        }
    }

    private void cleanupAfterFailure() {
        if (service == null) service = ObdService.current;
        if (service == null) return;
        try {
            if (!service.snapshot().optString("recording").isEmpty()) {
                service.stopRecording();
                waitFor(() -> service.snapshot().optString("recording").isEmpty(),
                    30_000, "failure recording close");
            }
        } catch (Throwable ignored) { }
        try {
            runOnMainSync(service::disconnect);
            waitFor(() -> {
                JSONObject state = service.snapshot();
                return "disconnected".equals(state.optString("phase"))
                    && !state.optBoolean("busy") && state.optString("recording").isEmpty();
            }, 30_000, "failure adapter disconnect");
        } catch (Throwable ignored) { }
    }

    private void ensureDisconnectedForScan() throws Exception {
        JSONObject state = service.snapshot();
        String phase = state.optString("phase");
        if (!"live".equals(state.optString("mode"))
                || (!"idle".equals(phase) && !"disconnected".equals(phase))
                || state.optBoolean("busy")) {
            runOnMainSync(service::disconnect);
            waitFor(() -> {
                JSONObject disconnected = service.snapshot();
                return "disconnected".equals(disconnected.optString("phase"))
                    && !disconnected.optBoolean("busy");
            }, 30_000, "clean live scan start");
        }
    }

    private void requireLiveValidatedTarget() {
        JSONObject state = service.snapshot();
        require("ready".equals(state.optString("phase")), "live service is not ready");
        require("live".equals(state.optString("mode")), "service left live mode");
        JSONObject vehicle = state.optJSONObject("vehicle");
        require(vehicle != null && vehicle.optBoolean("engine_target_validated"),
            "engine target is not validated");
    }

    private void requireFreshRpm() throws Exception {
        long requestedAt = System.currentTimeMillis();
        service.setSignals(new JSONArray().put("engine.rpm"));
        waitFor(() -> {
            JSONObject value = findValue(service.snapshot().optJSONArray("values"), "engine.rpm");
            return value != null && value.has("value") && !value.isNull("value")
                && value.optString("error", "").isEmpty()
                && Double.isFinite(value.optDouble("value", Double.NaN))
                && value.optLong("updatedAt") >= requestedAt;
        }, 25_000, "fresh live RPM");
    }

    private void runJob(String action) throws Exception {
        waitFor(() -> !service.snapshot().optBoolean("busy"), 30_000, "idle before vehicle job");
        service.runJob(action);
        waitFor(() -> !service.snapshot().optBoolean("busy"), 90_000, "vehicle job completion");
        JSONObject state = service.snapshot();
        require("ready".equals(state.optString("phase")) && "live".equals(state.optString("mode")),
            "vehicle job interrupted the live session");
        JSONObject job = state.optJSONObject("jobResult");
        require(job != null && job.optBoolean("ok"), "vehicle job did not complete");
        checks++;
    }

    private void keepRecordingScreenOff(File file, String filename) throws Exception {
        long initialLength = file.length();
        long previousLength = initialLength;
        boolean grew = false;
        screenOff = true;
        getUiAutomation().executeShellCommand("input keyevent 223").close();
        for (int second = 1; second <= SCREEN_OFF_SECONDS; second++) {
            SystemClock.sleep(1_000);
            JSONObject state = service.snapshot();
            require("ready".equals(state.optString("phase")) && "live".equals(state.optString("mode")),
                "live session left ready state during screen-off recording");
            require(filename.equals(state.optString("recording")), "recording stopped before the screen-off window ended");
            long length = file.length();
            require(length >= previousLength, "recording file size moved backwards");
            if (length > previousLength) grew = true;
            previousLength = length;
            maybeSendStatus(second);
        }
        require(grew && file.length() > initialLength, "screen-off recording did not append evidence");
        checks++;
        passed("Engine-drive recording stayed live and ready for " + SCREEN_OFF_SECONDS
            + " screen-off seconds; file grew");
    }

    private CaptureFacts inspectCapture(File file) throws Exception {
        CaptureFacts facts = new CaptureFacts();
        String lastType = "";
        boolean first = true;
        try (BufferedReader input = new BufferedReader(new InputStreamReader(
                new FileInputStream(file), StandardCharsets.UTF_8))) {
            String line;
            while ((line = input.readLine()) != null) {
                JSONObject event = new JSONObject(line);
                String type = event.optString("type");
                if (first) {
                    require("session".equals(type), "capture does not begin with a session header");
                    require("live".equals(event.optString("mode")), "capture header is not live mode");
                    require("engine-drive".equals(event.optString("profile")), "capture profile is not engine-drive");
                    first = false;
                }
                lastType = type;
                if ("wire".equals(type)) {
                    facts.tx += "TX".equals(event.optString("direction")) ? 1 : 0;
                    facts.rx += "RX".equals(event.optString("direction")) ? 1 : 0;
                }
                if ("action".equals(type)) {
                    String action = event.optString("action");
                    facts.reads += "read".equals(action) ? 1 : 0;
                    facts.hasInventory |= "inventory".equals(action);
                    facts.hasDtc |= "dtc.scan".equals(action);
                    JSONObject result = event.optJSONObject("result");
                    JSONObject data = result == null ? null : result.optJSONObject("data");
                    if ("read".equals(action) && "engine.rpm".equals(event.optString("argument"))
                            && result != null && result.optBoolean("ok") && data != null
                            && data.has("value") && !data.isNull("value")
                            && Double.isFinite(data.optDouble("value", Double.NaN))) {
                        facts.engineRpmPresent = true;
                        facts.recordedRpm = data.optDouble("value");
                    }
                }
            }
        }
        require(!first && "session_end".equals(lastType), "capture does not end with session_end");
        require(facts.tx > 0 && facts.rx > 0 && facts.reads > 0,
            "capture is missing TX, RX, or decoded read events");
        return facts;
    }

    private File copyCapture(File source) throws Exception {
        File directory = getTargetContext().getExternalFilesDir("acceptance");
        if (directory == null) directory = new File(getTargetContext().getFilesDir(), "acceptance");
        if (!directory.exists() && !directory.mkdirs()) throw new AssertionError("acceptance evidence directory unavailable");
        File destination = new File(directory, "vehicle-recording.jsonl");
        Files.copy(source.toPath(), destination.toPath(), StandardCopyOption.REPLACE_EXISTING);
        require(destination.isFile() && destination.length() == source.length(), "copied capture size differs");
        return destination;
    }

    private void verifyReadOnlyExport(String filename, long expectedLength) throws Exception {
        Uri uri = new Uri.Builder().scheme("content").authority(getTargetContext().getPackageName() + ".files")
            .appendPath("recordings").appendPath(filename).build();
        try (ParcelFileDescriptor descriptor = getTargetContext().getContentResolver().openFileDescriptor(uri, "r")) {
            require(descriptor != null && descriptor.getStatSize() == expectedLength,
                "read-only recording export did not open");
        }
        boolean writeDenied = false;
        try {
            ParcelFileDescriptor descriptor = getTargetContext().getContentResolver().openFileDescriptor(uri, "w");
            if (descriptor != null) descriptor.close();
        } catch (Exception expected) {
            writeDenied = true;
        }
        require(writeDenied, "recording provider accepted a write open");
        checks++;
    }

    private void requireReplayedRpm(double recordedRpm, JSONObject replay) {
        JSONObject rpm = findValue(replay.optJSONArray("values"), "engine.rpm");
        require(rpm != null && rpm.has("value") && !rpm.isNull("value")
            && rpm.optString("error", "").isEmpty(), "replay did not restore engine RPM");
        double replayedRpm = rpm.optDouble("value", Double.NaN);
        require(Double.isFinite(replayedRpm) && Math.abs(replayedRpm - recordedRpm) < 0.000001,
            "replay did not recompute engine RPM from its recorded response");
    }

    private JSONObject findValue(JSONArray values, String semantic) {
        if (values == null) return null;
        for (int i = 0; i < values.length(); i++) {
            JSONObject value = values.optJSONObject(i);
            if (value != null && semantic.equals(value.optString("semantic"))) return value;
        }
        return null;
    }

    private void waitFor(Condition condition, long timeoutMs, String description) throws Exception {
        long until = SystemClock.elapsedRealtime() + timeoutMs;
        while (SystemClock.elapsedRealtime() < until) {
            if (condition.ready()) return;
            maybeSendStatus(-1);
            SystemClock.sleep(100);
        }
        throw new AssertionError("timeout: " + description);
    }

    private void maybeSendStatus(int screenOffSecond) {
        long now = SystemClock.elapsedRealtime();
        if (now < nextStatusAt) return;
        Bundle status = new Bundle();
        status.putString("stage", stage);
        status.putInt("checks", checks);
        if (screenOffSecond >= 0) status.putInt("screen_off_seconds", screenOffSecond);
        sendStatus(1, status);
        nextStatusAt = now + STATUS_INTERVAL_MS;
    }

    private void passed(String text) {
        checks++;
        report.append("PASS: ").append(text).append('\n');
    }

    private void require(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private interface Condition { boolean ready() throws Exception; }

    private static final class CaptureFacts {
        int tx;
        int rx;
        int reads;
        boolean hasInventory;
        boolean hasDtc;
        boolean engineRpmPresent;
        double recordedRpm;
    }

    private void emitArtifact(File file) throws Exception {
        byte[] bytes = Files.readAllBytes(file.toPath());
        String encoded = android.util.Base64.encodeToString(bytes, android.util.Base64.NO_WRAP);
        for (int offset = 0; offset < encoded.length(); offset += 32_768) {
            Bundle chunk = new Bundle();
            chunk.putString("artifact", "vehicle-recording.jsonl");
            chunk.putInt("offset", offset);
            chunk.putString("data", encoded.substring(offset, Math.min(offset + 32_768, encoded.length())));
            sendStatus(10, chunk);
        }
    }
}
