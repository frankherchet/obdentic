package de.obdentic.app;

import java.util.List;
import org.json.JSONArray;
import org.json.JSONObject;

/** #216: decoded values render rounded, German, with display units and in logical order. */
final class FormatAcceptance {
    static void run(JSONArray extended) throws Exception {
        eq(MeasurementFormat.formatMeasurement(0.35000000000000003, "mg_stroke"), "0,35 mg/Hub");
        eq(MeasurementFormat.formatMeasurement(0.35000000000000003, "mg_stroke", 0.01), "0,35 mg/Hub");
        eq(MeasurementFormat.formatMeasurement(0.11, "liters"), "0,11 l");
        eq(MeasurementFormat.formatMeasurement(965, "hPa"), "965 hPa");
        eq(MeasurementFormat.formatMeasurement(1041.0, "hPa", 1.0), "1041 hPa");
        eq(MeasurementFormat.formatMeasurement(0, ""), "0");
        eq(MeasurementFormat.formatMeasurement(-0.03, "mg_stroke"), "-0,03 mg/Hub");
        eq(MeasurementFormat.formatMeasurement(-0.001, "mg_stroke"), "0 mg/Hub");
        eq(MeasurementFormat.formatMeasurement(123456.7, "km"), "123456,7 km");
        eq(MeasurementFormat.formatMeasurement(27.3, "°C", 0.1), "27,3 °C");
        eq(MeasurementFormat.formatMeasurement("Bereit", "%"), "Bereit %");
                for (int i = 0; i < extended.length(); i++) {
            JSONObject reading = extended.getJSONObject(i);
            String shown = MeasurementFormat.formatReading(reading);
            require(!shown.contains("None") && !shown.contains("mg_stroke") && !shown.contains("liters") && !shown.contains("celicium")
                && !shown.contains(".") && !shown.endsWith(" "), "#216 demo EA189 reading renders display-ready: " + shown);
            if (reading.getString("semantic").startsWith("dpf.") && reading.getString("unit").isEmpty())
                require(shown.matches("-?\\d+"), "#216 raw status shows the number without a unit: " + shown);
        }
        List<JSONObject> sorted = MeasurementFormat.sortReadings(extended);
        require(sorted.size() == extended.length(), "#216 sorting keeps every reading");
        int last = 0;
        for (JSONObject reading : sorted) {
            String semantic = reading.getString("semantic");
            if (!semantic.startsWith("engine.injector_deviation.cylinder_")) continue;
            int cylinder = Integer.parseInt(semantic.substring(semantic.length() - 1));
            require(cylinder == last + 1, "#216 cylinders ascend 1-4, got " + cylinder + " after " + last);
            last = cylinder;
        }
        require(last == 4, "#216 all four cylinders present");
    }
    private static void eq(String actual, String expected) { require(expected.equals(actual), "#216 formatter expected '" + expected + "' but was '" + actual + "'"); }
    private static void require(boolean value, String message) { if (!value) throw new AssertionError(message); }
}
