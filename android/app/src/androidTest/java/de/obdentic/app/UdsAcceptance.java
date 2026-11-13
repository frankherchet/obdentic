package de.obdentic.app;

import android.app.Activity;
import android.app.Dialog;
import android.content.Context;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.TextView;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileReader;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import org.json.JSONArray;
import org.json.JSONObject;

/** REQ-027.1: the extended-session control stays unavailable in demo and explains why. */
public final class UdsAcceptance {
    public interface OnMain { <T> T call(java.util.concurrent.Callable<T> call) throws Exception; }

    public static void verifyDemoGate(OnMain main, Activity activity, ObdService service) throws Exception {
        main.call(() -> { requireButton(activity.getWindow().getDecorView(), "Diagnose").performClick(); return null; });
        Button enable = main.call(() -> requireButton(activity.getWindow().getDecorView(), "Erweiterte Sitzung aktivieren"));
        require(!enable.isEnabled(), "REQ-027.1 extended session is disabled in demo");
        require(main.call(() -> hasText(activity.getWindow().getDecorView(), "Nur im Livebetrieb verfügbar")), "REQ-027.1 disabled reason is visible");
        org.json.JSONObject uds = service.snapshot().optJSONObject("uds");
        require(uds == null || !uds.optBoolean("active"), "REQ-027.1 demo does not report an active UDS session");
    }

    /** REQ-027 positive UI/service lifecycle through an injected fake link; never opens BLE. */
    public static void verifyLive(OnMain main, Activity activity, ObdService service, Context context) throws Exception {
        final MainActivity screen = (MainActivity) activity;
        final String address = "AA:BB:CC:00:00:27";
        final File recordings = new File(context.getFilesDir(), "recordings");
        final Set<String> before = names(recordings);
        final VehicleStore oldStore = service.vehicles;
        final ObdService.Connector oldConnector = service.connector;
        final Boolean oldPermission = service.permissionOverride, oldBluetooth = service.bluetoothOverride;
        final FakeLink[] fake = new FakeLink[1];
        final String[] filename = {""};
        final String[] testedSemantic = {""};
        File testStore = new File(context.getCacheDir(), "uds-acceptance-" + System.nanoTime() + ".json");
        try {
            service.permissionOverride = Boolean.TRUE;
            service.bluetoothOverride = Boolean.TRUE;
            service.vehicles = new VehicleStore(testStore);
            fake[0] = new FakeLink(service.simulatedLink());
            service.connector = ignored -> fake[0];
            service.disconnect();
            service.connect(address);
            waitFor(() -> service.snapshot().optString("mode").equals("live")
                && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "fake adapter ready");

            service.runJob("inventory");
            waitFor(() -> !service.snapshot().optBoolean("busy") && service.snapshot().optJSONObject("vehicle").optBoolean("engine_target_validated"), "synthetic engine target validated");
            service.runJob("ecu.discover");
            waitFor(() -> !service.snapshot().optBoolean("busy")
                && service.snapshot().optJSONObject("ecuDiscovery").optJSONArray("targets") != null
                && service.snapshot().optInt("udsCandidates") > 0, "Knowledge engine target identified");
            require(fake[0].commands.contains("7E0:22F187"), "REQ-027.1 discovery probes the identified engine target with F187");

            main.call(() -> { requireButton(activity.getWindow().getDecorView(), "Diagnose").performClick(); return null; });
            waitFor(() -> main.call(() -> {
                Button enable = findButtonOrNull(activity.getWindow().getDecorView(), "Erweiterte Sitzung aktivieren");
                return enable != null && enable.isEnabled();
            }), "live UDS control enabled after engine identification");
            main.call(() -> { requireButton(activity.getWindow().getDecorView(), "Erweiterte Sitzung aktivieren").performClick(); return null; });
            waitFor(() -> service.snapshot().optJSONObject("uds").optBoolean("active"), "extended session active");
            waitFor(() -> main.call(() -> hasTextContaining(activity.getWindow().getDecorView(), "UDS erweiterte Sitzung aktiv")),
                "persistent UDS header visible");
            require(!main.call(() -> hasTextContaining(activity.getWindow().getDecorView(), "pausiert")),
                "REQ-027.6 the session header does not claim that OBD2 is paused");
            require(fake[0].commands.contains("7E0:1003"), "REQ-027.1 exactly the closed 10 03 activation command is sent to engine target");

            JSONObject candidate = firstSessionSignal(service.snapshot().optJSONArray("signals"));
            require(candidate != null, "REQ-027.2 extended-session signal appears in the active catalog");
            String semantic = candidate.optString("semantic");
            testedSemantic[0] = semantic;
            main.call(() -> {
                requireButton(activity.getWindow().getDecorView(), "Live").performClick();
                requireButton(activity.getWindow().getDecorView(), "Messwerte auswählen").performClick();
                return null;
            });
            Dialog selector = main.call(() -> screen.activeSignalSelector);
            View selectorRoot = main.call(() -> selector.getWindow().getDecorView());
            CheckBox sessionSignal = main.call(() -> findSessionSignal(selectorRoot));
            require(sessionSignal != null, "REQ-027.2 selector marks the candidate as UDS");
            main.call(() -> { sessionSignal.performClick(); requireButton(selector.getWindow().getDecorView(), "Übernehmen").performClick(); return null; });
            waitFor(() -> contains(service.snapshot().optJSONArray("selected"), semantic), "UDS signal selected through the full-screen selector");
            waitFor(() -> hasValue(service.snapshot().optJSONArray("values"), semantic), "targeted UDS DID decoded by the core");
            waitFor(() -> main.call(() -> hasTextContaining(activity.getWindow().getDecorView(), "nicht abgeglichen")
                && hasTextContaining(activity.getWindow().getDecorView(), "UDS · erweiterte Sitzung")), "live tile shows validation and UDS labels");
            require(hasTargetedRead(fake[0].commands),
                "REQ-027.2 selected candidate uses a targeted 0x22 read");
            waitFor(() -> fake[0].commands.contains("7E0:3E80"), "tester-present reached the target ECU");

            // REQ-027.6: selected standard PIDs and the selected UDS DID are polled together; the session is neither left nor re-armed
            final int mixedFrom = fake[0].commands.size();
            final long mixedAt = System.currentTimeMillis();
            final JSONArray mixed = new JSONArray(service.snapshot().optJSONArray("selected").toString());
            if (!contains(mixed, "engine.rpm")) mixed.put("engine.rpm"); // never twice, whatever the earlier selection was
            service.setSignals(mixed);
            waitFor(() -> contains(service.snapshot().optJSONArray("selected"), "engine.rpm") && contains(service.snapshot().optJSONArray("selected"), semantic),
                "REQ-027.6 the standard PID joins the UDS selection while the session is active");
            waitFor(() -> service.snapshot().optJSONObject("uds").optBoolean("active")
                && hasFreshValue(service.snapshot().optJSONArray("values"), "engine.rpm", mixedAt) && hasFreshValue(service.snapshot().optJSONArray("values"), semantic, mixedAt),
                "REQ-027.6 a new successful engine.rpm value and a new UDS value are decoded while the session is active");
            waitFor(() -> alternations(fake[0].commands, mixedFrom, "010C") >= 3 && count(fake[0].commands, mixedFrom, "3E80") >= 2,
                "REQ-027.6 Mode 01 and 0x22 reads interleave while tester present keeps recurring");
            final List<String> mixedWindow = since(fake[0].commands, mixedFrom);
            require(indexOf(mixedWindow, "1001") < 0 && indexOf(mixedWindow, "1003") < 0,
                "REQ-027.6 polling a standard PID neither leaves nor re-arms the extended session: " + mixedWindow);
            require(service.snapshot().optJSONObject("uds").optBoolean("active"), "REQ-027.6 the extended session stays active while standard PIDs are polled");
            waitFor(() -> main.call(() -> hasText(activity.getWindow().getDecorView(), "OBD2")), "REQ-027.6 the standard tile is labelled OBD2 next to the UDS tile");
            require(!main.call(() -> hasTextContaining(activity.getWindow().getDecorView(), "pausiert")), "REQ-027.6 no tile claims that OBD2 is paused");
            main.call(() -> { requireButton(activity.getWindow().getDecorView(), "Diagnose").performClick(); return null; });
            // The persistent header is one text; the Diagnose card adds its own status line and its note and button, which exist only there
            waitFor(() -> main.call(() -> countTextStarting(activity.getWindow().getDecorView(), "UDS erweiterte Sitzung aktiv") >= 2
                && hasTextContaining(activity.getWindow().getDecorView(), "Modus 01")
                && findButtonOrNull(activity.getWindow().getDecorView(), "Standard-Sitzung wiederherstellen") != null), "REQ-027.6 the Diagnose session card shows its own status, Mode 01 note and switch-off button");
            require(!main.call(() -> hasTextContaining(activity.getWindow().getDecorView(), "pausiert")), "REQ-027.6 Diagnose does not claim that OBD2 is paused");
            main.call(() -> { requireButton(activity.getWindow().getDecorView(), "Live").performClick(); return null; });

            // REQ-027.3/.6: another functional job still leaves the session first; the OBD2 selection keeps polling, only the UDS signal goes
            final int jobFrom = fake[0].commands.size();
            service.runJob("dtc.scan");
            waitFor(() -> !service.snapshot().optBoolean("busy") && !service.snapshot().optJSONObject("uds").optBoolean("active")
                && service.snapshot().optJSONObject("diagnostics").length() > 0, "DTC job finished after leaving the extended session");
            final List<String> jobWindow = since(fake[0].commands, jobFrom);
            final int left = indexOf(jobWindow, "1001"), scan = indexOf(jobWindow, "03");
            require(left >= 0 && scan > left, "REQ-027.3 the DTC read is preceded by 10 01: " + jobWindow);
            require(contains(service.snapshot().optJSONArray("selected"), "engine.rpm") && !contains(service.snapshot().optJSONArray("selected"), semantic),
                "REQ-027.6 the OBD2 selection survives the session while the UDS signal is removed");
            final int afterJob = fake[0].commands.size();
            final long afterJobAt = System.currentTimeMillis();
            waitFor(() -> count(fake[0].commands, afterJob, "010C") >= 2 && hasFreshValue(service.snapshot().optJSONArray("values"), "engine.rpm", afterJobAt),
                "REQ-027.6 the OBD2 selection keeps polling after the session ended with a new successful value");
            require(count(fake[0].commands, jobFrom + left, "3E80") == 0, "REQ-027.2 no tester present once the session has ended");

            // The session is explicit: switch it on again and select the UDS signal next to the standard one
            final long reactivatedAt = System.currentTimeMillis();
            service.runJob("uds.enable");
            waitFor(() -> service.snapshot().optJSONObject("uds").optBoolean("active") && !service.snapshot().optBoolean("busy"), "extended session activated again explicitly");
            service.setSignals(mixed);
            waitFor(() -> service.snapshot().optJSONObject("uds").optBoolean("active")
                && hasFreshValue(service.snapshot().optJSONArray("values"), "engine.rpm", reactivatedAt) && hasFreshValue(service.snapshot().optJSONArray("values"), semantic, reactivatedAt),
                "REQ-027.6 new successful values of both kinds after the explicit re-activation");

            service.startRecording("live-selection");
            waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "selected UDS signal recording starts");
            filename[0] = service.snapshot().optString("recording");
            File evidence = new File(recordings, filename[0]);
            waitFor(() -> hasReadAction(evidence, semantic), "partial recording includes a selected UDS DID read");
            waitFor(() -> hasSessionMarkedRead(evidence, "engine.rpm"), "REQ-027.6 the recording marks the session state of the Mode 01 read");
            service.disconnect();
            waitFor(() -> service.snapshot().optString("phase").equals("disconnected")
                && service.snapshot().optString("recording").isEmpty(), "disconnect closes the partial recording");
            int leave = fake[0].commands.lastIndexOf("7E0:1001");
            int close = fake[0].commands.lastIndexOf("CLOSE");
            require(leave >= 0 && close > leave, "REQ-027.3 disconnect sends 10 01 before closing the fake link");
            require(fake[0].commands.contains("7E0:3E80"), "REQ-027.2 tester-present uses the target engine ECU");
            JSONObject end = lastEvent(evidence);
            require("session_end".equals(end.optString("type")) && "disconnected".equals(end.optString("reason")),
                "REQ-027.3 disconnect leaves the recording partial with a terminal marker");

            int commandsAfterDisconnect = fake[0].commands.size();
            service.openRecording(filename[0]);
            waitFor(() -> service.snapshot().optString("mode").equals("replay") && !service.snapshot().optBoolean("busy")
                && hasValue(service.snapshot().optJSONArray("values"), semantic), "offline re-decode of the recorded UDS DID");
            require(fake[0].commands.size() == commandsAfterDisconnect, "REQ-027.5 replay re-decodes without link traffic");
            require(hasReplayValue(service.snapshot().optJSONArray("values"), semantic), "REQ-027.5 replay marks the decoded UDS value as replayed");
        } finally {
            Dialog selector = screen.activeSignalSelector;
            if (selector != null) main.call(() -> { selector.dismiss(); return null; });
            if (filename[0].isEmpty()) filename[0] = service.snapshot().optString("recording");
            if (!service.snapshot().optString("phase").equals("disconnected")) service.disconnect();
            waitFor(() -> service.snapshot().optString("phase").equals("disconnected")
                && service.snapshot().optString("recording").isEmpty(), "fake link stopped after acceptance");
            service.connector = oldConnector;
            service.permissionOverride = oldPermission;
            service.bluetoothOverride = oldBluetooth;
            service.setDemo();
            waitFor(() -> service.snapshot().optString("mode").equals("demo")
                && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo restored after UDS acceptance");
            require(testedSemantic[0].isEmpty() || !contains(service.snapshot().optJSONArray("selected"), testedSemantic[0]),
                "REQ-027.3 session candidates are removed from the selection when leaving replay for demo");
            service.vehicles = oldStore;
            if (!filename[0].isEmpty() && !before.contains(filename[0])) new File(recordings, filename[0]).delete();
            new android.util.AtomicFile(testStore).delete();
            new File(testStore.getPath() + ".new").delete();
        }
    }

    private static final class FakeLink implements ObdService.Link {
        final ObdService.Link delegate;
        final List<String> commands = Collections.synchronizedList(new ArrayList<>());
        volatile boolean open = true;
        volatile String target = "", responder = "";
        FakeLink(ObdService.Link delegate) { this.delegate = delegate; }
        @Override public boolean isOpen() { return open && delegate.isOpen(); }
        @Override public void close() { if (open) { commands.add("CLOSE"); open = false; delegate.close(); } }
        @Override public String exchange(String command, long timeoutMs) throws Exception {
            String compact = command.trim().replace(" ", "").toUpperCase(java.util.Locale.ROOT);
            if (!open) throw new java.io.IOException("fake link closed");
            if (compact.startsWith("ATSH")) target = compact.substring(4);
            if (compact.startsWith("ATCRA")) responder = compact.substring(5);
            commands.add(target + ":" + compact);
            if (compact.equals("1003")) return target.equals("7E0") ? "7E8 06 50 03 00 32 01 F4\r>" : "NO DATA\r>";
            if (compact.equals("1001")) return target.equals("7E0") ? "7E8 02 50 01\r>" : "NO DATA\r>";
            if (compact.equals("3E80")) return "NO DATA\r>";
            if (compact.startsWith("22")) {
                if (!target.equals("7E0")) return "NO DATA\r>";
                int did = Integer.parseInt(compact.substring(2), 16);
                if (did == 0xF187) return "7E8 06 62 F1 87 31 32 33\r>";
                return String.format(java.util.Locale.ROOT, "7E8 05 62 %02X %02X 00 64\r>", did >> 8, did & 0xff);
            }
            return delegate.exchange(command, timeoutMs);
        }
    }

    private static JSONObject firstSessionSignal(JSONArray signals) {
        for (int i = 0; signals != null && i < signals.length(); i++) {
            JSONObject signal = signals.optJSONObject(i);
            if (signal != null && signal.optBoolean("extended_session_required")) return signal;
        }
        return null;
    }

    private static CheckBox findSessionSignal(View root) {
        if (root instanceof CheckBox) {
            CheckBox box = (CheckBox) root;
            if (box.getContentDescription() != null && box.getContentDescription().toString().contains("UDS · erweiterte Sitzung")) return box;
        }
        if (root instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) root;
            for (int i = 0; i < group.getChildCount(); i++) {
                CheckBox box = findSessionSignal(group.getChildAt(i));
                if (box != null) return box;
            }
        }
        return null;
    }

    private static boolean contains(JSONArray values, String expected) {
        for (int i = 0; values != null && i < values.length(); i++) if (expected.equals(values.optString(i))) return true;
        return false;
    }

    private static boolean hasValue(JSONArray values, String semantic) {
        for (int i = 0; values != null && i < values.length(); i++) {
            JSONObject value = values.optJSONObject(i);
            if (value == null || !semantic.equals(value.optString("semantic")) || !value.has("value") || value.isNull("value") || value.has("error")) continue;
            double number = value.optDouble("value", Double.NaN);
            if (!Double.isNaN(number) && !Double.isInfinite(number)) return true;
        }
        return false;
    }

    /** A value row written after {@code since} (wall clock) that decoded successfully: a number, no error. */
    private static boolean hasFreshValue(JSONArray values, String semantic, long since) {
        for (int i = 0; values != null && i < values.length(); i++) {
            JSONObject value = values.optJSONObject(i);
            if (value == null || !semantic.equals(value.optString("semantic")) || value.optLong("updatedAt", 0) <= since) continue;
            if (!value.has("value") || value.isNull("value") || value.has("error") || value.optBoolean("replay")) continue;
            if (Double.isFinite(value.optDouble("value", Double.NaN))) return true;
        }
        return false;
    }

    private static int countTextStarting(View view, String prefix) {
        int n = view instanceof TextView && view.getVisibility() == View.VISIBLE && ((TextView) view).getText().toString().startsWith(prefix) ? 1 : 0;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) n += countTextStarting(group.getChildAt(i), prefix);
        }
        return n;
    }

    private static boolean hasReplayValue(JSONArray values, String semantic) {
        for (int i = 0; values != null && i < values.length(); i++) {
            JSONObject value = values.optJSONObject(i);
            if (value != null && semantic.equals(value.optString("semantic")) && value.optBoolean("replay")
                && !Double.isNaN(value.optDouble("value", Double.NaN)) && !Double.isInfinite(value.optDouble("value", Double.NaN))) return true;
        }
        return false;
    }

    private static boolean hasTargetedRead(List<String> commands) {
        synchronized (commands) {
            for (String command : commands) if (command.startsWith("7E0:22") && !command.equals("7E0:22F187")) return true;
            return false;
        }
    }

    private static List<String> since(List<String> commands, int from) {
        synchronized (commands) { return new ArrayList<>(commands.subList(Math.min(from, commands.size()), commands.size())); }
    }

    /** The compact command of a recorded "target:command" entry. */
    private static String body(String entry) { return entry.substring(entry.indexOf(':') + 1); }

    private static int indexOf(List<String> window, String compact) {
        for (int i = 0; i < window.size(); i++) if (body(window.get(i)).equals(compact)) return i;
        return -1;
    }

    private static int count(List<String> commands, int from, String compact) {
        int n = 0;
        for (String entry : since(commands, from)) if (body(entry).equals(compact)) n++;
        return n;
    }

    /** How often the stream switches between the given Mode 01 read and a targeted 0x22 read (identification F187 excluded). */
    private static int alternations(List<String> commands, int from, String standard) {
        char last = 0;
        int changes = 0;
        for (String entry : since(commands, from)) {
            String command = body(entry);
            char kind = command.equals(standard) ? 'S' : command.startsWith("22") && !command.equals("22F187") ? 'U' : 0;
            if (kind == 0) continue;
            if (last != 0 && kind != last) changes++;
            last = kind;
        }
        return changes;
    }

    private static boolean hasSessionMarkedRead(File file, String semantic) throws Exception {
        if (!file.isFile()) return false;
        try (BufferedReader input = new BufferedReader(new FileReader(file))) {
            String line;
            while ((line = input.readLine()) != null) {
                JSONObject event;
                try { event = new JSONObject(line); } catch (org.json.JSONException partial) { continue; } // the writer may be mid-line
                if (!"action".equals(event.optString("type")) || !"read".equals(event.optString("action")) || !semantic.equals(event.optString("argument"))) continue;
                JSONObject result = event.optJSONObject("result");
                JSONObject uds = result == null ? null : result.optJSONObject("uds");
                if (uds == null && result != null && result.optJSONObject("data") != null) uds = result.optJSONObject("data").optJSONObject("uds");
                if (uds != null && uds.optBoolean("active")) return true;
            }
        }
        return false;
    }

    private static boolean hasReadAction(File file, String semantic) throws Exception {
        if (!file.isFile()) return false;
        try (BufferedReader input = new BufferedReader(new FileReader(file))) {
            String line;
            while ((line = input.readLine()) != null) {
                JSONObject event = new JSONObject(line);
                if ("action".equals(event.optString("type")) && "read".equals(event.optString("action"))
                    && semantic.equals(event.optString("argument"))) return true;
            }
        }
        return false;
    }

    private static JSONObject lastEvent(File file) throws Exception {
        JSONObject last = new JSONObject();
        try (BufferedReader input = new BufferedReader(new FileReader(file))) {
            String line;
            while ((line = input.readLine()) != null) last = new JSONObject(line);
        }
        return last;
    }

    private static Set<String> names(File directory) {
        String[] files = directory.list((ignored, name) -> name.endsWith(".jsonl"));
        return files == null ? new HashSet<>() : new HashSet<>(Arrays.asList(files));
    }

    private interface Condition { boolean ready() throws Exception; }
    private static void waitFor(Condition condition, String label) throws Exception {
        long deadline = SystemClock.elapsedRealtime() + 30000;
        while (SystemClock.elapsedRealtime() < deadline) { if (condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("Timeout: " + label);
    }

    private static Button requireButton(View view, String label) {
        Button found = findButtonOrNull(view, label);
        if (found == null) throw new AssertionError("Missing button: " + label);
        return found;
    }

    private static Button findButtonOrNull(View view, String label) {
        if (view instanceof Button && label.contentEquals(((Button) view).getText())) return (Button) view;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                Button found = findButtonOrNull(group.getChildAt(i), label);
                if (found != null) return found;
            }
        }
        return null;
    }

    private static boolean hasText(View view, String text) {
        if (view instanceof TextView && text.contentEquals(((TextView) view).getText())) return true;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) if (hasText(group.getChildAt(i), text)) return true;
        }
        return false;
    }

    private static boolean hasTextContaining(View view, String text) {
        if (view instanceof TextView && ((TextView) view).getText().toString().contains(text)) return true;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) if (hasTextContaining(group.getChildAt(i), text)) return true;
        }
        return false;
    }

    private static void require(boolean passed, String message) {
        if (!passed) throw new AssertionError(message);
    }
}
