package de.obdentic.app;

import android.app.Activity;
import android.app.Dialog;
import android.content.Context;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.TextView;
import org.json.JSONArray;
import java.io.File;
import java.util.HashSet;
import java.util.Set;

/** REQ-031 through the real selector dialog and Live tiles with a fake adapter. No BLE. */
final class AvailabilityUiAcceptance {
    interface OnMain { <T> T call(java.util.concurrent.Callable<T> call) throws Exception; }

    static void run(OnMain main, Activity activity, ObdService service, Context context) throws Exception {
        final MainActivity screen = (MainActivity) activity;
        File recordings = new File(context.getFilesDir(), "recordings"); Set<String> before = names(recordings);
        VehicleStore savedStore = service.vehicles; ObdService.Connector savedConnector = service.connector;
        Boolean savedPermission = service.permissionOverride, savedBluetooth = service.bluetoothOverride;
        try {
            service.permissionOverride = Boolean.TRUE; service.bluetoothOverride = Boolean.TRUE;
            service.vehicles = new VehicleStore(new File(context.getCacheDir(), "availability-ui-" + System.nanoTime() + ".json"));
            service.connector = address -> new AvailabilityAcceptance.Link(service.simulatedLink());
            service.setAutoConnect(false); service.disconnect();
            ObdService.autoSurvey = true; service.connect("AA:BB:CC:00:00:31");
            waitFor(() -> service.snapshot().optJSONObject("availability") != null && service.snapshot().optJSONObject("availability").optJSONArray("entries") != null && !service.snapshot().optBoolean("busy"), "availability measured");
            ObdService.autoSurvey = false;

            main.call(() -> { requireButton(activity.getWindow().getDecorView(), "Live").performClick(); requireButton(activity.getWindow().getDecorView(), "Messwerte auswählen").performClick(); return null; });
            Dialog selector = main.call(() -> screen.activeSignalSelector);
            View root = main.call(() -> selector.getWindow().getDecorView());

            // REQ-031.2 / .1: by default the signals known not to deliver are hidden and counted; answered ones show their state
            require(main.call(() -> hasTextContaining(root, "Verfügbarkeit: antwortet")), "REQ-031.1 an answered signal shows 'antwortet'");
            require(main.call(() -> !hasTextContaining(root, "Verfügbarkeit: lehnt ab") && !hasTextContaining(root, "Unterstützung: nicht unterstützt")), "REQ-031.2 measured-unavailable signals are hidden by default");
            require(main.call(() -> hasTextContaining(root, "nicht verfügbare Messwerte ausgeblendet")), "REQ-031.2 the selector says how many are hidden");

            // REQ-031.2: with the filter off everything is listed with its state
            main.call(() -> { requireButton(root, "Nur verfügbare: an").performClick(); return null; });
            require(main.call(() -> hasTextContaining(root, "Verfügbarkeit: lehnt ab (NRC 0x31)")), "REQ-031.1 a rejected signal shows its NRC");
            require(main.call(() -> hasTextContaining(root, "Verfügbarkeit: ohne Antwort")), "REQ-031.1 a silent signal shows 'ohne Antwort'");
            require(main.call(() -> hasTextContaining(root, "Unterstützung: nicht unterstützt")), "REQ-031.1 a PID the ECU does not report keeps its support label");
            require(main.call(() -> !hasTextContaining(root, "nicht verfügbare Messwerte ausgeblendet")), "REQ-031.2 nothing is hidden with the filter off");

            // REQ-031.3: every listed signal is selectable, including the unavailable ones
            int alreadySelected = service.snapshot().optJSONArray("selected").length();
            main.call(() -> { findCheckBox(root, "lehnt ab (NRC 0x31)", false).performClick(); findCheckBox(root, "nicht unterstützt", false).performClick(); return null; });
            main.call(() -> { findCheckBox(root, "Verfügbarkeit: antwortet", false).performClick(); requireButton(selector.getWindow().getDecorView(), "Übernehmen").performClick(); return null; });
            waitFor(() -> service.snapshot().optJSONArray("selected").length() == alreadySelected + 3, "three more signals selected");

            // REQ-031.4: tiles explain why there is no value
            waitFor(() -> main.call(() -> hasTextContaining(activity.getWindow().getDecorView(), "lehnt ab · NRC 0x31")
                && hasTextContaining(activity.getWindow().getDecorView(), "nicht unterstützt (Fahrzeug meldet PID nicht)")), "tiles state their reason");
            require(main.call(() -> hasTextContaining(activity.getWindow().getDecorView(), "Nicht verfügbar")), "REQ-031.4 'Nicht verfügbar' instead of a bare dash");
            JSONArray selected = service.snapshot().getJSONArray("selected");
            require(selected.length() == alreadySelected + 3, "REQ-031.3 the selection is exactly what was chosen: " + selected);
        } finally {
            ObdService.autoSurvey = false;
            service.disconnect(); service.connector = savedConnector; service.vehicles = savedStore; service.permissionOverride = savedPermission; service.bluetoothOverride = savedBluetooth;
            service.setDemo(); waitFor(() -> service.snapshot().optString("mode").equals("demo") && service.snapshot().optString("phase").equals("ready") && !service.snapshot().optBoolean("busy"), "demo ready");
            service.setAutoConnect(true);
            Set<String> created = names(recordings); created.removeAll(before);
            for (String n : created) new File(recordings, n).delete();
        }
    }

    private static CheckBox findCheckBox(View root, String needle, boolean byText) {
        if (root instanceof CheckBox) { CheckBox box = (CheckBox) root; CharSequence haystack = byText ? box.getText() : box.getContentDescription(); if (haystack != null && haystack.toString().contains(needle)) return box; }
        if (root instanceof ViewGroup) { ViewGroup group = (ViewGroup) root; for (int i = 0; i < group.getChildCount(); i++) { CheckBox found = findCheckBox(group.getChildAt(i), needle, byText); if (found != null) return found; } }
        if (root.getParent() == null || !(root instanceof ViewGroup)) { /* leaf */ }
        return null;
    }
    private static Button requireButton(View view, String label) {
        Button found = findButton(view, label); if (found == null) throw new AssertionError("Missing button: " + label); return found;
    }
    private static Button findButton(View view, String label) {
        if (view instanceof Button && label.contentEquals(((Button) view).getText())) return (Button) view;
        if (view instanceof ViewGroup) { ViewGroup group = (ViewGroup) view; for (int i = 0; i < group.getChildCount(); i++) { Button found = findButton(group.getChildAt(i), label); if (found != null) return found; } }
        return null;
    }
    private static boolean hasTextContaining(View view, String text) {
        if (view instanceof TextView && ((TextView) view).getText().toString().contains(text)) return true;
        if (view instanceof ViewGroup) { ViewGroup group = (ViewGroup) view; for (int i = 0; i < group.getChildCount(); i++) if (hasTextContaining(group.getChildAt(i), text)) return true; }
        return false;
    }
    private static Set<String> names(File dir) { String[] list = dir.list(); return list == null ? new HashSet<>() : new HashSet<>(java.util.Arrays.asList(list)); }
    private interface Condition { boolean ready() throws Exception; }
    private static void waitFor(Condition condition, String name) throws Exception {
        long until = SystemClock.elapsedRealtime() + 60000;
        while (SystemClock.elapsedRealtime() < until) { if (condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("Timeout: " + name);
    }
    private static void require(boolean condition, String message) { if (!condition) throw new AssertionError(message); }
}
