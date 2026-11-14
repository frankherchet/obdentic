package de.obdentic.spike.siwc;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.graphics.Typeface;
import android.os.Bundle;
import android.view.View;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import org.json.JSONObject;

/** Throwaway test harness: buttons or the launch extra {@code action} (signin, reauth, refresh, signout, status). */
public final class SpikeActivity extends Activity {
    private static final ExecutorService EXEC = Executors.newSingleThreadExecutor(r -> {
        Thread t = new Thread(r, "siwc-work");
        t.setDaemon(true);
        return t;
    });

    private TextView log;
    private ScrollView scroll;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setOnApplyWindowInsetsListener((v, insets) -> {
            v.setPadding(insets.getSystemWindowInsetLeft() + 16, insets.getSystemWindowInsetTop() + 16,
                    insets.getSystemWindowInsetRight() + 16, insets.getSystemWindowInsetBottom() + 16);
            return insets;
        });
        String[][] buttons = {
            {"Continue with ChatGPT", "signin"}, {"Erneut anmelden", "reauth"}, {"Token erneuern", "refresh"},
            {"Abmelden", "signout"}, {"Status", "status"}};
        for (String[] b : buttons) {
            Button btn = new Button(this);
            btn.setText(b[0]);
            btn.setAllCaps(false);
            final String action = b[1];
            btn.setOnClickListener((View v) -> run(action));
            root.addView(btn);
        }
        scroll = new ScrollView(this);
        log = new TextView(this);
        log.setTypeface(Typeface.MONOSPACE);
        log.setTextSize(11);
        scroll.addView(log);
        root.addView(scroll, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f));
        setContentView(root);
        Report.setListener(() -> runOnUiThread(this::render));
        render();
        if (savedInstanceState == null) handle(getIntent());
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        handle(intent);
    }

    @Override
    protected void onDestroy() {
        Report.setListener(null);
        super.onDestroy();
    }

    private void handle(Intent intent) {
        String action = intent == null ? null : intent.getStringExtra("action");
        if (action != null) run(action);
        // The deep link obdentic-siwc-spike://done carries no action: it only brings this activity to the front.
    }

    private void render() {
        log.setText(Report.all());
        scroll.post(() -> scroll.fullScroll(View.FOCUS_DOWN));
    }

    private void run(String action) {
        final Context ctx = this;
        final Context app = getApplicationContext();
        final boolean network = !action.equals("status");
        if (network) KeepAliveService.acquire(app); // activity is in front here; the service survives the browser
        EXEC.execute(() -> {
            try {
                switch (action) {
                    case "signin":
                        signin(ctx);
                        break;
                    case "reauth":
                        reauth(ctx);
                        break;
                    case "refresh":
                        refresh(ctx);
                        break;
                    case "signout":
                        signout(ctx);
                        break;
                    case "status":
                        status(ctx);
                        break;
                    default:
                        Report.line("action", "ERR", "unknown_action");
                }
            } catch (Throwable t) {
                Report.line("action", "ERR", action + " " + t.getClass().getSimpleName());
            } finally {
                if (network) KeepAliveService.release(app);
            }
        });
    }

    // ---------------------------------------------------------------- flows

    private static void signin(Context ctx) {
        JSONObject rec = Oauth.signIn(ctx, null);
        if (rec == null) return;
        Responses.runAll(rec.optString("access_token", ""));
    }

    private static void reauth(Context ctx) {
        JSONObject stored = load(ctx, "S14");
        if (stored == null) return;
        if (stored.optString("id_token", "").isEmpty() || stored.optString("client_id", "").isEmpty()) {
            Report.line("S14", "FAIL", "no_stored_session");
            return;
        }
        JSONObject rec = Oauth.signIn(ctx, stored);
        Report.line("S14", rec != null ? "PASS" : "FAIL", "callback_idtoken_sub_ok=" + (rec != null));
    }

    private static void status(Context ctx) {
        try {
            JSONObject rec = CredentialStore.load(ctx);
            if (rec == null) {
                Report.line("S6", "FAIL", "absent");
                return;
            }
            int refreshLen = rec.optString("refresh_token", "").length();
            Report.line("S6", refreshLen > 0 ? "PASS" : "FAIL", "decrypt=true refresh_len=" + refreshLen
                    + " access_len=" + rec.optString("access_token", "").length()
                    + " id_len=" + rec.optString("id_token", "").length()
                    + " client_id_present=" + !rec.optString("client_id", "").isEmpty());
        } catch (Exception e) {
            Report.line("S6", "FAIL", "decrypt " + e.getClass().getSimpleName());
        }
    }

    private static JSONObject load(Context ctx, String check) {
        try {
            JSONObject rec = CredentialStore.load(ctx);
            if (rec == null) Report.line(check, "FAIL", "absent");
            return rec;
        } catch (Exception e) {
            Report.line(check, "FAIL", "decrypt " + e.getClass().getSimpleName());
            return null;
        }
    }

    private static void refresh(Context ctx) throws Exception {
        JSONObject rec = load(ctx, "S12");
        if (rec == null) return;
        Oauth.Http h = Oauth.refreshCall(rec);
        if (!h.ok()) {
            Report.line("S12", "FAIL", h.summary());
            return;
        }
        JSONObject n = Oauth.applyRefresh(rec, new JSONObject(h.body));
        boolean changed = !n.getString("refresh_token").equals(rec.getString("refresh_token"));
        CredentialStore.save(ctx, n);
        boolean s8 = Responses.models(n.getString("access_token")) && Responses.s8(n.getString("access_token"));
        Report.line("S12", changed && s8 ? "PASS" : "FAIL", "http=" + h.status + " refresh_changed=" + changed
                + " s8=" + s8 + " expires_in=" + n.optLong("expires_in", 0)
                + " plan_scope=" + n.optString("scopes", "").contains(Oauth.PLAN_SCOPE));
    }

    private static void signout(Context ctx) throws Exception {
        JSONObject rec = load(ctx, "S13");
        if (rec == null) return;
        Oauth.Http rv = Oauth.revoke(rec);
        Oauth.Http reuse = Oauth.refreshCall(rec);
        boolean pass = rv.status == 200 && !reuse.ok();
        Report.line("S13", pass ? "PASS" : "FAIL", "revoke_http=" + rv.status + " reuse_" + reuse.summary());
        // Delete access, refresh and ID tokens locally; keep issued client id and host id.
        JSONObject kept = new JSONObject();
        kept.put("issuer", rec.optString("issuer", ""));
        kept.put("client_id", rec.optString("client_id", ""));
        kept.put("ext_agent_host_id", rec.optString("ext_agent_host_id", ""));
        kept.put("saved_at", System.currentTimeMillis());
        CredentialStore.save(ctx, kept);
        Report.line("signout", "INFO", "tokens_deleted=true");
    }
}
