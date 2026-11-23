package de.obdentic.app;

import android.util.AtomicFile;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;

/**
 * VIN-keyed vehicle records in one app-private file. Plain data only: no adapter access, no UI.
 * Every mutation works on a copy and replaces the in-memory state only after the file was written,
 * so a write failure leaves memory and file at the last persisted state. An unreadable file is
 * never overwritten implicitly; only {@link #reset()} replaces it.
 */
public final class VehicleStore {
    public static final String UNREADABLE = "Fahrzeugspeicher nicht lesbar", WRITE_FAILED = "Fahrzeug konnte nicht gespeichert werden";
    private final AtomicFile file;
    private JSONObject data = empty();
    private boolean unreadable;

    public VehicleStore(File path) {
        file = new AtomicFile(path);
        try {
            JSONObject loaded = new JSONObject(new String(file.readFully(), StandardCharsets.UTF_8));
            JSONArray rows = loaded.getJSONArray("vehicles");
            if (loaded.getInt("version") != 1) throw new JSONException("version");
            for (int i = 0; i < rows.length(); i++) rows.getJSONObject(i).getString("vin");
            loaded.getString("selected");
            data = loaded;
        } catch (FileNotFoundException missing) {
            // First start: empty store.
        } catch (IOException | JSONException | RuntimeException bad) {
            unreadable = true;
        }
    }

    private static JSONObject empty() { return build(new JSONArray(), ""); }
    private static JSONObject build(JSONArray vehicles, String selected) {
        try { return new JSONObject().put("version", 1).put("selected", selected).put("vehicles", vehicles); }
        catch (JSONException e) { throw new IllegalStateException(e); }
    }
    private static JSONObject copy(JSONObject value) {
        try { return new JSONObject(value.toString()); } catch (JSONException e) { throw new IllegalStateException(e); }
    }
    private static JSONObject find(JSONObject root, String vin) {
        JSONArray rows = root.optJSONArray("vehicles");
        for (int i = 0; i < rows.length(); i++) if (vin.equals(rows.optJSONObject(i).optString("vin"))) return rows.optJSONObject(i);
        return null;
    }
    private static void put(JSONObject object, String key, Object value) {
        try { object.put(key, value); } catch (JSONException e) { throw new IllegalStateException(e); }
    }
    private boolean write(JSONObject next) {
        FileOutputStream out = null;
        try {
            out = file.startWrite();
            out.write(next.toString().getBytes(StandardCharsets.UTF_8));
            file.finishWrite(out);
            data = next;
            return true;
        } catch (IOException e) {
            if (out != null) file.failWrite(out);
            return false;
        }
    }

    /** Records a live inventory. Returns unreadable_vin, store_unreadable, write_failed, created, updated or switched. */
    public synchronized String observe(JSONObject inventory, long now) {
        String vin = inventory.optString("vin", "");
        JSONObject identity = inventory.optJSONObject("vin_identity");
        if (vin.length() != 17 || (identity != null && "invalid_vin".equals(identity.optString("status")))) return "unreadable_vin";
        if (unreadable) return "store_unreadable";
        JSONObject next = copy(data);
        String previous = next.optString("selected");
        JSONObject row = find(next, vin);
        boolean created = row == null;
        if (created) {
            String hint = "";
            if (identity != null) {
                if (!identity.isNull("make_hint")) hint = identity.optString("make_hint", "");
                if (hint.isEmpty() && !identity.isNull("manufacturer")) hint = identity.optString("manufacturer", "");
            }
            row = new JSONObject();
            put(row, "vin", vin);
            put(row, "name", (hint.isEmpty() ? "Fahrzeug" : hint) + " ·" + vin.substring(13));
            if (!hint.isEmpty()) put(row, "manufacturer_hint", hint);
            put(row, "first_connected", now);
            next.optJSONArray("vehicles").put(row);
        }
        JSONArray responders = inventory.optJSONArray("responders"), signals = inventory.optJSONArray("signals");
        JSONArray keptSignals = new JSONArray();
        for (int i = 0; signals != null && i < signals.length(); i++) {
            JSONObject signal = signals.optJSONObject(i);
            if (signal != null) keptSignals.put(signalRow(signal));
        }
        put(row, "responders", responders == null ? new JSONArray() : copyArray(responders));
        put(row, "signals", keptSignals);
        put(row, "engine_target_validated", inventory.optBoolean("engine_target_validated"));
        put(row, "last_connected", now);
        put(next, "selected", vin);
        if (!write(next)) return "write_failed";
        return !previous.isEmpty() && !previous.equals(vin) ? "switched" : created ? "created" : "updated";
    }
    private static JSONObject signalRow(JSONObject signal) {
        JSONObject row = new JSONObject();
        put(row, "semantic", signal.optString("semantic"));
        put(row, "status", signal.optString("status"));
        return row;
    }
    private static JSONArray copyArray(JSONArray value) {
        try { return new JSONArray(value.toString()); } catch (JSONException e) { throw new IllegalStateException(e); }
    }

    /** Each mutation below returns null on success, otherwise a German error text. */
    private String mutate(String vin, java.util.function.Consumer<JSONObject> change) {
        if (unreadable) return UNREADABLE;
        JSONObject next = copy(data);
        if (vin != null && find(next, vin) == null) return "Fahrzeug nicht gefunden";
        change.accept(next);
        return write(next) ? null : WRITE_FAILED;
    }
    /** REQ-015.1/.4: stores the odometer; a lower value than the stored one is stored too and noted, never corrected. */
    public synchronized String odometer(String vin, double km, long now) {
        return mutate(vin, next -> {
            JSONObject row = find(next, vin), old = row.optJSONObject("odometer");
            row.remove("odometer_error");
            row.remove("odometer_decrease");
            if (old != null && km < old.optDouble("km")) {
                JSONObject decrease = new JSONObject();
                put(decrease, "previous_km", old.optDouble("km"));
                put(decrease, "previous_at", old.optLong("read_at"));
                put(row, "odometer_decrease", decrease);
            }
            JSONObject value = new JSONObject();
            put(value, "km", km);
            put(value, "read_at", now);
            put(value, "source", "OBD PID A6");
            put(row, "odometer", value);
        });
    }
    /** REQ-015.3: records the failure; stored value and date stay untouched. */
    public synchronized String odometerFailure(String vin, String error, long now) {
        return mutate(vin, next -> {
            JSONObject failure = new JSONObject();
            put(failure, "error", error);
            put(failure, "at", now);
            put(find(next, vin), "odometer_error", failure);
        });
    }
    /** REQ-014.1: the live signal selection of one record (semantics, caller-checked); replaces the previous one. */
    public synchronized String setSelection(String vin, JSONArray semantics) {
        return mutate(vin, next -> put(find(next, vin), "selection", copyArray(semantics)));
    }
    /** The stored signal selection of a record, or null when it has none. */
    public synchronized JSONArray selection(String vin) {
        JSONObject row = find(data, vin);
        return row == null || row.optJSONArray("selection") == null ? null : copyArray(row.optJSONArray("selection"));
    }
    /** REQ-025.4: saves a named selection on its vehicle record; a repeated name replaces that preset. */
    public synchronized String savePreset(String vin, String name, JSONArray semantics) {
        String trimmed = name == null ? "" : name.trim();
        if (trimmed.isEmpty() || trimmed.length() > 40) return "Der Presetname muss 1 bis 40 Zeichen lang sein";
        if (semantics == null || semantics.length() > 32) return "Ein Preset darf höchstens 32 Messwerte enthalten";
        JSONArray signals = new JSONArray();
        java.util.HashSet<String> seen = new java.util.HashSet<>();
        for (int i = 0; i < semantics.length(); i++) {
            Object value = semantics.opt(i);
            if (!(value instanceof String) || ((String) value).isEmpty()) return "Preset enthält einen ungültigen Messwert";
            if (seen.add((String) value)) signals.put(value);
        }
        return mutate(vin, next -> {
            JSONObject row = find(next, vin);
            JSONArray presets = row.optJSONArray("presets");
            JSONArray kept = new JSONArray();
            for (int i = 0; presets != null && i < presets.length(); i++) {
                JSONObject item = presets.optJSONObject(i);
                if (item != null && !trimmed.equals(item.optString("name"))) kept.put(item);
            }
            JSONObject preset = new JSONObject();
            put(preset, "name", trimmed);
            put(preset, "signals", signals);
            kept.put(preset);
            put(row, "presets", kept);
        });
    }
    /** REQ-025.4: returns the named presets for a vehicle, in saved order. */
    public synchronized JSONArray presets(String vin) {
        JSONObject row = find(data, vin);
        JSONArray saved = row == null ? null : row.optJSONArray("presets");
        return saved == null ? new JSONArray() : copyArray(saved);
    }
    /** REQ-025.4: returns a copy of the named preset's semantics, or null when it does not exist. */
    public synchronized JSONArray preset(String vin, String name) {
        JSONArray saved = presets(vin);
        for (int i = 0; i < saved.length(); i++) {
            JSONObject item = saved.optJSONObject(i);
            if (item != null && name != null && name.equals(item.optString("name"))) {
                JSONArray signals = item.optJSONArray("signals");
                return signals == null ? new JSONArray() : copyArray(signals);
            }
        }
        return null;
    }
    /** REQ-023.5/REQ-026.2: persists only the closed diagnostic result types (ECU discovery, learning, availability) on their vehicle. */
    public synchronized String setDiagnostic(String vin, String key, JSONObject result) {
        if (!"ecu_discovery".equals(key) && !"knowledge_learning".equals(key) && !"availability".equals(key)) return "Unbekanntes Diagnoseergebnis";
        if (result == null) return "Diagnoseergebnis fehlt";
        JSONObject saved = copy(result);
        return mutate(vin, next -> put(find(next, vin), key, saved));
    }
    public synchronized String select(String vin) { return mutate(vin, next -> put(next, "selected", vin)); }
    public synchronized String rename(String vin, String name) {
        String trimmed = name == null ? "" : name.trim();
        if (trimmed.isEmpty() || trimmed.length() > 40) return "Der Name muss 1 bis 40 Zeichen lang sein";
        return mutate(vin, next -> put(find(next, vin), "name", trimmed));
    }
    public synchronized String delete(String vin) {
        return mutate(vin, next -> {
            JSONArray rows = next.optJSONArray("vehicles"), kept = new JSONArray();
            for (int i = 0; i < rows.length(); i++) if (!vin.equals(rows.optJSONObject(i).optString("vin"))) kept.put(rows.optJSONObject(i));
            put(next, "vehicles", kept);
            if (vin.equals(next.optString("selected"))) put(next, "selected", "");
        });
    }
    /** The only way to replace an unreadable store; callers must have asked the user. */
    public synchronized String reset() {
        if (!write(empty())) return WRITE_FAILED;
        unreadable = false;
        return null;
    }
    public synchronized String name(String vin) { JSONObject row = find(data, vin); return row == null ? "" : row.optString("name"); }
    public synchronized boolean unreadable() { return unreadable; }
    /**
     * REQ-038.6: the vehicle coverage ledger of a record, a copy, or null when the record has none. It lives in the record (so deleting the
     * record deletes it, REQ-013.9) and is kept out of {@link #snapshot()}: the service state and the UI never carry it.
     */
    public synchronized JSONObject coverageLedger(String vin) {
        JSONObject row = unreadable ? null : find(data, vin), ledger = row == null ? null : row.optJSONObject(COVERAGE);
        return ledger == null ? null : copy(ledger);
    }
    /** True when the record carries a ledger entry of any shape: a malformed one is unreadable, never "no ledger yet" (REQ-038.8). */
    public synchronized boolean hasCoverageEntry(String vin) { JSONObject row = unreadable ? null : find(data, vin); return row != null && row.has(COVERAGE); }
    /** REQ-038.6/.8: stores the ledger on its record; a failed write leaves the last persisted state. Returns null on success, else a German error text. */
    public synchronized String setCoverageLedger(String vin, JSONObject ledger) {
        return mutate(vin, next -> put(find(next, vin), COVERAGE, copy(ledger)));
    }
    /** The selected VIN, or "" when none; no copy of the store. */
    public synchronized String selected() { return data.optString("selected"); }
    /** A copy of every record without its ledger: the ledger is never serialised here, however large it has grown. */
    public synchronized JSONObject snapshot() {
        JSONArray rows = data.optJSONArray("vehicles"), kept = new JSONArray();
        for (int i = 0; rows != null && i < rows.length(); i++) {
            JSONObject row = rows.optJSONObject(i), out = new JSONObject();
            for (java.util.Iterator<String> keys = row.keys(); keys.hasNext(); ) {
                String key = keys.next();
                if (COVERAGE.equals(key)) continue;
                Object value = row.opt(key);
                put(out, key, value instanceof JSONObject ? copy((JSONObject) value) : value instanceof JSONArray ? copyArray((JSONArray) value) : value);
            }
            kept.put(out);
        }
        JSONObject out = build(kept, data.optString("selected"));
        put(out, "unreadable", unreadable);
        return out;
    }
    private static final String COVERAGE = "coverage_ledger";
}
