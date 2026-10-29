package de.obdentic.app;

import android.app.Activity;
import android.app.Dialog;
import android.content.Context;
import java.io.File;
import java.util.ArrayList;
import org.json.JSONArray;
import org.json.JSONObject;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.TextView;

/** REQ-025.4: named signal presets stay on the vehicle record and round-trip through disk. */
public final class SignalSelectorAcceptance {
    public interface OnMain { <T> T call(java.util.concurrent.Callable<T> call) throws Exception; }

    public static void run(Context context) throws Exception {
        require("ASCII: 123 · Bytes: 31 32 33".equals(MainActivity.identityValue(new JSONObject().put("value",new JSONArray().put(49).put(50).put(51)))), "REQ-023.5 identity bytes have a labelled printable preview");
        require("Bytes: 00 FF".equals(MainActivity.identityValue(new JSONObject().put("value",new JSONArray().put(0).put(255)))), "REQ-023.5 binary identity is never guessed as text");
        require(MainActivity.identityValue(new JSONObject()).isEmpty(), "REQ-023.5 missing identity is not the text null");
        require("nicht abgeglichen".equals(MainActivity.knowledgeValidationBadge(
            new JSONObject().put("definition_id", "vag.ea189.example").put("validation", "not_validated"))),
            "REQ-024.3 Knowledge tiles label unvalidated definitions as nicht abgeglichen");
        require("UDS · erweiterte Sitzung".equals(MainActivity.signalProtocolLabel(
            new JSONObject().put("protocol", "VW").put("extended_session_required", true), true)),
            "REQ-027.2 session-flagged VAG signal is labelled UDS even when the base protocol is VW");
        require("VW".equals(MainActivity.signalProtocolLabel(new JSONObject().put("protocol", "VW"), true)),
            "REQ-027.2 ordinary VAG Knowledge signal keeps its VW label");
        File path = File.createTempFile("signal-selector-", ".json", context.getCacheDir());
        path.delete();
        try {
            VehicleStore store = new VehicleStore(path);
            String vin = "TESTVIN1234567890";
            require("created".equals(store.observe(new JSONObject().put("vin", vin), 1)), "REQ-025.4 creates a private test record");
            JSONArray semantics = new JSONArray().put("engine.rpm").put("vehicle.speed");
            require(store.savePreset(vin, "Fahrt", semantics) == null, "REQ-025.4 saves a named preset");
            require(store.presets(vin).length() == 1, "REQ-025.4 lists the saved preset");
            require(semantics.toString().equals(store.preset(vin, "Fahrt").toString()), "REQ-025.4 returns saved semantics");
            JSONObject discovery = new JSONObject().put("targets", new JSONArray().put(new JSONObject().put("status", "not_observed")));
            JSONObject learning = new JSONObject().put("results", new JSONArray().put(new JSONObject().put("status", "answered")));
            require(store.setDiagnostic(vin, "ecu_discovery", discovery) == null, "REQ-023.5 stores ECU discovery on its vehicle");
            require(store.setDiagnostic(vin, "knowledge_learning", learning) == null, "REQ-026.2 stores learning results on its vehicle");
            require("Unbekanntes Diagnoseergebnis".equals(store.setDiagnostic(vin, "arbitrary", discovery)), "REQ-023.5 refuses an unrecognized result key");

            VehicleStore reopened = new VehicleStore(path);
            require(semantics.toString().equals(reopened.preset(vin, "Fahrt").toString()), "REQ-025.4 preset survives store reload");
            JSONObject row = vehicle(reopened.snapshot(), vin);
            require("not_observed".equals(row.optJSONObject("ecu_discovery").optJSONArray("targets").optJSONObject(0).optString("status")), "REQ-023.5 ECU discovery survives store reload");
            require("answered".equals(row.optJSONObject("knowledge_learning").optJSONArray("results").optJSONObject(0).optString("status")), "REQ-026.2 learning result survives store reload");
            require("Fahrzeug nicht gefunden".equals(reopened.savePreset("MISSING", "Fahrt", semantics)), "REQ-025.4 refuses a preset without a vehicle record");
            JSONArray tooMany = new JSONArray();
            for (int i = 0; i < 33; i++) tooMany.put("signal." + i);
            require("Ein Preset darf höchstens 32 Messwerte enthalten".equals(reopened.savePreset(vin, "Zu groß", tooMany)), "REQ-025.3 caps saved selections at 32");
        } finally {
            new android.util.AtomicFile(path).delete();
            new File(path.getPath() + ".new").delete();
        }
    }

    /** REQ-025.1/.2/.3/.5: exercise the actual full-screen selector against the synthetic demo catalog. */
    public static void verifyUi(OnMain main, Activity activity, ObdService service) throws Exception {
        MainActivity screen = (MainActivity) activity;
        JSONArray original = copy(service.snapshot().optJSONArray("selected"));
        service.setSignals(new JSONArray());
        try {
            main.call(() -> {
                requireButton(activity.getWindow().getDecorView(), "Live").performClick();
                requireButton(activity.getWindow().getDecorView(), "Messwerte auswählen").performClick();
                return null;
            });
            Dialog dialog = main.call(() -> screen.activeSignalSelector);
            require(dialog != null && dialog.isShowing(), "REQ-025.1 selector opens as a visible dialog");
            View root = main.call(() -> dialog.getWindow().getDecorView());
            JSONArray catalog = service.snapshot().optJSONArray("signals");
            require(catalog != null && catalog.length() >= 33, "REQ-025.3 synthetic catalog has at least 33 selectable signals");
            JSONObject first = catalog.optJSONObject(0);
            String semantic = first.optString("semantic");
            String shortName = SignalNames.shortName(semantic, first.optString("title", semantic));
            String group = group(first);
            String unit = MeasurementFormat.displayUnit(first.optString("unit"));
            require(hasText(root, "OBD-Standard"), "REQ-025.1 shows the standard OBD section");
            require(hasText(root, group), "REQ-025.1 shows a signal group");
            require(hasTextContaining(root, "Unterstützung:") && hasTextContaining(root, "Validierung:"),
                "REQ-025.1 shows support and validation state");
            require(hasCheckbox(root, shortName) && (unit.isEmpty() || hasCheckbox(root, shortName + " · " + unit)),
                "REQ-025.1 lists a signal by short name and unit");

            EditText search = find(root, EditText.class);
            require(search != null, "REQ-025.2 search field is visible");
            setSearch(main, search, shortName);
            require(hasCheckbox(main.call(() -> dialog.getWindow().getDecorView()), shortName),
                "REQ-025.2 search matches a short name");
            setSearch(main, search, group);
            require(checkBoxes(main.call(() -> dialog.getWindow().getDecorView())).size() > 0,
                "REQ-025.2 search matches a group");
            if (!unit.isEmpty()) {
                setSearch(main, search, unit);
                require(checkBoxes(main.call(() -> dialog.getWindow().getDecorView())).size() > 0,
                    "REQ-025.2 search matches a unit");
            }
            setSearch(main, search, "");
            root = main.call(() -> dialog.getWindow().getDecorView());
            require(hasText(root, "0 / 32 gewählt"), "REQ-025.3 shows the current selection count");

            main.call(() -> { requireButton(dialog.getWindow().getDecorView(), "Alle").performClick(); return null; });
            root = main.call(() -> dialog.getWindow().getDecorView());
            int afterAll = selectionCount(root);
            require(afterAll > 0 && afterAll <= 32, "REQ-025.3 group All selects within the limit");
            main.call(() -> { requireButton(dialog.getWindow().getDecorView(), "Keine").performClick(); return null; });
            root = main.call(() -> dialog.getWindow().getDecorView());
            require(selectionCount(root) == 0, "REQ-025.3 group None clears that group");

            ArrayList<CheckBox> checks = checkBoxes(root);
            require(checks.size() >= 33, "REQ-025.3 at least 33 checkboxes are available in the selector");
            for (int i = 0; i < 32; i++) {
                CheckBox check = checks.get(i);
                main.call(() -> { check.performClick(); return null; });
            }
            CheckBox thirtyThird = checks.get(32);
            main.call(() -> { thirtyThird.performClick(); return null; });
            root = main.call(() -> dialog.getWindow().getDecorView());
            require(selectionCount(root) == 32 && hasText(root, "Maximal 32 Messwerte · Auswahl nicht erweitert"),
                "REQ-025.3 visibly refuses the 33rd selection");
            main.call(() -> { requireButton(dialog.getWindow().getDecorView(), "Abbrechen").performClick(); return null; });

            service.setSignals(original);
            require(original.length() > 0, "REQ-024 selected-signal recording has a selection");
            main.call(() -> { requireButton(activity.getWindow().getDecorView(), "Gewählte Messwerte aufzeichnen").performClick(); return null; });
            waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "synthetic recording starts for selector lock");
            require(original.toString().equals(service.snapshot().optJSONArray("selected").toString()),
                "REQ-024 selected-signal recording preserves the current selection");
            waitFor(() -> main.call(() -> hasTextContaining(activity.getWindow().getDecorView(), "Aufzeichnung läuft")),
                "live screen observes the recording lock");
            String before = service.snapshot().optJSONArray("selected").toString();
            main.call(() -> { requireButton(activity.getWindow().getDecorView(), "Messwerte auswählen").performClick(); return null; });
            require(screen.activeSignalSelector == null, "REQ-025.5 selector does not open during recording");
            require(before.equals(service.snapshot().optJSONArray("selected").toString()),
                "REQ-025.5 recording keeps its signal selection unchanged");
        } finally {
            Dialog active = screen.activeSignalSelector;
            if (active != null) main.call(() -> { active.dismiss(); return null; });
            if (!service.snapshot().optString("recording").isEmpty()) {
                service.stopRecording();
                waitFor(() -> service.snapshot().optString("recording").isEmpty(), "synthetic recording stops after selector lock");
            }
            service.setSignals(original);
        }
    }

    private static void setSearch(OnMain main, EditText search, String query) throws Exception {
        main.call(() -> { search.setText(query); return null; });
    }

    private static JSONArray copy(JSONArray source) throws Exception {
        return source == null ? new JSONArray() : new JSONArray(source.toString());
    }

    private static String group(JSONObject signal) {
        String value = field(signal, "group");
        if (value.isEmpty()) value = field(signal, "subsystem");
        return value.isEmpty() ? "Allgemein" : value;
    }

    private static String field(JSONObject signal, String key) {
        Object value = signal.opt(key);
        if (value instanceof JSONObject) {
            JSONObject object = (JSONObject) value;
            return object.optString("status", object.optString("state", object.optString("name", "")));
        }
        return value == null || JSONObject.NULL.equals(value) ? "" : String.valueOf(value);
    }

    private static int selectionCount(View root) {
        TextView count = findTextContaining(root, " / 32 gewählt");
        if (count == null) return -1;
        try { return Integer.parseInt(count.getText().toString().split(" /", 2)[0]); }
        catch (NumberFormatException e) { return -1; }
    }

    private static boolean hasCheckbox(View root, String text) {
        for (CheckBox box : checkBoxes(root)) if (box.getText().toString().contains(text)) return true;
        return false;
    }

    private static ArrayList<CheckBox> checkBoxes(View root) {
        ArrayList<CheckBox> found = new ArrayList<>(); collect(root, CheckBox.class, found); return found;
    }

    private static <T extends View> ArrayList<T> findAll(View root, Class<T> type) {
        ArrayList<T> found = new ArrayList<>(); collect(root, type, found); return found;
    }

    private static <T extends View> void collect(View view, Class<T> type, ArrayList<T> found) {
        if (type.isInstance(view)) found.add(type.cast(view));
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collect(group.getChildAt(i), type, found);
        }
    }

    private static <T extends View> T find(View root, Class<T> type) {
        ArrayList<T> found = findAll(root, type); return found.isEmpty() ? null : found.get(0);
    }

    private static boolean hasText(View root, String wanted) {
        for (TextView text : findAll(root, TextView.class)) if (wanted.contentEquals(text.getText())) return true;
        return false;
    }

    private static boolean hasTextContaining(View root, String wanted) {
        return findTextContaining(root, wanted) != null;
    }

    private static TextView findTextContaining(View root, String wanted) {
        for (TextView text : findAll(root, TextView.class)) if (text.getText().toString().contains(wanted)) return text;
        return null;
    }

    private static Button requireButton(View root, String label) {
        for (Button button : findAll(root, Button.class)) if (label.contentEquals(button.getText())) return button;
        throw new AssertionError("Missing button: " + label);
    }

    private static void waitFor(Check condition, String label) throws Exception {
        long deadline = SystemClock.elapsedRealtime() + 5000;
        while (!condition.ok() && SystemClock.elapsedRealtime() < deadline) SystemClock.sleep(50);
        require(condition.ok(), label);
    }

    private interface Check { boolean ok() throws Exception; }

    private static void require(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static JSONObject vehicle(JSONObject store, String vin) {
        JSONArray rows = store.optJSONArray("vehicles");
        for (int i = 0; rows != null && i < rows.length(); i++) {
            JSONObject row = rows.optJSONObject(i);
            if (row != null && vin.equals(row.optString("vin"))) return row;
        }
        throw new AssertionError("Vehicle record missing");
    }
}
