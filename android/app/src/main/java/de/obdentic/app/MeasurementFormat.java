package de.obdentic.app;

import java.text.NumberFormat;
import java.util.Locale;
import org.json.JSONObject;

/** Display-only formatting of decoded values (German number format, display units); never touches decoding or evidence. */
public final class MeasurementFormat {
    private MeasurementFormat() {}

    /** Core unit identifier to display unit; empty means no unit is shown. */
    public static String displayUnit(String unit) {
        if (unit == null) return "";
        switch (unit) {
            case "count": case "ratio": return "";
            default: return unit;
        }
    }

    /** Numbers: German format, no grouping, at most two decimals; other values and empty units pass through. */
    public static String formatMeasurement(Object value, String unit) { return formatMeasurement(value, unit, Double.NaN); }

    /** `resolution` (definition multiplier, e.g. 0.01) fixes the maximum decimals; NaN means two. */
    public static String formatMeasurement(Object value, String unit, double resolution) {
        String text;
        if (value instanceof Number) {
            double number = ((Number) value).doubleValue();
            int decimals = 2;
            if (resolution > 0 && !Double.isInfinite(resolution)) { decimals = 0; for (double r = resolution; r < 0.999999 && decimals < 6; r *= 10) decimals++; }
            NumberFormat f = NumberFormat.getNumberInstance(Locale.GERMANY);
            f.setGroupingUsed(false); f.setMaximumFractionDigits(decimals); f.setMinimumFractionDigits(0);
            text = Double.isFinite(number) ? f.format(number) : String.valueOf(value);
            if (text.matches("-0(,0*)?")) text = text.substring(1);
        } else text = value == null || value == JSONObject.NULL ? "—" : String.valueOf(value);
        String shown = displayUnit(unit);
        return shown.isEmpty() ? text : text + " " + shown;
    }

    /** A reading JSON as the core returns it: value, unit and, when present, the definition multiplier as resolution. */
    public static String formatReading(JSONObject reading) {
        JSONObject definition = reading.optJSONObject("source_definition");
        return formatMeasurement(reading.opt("value"), reading.optString("unit", ""), definition == null ? Double.NaN : definition.optDouble("multiplier", Double.NaN));
    }
}
