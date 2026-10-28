package de.obdentic.app;

/** #216: decoded values render rounded, German and with display units. */
final class FormatAcceptance {
    static void run() {
        eq(MeasurementFormat.formatMeasurement(0.35000000000000003, "V"), "0,35 V");
        eq(MeasurementFormat.formatMeasurement(0.35000000000000003, "V", 0.01), "0,35 V");
        eq(MeasurementFormat.formatMeasurement(965, "hPa"), "965 hPa");
        eq(MeasurementFormat.formatMeasurement(1041.0, "hPa", 1.0), "1041 hPa");
        eq(MeasurementFormat.formatMeasurement(0, ""), "0");
        eq(MeasurementFormat.formatMeasurement(-0.03, "V"), "-0,03 V");
        eq(MeasurementFormat.formatMeasurement(-0.001, "V"), "0 V");
        eq(MeasurementFormat.formatMeasurement(123456.7, "km"), "123456,7 km");
        eq(MeasurementFormat.formatMeasurement(27.3, "°C", 0.1), "27,3 °C");
        eq(MeasurementFormat.formatMeasurement("Bereit", "%"), "Bereit %");
        eq(MeasurementFormat.formatMeasurement(12.3456, "V", 0.001), "12,35 V");
    }
    /** #232: every catalog signal has a short non-id name. */
    static void names(org.json.JSONArray signals) {
        require(signals.length() == 50, "#232 catalog has 50 signals but " + signals.length());
        for (int i = 0; i < signals.length(); i++) {
            String id = signals.optJSONObject(i).optString("semantic");
            String n = SignalNames.shortName(id, signals.optJSONObject(i).optString("title"));
            require(!n.isEmpty() && n.length() <= 24 && !n.equals(id) && !n.equals("Messwert"), "#232 bad short name for " + id + ": '" + n + "'");
        }
    }
    /** #232: no TextView under v shows a semantic id. */
    static void noSemanticIds(android.view.View v) {
        if (v instanceof android.widget.TextView) {
            String t = ((android.widget.TextView) v).getText().toString();
            require(!t.matches("^[a-z]+(\\.[a-z0-9_]+)+$"), "#232 live card shows semantic id '" + t + "'");
        }
        if (v instanceof android.view.ViewGroup) { android.view.ViewGroup g = (android.view.ViewGroup) v; for (int i = 0; i < g.getChildCount(); i++) noSemanticIds(g.getChildAt(i)); }
    }
    private static void eq(String actual, String expected) { require(expected.equals(actual), "#216 formatter expected '" + expected + "' but was '" + actual + "'"); }
    private static void require(boolean value, String message) { if (!value) throw new AssertionError(message); }
}
