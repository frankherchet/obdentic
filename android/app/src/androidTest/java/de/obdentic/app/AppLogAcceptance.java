package de.obdentic.app;

import android.content.Context;
import android.os.SystemClock;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.*;
import java.util.regex.Pattern;

/** REQ-029 through the public AppLog API and the real service with a fake adapter link. GATT status logging stays with the vehicle session (#178). */
final class AppLogAcceptance {
    private static final String ADAPTER = "AA:BB:CC:00:00:07";
    private static final Pattern LINE = Pattern.compile("\\d{4}-\\d\\d-\\d\\dT\\d\\d:\\d\\d:\\d\\d\\.\\d{3}[+-]\\d{4} INFO unit hello world");

    private static final class DirTarget implements RecordingExporter.Target {
        final File dir; DirTarget(File dir) { this.dir = dir; }
        public Map<String, Long> list() { Map<String, Long> names = new HashMap<>(); File[] files = dir.listFiles(); if (files != null) for (File f : files) names.put(f.getName(), f.length()); return names; }
        public OutputStream create(String name) throws IOException { return new FileOutputStream(new File(dir, name)); }
        public void delete(String name) { new File(dir, name).delete(); }
    }

    static void run(ObdService service, Context context) throws Exception {
        File original = new File(context.getFilesDir(), "logs"), work = new File(context.getCacheDir(), "applog-" + System.nanoTime());
        VehicleStore savedStore = service.vehicles; ObdService.Connector savedConnector = service.connector; RecordingExporter.Target savedTarget = service.exportTarget;
        boolean[] reachable = {true};
        Set<String> beforeRecordings = names(new File(context.getFilesDir(), "recordings"));
        try {
            require(work.mkdirs(), "test setup"); AppLog.setDirectory(work);

            // REQ-029.1: line format, size seal with unique names, newest 50 kept
            AppLog.i("unit", "hello world");
            String first = read(new File(work, "app.log")).trim();
            require(LINE.matcher(first).matches(), "REQ-029.1 line format: " + first);
            AppLog.maxBytes = 50;
            for (int i = 0; i < 70; i++) { AppLog.i("unit", "line number " + i + " padded to be long enough to seal the file quickly, " + i); SystemClock.sleep(2); }
            AppLog.maxBytes = 1024 * 1024;
            List<File> sealed = AppLog.sealed();
            require(sealed.size() == 50, "REQ-029.1 only the newest 50 sealed files are kept: " + sealed.size());
            require(read(sealed.get(0)).contains("line number 69") || read(new File(work, "app.log")).contains("line number 69"), "REQ-029.1 the newest line is kept");
            require(new HashSet<>(names(work)).stream().allMatch(n -> n.equals("app.log") || n.matches("app-[0-9]{8}-[0-9]{6}-[0-9]{3}\\.log")), "REQ-029.1 unique sealed names: " + names(work));
            for (String n : names(work)) new File(work, n).delete();

            // REQ-029.3 / .4: stack traces with causes, uncaught exceptions, scrubbed identifiers
            AppLog.logUncaught(Thread.currentThread(), new RuntimeException("boom", new IOException("root cause WVWZZZ1KZAW123456")));
            AppLog.i("unit", "vin WVWZZZ1KZAW123456 addr AA:BB:CC:DD:EE:FF raw 7E8 05 62 11 4F 05 8E 55 55 ok");
            String text = read(new File(work, "app.log"));
            require(text.contains("FATAL uncaught") && text.contains("java.lang.RuntimeException: boom") && text.contains("Caused by: java.io.IOException: root cause") && text.contains("at de.obdentic.app.AppLogAcceptance.run"), "REQ-029.3 uncaught exception with stack trace and cause: " + text);
            require(!text.contains("WVWZZZ1KZAW123456") && !text.contains("AA:BB:CC:DD:EE:FF") && !text.contains("05 62 11 4F") && text.contains("[VIN]") && text.contains("…EE:FF") && text.contains("[hex]"), "REQ-029.4 identifiers and raw traffic are scrubbed: " + text);
            for (String n : names(work)) new File(work, n).delete();

            // REQ-029.5: an unwritable log never disturbs a session
            service.vehicles = new VehicleStore(new File(context.getCacheDir(), "applog-vehicles-" + System.nanoTime() + ".json"));
            service.connector = address -> { if (!reachable[0]) throw new IOException("Adapter nicht erreichbar"); return service.simulatedLink(); };
            service.setAutoConnect(false); service.disconnect();
            File blocked = new File(context.getCacheDir(), "applog-blocked-" + System.nanoTime()); require(blocked.createNewFile(), "test setup: a file where the log folder should be");
            AppLog.setDirectory(blocked); AppLog.i("unit", "ignored"); AppLog.e("unit", "ignored", new IOException("x")); AppLog.seal();
            service.connect(ADAPTER); ready(service); service.runJob("dtc.scan"); waitFor(() -> service.snapshot().optJSONObject("diagnostics").length() > 0, "job with an unwritable log");
            service.disconnect(); blocked.delete(); AppLog.setDirectory(work);

            // REQ-029.2: attempt, ready, job, recording and the sealed log per connection
            service.connect(ADAPTER); ready(service); service.runJob("inventory"); waitFor(() -> !service.snapshot().optBoolean("busy"), "inventory job");
            service.disconnect(); waitFor(() -> !AppLog.sealed().isEmpty(), "log sealed when the connection ends");
            String session = read(AppLog.sealed().get(0));
            for (String needle : new String[]{"service create", "conn attempt simulated …00:07", "conn ready …00:07", "job initialize start", "job inventory end ok=true", "rec start", "profile session-auto", "conn disconnect requested", "rec end"})
                if (!session.contains(needle) && !needle.equals("service create")) throw new AssertionError("REQ-029.2 session log lacks '" + needle + "': " + session);
            require(!new File(work, "app.log").exists() || read(new File(work, "app.log")).trim().isEmpty(), "REQ-029.1 the active log was sealed at the end of the connection");

            // REQ-029.2 / .3: a failed attempt, its stack trace and the reconnect delay
            service.setAutoConnect(true); service.connect(ADAPTER); ready(service); service.disconnect();
            reachable[0] = false; service.autoStart();
            waitFor(() -> { try { return read(new File(work, "app.log")).contains("reconnect"); } catch (IOException e) { return false; } }, "reconnect delay logged");
            String failed = read(AppLog.sealed().get(0)) + read(new File(work, "app.log")); // the failure ends a connection, so it sits in the sealed file
            require(failed.contains("ERROR conn attempt failed: Adapter nicht erreichbar") && failed.contains("java.io.IOException: Adapter nicht erreichbar") && failed.contains("at de.obdentic.app"), "REQ-029.3 failed attempt logged with stack trace: " + failed);
            require(failed.contains("INFO reconnect") && failed.contains("next attempt in 2 s"), "REQ-029.2 reconnect delay logged: " + failed);
            service.disconnect(); service.setAutoConnect(false);

            // REQ-029.6: each export run also copies the sealed logs unchanged, with the usual skip rule and without changing the recording report
            List<File> logs = AppLog.sealed(); require(!logs.isEmpty(), "test setup: sealed logs");
            File target = new File(context.getCacheDir(), "applog-export-" + System.nanoTime()); require(target.mkdirs(), "test setup");
            service.exportTarget = new DirTarget(target); service.exportAll();
            waitFor(() -> new File(target, logs.get(0).getName()).isFile(), "logs exported");
            for (File log : logs) require(Arrays.equals(Files.readAllBytes(log.toPath()), Files.readAllBytes(new File(target, log.getName()).toPath())), "REQ-029.6 byte-identical copy: " + log.getName());
            long stamp = new File(target, logs.get(0).getName()).lastModified(); SystemClock.sleep(1100); service.exportAll(); SystemClock.sleep(1500);
            require(new File(target, logs.get(0).getName()).lastModified() == stamp, "REQ-029.6 a second run skips present logs");
        } finally {
            service.disconnect(); service.connector = savedConnector; service.vehicles = savedStore; service.exportTarget = savedTarget; AppLog.maxBytes = 1024 * 1024; AppLog.setDirectory(original);
            service.setDemo(); waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
            service.setAutoConnect(true); // default; in demo mode it does not start an attempt
            Set<String> created = names(new File(context.getFilesDir(), "recordings")); created.removeAll(beforeRecordings);
            for (String n : created) new File(new File(context.getFilesDir(), "recordings"), n).delete();
        }
    }

    private static Set<String> names(File dir) { String[] list = dir.list(); return list == null ? new HashSet<>() : new HashSet<>(Arrays.asList(list)); }
    private static String read(File file) throws IOException { return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8); }
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
