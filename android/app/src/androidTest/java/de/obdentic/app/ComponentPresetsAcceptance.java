package de.obdentic.app;

import org.json.JSONArray;
import org.json.JSONObject;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/** #291 / REQ-035: component presets resolve against simulated catalogs; no adapter, no BLE, no vehicle traffic. */
final class ComponentPresetsAcceptance {
    private static final String ENGINE = "vag.ecu_target.7e0", OTHER = "vag.ecu_target.7e1";

    static void run() throws Exception {
        registry();
        standardKeyWinsAndUnsupportedFallsBack();
        defaultSessionSignalsAreUniqueKeysFromBothCatalogShapes();
        sessionSignalsNeverBecomeSelectableBeforeTheSessionIsActive();
        ambiguousAndMissingAreReportedNotDropped();
    }

    private static void registry() throws Exception {
        JSONArray all = ComponentPresets.list();
        String[] ids = {"dpf", "egr", "cooling", "turbo", "rail"};
        require(all.length() == ids.length, "five built-in presets: " + all);
        for (int i = 0; i < ids.length; i++) {
            JSONObject preset = all.getJSONObject(i);
            require(ids[i].equals(preset.getString("id")), "preset order and id " + ids[i]);
            require(!preset.getString("label").isEmpty(), "label of " + ids[i]);
            JSONArray signals = preset.getJSONArray("signals");
            require(signals.length() >= 5 && signals.length() <= 12, ids[i] + " has 5-12 signals: " + signals.length());
            Set<String> unique = new HashSet<>();
            for (int s = 0; s < signals.length(); s++) {
                String semantic = signals.getString(s);
                require(semantic.matches("[a-z0-9_]+(\\.[a-z0-9_]+)+"), "a preset names semantics only, never a command: " + semantic);
                require(unique.add(semantic), "no duplicate semantic in " + ids[i] + ": " + semantic);
            }
        }
        require(contains(all.getJSONObject(0).getJSONArray("signals"), "dpf.surface_temperature"), "the DPF preset carries the session-only surface temperature");
        try { ComponentPresets.resolve("unknown", new JSONArray(), new JSONArray(), false); require(false, "unknown preset id is rejected"); }
        catch (IllegalArgumentException expected) { /* rejected as required */ }
    }

    /** Identical semantics: the standard key wins; an unsupported standard PID falls back to the ECU's Knowledge signal. */
    private static void standardKeyWinsAndUnsupportedFallsBack() throws Exception {
        JSONArray catalog = new JSONArray()
            .put(standard("engine.maf", "supported")).put(normalized("engine.maf", ENGINE, false))
            .put(standard("engine.rpm", "supported"))
            .put(standard("engine.load", "unsupported")).put(normalized("engine.load", ENGINE, false))
            .put(standard("engine.egr.commanded", "unsupported"));
        JSONObject result = ComponentPresets.resolve("egr", catalog, new JSONArray(), false);
        JSONArray selected = result.getJSONArray("selected");
        require(contains(selected, "engine.maf") && !contains(selected, "engine.maf@" + ENGINE), "standard key preferred for identical semantics: " + result);
        require(contains(selected, "engine.load@" + ENGINE), "unsupported PID falls back to the Knowledge key: " + result);
        require(contains(selected, "engine.rpm"), "plain standard PID selected: " + result);
        require(contains(result.getJSONArray("unavailable"), "engine.egr.commanded") && "unsupported".equals(result.getJSONObject("reasons").getString("engine.egr.commanded")), "unsupported PID is reported, not dropped: " + result);
        accountsForEverySignal("egr", result);
    }

    /** The raw core shape ({semantic, key}) and the Android-normalized shape ({semantic = key, meaning}) give the same unique key. */
    private static void defaultSessionSignalsAreUniqueKeysFromBothCatalogShapes() throws Exception {
        JSONArray raw = new JSONArray().put(raw("boost.regulator_feedback", ENGINE, false)).put(raw("engine.boost_pressure_actual", ENGINE, false)).put(standard("engine.rpm", "supported"));
        JSONArray normalized = new JSONArray().put(normalized("boost.regulator_feedback", ENGINE, false)).put(normalized("engine.boost_pressure_actual", ENGINE, false)).put(standard("engine.rpm", "supported"));
        JSONObject fromRaw = ComponentPresets.resolve("turbo", raw, new JSONArray(), false), fromNormalized = ComponentPresets.resolve("turbo", normalized, new JSONArray(), false);
        require(fromRaw.getJSONArray("selected").toString().equals(fromNormalized.getJSONArray("selected").toString()), "both shapes resolve identically: " + fromRaw + " vs " + fromNormalized);
        JSONArray selected = fromNormalized.getJSONArray("selected");
        require(selected.length() == 3 && contains(selected, "boost.regulator_feedback@" + ENGINE) && contains(selected, "engine.rpm"), "keys, not semantics, are selected: " + selected);
        require(unique(selected), "selected keys are unique: " + selected);
        accountsForEverySignal("turbo", fromNormalized);
        // A repeated catalog row never repeats a key.
        JSONArray doubled = new JSONArray().put(standard("engine.rpm", "supported")).put(standard("engine.rpm", "supported"));
        require(ComponentPresets.resolve("rail", doubled, null, false).getJSONArray("selected").length() == 1, "duplicate rows resolve to one key");
    }

    /** Candidates are metadata: excluded without the switch, pending (requires_uds) while inactive, selected once the live catalog has them. */
    private static void sessionSignalsNeverBecomeSelectableBeforeTheSessionIsActive() throws Exception {
        JSONArray candidates = new JSONArray().put(raw("dpf.surface_temperature", ENGINE, true));
        JSONArray inactive = new JSONArray().put(raw("dpf.differential_pressure", ENGINE, false)).put(standard("engine.rpm", "supported"));
        String surface = "dpf.surface_temperature@" + ENGINE;

        JSONObject excluded = ComponentPresets.resolve("dpf", inactive, candidates, false);
        require(!excluded.getBoolean("requires_uds"), "extended not requested: no UDS needed: " + excluded);
        require(contains(excluded.getJSONArray("unavailable"), "dpf.surface_temperature") && "session_excluded".equals(excluded.getJSONObject("reasons").getString("dpf.surface_temperature")), "the excluded session signal is reported unavailable: " + excluded);
        require(!contains(excluded.getJSONArray("selected"), surface), "never selected without the extended request: " + excluded);

        JSONObject pending = ComponentPresets.resolve("dpf", inactive, candidates, true);
        require(pending.getBoolean("requires_uds"), "an applicable desired candidate absent from the catalog needs the session: " + pending);
        require(contains(pending.getJSONArray("pending_uds"), "dpf.surface_temperature") && !contains(pending.getJSONArray("unavailable"), "dpf.surface_temperature"), "pending, not unavailable: " + pending);
        require(!contains(pending.getJSONArray("selected"), surface), "an inactive candidate is never selectable: " + pending);
        require(contains(pending.getJSONArray("selected"), "dpf.differential_pressure@" + ENGINE) && contains(pending.getJSONArray("selected"), "engine.rpm"), "default-session signals stay selected: " + pending);
        accountsForEverySignal("dpf", pending);

        JSONArray active = new JSONArray(inactive.toString()).put(normalized("dpf.surface_temperature", ENGINE, true));
        JSONObject live = ComponentPresets.resolve("dpf", active, candidates, true);
        require(!live.getBoolean("requires_uds") && contains(live.getJSONArray("selected"), surface), "after activation the live catalog key is selected: " + live);
        require(live.getJSONArray("pending_uds").length() == 0, "nothing pending once active: " + live);
        JSONObject stillExcluded = ComponentPresets.resolve("dpf", active, candidates, false);
        require(!contains(stillExcluded.getJSONArray("selected"), surface) && contains(stillExcluded.getJSONArray("unavailable"), "dpf.surface_temperature"), "an active session signal stays out without the extended request: " + stillExcluded);

        JSONObject notApplicable = ComponentPresets.resolve("dpf", inactive, new JSONArray(), true);
        require(!notApplicable.getBoolean("requires_uds") && contains(notApplicable.getJSONArray("unavailable"), "dpf.surface_temperature"), "no applicable candidate: missing, no UDS needed: " + notApplicable);
        require("missing".equals(notApplicable.getJSONObject("reasons").getString("dpf.surface_temperature")), "reported as missing: " + notApplicable);
    }

    /** A semantic offered by several ECUs without a standard key is marked ambiguous; absent semantics are listed, nothing disappears. */
    private static void ambiguousAndMissingAreReportedNotDropped() throws Exception {
        JSONArray catalog = new JSONArray().put(normalized("fuel.metering_valve_duty", ENGINE, false)).put(normalized("fuel.metering_valve_duty", OTHER, false)).put(standard("engine.rpm", "supported"));
        JSONObject result = ComponentPresets.resolve("rail", catalog, new JSONArray(), true);
        require(contains(result.getJSONArray("ambiguous"), "fuel.metering_valve_duty") && contains(result.getJSONArray("unavailable"), "fuel.metering_valve_duty"), "ambiguous semantic is marked: " + result);
        require(!contains(result.getJSONArray("selected"), "fuel.metering_valve_duty@" + ENGINE) && !contains(result.getJSONArray("selected"), "fuel.metering_valve_duty@" + OTHER), "no guess between ECUs: " + result);
        require("missing".equals(result.getJSONObject("reasons").getString("engine.rail_pressure_actual")), "absent semantic is missing: " + result);
        accountsForEverySignal("rail", result);

        JSONObject empty = ComponentPresets.resolve("cooling", null, null, true);
        require(empty.getJSONArray("selected").length() == 0 && !empty.getBoolean("requires_uds"), "an empty catalog selects nothing: " + empty);
        accountsForEverySignal("cooling", empty);
    }

    /** Every semantic of the preset is exactly one of selected (as a key), unavailable or pending: none vanishes. */
    private static void accountsForEverySignal(String id, JSONObject result) throws Exception {
        int signals = 0;
        JSONArray all = ComponentPresets.list();
        for (int i = 0; i < all.length(); i++) if (id.equals(all.getJSONObject(i).getString("id"))) signals = all.getJSONObject(i).getJSONArray("signals").length();
        int accounted = result.getJSONArray("selected").length() + result.getJSONArray("unavailable").length() + result.getJSONArray("pending_uds").length();
        require(accounted == signals, id + " accounts for all " + signals + " semantics, got " + accounted + ": " + result);
        require(result.getJSONObject("reasons").length() == result.getJSONArray("unavailable").length() + result.getJSONArray("pending_uds").length(), id + " gives a reason for every non-selected semantic: " + result);
    }

    /** A standard PID: its semantic is its id; the catalog has no key. */
    private static JSONObject standard(String semantic, String status) throws Exception {
        return new JSONObject().put("semantic", semantic).put("protocol", "OBD2").put("status", status);
    }

    /** A Knowledge signal as the Rust core reports it: original semantic plus its own key. */
    private static JSONObject raw(String semantic, String target, boolean session) throws Exception {
        return new JSONObject().put("semantic", semantic).put("key", semantic + "@" + target).put("definition_id", "vag.ea189." + semantic)
            .put("target_id", target).put("protocol", session ? "UDS" : "VW").put("extended_session_required", session);
    }

    /** The same signal after the Android service made its key the id and kept the semantic as meaning. */
    private static JSONObject normalized(String semantic, String target, boolean session) throws Exception {
        JSONObject signal = raw(semantic, target, session);
        return signal.put("meaning", semantic).put("semantic", signal.getString("key"));
    }

    private static boolean contains(JSONArray array, String value) {
        for (int i = 0; i < array.length(); i++) if (value.equals(array.optString(i))) return true;
        return false;
    }

    private static boolean unique(JSONArray array) {
        List<String> seen = new ArrayList<>();
        for (int i = 0; i < array.length(); i++) { String value = array.optString(i); if (seen.contains(value)) return false; seen.add(value); }
        return true;
    }

    private static void require(boolean value, String message) { if (!value) throw new AssertionError("REQ-035 " + message); }
}
