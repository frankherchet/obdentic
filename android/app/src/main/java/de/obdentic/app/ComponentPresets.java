package de.obdentic.app;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * #291 (REQ-035): the built-in component presets owned by OBDentic (not by the Knowledge repository) and their
 * resolution against the effective catalog of the connected vehicle.
 *
 * A preset names semantics only, never commands or addresses. {@link #resolve} maps each semantic to the unique
 * live signal key the catalog offers for it. A semantic without a key is reported, never dropped silently:
 * missing, unsupported, ambiguous, excluded because it needs the extended session, or pending until it is active.
 * Pure logic without a transport; nothing here reads a vehicle.
 */
public final class ComponentPresets {
    /** A preset stays within the measured cost bound of one polling cycle (about 2.8 s for 16 signals). */
    public static final int MAX_SIGNALS = 12;

    private static final Map<String, String> LABELS = new LinkedHashMap<>();
    private static final Map<String, List<String>> SIGNALS = new LinkedHashMap<>();
    static {
        add("dpf", "DPF", "dpf.differential_pressure", "dpf.differential_pressure_offset", "dpf.soot_mass_calculated",
            "dpf.soot_mass_measured", "dpf.distance_since_regeneration", "dpf.time_since_regeneration",
            "dpf.field_regeneration_request_status", "dpf.field_regeneration_lock_status",
            "exhaust.temperature.pre_dpf", "exhaust.temperature.post_dpf", "dpf.surface_temperature", "engine.rpm");
        add("egr", "AGR", "engine.egr.commanded", "engine.egr.error", "egr.valve_duty", "egr.low_pressure_closing",
            "engine.maf", "engine.intake_manifold_pressure", "engine.boost_pressure_actual",
            "engine.boost_pressure_commanded", "engine.rpm", "engine.load");
        add("cooling", "K\u00fchlung/Thermostat", "engine.coolant_temperature", "engine.oil_temperature",
            "cooling.radiator_outlet_temperature_actual", "cooling.radiator_outlet_temperature_commanded",
            "cooling.fan1_duty", "cooling.fan2_duty", "vehicle.ambient_air_temperature", "vehicle.speed", "engine.load");
        add("turbo", "Turbo/Ladedruck", "engine.boost_pressure_actual", "engine.boost_pressure_commanded",
            "boost.regulator_feedback", "boost.turbine_actuator_activation", "air.intercooler_outlet_temperature",
            "engine.maf", "engine.intake_manifold_pressure", "engine.barometric_pressure",
            "pedal.accelerator_sensor_1", "engine.rpm");
        add("rail", "Rail/Einspritzung", "engine.rail_pressure_actual", "engine.rail_pressure_commanded",
            "fuel.rail_pressure_control_deviation", "fuel.metering_valve_duty", "fuel.pressure_regulator_duty",
            "injection.quantity_deviation_cylinder1", "injection.quantity_deviation_cylinder2",
            "injection.quantity_deviation_cylinder3", "injection.quantity_deviation_cylinder4", "engine.rpm");
    }
    private ComponentPresets() {}

    private static void add(String id, String label, String... semantics) {
        if (semantics.length > MAX_SIGNALS) throw new IllegalStateException("preset " + id + " exceeds " + MAX_SIGNALS + " signals");
        LABELS.put(id, label);
        SIGNALS.put(id, Arrays.asList(semantics));
    }

    /** Every preset as {id, label, signals:[semantic, ...]}, in a fixed order. */
    public static JSONArray list() {
        JSONArray all = new JSONArray();
        for (Map.Entry<String, List<String>> preset : SIGNALS.entrySet()) {
            JSONObject item = new JSONObject();
            put(item, "id", preset.getKey());
            put(item, "label", LABELS.get(preset.getKey()));
            put(item, "signals", new JSONArray(preset.getValue()));
            all.put(item);
        }
        return all;
    }

    /**
     * Resolve one preset against the live catalog.
     *
     * @param catalog           selectable signals; standard PIDs carry their semantic, Knowledge signals either the raw
     *                          {semantic, key} of the core or the normalized {semantic = key, meaning = semantic}
     * @param sessionCandidates applicable engine session signals that are not selectable yet (same shape), may be null
     * @param includeExtended   false: signals that need the extended session are excluded and reported unavailable;
     *                          true: they are selected once active and reported pending (requires_uds) before
     * @return {selected: unique live keys in preset order, unavailable: semantics without a key, requires_uds,
     *          pending_uds: semantics waiting for the extended session, ambiguous: semantics that match several
     *          signals, reasons: {semantic: missing|unsupported|ambiguous|session_excluded|pending_uds}}
     */
    public static JSONObject resolve(String id, JSONArray catalog, JSONArray sessionCandidates, boolean includeExtended) {
        List<String> semantics = SIGNALS.get(id);
        if (semantics == null) throw new IllegalArgumentException("unknown component preset: " + id);
        List<JSONObject> signals = objects(catalog), candidates = objects(sessionCandidates);
        Set<String> selected = new LinkedHashSet<>();
        JSONArray unavailable = new JSONArray(), pending = new JSONArray(), ambiguous = new JSONArray();
        JSONObject reasons = new JSONObject();
        for (String semantic : semantics) {
            List<JSONObject> standard = new ArrayList<>(), knowledge = new ArrayList<>(), session = new ArrayList<>(), pendingEntries = new ArrayList<>();
            boolean unsupported = false;
            for (JSONObject signal : signals) {
                if (!semantic.equals(meaning(signal))) continue;
                if ("unsupported".equals(signal.optString("status"))) { unsupported = true; continue; }
                if (signal.optBoolean("extended_session_required")) session.add(signal);
                else if (isStandard(signal)) standard.add(signal);
                else knowledge.add(signal);
            }
            for (JSONObject candidate : candidates) if (semantic.equals(meaning(candidate))) pendingEntries.add(candidate);

            String reason;
            if (!standard.isEmpty()) { selected.add(key(standard.get(0))); continue; } // identical semantics: the standard key wins
            else if (knowledge.size() == 1) { selected.add(key(knowledge.get(0))); continue; }
            else if (knowledge.size() > 1) reason = "ambiguous";
            else if (!session.isEmpty()) {
                if (!includeExtended) reason = "session_excluded";
                else if (session.size() == 1) { selected.add(key(session.get(0))); continue; }
                else reason = "ambiguous";
            }
            else if (unsupported) reason = "unsupported";
            else if (!pendingEntries.isEmpty()) {
                if (!includeExtended) reason = "session_excluded";
                else if (pendingEntries.size() == 1) reason = "pending_uds";
                else reason = "ambiguous";
            }
            else reason = "missing";

            put(reasons, semantic, reason);
            if (reason.equals("pending_uds")) pending.put(semantic);
            else unavailable.put(semantic);
            if (reason.equals("ambiguous")) ambiguous.put(semantic);
        }
        JSONObject result = new JSONObject();
        put(result, "selected", new JSONArray(selected));
        put(result, "unavailable", unavailable);
        put(result, "requires_uds", pending.length() > 0);
        put(result, "pending_uds", pending);
        put(result, "ambiguous", ambiguous);
        put(result, "reasons", reasons);
        return result;
    }

    /** The original semantic: Android-normalized catalogs keep it as "meaning", the raw core catalog as "semantic". */
    private static String meaning(JSONObject signal) { return signal.optString("meaning", signal.optString("semantic")); }

    /** The unique live key: the core's key for a Knowledge signal, the plain semantic for a standard PID. */
    private static String key(JSONObject signal) { return signal.optString("key", signal.optString("semantic")); }

    private static boolean isStandard(JSONObject signal) { return !signal.has("key") && !signal.has("definition_id"); }

    private static List<JSONObject> objects(JSONArray array) {
        List<JSONObject> list = new ArrayList<>();
        if (array != null) for (int i = 0; i < array.length(); i++) { JSONObject item = array.optJSONObject(i); if (item != null) list.add(item); }
        return list;
    }

    private static void put(JSONObject object, String name, Object value) {
        try { object.put(name, value); } catch (JSONException e) { throw new IllegalStateException(e); }
    }
}
