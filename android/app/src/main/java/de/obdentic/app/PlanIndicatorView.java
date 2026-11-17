package de.obdentic.app;

import android.content.Context;
import android.graphics.Color;
import android.graphics.Paint;
import android.view.Gravity;
import android.widget.LinearLayout;
import android.widget.TextView;

/**
 * REQ-042.10: the OpenAI UI guideline pair 'Using ChatGPT plan' and the 'Manage usage' link, as one view that the account section
 * shows now and the assistant screen can place near its composer later. Visible only while the plan is connected and usable.
 */
final class PlanIndicatorView extends LinearLayout {
    static final String INDICATOR = "Using ChatGPT plan", MANAGE = "Manage usage";
    private static final int TEXT = 0xffedf3fa, ACCENT = 0xff63dfc1;

    PlanIndicatorView(Context context, ChatGptAccount account) {
        super(context);
        setOrientation(HORIZONTAL);
        setGravity(Gravity.CENTER_VERTICAL);
        TextView indicator = new TextView(context);
        indicator.setText("● " + INDICATOR);
        indicator.setTextColor(ACCENT);
        indicator.setTextSize(14);
        addView(indicator, new LayoutParams(0, LayoutParams.WRAP_CONTENT, 1));
        TextView link = new TextView(context);
        link.setText(MANAGE);
        link.setTextColor(TEXT);
        link.setTextSize(14);
        link.setPaintFlags(link.getPaintFlags() | Paint.UNDERLINE_TEXT_FLAG);
        link.setMinHeight(Math.round(48 * context.getResources().getDisplayMetrics().density));
        link.setGravity(Gravity.CENTER_VERTICAL);
        link.setContentDescription(MANAGE + ": ChatGPT-Nutzung in den Einstellungen verwalten");
        link.setClickable(true);
        link.setOnClickListener(v -> {
            if (!account.openUsage()) link.setText(MANAGE + " · kein Browser verfügbar");
        });
        addView(link);
        setBackgroundColor(Color.TRANSPARENT);
    }
}
