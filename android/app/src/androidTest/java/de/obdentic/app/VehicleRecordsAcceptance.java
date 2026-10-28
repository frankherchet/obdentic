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
    interface Screen { void show(String vehicleName, String listText) throws Exception; }
    private static final String VIN_A = "WVW00000000000000", VIN_B = "WVW00000000001111", VIN_C = "XXX00000000002222", VIN_UI = "WVW00000000009999", VIN_SEL_A = "WVW00000000008888", VIN_SEL_B = "WVW00000000007777";

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
        // REQ-013.4 / REQ-014.1: a signal selection change in a demo session never touches a record.
        JSONArray demoSelection = new JSONArray(service.snapshot().getJSONArray("selected").toString());
        service.setSignals(new JSONArray().put("engine.rpm"));
        require(service.snapshot().getJSONArray("selected").toString().equals("[\"engine.rpm\"]") && own.snapshot().toString().equals(ownBefore), "REQ-013.4 REQ-014.1 demo signal selection changes no record");
        service.setSignals(demoSelection);
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

        // REQ-014.1/.4: a stored selection persists per record; a new record has none.
        JSONArray picked = new JSONArray().put("engine.rpm").put("vehicle.speed");
        require(store.selection(VIN_A) == null && store.selection(VIN_B) == null && !record(store, VIN_A).has("selection"), "REQ-014.4 a record without a stored selection has none");
        require(store.setSelection(VIN_A, picked) == null && store.selection(VIN_A).toString().equals(picked.toString()) && store.selection(VIN_B) == null, "REQ-014.1 selection is stored for that record only");
        require(new VehicleStore(file).selection(VIN_A).toString().equals(picked.toString()), "REQ-014.1 stored selection survives a restart");
        require(store.setSelection("WVW00000000007777", picked) != null, "REQ-014.1 unknown record is refused");
        require(store.observe(again, 6000).equals("updated") && store.selection(VIN_A).toString().equals(picked.toString()), "REQ-014.1 a later inventory keeps the stored selection");
        require(store.setSelection(VIN_A, new JSONArray()) == null && store.selection(VIN_A).length() == 0, "REQ-014.1 an emptied selection is stored as empty, not as none");
        store.setSelection(VIN_A, picked);

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
        require(bad.setSelection(VIN_A, new JSONArray().put("engine.rpm")) != null && bad.selection(VIN_A) == null, "REQ-014.1 an unreadable store refuses selection writes");
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
            require(readOnly.setSelection(VIN_A, picked).equals(VehicleStore.WRITE_FAILED) && readOnly.selection(VIN_A) == null, "REQ-014.1 selection write failure is reported and not applied to the stored record");
            require(readOnly.snapshot().toString().equals(persistedState), "REQ-013.12 in-memory state equals the last persisted state");
            require(Arrays.equals(Files.readAllBytes(lockedFile.toPath()), persisted), "REQ-013.12 previous file is intact");
        } finally { locked.setWritable(true); lockedFile.setWritable(true); }

        // REQ-015: odometer value, failure and decrease on the record.
        File odoFile = new File(dir, "vehicles-odometer-" + System.nanoTime() + ".json");
        VehicleStore odo = new VehicleStore(odoFile);
        require(odo.observe(demo, 1000).equals("created"), "odometer test record");
        require(!record(odo, VIN_A).has("odometer") && !record(odo, VIN_A).has("odometer_error") && !record(odo, VIN_A).has("odometer_decrease"), "REQ-015.2 a new record has no odometer");
        require(odo.odometer(VIN_A, 123456.7, 5000) == null, "REQ-015.1 store odometer");
        JSONObject stored = record(odo, VIN_A).getJSONObject("odometer");
        require(stored.getDouble("km") == 123456.7 && stored.getLong("read_at") == 5000 && stored.getString("source").equals("OBD PID A6"), "REQ-015.1 value, read timestamp and source are stored: " + stored);
        require(new VehicleStore(odoFile).snapshot().getJSONArray("vehicles").getJSONObject(0).getJSONObject("odometer").toString().equals(stored.toString()), "REQ-015.1 odometer survives a restart");
        require(odo.observe(again, 5500).equals("updated") && record(odo, VIN_A).getJSONObject("odometer").toString().equals(stored.toString()), "REQ-015.1 a later inventory keeps the stored odometer");
        require(odo.odometerFailure(VIN_A, "NO DATA", 6000) == null, "REQ-015.3 store failure");
        require(record(odo, VIN_A).getJSONObject("odometer").toString().equals(stored.toString()), "REQ-015.3 failure keeps stored value and date");
        JSONObject failure = record(odo, VIN_A).getJSONObject("odometer_error");
        require(failure.getString("error").equals("NO DATA") && failure.getLong("at") == 6000, "REQ-015.3 failure and its date are recorded: " + failure);
        require(odo.odometer(VIN_A, 100000.0, 7000) == null, "REQ-015.4 lower value stored");
        JSONObject lower = record(odo, VIN_A);
        require(lower.getJSONObject("odometer").getDouble("km") == 100000.0 && lower.getJSONObject("odometer").getLong("read_at") == 7000, "REQ-015.4 lower value replaces the stored one");
        require(lower.getJSONObject("odometer_decrease").getDouble("previous_km") == 123456.7 && lower.getJSONObject("odometer_decrease").getLong("previous_at") == 5000, "REQ-015.4 decrease names the previous value and date");
        require(!lower.has("odometer_error"), "REQ-015.1 a successful read clears the failure");
        require(odo.odometer(VIN_A, 100000.0, 8000) == null && !record(odo, VIN_A).has("odometer_decrease"), "REQ-015.4 an equal value is not a decrease");
        require(odo.odometer(VIN_A, 100000.5, 9000) == null && record(odo, VIN_A).getJSONObject("odometer").getDouble("km") == 100000.5 && !record(odo, VIN_A).has("odometer_decrease"), "REQ-015.4 a higher value clears the decrease note");
        require(odo.odometer(VIN_B, 1, 1) != null && odo.odometerFailure(VIN_B, "x", 1) != null, "REQ-015 unknown vehicle is refused");
        File garbageFile = new File(dir, "vehicles-odometer-bad-" + System.nanoTime() + ".json");
        Files.write(garbageFile.toPath(), garbage);
        VehicleStore garbled = new VehicleStore(garbageFile);
        require(garbled.odometer(VIN_A, 1, 1) != null && garbled.odometerFailure(VIN_A, "x", 1) != null && Arrays.equals(Files.readAllBytes(garbageFile.toPath()), garbage), "REQ-015 an unreadable store refuses odometer writes and stays untouched");
        garbageFile.delete(); new File(odoFile.getPath() + ".new").delete(); new File(odoFile.getPath() + ".bak").delete(); odoFile.delete();

        // REQ-013.10: private storage excluded from backup; nothing exports records.
        require((service.getApplicationInfo().flags & ApplicationInfo.FLAG_ALLOW_BACKUP) == 0, "REQ-013.10 backup is disabled for the app");

        // REQ-013.5 / .1 / .10 on the service's own store, UI header chip on every tab.
        JSONObject synthetic = inventory(demo, VIN_UI);
        String outcome = own.observe(synthetic, System.currentTimeMillis());
        require(outcome.equals("created") || outcome.equals("switched"), "synthetic record stored: " + outcome);
        require(new File(service.getFilesDir(), "vehicles.json").isFile(), "REQ-013.10 service store lives in the app-private files directory");
        service.renameVehicle(VIN_UI, "  Testwagen ");
        waitFor(() -> service.snapshot().optString("message").equals("Fahrzeug umbenannt: Testwagen"), "service rename");
        screen.show("Testwagen", "WVW…9999 · km-Stand nicht verfügbar"); // REQ-015.2: no value stored
        long readAt = System.currentTimeMillis();
        String day = new java.text.SimpleDateFormat("dd.MM.yyyy", java.util.Locale.GERMANY).format(new java.util.Date(readAt));
        require(own.odometer(VIN_UI, 123456.7, readAt) == null, "odometer stored on the service store");
        screen.show("Testwagen", "WVW…9999 · 123.456,7 km · " + new java.text.SimpleDateFormat("dd.MM.yyyy", java.util.Locale.GERMANY).format(new java.util.Date(readAt))); // REQ-015.2: value and date
        String details = MainActivity.vehicleDetails(record(own, VIN_UI));
        require(details.contains("km-Stand: 123.456,7 km (Quelle: OBD PID A6, gelesen " + day + ")"), "REQ-015 details show the PID A6 odometer with date and source: " + details);
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

    /** REQ-014 on the service in live mode without a connection (idle): selection restore, stored support labels, discovery untouched. */
    static void selection(ObdService service) throws Exception {
        require(service.snapshot().optString("mode").equals("live") && !service.snapshot().optString("phase").equals("ready"), "live mode without a session expected");
        VehicleStore own = service.vehicles;
        for (String vin : new String[]{VIN_SEL_A, VIN_SEL_B}) own.delete(vin); // leftovers of an aborted run
        String before = own.snapshot().toString(), previous = own.snapshot().optString("selected");
        JSONArray selectedBefore = new JSONArray(service.snapshot().getJSONArray("selected").toString());
        String signalsBefore = service.snapshot().getJSONArray("signals").toString();
        int traffic = service.snapshot().getJSONArray("debug").length();
        long seen = System.currentTimeMillis();
        for (String vin : new String[]{VIN_SEL_B, VIN_SEL_A}) { // A last, so A is selected
            JSONObject inventory = new JSONObject().put("vin", vin).put("vin_identity", new JSONObject().put("status", "unidentified")).put("responders", new JSONArray().put("7E0"))
                .put("signals", new JSONArray().put(new JSONObject().put("semantic", "engine.rpm").put("status", "supported")).put(new JSONObject().put("semantic", "vehicle.speed").put("status", "unsupported")))
                .put("engine_target_validated", true);
            require(!own.observe(inventory, seen).equals("write_failed"), "synthetic record " + vin);
        }
        try {
            JSONArray rpmSpeed = new JSONArray().put("engine.rpm").put("vehicle.speed"), coolant = new JSONArray().put("engine.coolant_temperature");
            selectVehicle(service, VIN_SEL_A);
            service.setSignals(rpmSpeed);
            require(own.selection(VIN_SEL_A).toString().equals(rpmSpeed.toString()), "REQ-014.1 a changed selection is stored for the selected record");
            selectVehicle(service, VIN_SEL_B);
            require(own.selection(VIN_SEL_B) == null && service.snapshot().getJSONArray("selected").toString().equals(rpmSpeed.toString()), "REQ-014.1 a record without a stored selection keeps the current selection");
            service.setSignals(coolant);
            require(own.selection(VIN_SEL_A).toString().equals(rpmSpeed.toString()) && own.selection(VIN_SEL_B).toString().equals(coolant.toString()), "REQ-014.1 each record keeps its own selection");
            selectVehicle(service, VIN_SEL_A);
            require(service.snapshot().getJSONArray("selected").toString().equals(rpmSpeed.toString()), "REQ-014.1 selecting a record restores its selection");
            selectVehicle(service, VIN_SEL_B);
            require(service.snapshot().getJSONArray("selected").toString().equals(coolant.toString()), "REQ-014.1 selecting the other record restores its selection");
            service.setSignals(new JSONArray().put("no.such.signal"));
            require(own.selection(VIN_SEL_B).length() == 0, "REQ-014.1 unknown semantics are filtered before storing");
            require(service.snapshot().getJSONArray("debug").length() == traffic, "REQ-014.1 selecting and restoring sends no adapter traffic");
            require(service.snapshot().getJSONArray("signals").toString().equals(signalsBefore), "REQ-014.3 stored support is never copied into the current session's signal status");
            require(new VehicleStore(new File(service.getFilesDir(), "vehicles.json")).selection(VIN_SEL_A).toString().equals(rpmSpeed.toString()), "REQ-014.1 the service's selection is on disk");

            // REQ-014.2: labels.
            String date = new java.text.SimpleDateFormat("dd.MM.yyyy", java.util.Locale.GERMANY).format(new java.util.Date(seen));
            require(MainActivity.supportLabel("", "supported", seen).equals(" · unterstützt (zuletzt gesehen " + date + ")"), "REQ-014.2 stored support is marked with the observation date: " + MainActivity.supportLabel("", "supported", seen));
            require(MainActivity.supportLabel("", "unsupported", seen).equals(" · nicht unterstützt (zuletzt gesehen " + date + ")"), "REQ-014.2 stored non-support is marked with the observation date");
            require(MainActivity.supportLabel("", "", seen).equals(" · ungeprüft") && MainActivity.supportLabel("", "unknown", seen).equals(" · ungeprüft"), "REQ-014.2 unknown signals stay unchecked");
            require(MainActivity.supportLabel("supported", "unsupported", seen).equals("") && MainActivity.supportLabel("unsupported", "supported", seen).equals(" · nicht unterstützt"), "REQ-014.2 current-session support wins and carries no date");
            require(!MainActivity.supportLabel("", "supported", seen).equals(MainActivity.supportLabel("supported", "supported", seen)), "REQ-014.2 stored support is distinguishable from current support");

            // REQ-014.4: default selection decision (the live initialize path applies it for a created record).
            JSONArray catalog = new JSONArray();
            for (String[] row : new String[][]{{"engine.maf", "supported"}, {"vehicle.speed", "supported"}, {"engine.rpm", "supported"}, {"engine.coolant_temperature", "unsupported"}, {"engine.load", "supported"}})
                catalog.put(new JSONObject().put("semantic", row[0]).put("status", row[1]));
            catalog.put(new JSONObject().put("semantic", "engine.coolant_temperature_x"));
            require(ObdService.defaultSelection(catalog).toString().equals("[\"engine.rpm\",\"vehicle.speed\",\"engine.maf\"]"), "REQ-014.4 default selection is the supported ones of rpm, speed, coolant, MAF: " + ObdService.defaultSelection(catalog));
        } finally {
            service.setSignals(selectedBefore); // while a synthetic record is selected, so no real record is written
            for (String vin : new String[]{VIN_SEL_A, VIN_SEL_B}) own.delete(vin);
            if (!previous.isEmpty()) own.select(previous);
        }
        require(own.snapshot().toString().equals(before), "synthetic selection records removed, service store restored");
    }
    private static void selectVehicle(ObdService service, String vin) throws Exception {
        String expected = "Fahrzeug gewählt: " + service.vehicles.name(vin);
        service.selectVehicle(vin);
        waitFor(() -> service.snapshot().optString("message").equals(expected) && service.vehicles.snapshot().optString("selected").equals(vin), "select " + vin);
        SystemClock.sleep(100);
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
