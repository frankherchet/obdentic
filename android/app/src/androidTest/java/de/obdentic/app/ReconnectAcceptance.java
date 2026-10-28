package de.obdentic.app;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.pm.ApplicationInfo;
import android.os.SystemClock;
import org.json.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.*;

/**
 * REQ-020 through the real service with a fake adapter link (answers like the demo, but in live mode) and fake Bluetooth state.
 * No BLE traffic happens. Direct GATT connect by address, the real Bluetooth on/off broadcast, engine start/stop and the app restart
 * itself remain for the vehicle session (#178); here the same code paths are entered through the public seams.
 */
final class ReconnectAcceptance {
    private static final String A = "AA:BB:CC:00:00:01", B = "AA:BB:CC:00:00:02";

    /** Adapter that can be unreachable and whose open link can be cut. */
    private static final class Fake implements ObdService.Connector {
        final ObdService service; volatile boolean reachable = true; volatile Cut last;
        final List<String> opens = Collections.synchronizedList(new ArrayList<>()); final List<Long> times = Collections.synchronizedList(new ArrayList<>());
        Fake(ObdService service) { this.service = service; }
        public ObdService.Link open(String address) throws Exception {
            times.add(SystemClock.elapsedRealtime()); opens.add(address);
            if (!reachable) throw new IOException("Adapter nicht erreichbar");
            last = new Cut(service.simulatedLink()); return last;
        }
    }
    private static final class Cut implements ObdService.Link {
        final ObdService.Link inner; volatile boolean up = true;
        Cut(ObdService.Link inner) { this.inner = inner; }
        public boolean isOpen() { return up && inner.isOpen(); }
        public void close() { inner.close(); }
        public String exchange(String command, long timeoutMs) throws Exception { if (!up) throw new IOException("Verbindung unterbrochen"); return inner.exchange(command, timeoutMs); }
    }

    static void run(ObdService service, Context context) throws Exception {
        policy();
        SharedPreferences prefs = context.getSharedPreferences("adapter", Context.MODE_PRIVATE);
        File recordings = new File(context.getFilesDir(), "recordings");
        Set<String> before = names(recordings);
        VehicleStore savedStore = service.vehicles;
        Fake fake = new Fake(service);
        try {
            prefs.edit().clear().commit();
            require(ObdService.autoConnectEnabled(context), "REQ-020.6 automatic connection is on by default");
            service.vehicles = new VehicleStore(new File(context.getCacheDir(), "reconnect-vehicles-" + System.nanoTime() + ".json"));
            service.connector = fake; service.setAutoConnect(true);

            // REQ-020.1 / REQ-020.5: the address is stored after a successful connect, replaced by the next one, app-private and not backed up
            service.disconnect(); service.connect(A); ready(service);
            require(A.equals(service.knownAdapter()), "REQ-020.1 known adapter stored after the first connection");
            String xml = new String(Files.readAllBytes(new File(context.getFilesDir().getParentFile(), "shared_prefs/adapter.xml").toPath()), "UTF-8");
            require(xml.contains(A), "REQ-020.1 address is written to app-private storage: " + xml);
            require((context.getApplicationInfo().flags & ApplicationInfo.FLAG_ALLOW_BACKUP) == 0, "REQ-020.1 backup is disabled for the app");
            service.disconnect(); service.connect(B); ready(service);
            xml = new String(Files.readAllBytes(new File(context.getFilesDir().getParentFile(), "shared_prefs/adapter.xml").toPath()), "UTF-8");
            require(B.equals(service.knownAdapter()) && xml.contains(B) && !xml.contains(A), "REQ-020.1 a new connection replaces the known adapter: " + xml);
            require(service.snapshot().optString("knownAdapter").equals("…00:02") && !service.snapshot().toString().contains(B), "REQ-020.1 the UI state only carries the masked address");
            int fromHere = fake.opens.size();

            // REQ-020.3 / .7 / .8: loss with a running recording and a job result on screen
            service.runJob("dtc.scan"); waitFor(() -> !service.snapshot().optBoolean("busy") && service.snapshot().optJSONObject("diagnostics").length() > 0, "dtc scan before the loss");
            service.startRecording("engine-drive"); waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "recording before the loss");
            String first = service.snapshot().getString("recording");
            fake.reachable = false; int opens = fake.opens.size(); long cut = SystemClock.elapsedRealtime(); fake.last.up = false;
            waitFor(() -> service.snapshot().optString("phase").equals("error") && service.snapshot().optString("message").contains("neuer Versuch in"), "REQ-020.3 loss shows when the next attempt follows");
            require(service.snapshot().optString("recording").isEmpty() && lastEvent(new File(recordings, first)).contains("\"reason\":\"error\""), "REQ-020.8 the interrupted recording stays saved as a partial recording (session_end error)");
            waitFor(() -> fake.opens.size() > opens, "REQ-020.3 first retry");
            long first_gap = fake.times.get(opens) - cut;
            require(first_gap >= 1800 && first_gap < 3500, "REQ-020.3 first retry follows after about 2 s (poll detection plus pause): " + first_gap + " ms");
            waitFor(() -> service.snapshot().optString("phase").equals("error") && service.snapshot().optString("message").contains("neuer Versuch in"), "failed attempt shows the next one");
            fake.reachable = true;
            waitFor(() -> fake.opens.size() > opens + 1, "REQ-020.3 second retry");
            long second_gap = fake.times.get(opens + 1) - fake.times.get(opens);
            require(second_gap >= 4500 && second_gap < 7000, "REQ-020.3 second retry follows after about 5 s: " + second_gap + " ms");
            ready(service);
            require(fake.opens.subList(fromHere, fake.opens.size()).stream().allMatch(B::equals), "REQ-020.5 every automatic attempt targets the known adapter only: " + fake.opens);
            require(service.snapshot().optJSONObject("diagnostics").length() == 0 && service.snapshot().optJSONObject("jobResult").length() == 0, "REQ-020.7 reconnecting does not repeat the earlier job (only initialize and inventory ran)");
            waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "REQ-020.8 a new recording starts after the automatic reconnection");
            String second = service.snapshot().getString("recording");
            require(!second.equals(first) && headerProfile(new File(recordings, second)).equals("engine-drive"), "REQ-020.8 the new recording uses the same profile in a new file");

            // REQ-020.4: every manual stop ends the attempts, and none start again
            String stored = first;
            stopCases(service, fake, "REQ-020.4 manual disconnect", service::disconnect);
            stopCases(service, fake, "REQ-020.4 stop button", service::stopAutoConnect);
            stopCases(service, fake, "REQ-020.4 demo start", service::setDemo);
            int demoOpens = fake.opens.size(); service.autoStart(); SystemClock.sleep(500);
            require(fake.opens.size() == demoOpens && service.snapshot().optString("mode").equals("demo"), "REQ-020.4 demo never triggers an automatic connection");
            stopCases(service, fake, "REQ-020.4 replay", () -> service.openRecording(stored));
            int replayOpens = fake.opens.size(); service.autoStart(); SystemClock.sleep(500);
            require(fake.opens.size() == replayOpens && service.snapshot().optString("mode").equals("replay"), "REQ-020.4 replay never triggers an automatic connection");

            // REQ-020.2: app start with a known adapter connects without user action
            service.disconnect(); service.connect(B); ready(service); service.disconnect(); // back to live mode, nothing connected, as at app start
            int startOpens = fake.opens.size(); service.autoStart();
            ready(service);
            require(fake.opens.size() == startOpens + 1 && fake.opens.get(startOpens).equals(B), "REQ-020.2 app start connects the known adapter without user action");

            // REQ-020.6: switched off, a loss starts no attempt; switching on connects again; persisted
            service.setAutoConnect(false);
            xml = new String(Files.readAllBytes(new File(context.getFilesDir().getParentFile(), "shared_prefs/adapter.xml").toPath()), "UTF-8");
            require(xml.contains("name=\"auto\" value=\"false\"") && !ObdService.autoConnectEnabled(context), "REQ-020.6 the switch is persisted: " + xml);
            int offOpens = fake.opens.size(); fake.last.up = false;
            waitFor(() -> service.snapshot().optString("phase").equals("error"), "loss with automatic connection off");
            SystemClock.sleep(2600);
            require(fake.opens.size() == offOpens && !service.snapshot().optBoolean("autoPending") && !service.snapshot().optString("message").contains("neuer Versuch"), "REQ-020.6 no attempt while switched off");
            service.setAutoConnect(true); ready(service);
            require(ObdService.autoConnectEnabled(context) && fake.opens.size() == offOpens + 1, "REQ-020.6 switching on connects the known adapter again");

            // REQ-020.9: permission missing, Bluetooth off, at loss and at app start; resume when allowed again
            for (String why : new String[]{"Berechtigungen fehlen", "Bluetooth einschalten"}) {
                boolean permission = why.startsWith("Berechtigungen");
                fake.last.up = false; setEnvironment(service, permission, false);
                waitFor(() -> service.snapshot().optString("message").contains(why), "REQ-020.9 loss shows the reason: " + why);
                int blockedOpens = fake.opens.size(); SystemClock.sleep(2600);
                require(fake.opens.size() == blockedOpens && service.snapshot().optBoolean("autoPending"), "REQ-020.9 no attempt while blocked (" + why + ")");
                setEnvironment(service, permission, true); service.resumeIfBlocked(); ready(service);
                require(fake.opens.size() == blockedOpens + 1, "REQ-020.9 attempts resume once allowed again (" + why + ")");
                service.disconnect(); setEnvironment(service, permission, false); service.autoStart();
                waitFor(() -> service.snapshot().optString("message").contains(why), "REQ-020.9 app start shows the reason: " + why);
                int startBlocked = fake.opens.size(); SystemClock.sleep(500);
                require(fake.opens.size() == startBlocked, "REQ-020.9 no attempt at app start while blocked (" + why + ")");
                setEnvironment(service, permission, true); service.resumeIfBlocked(); ready(service);
                require(fake.opens.size() == startBlocked + 1, "REQ-020.9 app start attempt resumes once allowed again (" + why + ")");
            }
        } finally {
            service.permissionOverride = null; service.bluetoothOverride = null; service.setAutoConnect(false); service.connector = null;
            service.setDemo(); waitFor(() -> service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy") && service.snapshot().optString("recording").isEmpty(), "demo restored after the reconnect checks");
            service.vehicles = savedStore;
            for (String name : names(recordings)) if (!before.contains(name)) new File(recordings, name).delete();
            prefs.edit().clear().commit();
        }
    }

    /** Loss, wait for a pending attempt, apply the stop, and require that no attempt follows within the first pause. */
    private static void stopCases(ObdService service, Fake fake, String name, Runnable stop) throws Exception {
        service.disconnect(); fake.reachable = true; service.connect(B); ready(service);
        fake.reachable = false; fake.last.up = false;
        waitFor(() -> service.snapshot().optString("phase").equals("error") && service.snapshot().optString("message").contains("neuer Versuch in"), name + ": attempt pending");
        require(service.snapshot().optBoolean("autoPending"), name + ": pending state visible");
        int opens = fake.opens.size(); stop.run(); SystemClock.sleep(2600);
        require(fake.opens.size() == opens && !service.snapshot().optBoolean("autoPending"), name + ": no automatic attempt follows (" + (fake.opens.size() - opens) + ")");
        fake.reachable = true;
    }

    private static void setEnvironment(ObdService service, boolean permission, boolean allowed) {
        if (permission) service.permissionOverride = allowed ? null : Boolean.FALSE; else service.bluetoothOverride = allowed ? null : Boolean.FALSE;
    }

    private static void policy() {
        ReconnectPolicy p = new ReconnectPolicy(true);
        long[] expected = {2000, 5000, 10000, 30000, 30000, 30000};
        for (int i = 0; i < expected.length; i++) require(ReconnectPolicy.delayMs(i) == expected[i], "REQ-020.3 pause " + i);
        require(p.lost() == 2000, "REQ-020.3 loss: first pause 2 s");
        for (int i = 1; i < expected.length; i++) { require(p.attemptDue(), "REQ-020.3 pause over"); require(p.failed() == expected[i], "REQ-020.3 pause after failure " + i + " is " + expected[i]); }
        require(p.attemptDue(), "REQ-020.3 attempt"); p.success();
        require(p.lost() == 2000, "REQ-020.3 the schedule restarts after a success");
        p.stop();
        require(p.lost() == -1 && p.failed() == -1 && !p.attemptDue() && !p.pending(), "REQ-020.4 after a manual stop nothing is scheduled");
        p.userConnect();
        require(p.lost() == 2000, "REQ-020.4 a user connection allows attempts after a later loss again");
        p.stop(); require(p.startup() && p.state() == ReconnectPolicy.State.CONNECTING, "REQ-020.2 an app start begins with an immediate attempt");
        p.block(); require(p.pending() && !p.attemptDue() && p.state() == ReconnectPolicy.State.BLOCKED, "REQ-020.9 blocked: no attempt");
        require(p.unblock() && p.state() == ReconnectPolicy.State.CONNECTING && !p.unblock(), "REQ-020.9 unblock resumes once");
        p.setEnabled(false);
        require(!p.enabled() && !p.pending() && p.lost() == -1 && !p.startup(), "REQ-020.6 switched off: no attempts");
        require(new ReconnectPolicy(true).failed() == -1, "REQ-020.4 a failure outside an automatic attempt (manual connect) schedules nothing");
        require(ReconnectPolicy.resumeProfile("engine-drive").equals("engine-drive"), "REQ-020.8 a profile continues with the same profile");
        require(ReconnectPolicy.resumeProfile("").isEmpty() && ReconnectPolicy.resumeProfile(null).isEmpty(), "REQ-020.8 no recording never continues");
    }

    private static Set<String> names(File dir) { String[] list = dir.list(); return list == null ? new HashSet<>() : new HashSet<>(Arrays.asList(list)); }
    private static String read(File file) throws IOException { return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8); }
    private static String lastEvent(File file) throws IOException { String[] lines = read(file).trim().split("\n"); return lines[lines.length - 1]; }
    private static String headerProfile(File file) throws Exception { return new JSONObject(read(file).split("\n")[0]).optString("profile"); }
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
