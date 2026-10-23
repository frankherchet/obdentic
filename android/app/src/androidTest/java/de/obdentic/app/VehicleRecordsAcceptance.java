package de.obdentic.app;

import android.content.pm.ApplicationInfo;
import android.os.SystemClock;
import org.json.*;
import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Arrays;

/** REQ-013 checks: store semantics on temp files, the service's own store, and (via {@link Screen}) the header chip. */
final class VehicleRecordsAcceptance {
    interface Screen { void show(String vehicleName) throws Exception; }
    private static final String VIN_A = "WVW00000000000000", VIN_B = "WVW00000000001111", VIN_C = "XXX00000000002222", VIN_UI = "WVW00000000009999";

    static void run(ObdService service, Screen screen) throws Exception {
        File dir = service.getCacheDir();
        VehicleStore own = service.vehicles;
        own.delete(VIN_UI); // leftover of an aborted earlier run, if any
        String ownBefore = own.snapshot().toString(), ownSelection = own.snapshot().optString("selected");

        // REQ-013.4: demo initialization, a demo inventory job and a replay never touch the store.
        service.setDemo();
        waitFor(() -> service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
        service.runJob("inventory");
        waitFor(() -> !service.snapshot().optBoolean("busy"), "demo inventory job");
        JSONObject demo = new JSONObject(service.snapshot().getJSONObject("vehicle").toString());
        require(demo.optString("vin").equals(VIN_A), "demo inventory carries the synthetic VIN");
        require(own.snapshot().toString().equals(ownBefore), "REQ-013.4 demo initialize and demo inventory never create, update or select a record");
        File recordings = new File(service.getFilesDir(), "recordings");
        recordings.mkdirs();
        File replay = new File(recordings, "vehicles-replay-" + System.nanoTime() + ".jsonl");
        JSONObject header = new JSONObject().put("type", "session").put("schema", "OBDENTIC-MOBILE").put("version", 1).put("mode", "demo")
            .put("profile", "engine-drive").put("selected", new JSONArray()).put("vehicle", demo);
        Files.write(replay.toPath(), Arrays.asList(header.toString(), "{\"type\":\"session_end\",\"reason\":\"stopped\"}"), StandardCharsets.UTF_8);
        service.openRecording(replay.getName());
        waitFor(() -> service.snapshot().optString("mode").equals("replay") && !service.snapshot().optBoolean("busy"), "vehicle replay");
        require(service.snapshot().getJSONObject("vehicle").optString("vin").equals(VIN_A), "replay restored the vehicle");
        require(own.snapshot().toString().equals(ownBefore), "REQ-013.4 replay never creates, updates or selects a record");
        service.setDemo();
        waitFor(() -> service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "return to demo");
        service.deleteRecording(replay.getName());
        waitFor(() -> !replay.exists(), "replay file deleted");

        // REQ-013.1: new VIN creates a record with all fields.
        File file = new File(dir, "vehicles-test-" + System.nanoTime() + ".json");
        VehicleStore store = new VehicleStore(file);
        require(store.observe(demo, 1000).equals("created"), "REQ-013.1 live inventory with a valid VIN creates a record");
        JSONObject record = record(store, VIN_A);
        require(record.getString("name").equals("Volkswagen ·0000"), "REQ-013.8 default name is manufacturer hint + last 4 VIN characters: " + record.getString("name"));
        require(record.getString("manufacturer_hint").equals("Volkswagen"), "REQ-013.1 manufacturer hint stored");
        require(record.getJSONArray("responders").toString().equals(demo.getJSONArray("responders").toString()) && record.getJSONArray("responders").length() > 0, "REQ-013.1 responders stored");
        require(record.getJSONArray("signals").length() == demo.getJSONArray("signals").length() && record.getJSONArray("signals").length() > 0
            && record.getJSONArray("signals").getJSONObject(0).has("semantic") && record.getJSONArray("signals").getJSONObject(0).has("status"), "REQ-013.1 signal support stored");
        require(record.getBoolean("engine_target_validated") == demo.getBoolean("engine_target_validated"), "REQ-013.1 engine target validation stored");
        require(record.getLong("first_connected") == 1000 && record.getLong("last_connected") == 1000, "REQ-013.1 first and last connection timestamps");
        require(store.snapshot().getString("selected").equals(VIN_A), "REQ-013.1 new record becomes the selected vehicle");

        // REQ-013.2: known VIN replaces observations, keeps name and first connection, keeps no history.
        require(store.rename(VIN_A, "Mein Golf") == null, "rename for update test");
        JSONObject again = inventory(demo, VIN_A).put("responders", new JSONArray().put("7E0").put("7E1")).put("engine_target_validated", !demo.getBoolean("engine_target_validated"))
            .put("signals", new JSONArray().put(new JSONObject().put("semantic", "engine.rpm").put("status", "unsupported")));
        require(store.observe(again, 2000).equals("updated"), "REQ-013.2 known VIN is updated");
        record = record(store, VIN_A);
        require(record.getString("name").equals("Mein Golf") && record.getLong("first_connected") == 1000, "REQ-013.2 name and first connection are kept");
        require(record.getLong("last_connected") == 2000 && record.getJSONArray("responders").length() == 2 && record.getJSONArray("signals").length() == 1
            && record.getBoolean("engine_target_validated") != demo.getBoolean("engine_target_validated"), "REQ-013.2 observations replaced");
        require(record.length() == 8 && store.snapshot().getJSONArray("vehicles").length() == 1, "REQ-013.2 no history fields or duplicate records");

        // REQ-013.3: missing or invalid VIN changes nothing.
        String stable = store.snapshot().toString();
        JSONObject noVin = inventory(demo, ""), invalid = inventory(demo, "WVW0000000000XXXX"), shortVin = inventory(demo, "WVW000");
        invalid.getJSONObject("vin_identity").put("status", "invalid_vin");
        noVin.remove("vin");
        for (JSONObject bad : new JSONObject[]{noVin, inventory(demo, ""), invalid, shortVin})
            require(store.observe(bad, 3000).equals("unreadable_vin"), "REQ-013.3 unreadable or invalid VIN is reported");
        require(store.snapshot().toString().equals(stable), "REQ-013.3 unreadable VIN creates, updates and selects nothing");
        File untouched = new File(dir, "vehicles-empty-" + System.nanoTime() + ".json");
        require(new VehicleStore(untouched).observe(noVin, 1).equals("unreadable_vin") && !untouched.exists(), "REQ-013.3 unreadable VIN writes no file");

        // REQ-013.7: another VIN selects its record (created if needed) and leaves the old record unchanged.
        String oldRecord = record(store, VIN_A).toString();
        require(store.observe(inventory(demo, VIN_B), 4000).equals("switched"), "REQ-013.7 different VIN is reported as switched");
        require(store.snapshot().getString("selected").equals(VIN_B) && store.name(VIN_B).equals("Volkswagen ·1111"), "REQ-013.7 read VIN's record is selected");
        require(record(store, VIN_A).toString().equals(oldRecord), "REQ-013.7 previously selected record is unchanged");

        // REQ-013.6: selection sends no traffic (store has no transport) and survives a process restart.
        require(store.select(VIN_A) == null && store.select("WVW00000000007777") != null, "REQ-013.6 select known VIN, reject unknown");
        require(new VehicleStore(file).snapshot().getString("selected").equals(VIN_A), "REQ-013.6 selection restored by a new store instance on the same file");

        // REQ-013.8: names.
        JSONObject plain = inventory(demo, VIN_C).put("vin_identity", new JSONObject().put("status", "unidentified"));
        require(store.observe(plain, 5000).equals("switched") && store.name(VIN_C).equals("Fahrzeug ·2222"), "REQ-013.8 default name without hint is Fahrzeug + last 4: " + store.name(VIN_C));
        require(store.rename(VIN_C, "  Zweitwagen  ") == null && store.name(VIN_C).equals("Zweitwagen"), "REQ-013.8 rename trims");
        String forty = new String(new char[40]).replace((char) 0, 'x');
        for (String bad : new String[]{"", "   ", forty + "y"})
            require(store.rename(VIN_C, bad) != null && store.name(VIN_C).equals("Zweitwagen"), "REQ-013.8 rename rejects '" + bad.length() + "' chars and keeps the old name");
        require(store.rename(VIN_C, forty) == null && store.name(VIN_C).equals(forty), "REQ-013.8 40 characters are accepted");

        // REQ-013.9: delete clears the selection, leaves recordings alone.
        int recordingCount = recordings.list().length;
        require(store.snapshot().getString("selected").equals(VIN_C), "selected before delete");
        require(store.delete(VIN_C) == null, "REQ-013.9 delete");
        require(store.snapshot().getString("selected").isEmpty() && store.name(VIN_C).isEmpty() && store.snapshot().getJSONArray("vehicles").length() == 2, "REQ-013.9 deleting the selected record clears the selection");
        require(new VehicleStore(file).snapshot().getJSONArray("vehicles").length() == 2 && recordings.list().length == recordingCount, "REQ-013.9 deletion is permanent and recordings are unchanged");

        // REQ-013.11: unreadable store is never overwritten implicitly.
        File broken = new File(dir, "vehicles-broken-" + System.nanoTime() + ".json");
        byte[] garbage = "{not json".getBytes(StandardCharsets.UTF_8);
        Files.write(broken.toPath(), garbage);
        VehicleStore bad = new VehicleStore(broken);
        require(bad.snapshot().getBoolean("unreadable"), "REQ-013.11 garbage file is reported unreadable");
        require(bad.observe(demo, 1).equals("store_unreadable"), "REQ-013.11 live inventory reports the unreadable store");
        require(bad.select(VIN_A) != null && bad.rename(VIN_A, "x") != null && bad.delete(VIN_A) != null, "REQ-013.11 every mutation refuses");
        require(Arrays.equals(Files.readAllBytes(broken.toPath()), garbage), "REQ-013.11 unreadable file is byte-identical, never overwritten implicitly");
        Files.write(broken.toPath(), "{\"version\":2,\"selected\":\"\",\"vehicles\":[]}".getBytes(StandardCharsets.UTF_8));
        require(new VehicleStore(broken).snapshot().getBoolean("unreadable"), "REQ-013.11 unknown version is unreadable");
        require(bad.reset() == null && !bad.snapshot().getBoolean("unreadable") && bad.observe(demo, 1).equals("created"), "REQ-013.11 explicit reset replaces the file and the store works again");
        require(!new VehicleStore(broken).snapshot().getBoolean("unreadable"), "REQ-013.11 reset file is readable");

        // REQ-013.12: write failures keep memory and file at the last persisted state.
        File blocker = new File(dir, "vehicles-blocker-" + System.nanoTime());
        Files.write(blocker.toPath(), new byte[]{1});
        VehicleStore cannot = new VehicleStore(new File(blocker, "vehicles.json"));
        String emptyState = cannot.snapshot().toString();
        require(cannot.observe(demo, 1).equals("write_failed") && cannot.snapshot().toString().equals(emptyState), "REQ-013.12 write failure rolls the in-memory state back");
        File locked = new File(dir, "vehicles-locked-" + System.nanoTime());
        require(locked.mkdir(), "locked dir");
        File lockedFile = new File(locked, "vehicles.json");
        VehicleStore readOnly = new VehicleStore(lockedFile);
        require(readOnly.observe(demo, 1).equals("created"), "store in a writable dir");
        byte[] persisted = Files.readAllBytes(lockedFile.toPath());
        String persistedState = readOnly.snapshot().toString();
        lockedFile.setWritable(false);
        locked.setWritable(false);
        try {
            require(readOnly.observe(inventory(demo, VIN_B), 2).equals("write_failed"), "REQ-013.12 observe reports the write failure");
            require(readOnly.rename(VIN_A, "Neu").equals(VehicleStore.WRITE_FAILED) && readOnly.delete(VIN_A).equals(VehicleStore.WRITE_FAILED) && readOnly.select(VIN_A).equals(VehicleStore.WRITE_FAILED), "REQ-013.12 rename, delete and select report the write failure");
            require(readOnly.snapshot().toString().equals(persistedState), "REQ-013.12 in-memory state equals the last persisted state");
            require(Arrays.equals(Files.readAllBytes(lockedFile.toPath()), persisted), "REQ-013.12 previous file is intact");
        } finally { locked.setWritable(true); lockedFile.setWritable(true); }

        // REQ-013.10: private storage excluded from backup; nothing exports records.
        require((service.getApplicationInfo().flags & ApplicationInfo.FLAG_ALLOW_BACKUP) == 0, "REQ-013.10 backup is disabled for the app");

        // REQ-013.5 / .1 / .10 on the service's own store, UI header chip on every tab.
        JSONObject synthetic = inventory(demo, VIN_UI);
        String outcome = own.observe(synthetic, System.currentTimeMillis());
        require(outcome.equals("created") || outcome.equals("switched"), "synthetic record stored: " + outcome);
        require(new File(service.getFilesDir(), "vehicles.json").isFile(), "REQ-013.10 service store lives in the app-private files directory");
        service.renameVehicle(VIN_UI, "  Testwagen ");
        waitFor(() -> service.snapshot().optString("message").equals("Fahrzeug umbenannt: Testwagen"), "service rename");
        screen.show("Testwagen");
        service.deleteVehicle(VIN_UI);
        waitFor(() -> service.snapshot().optString("message").equals("Fahrzeug gelöscht: Testwagen"), "service delete");
        if (!ownSelection.isEmpty()) {
            service.selectVehicle(ownSelection);
            waitFor(() -> service.snapshot().optString("message").startsWith("Fahrzeug gewählt"), "restore selection");
        }
        require(own.snapshot().toString().equals(ownBefore), "synthetic record removed, service store restored");

        for (File f : new File[]{file, broken, new File(blocker, "x"), untouched}) { f.delete(); new File(f.getPath() + ".new").delete(); new File(f.getPath() + ".bak").delete(); }
        blocker.delete();
        lockedFile.delete(); new File(lockedFile.getPath() + ".new").delete(); new File(lockedFile.getPath() + ".bak").delete(); locked.delete();
    }

    private static JSONObject inventory(JSONObject base, String vin) throws JSONException {
        return new JSONObject(base.toString()).put("vin", vin);
    }
    private static JSONObject record(VehicleStore store, String vin) throws JSONException {
        JSONArray rows = store.snapshot().getJSONArray("vehicles");
        for (int i = 0; i < rows.length(); i++) if (vin.equals(rows.getJSONObject(i).getString("vin"))) return rows.getJSONObject(i);
        throw new AssertionError("no record " + vin);
    }
    private interface Check { boolean ok() throws Exception; }
    private static void waitFor(Check check, String name) throws Exception {
        long until = SystemClock.elapsedRealtime() + 45000;
        while (SystemClock.elapsedRealtime() < until) { if (check.ok()) return; SystemClock.sleep(50); }
        throw new AssertionError(name + " timed out");
    }
    private static void require(boolean value, String message) { if (!value) throw new AssertionError(message); }
}
