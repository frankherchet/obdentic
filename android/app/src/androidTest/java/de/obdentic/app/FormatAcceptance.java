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
    }
    private static void eq(String actual, String expected) { require(expected.equals(actual), "#216 formatter expected '" + expected + "' but was '" + actual + "'"); }
    private static void require(boolean value, String message) { if (!value) throw new AssertionError(message); }
}
