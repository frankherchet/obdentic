package de.obdentic.app;

import java.util.HashMap;
import java.util.Map;

/** Short practical German display names for catalog signals (display only; semantic ids stay in Debug). */
public final class SignalNames {
    public static final int MAX = 24;
    private static final Map<String, String> NAMES = new HashMap<>();
    static {
        Map<String, String> m = NAMES;
        m.put("engine.rpm", "Drehzahl");
        m.put("engine.coolant_temperature", "Kühlmittel");
        m.put("vehicle.speed", "Geschwindigkeit");
        m.put("engine.maf", "Luftmasse");
        m.put("engine.load", "Motorlast");
        m.put("engine.intake_manifold_pressure", "Saugrohrdruck");
        m.put("engine.intake_air_temperature", "Ansaugluft");
        m.put("engine.egr.commanded", "AGR Soll");
        m.put("engine.egr.error", "AGR Fehler");
        m.put("engine.runtime", "Laufzeit");
        m.put("vehicle.accelerator_pedal_d", "Gaspedal D");
        m.put("vehicle.accelerator_pedal_e", "Gaspedal E");
        m.put("vehicle.accelerator_pedal_f", "Gaspedal F");
        m.put("engine.relative_throttle", "Drosselklappe rel.");
        m.put("engine.barometric_pressure", "Umgebungsdruck");
        m.put("engine.control_module_voltage", "Bordspannung");
        m.put("engine.throttle_position", "Drosselklappe");
        m.put("engine.throttle_position_b", "Drosselklappe B");
        m.put("engine.throttle_position_c", "Drosselklappe C");
        m.put("vehicle.distance_with_mil_on", "Strecke mit MIL");
        m.put("engine.fuel_rail_gauge_pressure", "Raildruck");
        m.put("vehicle.warmups_since_dtc_clear", "Warmläufe seit Löschung");
        m.put("vehicle.distance_since_dtc_clear", "Strecke seit Löschung");
        m.put("vehicle.ambient_air_temperature", "Außentemperatur");
        m.put("engine.throttle_actuator.commanded", "Drosselklappe Soll");
        m.put("engine.short_term_fuel_trim_bank1", "Kurzzeit-Trim B1");
        m.put("engine.long_term_fuel_trim_bank1", "Langzeit-Trim B1");
        m.put("engine.short_term_fuel_trim_bank2", "Kurzzeit-Trim B2");
        m.put("engine.long_term_fuel_trim_bank2", "Langzeit-Trim B2");
        m.put("engine.fuel_rail_gauge_pressure_low", "Raildruck Niederdruck");
        m.put("engine.timing_advance", "Zündwinkel");
        m.put("engine.fuel_rail_pressure_relative", "Raildruck rel. Saugrohr");
        m.put("engine.evap_purge_commanded", "Tankentlüftung Soll");
        m.put("vehicle.fuel_tank_level", "Tankfüllstand");
        m.put("engine.catalyst_temperature_b1s1", "Kat-Temp B1S1");
        m.put("engine.catalyst_temperature_b2s1", "Kat-Temp B2S1");
        m.put("engine.catalyst_temperature_b1s2", "Kat-Temp B1S2");
        m.put("engine.catalyst_temperature_b2s2", "Kat-Temp B2S2");
        m.put("engine.absolute_load", "Absolute Last");
        m.put("engine.commanded_equivalence_ratio", "Lambda Soll");
        m.put("engine.ethanol_fuel_percentage", "Ethanolanteil");
        m.put("engine.fuel_rail_pressure_absolute", "Raildruck absolut");
        m.put("vehicle.relative_accelerator_pedal_position", "Gaspedal relativ");
        m.put("engine.oil_temperature", "Öltemperatur");
        m.put("engine.fuel_injection_timing", "Einspritzbeginn");
        m.put("engine.fuel_rate", "Kraftstoffrate");
        m.put("engine.driver_demand_torque_percent", "Fahrerwunschmoment");
        m.put("engine.actual_torque_percent", "Istmoment");
        m.put("engine.reference_torque", "Referenzmoment");
        m.put("vehicle.odometer", "km-Stand");
    }
    private SignalNames() {}

    /** Short name for a semantic id; unknown ids fall back to the catalog title cut to 24 characters. */
    public static String shortName(String semantic, String fallbackTitle) {
        String name = NAMES.get(semantic);
        if (name != null) return name;
        String t = fallbackTitle == null || fallbackTitle.isEmpty() ? "Messwert" : fallbackTitle;
        return t.length() <= MAX ? t : t.substring(0, MAX - 1) + "\u2026";
    }
}
