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
    public synchronized JSONObject snapshot() {
        JSONObject out = copy(data);
        put(out, "unreadable", unreadable);
        return out;
    }
}
