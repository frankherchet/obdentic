package de.obdentic.app;

import android.app.Activity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;

/** User paths through the reorganized UI, for the acceptance classes. Every method runs on the main thread. */
final class UiNav {
    private UiNav() {}

    /** The labelled connection/mode button in the header opens the adapter view. */
    static void openAdapter(Activity a) { tagged(a, "context-adapter").performClick(); }

    /** The vehicle name in the header opens the vehicle list. */
    static void openVehicles(Activity a) { tagged(a, "context-vehicle").performClick(); }

    /** Diagnose: opens "Weitere Prüfungen" when it is closed; opening starts nothing. */
    static void openMoreChecks(Activity a) { Button b = button(a.getWindow().getDecorView(), "Weitere Prüfungen anzeigen"); if (b != null) b.performClick(); }

    /** Diagnose: closes "Weitere Prüfungen" when it is open. */
    static void closeMoreChecks(Activity a) { Button b = button(a.getWindow().getDecorView(), "Weitere Prüfungen ausblenden"); if (b != null) b.performClick(); }

    static View tagged(Activity a, String tag) {
        View v = a.getWindow().getDecorView().findViewWithTag(tag);
        if (v == null) throw new AssertionError("view tagged " + tag);
        return v;
    }

    static Button button(View view, String text) {
        if (view instanceof Button && text.contentEquals(((Button) view).getText())) return (Button) view;
        if (view instanceof ViewGroup) { ViewGroup g = (ViewGroup) view; for (int i = 0; i < g.getChildCount(); i++) { Button f = button(g.getChildAt(i), text); if (f != null) return f; } }
        return null;
    }
}
