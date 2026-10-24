package de.obdentic.app;

import android.content.ClipData;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.SystemClock;
import org.json.*;
import java.io.*;
import java.nio.file.Files;
import java.util.*;

/**
 * REQ-018 through the real service and exporter. The system folder picker cannot be driven from
 * instrumentation, so the copy logic runs against a plain directory behind the same
 * {@link RecordingExporter.Target} seam the picker's tree URI uses (REQ-018.1 is checked as
 * persistence of the chosen URI, not as the picker UI).
 */
final class RecordingExportAcceptance {
    private static final List<String> ALLOWED = Arrays.asList("android.permission.BLUETOOTH", "android.permission.BLUETOOTH_ADMIN",
        "android.permission.ACCESS_FINE_LOCATION", "android.permission.BLUETOOTH_SCAN", "android.permission.BLUETOOTH_CONNECT",
        "android.permission.POST_NOTIFICATIONS", "android.permission.FOREGROUND_SERVICE", "android.permission.FOREGROUND_SERVICE_CONNECTED_DEVICE",
        "android.permission.WAKE_LOCK");
    private static final List<String> ALWAYS = Arrays.asList("android.permission.BLUETOOTH_SCAN", "android.permission.BLUETOOTH_CONNECT",
        "android.permission.POST_NOTIFICATIONS", "android.permission.FOREGROUND_SERVICE", "android.permission.FOREGROUND_SERVICE_CONNECTED_DEVICE",
        "android.permission.WAKE_LOCK");

    static void run(ObdService service, Context context) throws Exception {
        File recordings = new File(context.getFilesDir(), "recordings");
        SharedPreferences prefs = context.getSharedPreferences("export", Context.MODE_PRIVATE);
        Map<String, ?> previous = new HashMap<>(prefs.getAll());
        List<File> dirs = new ArrayList<>(); List<String> created = new ArrayList<>();
        try {
            prefs.edit().clear().commit();
            // REQ-018.4 default and persistence, REQ-018.1 persisted choice
            require(!RecordingExporter.auto(context), "REQ-018.4 automatic export is off by default");
            require(RecordingExporter.folder(context).isEmpty(), "REQ-018.1 no folder before the first choice");
            String tree = "content://com.android.externalstorage.documents/tree/primary%3ADownload%2FOBDentic";
            RecordingExporter.setFolder(context, tree); RecordingExporter.setAuto(context, true);
            File prefsFile = new File(context.getFilesDir().getParentFile(), "shared_prefs/export.xml");
            String xml = new String(Files.readAllBytes(prefsFile.toPath()), "UTF-8");
            require(xml.contains(tree) && xml.contains("name=\"auto\" value=\"true\""), "REQ-018.1/.4 folder and flag are written to app-private storage: " + xml);
            require(tree.equals(RecordingExporter.folder(context)) && RecordingExporter.auto(context), "REQ-018.1/.4 values read back");
            RecordingExporter.setAuto(context, false); RecordingExporter.setFolder(context, "");
            require(!RecordingExporter.auto(context), "REQ-018.4 flag can be turned off again");

            // REQ-018.3: an active recording exists while exporting
            service.startRecording("engine-drive");
            waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "export test recording starts");
            String active = service.snapshot().getString("recording"); created.add(active);
            List<String> names = RecordingExporter.completed(service.snapshot().getJSONArray("records"), active);
            int n = names.size();
            require(n >= 3 && !names.contains(active), "REQ-018.3 completed list excludes the active recording (" + n + " completed)");
            Map<String, byte[]> originals = new HashMap<>(); Map<String, Long> times = new HashMap<>();
            for (String name : names) { File f = new File(recordings, name); originals.put(name, Files.readAllBytes(f.toPath())); times.put(name, f.lastModified()); }

            // REQ-018.2/.3/.7: first export copies all completed files, never the active one
            File dir = tempDir(context, dirs); service.exportTarget = new DirTarget(dir);
            exportAndWait(service, "Export: " + n + " kopiert, 0 übersprungen, 0 fehlgeschlagen");
            require(new HashSet<>(Arrays.asList(dir.list())).equals(new HashSet<>(names)), "REQ-018.3 only completed recordings were copied, not " + active);
            for (String name : names) require(Arrays.equals(originals.get(name), Files.readAllBytes(new File(dir, name).toPath())), "REQ-018.7 byte-identical copy of " + name);
            verifyUnchanged(recordings, originals, times);
            // REQ-018.2: second run skips what is present with the same name and size
            exportAndWait(service, "Export: 0 kopiert, " + n + " übersprungen, 0 fehlgeschlagen");
            // REQ-018.2 + .7: one missing file is copied; same name with another size is a failure and is not overwritten
            File missing = new File(dir, names.get(0)), different = new File(dir, names.get(1));
            require(missing.delete(), "remove one target file");
            Files.write(different.toPath(), new byte[]{1, 2, 3});
            exportAndWait(service, "Export: 1 kopiert, " + (n - 2) + " übersprungen, 1 fehlgeschlagen");
            require(different.length() == 3, "REQ-018.7 a different file with the same name is never overwritten");
            require(Arrays.equals(originals.get(names.get(0)), Files.readAllBytes(missing.toPath())), "REQ-018.2 deleted target file copied again");
            verifyUnchanged(recordings, originals, times);

            // REQ-018.6: a copy failing midway leaves no partial target file
            File faulty = tempDir(context, dirs); service.exportTarget = new FaultTarget(faulty, 100);
            for (String name : names) require(originals.get(name).length > 100, "fixture is longer than the failure point: " + name);
            exportAndWait(service, "Export: 0 kopiert, 0 übersprungen, " + n + " fehlgeschlagen");
            require(faulty.list().length == 0, "REQ-018.6 no partial target left: " + Arrays.toString(faulty.list()));
            verifyUnchanged(recordings, originals, times);

            // REQ-018.6: missing folder -> message, internals untouched
            service.exportTarget = new DirTarget(new File(dir, "gone"));
            exportAndWait(service, RecordingExporter.UNAVAILABLE);
            verifyUnchanged(recordings, originals, times);
            // REQ-018.6: stored folder without persisted permission (revoked) -> message and stored choice cleared
            service.exportTarget = null; RecordingExporter.setFolder(context, tree);
            exportAndWait(service, RecordingExporter.UNAVAILABLE);
            require(RecordingExporter.folder(context).isEmpty(), "REQ-018.6 unusable folder is forgotten so the user chooses again");
            verifyUnchanged(recordings, originals, times);
            exportAndWait(service, "Kein Export-Ordner gewählt");

            // REQ-018.5: share intent
            Intent share = RecordingExporter.share(names);
            require(Intent.ACTION_SEND_MULTIPLE.equals(share.getAction()) && RecordingExporter.MIME.equals(share.getType()), "REQ-018.5 SEND_MULTIPLE with the recording MIME type");
            require((share.getFlags() & Intent.FLAG_GRANT_READ_URI_PERMISSION) != 0 && (share.getFlags() & Intent.FLAG_GRANT_WRITE_URI_PERMISSION) == 0, "REQ-018.5 read grant only");
            ArrayList<Uri> uris = share.getParcelableArrayListExtra(Intent.EXTRA_STREAM);
            ClipData clip = share.getClipData();
            require(uris != null && uris.size() == n && clip != null && clip.getItemCount() == n, "REQ-018.5 every recording is a stream and a clip item");
            for (int i = 0; i < n; i++) {
                Uri uri = uris.get(i);
                require(uri.equals(clip.getItemAt(i).getUri()) && "content".equals(uri.getScheme()) && "de.obdentic.app.files".equals(uri.getAuthority())
                    && uri.getPath().equals("/recordings/" + names.get(i)), "REQ-018.5 provider URI for " + names.get(i) + ": " + uri);
                try (InputStream in = context.getContentResolver().openInputStream(uri)) { require(in != null && in.read() == '{', "REQ-018.5 provider serves " + names.get(i)); }
            }
            require(!uris.contains(RecordingExporter.uri(active)), "REQ-018.5 the active recording is not shared");
            try { context.getContentResolver().openFileDescriptor(uris.get(0), "w").close(); require(false, "REQ-018.5 provider must stay read-only"); }
            catch (FileNotFoundException expected) { /* read-only */ }

            // REQ-018.4: automatic export after stop and after disconnect, only while enabled, never during recording
            service.stopRecording();
            waitFor(() -> service.snapshot().optString("recording").isEmpty(), "export test recording stops");
            File auto = tempDir(context, dirs); service.exportTarget = new DirTarget(auto);
            RecordingExporter.setAuto(context, true);
            service.startRecording("engine-drive");
            waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "auto-export recording starts");
            String stopped = service.snapshot().getString("recording"); created.add(stopped);
            SystemClock.sleep(1200);
            require(auto.list().length == 0, "REQ-018.3/.4 nothing is exported while the recording runs");
            service.stopRecording();
            waitFor(() -> new File(auto, stopped).isFile() && service.snapshot().optString("message").startsWith("Export: 1 kopiert"), "REQ-018.4 automatic export after stop");
            require(Arrays.equals(Files.readAllBytes(new File(recordings, stopped).toPath()), Files.readAllBytes(new File(auto, stopped).toPath())), "REQ-018.7 automatic copy is byte-identical");
            RecordingExporter.setAuto(context, false);
            service.startRecording("engine-drive");
            waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "recording without auto-export starts");
            String quiet = service.snapshot().getString("recording"); created.add(quiet);
            service.stopRecording();
            waitFor(() -> service.snapshot().optString("recording").isEmpty(), "recording without auto-export stops");
            SystemClock.sleep(800);
            require(!new File(auto, quiet).exists() && auto.list().length == 1, "REQ-018.4 nothing is exported while the flag is off");
            RecordingExporter.setAuto(context, true);
            service.startRecording("engine-drive");
            waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "disconnect-export recording starts");
            String cut = service.snapshot().getString("recording"); created.add(cut);
            service.disconnect();
            waitFor(() -> new File(auto, cut).isFile(), "REQ-018.4 automatic export after disconnect");
            waitFor(() -> service.snapshot().optString("message").startsWith("Export: 1 kopiert"), "disconnect export finished");
            require(Arrays.equals(Files.readAllBytes(new File(recordings, cut).toPath()), Files.readAllBytes(new File(auto, cut).toPath())), "REQ-018.7 copy after disconnect is byte-identical");
            service.exportTarget = null;
            service.setDemo();
            waitFor(() -> service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "return to demo after export test");
            service.setSignals(new JSONArray()); SystemClock.sleep(800); // the engine-drive profile selected signals; stop polling so later steps see no background traffic

            // REQ-018.8: no new permissions
            String[] requested = context.getPackageManager().getPackageInfo(context.getPackageName(), PackageManager.GET_PERMISSIONS).requestedPermissions;
            List<String> list = Arrays.asList(requested);
            require(ALLOWED.containsAll(list) && list.containsAll(ALWAYS), "REQ-018.8 only the existing permissions: " + list);
            for (String forbidden : new String[]{"android.permission.INTERNET", "android.permission.READ_EXTERNAL_STORAGE", "android.permission.WRITE_EXTERNAL_STORAGE", "android.permission.MANAGE_EXTERNAL_STORAGE"})
                require(!list.contains(forbidden), "REQ-018.8 no " + forbidden);
        } finally {
            service.exportTarget = null;
            for (String name : created) { File f = new File(recordings, name); if (f.isFile()) f.delete(); }
            for (File dir : dirs) deleteTree(dir);
            SharedPreferences.Editor editor = prefs.edit().clear();
            for (Map.Entry<String, ?> entry : previous.entrySet()) {
                if (entry.getValue() instanceof String) editor.putString(entry.getKey(), (String) entry.getValue());
                else if (entry.getValue() instanceof Boolean) editor.putBoolean(entry.getKey(), (Boolean) entry.getValue());
            }
            editor.commit();
        }
    }

    private static void exportAndWait(ObdService service, String expectedPrefix) throws Exception {
        String reset = "missing-" + System.nanoTime() + ".jsonl"; // a known message that cannot be the export result
        service.deleteRecording(reset);
        waitFor(() -> "Aufnahme nicht gefunden".equals(service.snapshot().optString("message")), "message reset");
        service.exportAll();
        waitFor(() -> !"Aufnahme nicht gefunden".equals(service.snapshot().optString("message")), "export result");
        String message = service.snapshot().optString("message");
        require(message.startsWith(expectedPrefix), "export message '" + message + "' should start with '" + expectedPrefix + "'");
    }
    private static void verifyUnchanged(File recordings, Map<String, byte[]> originals, Map<String, Long> times) throws IOException {
        for (String name : originals.keySet()) {
            File f = new File(recordings, name);
            require(f.isFile() && f.lastModified() == times.get(name) && Arrays.equals(originals.get(name), Files.readAllBytes(f.toPath())), "REQ-018.7 export never modifies or removes " + name);
        }
    }
    private static File tempDir(Context context, List<File> dirs) throws IOException {
        File dir = new File(context.getCacheDir(), "export-test-" + System.nanoTime());
        if (!dir.mkdirs()) throw new IOException("temp dir"); dirs.add(dir); return dir;
    }
    private static void deleteTree(File file) {
        File[] children = file.listFiles(); if (children != null) for (File child : children) deleteTree(child);
        file.delete();
    }

    private static class DirTarget implements RecordingExporter.Target {
        final File dir; DirTarget(File dir) { this.dir = dir; }
        public Map<String, Long> list() throws IOException {
            File[] files = dir.listFiles(); if (files == null) throw new RecordingExporter.Unavailable("missing");
            Map<String, Long> names = new HashMap<>(); for (File f : files) names.put(f.getName(), f.length()); return names;
        }
        public OutputStream create(String name) throws IOException { return new FileOutputStream(new File(dir, name)); }
        public void delete(String name) { new File(dir, name).delete(); }
    }
    /** Fails like a full disk once {@code limit} bytes were written to one file. */
    private static final class FaultTarget extends DirTarget {
        final int limit; FaultTarget(File dir, int limit) { super(dir); this.limit = limit; }
        @Override public OutputStream create(String name) throws IOException {
            return new FilterOutputStream(super.create(name)) {
                int written;
                @Override public void write(int b) throws IOException { write(new byte[]{(byte) b}, 0, 1); }
                @Override public void write(byte[] b, int off, int len) throws IOException {
                    if (written + len > limit) { out.write(b, off, Math.max(0, limit - written)); throw new IOException("No space left on device"); }
                    out.write(b, off, len); written += len;
                }
            };
        }
    }

    private interface Check { boolean ok() throws Exception; }
    private static void waitFor(Check check, String name) throws Exception {
        long until = SystemClock.elapsedRealtime() + 45000;
        while (SystemClock.elapsedRealtime() < until) { if (check.ok()) return; SystemClock.sleep(50); }
        throw new AssertionError(name + " timed out");
    }
    private static void require(boolean value, String message) { if (!value) throw new AssertionError(message); }
}
