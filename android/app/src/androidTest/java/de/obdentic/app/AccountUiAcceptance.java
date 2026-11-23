package de.obdentic.app;

import static de.obdentic.app.AccountFixtures.*;

import android.app.Activity;
import android.app.AlertDialog;
import android.app.Instrumentation;
import android.app.Notification;
import android.app.NotificationManager;
import android.app.UiAutomation;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ServiceInfo;
import android.net.Uri;
import android.os.Build;
import android.os.SystemClock;
import android.provider.Settings;
import android.system.ErrnoException;
import android.system.OsConstants;
import android.view.View;
import android.view.ViewGroup;
import android.view.accessibility.AccessibilityNodeInfo;
import android.widget.Button;
import android.widget.TextView;
import java.io.File;
import java.io.IOException;
import java.net.BindException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Iterator;
import java.util.List;
import java.util.concurrent.Callable;
import java.util.concurrent.FutureTask;
import org.json.JSONObject;

/**
 * #327 through the real MainActivity: the account section (REQ-042.1, .5, .10, .11, .12), the disconnect confirmation (REQ-043.10), the
 * sign-in service (REQ-044.5) and the unchanged vehicle service (REQ-044.4). Fake authorization server and browser; no real account.
 */
final class AccountUiAcceptance {
    private static final List<String> STATUSES = Arrays.asList("Nicht verbunden", "Verbunden", ChatGptAccount.MSG_EXPIRED, "ChatGPT nicht erreichbar", "ChatGPT-Plan-Nutzung nicht freigegeben");

    private AccountUiAcceptance() {}

    private static <T> T onMain(Instrumentation inst, Callable<T> call) throws Exception {
        FutureTask<T> task = new FutureTask<>(call);
        inst.runOnMainSync(task);
        return task.get();
    }

    private static boolean visible(View view) {
        for (View v = view; v != null && v.getId() != android.R.id.content; ) {
            if (v.getVisibility() != View.VISIBLE) return false;
            v = v.getParent() instanceof View ? (View) v.getParent() : null;
        }
        return true;
    }

    private static void collect(View view, List<TextView> out) {
        if (view instanceof TextView && visible(view)) out.add((TextView) view);
        if (view instanceof ViewGroup) { ViewGroup g = (ViewGroup) view; for (int i = 0; i < g.getChildCount(); i++) collect(g.getChildAt(i), out); }
    }

    private static List<TextView> views(Instrumentation inst, Activity a) throws Exception {
        return onMain(inst, () -> { List<TextView> out = new ArrayList<>(); collect(a.getWindow().getDecorView(), out); return out; });
    }

    private static String all(Instrumentation inst, Activity a) throws Exception {
        return onMain(inst, () -> { StringBuilder sb = new StringBuilder(); for (TextView t : views(a)) sb.append(t.getText()).append('\n'); return sb.toString(); });
    }

    private static List<TextView> views(Activity a) {
        List<TextView> out = new ArrayList<>();
        collect(a.getWindow().getDecorView(), out);
        return out;
    }

    private static List<Button> buttons(Activity a, String text) {
        List<Button> out = new ArrayList<>();
        for (TextView t : views(a)) if (t instanceof Button && text.contentEquals(t.getText())) out.add((Button) t);
        return out;
    }

    private static int count(Instrumentation inst, Activity a, String text) throws Exception { return onMain(inst, () -> buttons(a, text).size()); }

    private static void click(Instrumentation inst, Activity a, String text) throws Exception {
        onMain(inst, () -> { List<Button> b = buttons(a, text); require(!b.isEmpty(), "button '" + text + "' exists"); b.get(0).performClick(); return null; });
    }

    private static void waitText(Instrumentation inst, Activity a, String text, boolean present, String what) throws Exception {
        waitUntil(() -> all(inst, a).contains(text) == present, 8000, what + " ('" + text + "' " + (present ? "shown" : "gone") + ")");
    }

    /** The one status among the five (REQ-042.10). */
    private static String status(Instrumentation inst, Activity a) throws Exception {
        return onMain(inst, () -> {
            List<String> found = new ArrayList<>();
            for (TextView t : views(a)) if (STATUSES.contains(t.getText().toString())) found.add(t.getText().toString());
            require(found.size() == 1, "REQ-042.10 exactly one status is shown: " + found);
            return found.get(0);
        });
    }

    private static void openAccountSection(Instrumentation inst, Activity a) throws Exception {
        onMain(inst, () -> { UiNav.openAdapter(a); return null; }); // the labelled connection header
        click(inst, a, "Konto öffnen"); // REQ-042.1: two taps from the main view
        waitText(inst, a, "ChatGPT-Konto", true, "account section");
    }

    private static AccessibilityNodeInfo dialogButton(UiAutomation ui, String text) {
        AccessibilityNodeInfo root = ui.getRootInActiveWindow();
        if (root == null) return null;
        for (AccessibilityNodeInfo node : root.findAccessibilityNodeInfosByText(text)) {
            if (node.getText() != null && text.equalsIgnoreCase(node.getText().toString()) && node.isClickable()) return node;
        }
        return null;
    }

    private static void returnFromSettings(UiAutomation ui, MainActivity activity) throws Exception {
        // the back key must reach Settings, not the app's still-focused window during the transition
        waitUntil(() -> { AccessibilityNodeInfo root = ui.getRootInActiveWindow(); return root != null && !activity.getPackageName().contentEquals(root.getPackageName()); }, 8000, "#352 settings in front");
        try (android.os.ParcelFileDescriptor ignored = ui.executeShellCommand("input keyevent 4")) {
            waitUntil(() -> MainActivity.resumed == activity, 8000, "#352 return from settings");
        }
    }

    /** #352 / REQ-042.13: denied sockets offer OS settings, not for unrelated bind errors. Returning does not restart OAuth. */
    private static void networkSettings(Instrumentation inst, MainActivity activity, Fixture f, Context ctx) throws Exception {
        UiAutomation ui = inst.getUiAutomation();
        try {
            f.config.opener = (p, t, r) -> { throw new IOException(new ErrnoException("bind", OsConstants.EADDRINUSE)); };
            click(inst, activity, "Continue with ChatGPT");
            waitText(inst, activity, "Lokaler Port für die Anmeldung belegt", true, "#352 occupied port is reported separately");
            require(dialogButton(ui, "Einstellungen öffnen") == null, "#352 no settings dialog for a port conflict");
            f.config.opener = (p, t, r) -> { throw new IOException("socket unavailable"); };
            click(inst, activity, "Continue with ChatGPT");
            waitText(inst, activity, "Lokaler Anmeldedienst konnte nicht gestartet werden", true, "#352 unknown I/O failure is not called a port conflict");
            require(dialogButton(ui, "Einstellungen öffnen") == null, "#352 no settings dialog for an unknown I/O failure");

            for (int errno : new int[]{OsConstants.EPERM, OsConstants.EACCES}) {
                f.config.opener = (p, t, r) -> {
                    BindException denied = new BindException("socket unavailable");
                    denied.initCause(new ErrnoException("socket", errno));
                    throw denied;
                };
                click(inst, activity, "Continue with ChatGPT");
                waitUntil(() -> dialogButton(ui, "Einstellungen öffnen") != null, 8000, "#352 denied socket offers settings");
                require(f.browser.opened.isEmpty() && f.auth.requests.isEmpty(), "#352 no OAuth with a denied socket");
                if (errno == OsConstants.EPERM) {
                    inst.runOnMainSync(() -> activity.startActivity(new Intent(Settings.ACTION_SETTINGS)));
                    waitUntil(() -> MainActivity.resumed != activity, 8000, "#352 app moved to background");
                    returnFromSettings(ui, activity);
                    waitUntil(() -> dialogButton(ui, "Einstellungen öffnen") != null, 8000, "#352 unresolved dialog returns");
                }
                String action = errno == OsConstants.EPERM ? "Abbrechen" : "Einstellungen öffnen";
                AccessibilityNodeInfo button = dialogButton(ui, action);
                require(button != null && button.performAction(AccessibilityNodeInfo.ACTION_CLICK), "#352 " + action);
                if (errno == OsConstants.EACCES) {
                    String appLabel = ctx.getApplicationInfo().loadLabel(ctx.getPackageManager()).toString();
                    waitUntil(() -> {
                        AccessibilityNodeInfo root = ui.getRootInActiveWindow();
                        return root != null && !ctx.getPackageName().contentEquals(root.getPackageName())
                            && !root.findAccessibilityNodeInfosByText(appLabel).isEmpty();
                    }, 8000, "#352 opens system settings");
                    returnFromSettings(ui, activity);
                }
                waitUntil(() -> dialogButton(ui, "Einstellungen öffnen") == null, 8000, "#352 prompt dismissed");
                require(count(inst, activity, "Continue with ChatGPT") == 1 && f.browser.opened.isEmpty()
                    && f.auth.requests.isEmpty(), "#352 only another tap retries sign-in");
            }
        } finally {
            f.config.opener = LoopbackListener::open;
        }
    }

    static void run(Instrumentation inst, MainActivity activity, ObdService service, Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        ChatGptAccount.install(f.account);
        JSONObject vehicleBefore = service.snapshot();
        try {
            openAccountSection(inst, activity);
            String text = all(inst, activity);
            require(text.contains("Use your ChatGPT plan") && text.contains("keine Fahrzeugdaten") && text.contains("ohne Konto") && text.contains("ChatGPT-Plan"), "REQ-042.1 the card explains the plan, no vehicle data and use without an account: " + text);
            require(count(inst, activity, "Continue with ChatGPT") == 1 && status(inst, activity).equals("Nicht verbunden"), "REQ-042.1 one button 'Continue with ChatGPT'; REQ-042.10 'Nicht verbunden'");
            require(!service.snapshot().optString("phase").equals("ready") || service.snapshot().optString("mode").equals("demo"), "REQ-042.1 no vehicle connection needed");
            require(f.auth.requests.isEmpty(), "REQ-044.1 opening the section sends no request");
            networkSettings(inst, activity, f, ctx);

            // REQ-042.5, REQ-042.12: waiting state, pending feedback, second tap ignored, cancel
            f.browser.callback = null;
            click(inst, activity, "Continue with ChatGPT");
            require(onMain(inst, () -> { List<Button> b = buttons(activity, "Anmeldung läuft …"); return b.size() == 1 && !b.get(0).isEnabled() && "Anmeldung läuft …".contentEquals(b.get(0).getContentDescription()); }),
                "REQ-042.12 immediate pending feedback on the sign-in button");
            waitText(inst, activity, "Warte auf den Browser", true, "REQ-042.5 waiting state");
            onMain(inst, () -> { buttons(activity, "Anmeldung läuft …").get(0).performClick(); return null; });
            waitUntil(() -> f.browser.opened.size() == 1, 5000, "browser opened");
            SystemClock.sleep(300);
            require(f.browser.opened.size() == 1, "REQ-042.12 a second tap during an attempt is ignored");
            require(count(inst, activity, "Anmeldung abbrechen") == 1, "REQ-042.5 a cancel button while waiting");
            long t0 = SystemClock.elapsedRealtime();
            click(inst, activity, "Anmeldung abbrechen");
            waitText(inst, activity, "Warte auf den Browser", false, "REQ-042.5 cancel ends the wait");
            require(SystemClock.elapsedRealtime() - t0 < 1500 && count(inst, activity, "Continue with ChatGPT") == 1, "REQ-042.5 cancel within 1 s, sign-in offered again");

            // REQ-042.12, .10, .11: connect
            f.browser.callback = FakeBrowser::approve;
            click(inst, activity, "Continue with ChatGPT");
            waitText(inst, activity, "Using ChatGPT plan", true, "connected indicator");
            require(status(inst, activity).equals("Verbunden") && all(inst, activity).contains("tester@example.test"), "REQ-042.10 'Verbunden' with the e-mail address");
            require(count(inst, activity, "Continue with ChatGPT") == 0 && count(inst, activity, "Trennen …") == 1, "REQ-042.12 while connected 'Continue with ChatGPT' is not offered");
            waitUntil(() -> activity.firstUseDialog != null && activity.firstUseDialog.isShowing(), 8000, "REQ-042.11 the one-time dialog");
            AlertDialog dialog = (AlertDialog) activity.firstUseDialog;
            String message = onMain(inst, () -> ((TextView) dialog.findViewById(android.R.id.message)).getText().toString());
            int titleId = ctx.getResources().getIdentifier("alertTitle", "id", "android");
            String title = onMain(inst, () -> ((TextView) dialog.findViewById(titleId)).getText().toString());
            require(title.equals("You're using your ChatGPT plan") && message.contains("ChatGPT-Plan") && message.contains("ChatGPT-Einstellungen") && "Got it".contentEquals(dialog.getButton(AlertDialog.BUTTON_POSITIVE).getText()), "REQ-042.11 dialog content: " + title + " / " + message);
            onMain(inst, () -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick());
            waitUntil(() -> !f.account.firstUseDue() && activity.firstUseDialog == null, 5000, "REQ-042.11 'Got it' closes the dialog for good");
            SystemClock.sleep(1200);
            require(activity.firstUseDialog == null, "REQ-042.11 the dialog does not come back");

            // REQ-042.10 Manage usage
            int before = f.usageOpened.size();
            onMain(inst, () -> { for (TextView t : views(activity)) if ("Manage usage".contentEquals(t.getText())) { t.performClick(); return null; } throw new AssertionError("Manage usage link"); });
            require(f.usageOpened.size() == before + 1 && f.usageOpened.get(before).equals("https://chatgpt.com/settings/usage"), "REQ-042.10 'Manage usage' opens the ChatGPT usage settings");
            f.browser.noBrowser = true;
            onMain(inst, () -> { for (TextView t : views(activity)) if (t.getText().toString().startsWith("Manage usage")) { t.performClick(); return null; } throw new AssertionError("Manage usage link"); });
            waitText(inst, activity, "kein Browser verfügbar", true, "REQ-042.10 no browser for Manage usage");
            f.browser.noBrowser = false;
            PlanIndicatorView reusable = onMain(inst, () -> new PlanIndicatorView(ctx, f.account));
            require(onMain(inst, () -> { List<TextView> t = new ArrayList<>(); collect(reusable, t); return t.size() == 2 && t.get(0).getText().toString().contains("Using ChatGPT plan"); }), "REQ-042.10 the indicator and link are one reusable view");

            // REQ-043.10: disconnecting asks first and gives pending feedback
            java.util.concurrent.CountDownLatch release = latch();
            f.auth.revoke = r -> { try { release.await(); } catch (InterruptedException e) { throw new java.io.IOException(e); } return json(200, "{}"); };
            click(inst, activity, "Trennen …");
            waitUntil(() -> activity.disconnectDialog != null && activity.disconnectDialog.isShowing(), 5000, "REQ-043.10 confirmation dialog");
            require(f.account.snapshot().status == ChatGptAccount.Status.CONNECTED && f.auth.count(REVOKE_URL) == 0, "REQ-043.10 nothing happens before the confirmation");
            AlertDialog confirm = (AlertDialog) activity.disconnectDialog;
            onMain(inst, () -> confirm.getButton(AlertDialog.BUTTON_NEGATIVE).performClick());
            SystemClock.sleep(300);
            require(f.account.snapshot().status == ChatGptAccount.Status.CONNECTED && f.auth.count(REVOKE_URL) == 0, "REQ-043.10 'Abbrechen' keeps the account");
            click(inst, activity, "Trennen …");
            waitUntil(() -> activity.disconnectDialog != null && activity.disconnectDialog.isShowing(), 5000, "confirmation dialog again");
            AlertDialog confirm2 = (AlertDialog) activity.disconnectDialog;
            onMain(inst, () -> confirm2.getButton(AlertDialog.BUTTON_POSITIVE).performClick());
            waitText(inst, activity, "Trenne …", true, "REQ-043.10 pending feedback while revoking");
            require(onMain(inst, () -> { List<Button> b = buttons(activity, "Trenne …"); return b.size() == 1 && !b.get(0).isEnabled(); }), "REQ-042.12 the disconnect button is disabled while pending");
            release.countDown();
            waitText(inst, activity, "Abgemeldet · Zugriff widerrufen", true, "REQ-043.10 sign-out result");
            require(status(inst, activity).equals("Nicht verbunden") && count(inst, activity, "Continue with ChatGPT") == 1, "REQ-043.10 not connected again");
            f.auth.revoke = null;

            // REQ-042.9, REQ-042.10: a sign-in without the plan permission stays inactive
            f.auth.scope = "openid profile email offline_access resource.invoke";
            click(inst, activity, "Continue with ChatGPT");
            waitText(inst, activity, "ChatGPT-Plan-Nutzung nicht freigegeben", true, "REQ-042.9 inactive state");
            require(status(inst, activity).equals("ChatGPT-Plan-Nutzung nicht freigegeben") && all(inst, activity).contains("tester@example.test") && !all(inst, activity).contains("Using ChatGPT plan")
                && count(inst, activity, "Continue with ChatGPT") == 1 && count(inst, activity, "Trennen …") == 1, "REQ-042.9 e-mail shown, no plan indicator, re-authorization and disconnect available");
            SystemClock.sleep(1200);
            require(activity.firstUseDialog == null, "REQ-042.11 no dialog without the plan permission");
            f.auth.scope = ChatGptAccount.SCOPE;
            click(inst, activity, "Continue with ChatGPT");
            waitText(inst, activity, "Using ChatGPT plan", true, "plan activated");
            require("consent".equals(f.browser.authorizations.get(f.browser.authorizations.size() - 1).get("prompt")), "REQ-042.9 the button repeats with prompt=consent");

            // REQ-043.8, REQ-043.7 in the section
            f.auth.clock.addAndGet(3500 * 1000L);
            f.auth.token = r -> { throw new java.io.IOException("down"); };
            new Thread(() -> f.account.accessToken()).start();
            waitText(inst, activity, "ChatGPT nicht erreichbar", true, "REQ-043.8 unreachable status");
            status(inst, activity);
            f.auth.token = r -> json(400, "{\"error\":\"invalid_grant\"}");
            new Thread(() -> f.account.accessToken()).start();
            waitText(inst, activity, ChatGptAccount.MSG_EXPIRED, true, "REQ-043.7 expired status");
            require(status(inst, activity).equals(ChatGptAccount.MSG_EXPIRED) && count(inst, activity, "Continue with ChatGPT") == 1, "REQ-043.7 sign in again is offered");
            f.auth.token = null;

            // REQ-042.6 the link takes no value from the intent that opens it
            int requests = f.auth.requests.size();
            long version = f.account.snapshot().version;
            ctx.startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse("obdentic://account?code=SECRET&access_token=ZZZ&state=x")).putExtra("code", "SECRET").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            waitUntil(() -> MainActivity.resumed != null, 8000, "REQ-042.6 the app is in front after the return link");
            SystemClock.sleep(800);
            require(f.auth.requests.size() == requests && f.account.snapshot().version == version && f.account.snapshot().status == ChatGptAccount.Status.EXPIRED, "REQ-042.6 the return link starts nothing and changes nothing");
        } finally {
            ChatGptAccount.install(null);
            f.close();
        }
        unconfirmedUsageAction(inst, activity, ctx);
        signInService(inst, activity, ctx);
        JSONObject vehicleAfter = service.snapshot();
        require(keys(vehicleBefore).equals(keys(vehicleAfter)), "REQ-044.4 the vehicle service snapshot has the same capabilities with and without account activity: " + keys(vehicleBefore) + " vs " + keys(vehicleAfter));
    }

    /** REQ-043.11: the cleanup link remains actionable after disconnect, without a connected-plan indicator. */
    private static void unconfirmedUsageAction(Instrumentation inst, Activity activity, Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            f.connect();
            f.account.firstUseAcknowledged();
            f.auth.clock.addAndGet(3500 * 1000L);
            f.auth.token = r -> { f.store.failSave = true; return f.auth.issue(r); };
            f.account.accessToken();
            f.store.failSave = false;
            ChatGptAccount restarted = f.restarted();
            ChatGptAccount.install(restarted);
            restarted.disconnect();
            waitUntil(() -> restarted.snapshot().phase == ChatGptAccount.Phase.IDLE, 15000, "uncertain disconnect");
            openAccountSection(inst, activity);
            waitText(inst, activity, "Widerruf nicht bestätigt", true, "REQ-043.11 uncertainty visible");
            require(!all(inst, activity).contains("Using ChatGPT plan") && count(inst, activity, "Manage usage") == 1,
                "REQ-043.11 disconnected cleanup action without plan indicator");
            click(inst, activity, "Manage usage");
            require(f.usageOpened.size() == 1 && f.usageOpened.get(0).equals("https://chatgpt.com/settings/usage"),
                "REQ-043.11 cleanup action opens the usage settings");
        } finally { ChatGptAccount.install(null); f.close(); }
    }

    private static List<String> keys(JSONObject o) {
        List<String> k = new ArrayList<>();
        for (Iterator<String> it = o.keys(); it.hasNext(); ) k.add(it.next());
        java.util.Collections.sort(k);
        return k;
    }

    /** REQ-044.5 with the real foreground service. */
    private static void signInService(Instrumentation inst, Activity activity, Context ctx) throws Exception {
        PackageManager pm = ctx.getPackageManager();
        if (Build.VERSION.SDK_INT >= 29) {
            ServiceInfo info = pm.getServiceInfo(new ComponentName(ctx, SignInService.class), 0);
            require(info.getForegroundServiceType() == ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC, "REQ-044.5 the service is of type dataSync");
        }
        require(!SignInService.running, "REQ-044.1/REQ-044.5 no service without a pending sign-in");
        Context app = ctx.getApplicationContext();
        File debugSwitch = new File(ctx.getFilesDir(), SignInService.DEBUG_SWITCH);
        debugSwitch.delete();
        // #350: a failed listener can finish before Android delivers the service's onCreate.
        Fixture immediate = new Fixture(ctx);
        try {
            immediate.config.keepalive = new ChatGptAccount.Keepalive() {
                @Override public void start() { SignInService.start(app); }
                @Override public void stop() { SignInService.stop(app); }
            };
            immediate.config.opener = (port, timeout, returnLink) -> { throw new IOException(new ErrnoException("socket", OsConstants.EPERM)); };
            onMain(inst, () -> {
                immediate.account.signIn(); immediate.waitIdle();
                immediate.account.signIn(); immediate.waitIdle();
                return null;
            });
            SystemClock.sleep(1000);
            require(!SignInService.running && immediate.account.snapshot().message.contains("verweigert den lokalen Netzwerkzugriff"),
                "#350 failed sign-in reports the error without crashing or leaving the service running");
        } finally { immediate.close(); }
        String[] ends = {"cancel", "success", "denied", "timeout"};
        for (String end : ends) {
            Fixture f = new Fixture(ctx);
            try {
                f.config.keepalive = new ChatGptAccount.Keepalive() {
                    @Override public void start() { SignInService.start(app); }
                    @Override public void stop() { SignInService.stop(app); }
                };
                if (end.equals("timeout")) f.config.callbackTimeoutMs = 1500;
                f.browser.callback = end.equals("success") ? FakeBrowser::approve : end.equals("denied") ? (q, a) -> {
                    SystemClock.sleep(800);
                    return "error=access_denied&state=" + FakeBrowser.enc(q.get("state"));
                } : null;
                if (end.equals("success")) f.browser.delayMs = 800;
                onMain(inst, () -> f.account.signIn());
                waitUntil(() -> SignInService.running, 5000, "REQ-044.5 the service runs while a sign-in waits (" + end + ")");
                if (end.equals("cancel")) {
                    SystemClock.sleep(500);
                    require(SignInService.running, "REQ-044.5 the service keeps running while the sign-in waits");
                    notification(ctx);
                    f.account.cancel();
                }
                long t0 = SystemClock.elapsedRealtime();
                waitUntil(() -> !SignInService.running, 12000, "REQ-044.5 the service stops (" + end + ")");
                long stopped = SystemClock.elapsedRealtime() - t0;
                f.waitIdle();
                if (end.equals("cancel")) require(stopped <= 2000, "REQ-044.5 the service stops within 2 s after cancel: " + stopped);
                require(!SignInService.running, "REQ-044.5 no service after " + end);
                if (end.equals("success")) require(f.account.snapshot().status == ChatGptAccount.Status.CONNECTED, "sign-in completed");
            } finally { f.close(); }
        }
        // timing of the stop after the end of the attempt
        Fixture timing = new Fixture(ctx);
        try {
            timing.config.keepalive = new ChatGptAccount.Keepalive() {
                @Override public void start() { SignInService.start(app); }
                @Override public void stop() { SignInService.stop(app); }
            };
            timing.browser.callback = null;
            onMain(inst, () -> timing.account.signIn());
            waitUntil(() -> SignInService.running, 5000, "service runs");
            long t0 = SystemClock.elapsedRealtime();
            timing.account.cancel();
            waitUntil(() -> !SignInService.running, 2000, "REQ-044.5 within 2 s after the attempt ends");
            require(SystemClock.elapsedRealtime() - t0 <= 2000, "REQ-044.5 stop timing");
            timing.waitIdle();
        } finally { timing.close(); }
        // the debug-build-only switch for the phone measurement
        Fixture off = new Fixture(ctx);
        try {
            off.config.keepalive = new ChatGptAccount.Keepalive() {
                @Override public void start() { SignInService.start(app); }
                @Override public void stop() { SignInService.stop(app); }
            };
            off.browser.callback = null;
            require(debugSwitch.createNewFile(), "test setup");
            onMain(inst, () -> off.account.signIn());
            waitUntil(() -> off.browser.opened.size() == 1, 5000, "browser opened");
            SystemClock.sleep(1200);
            require(!SignInService.running && SignInService.disabledForMeasurement(ctx), "REQ-044.5 the debug switch disables the service");
            off.account.cancel();
            off.waitIdle();
        } finally { debugSwitch.delete(); off.close(); }
    }

    /** REQ-044.5: the ongoing notification, asserted when notifications are allowed (Android 13+ hides it otherwise). */
    private static void notification(Context ctx) throws Exception {
        boolean allowed = Build.VERSION.SDK_INT < 33 || ctx.checkSelfPermission("android.permission.POST_NOTIFICATIONS") == PackageManager.PERMISSION_GRANTED;
        if (!allowed) return;
        NotificationManager nm = ctx.getSystemService(NotificationManager.class);
        waitUntil(() -> { for (android.service.notification.StatusBarNotification n : nm.getActiveNotifications()) if (n.getId() == SignInService.NOTIFICATION_ID) return true; return false; }, 5000, "REQ-044.5 the sign-in notification");
        for (android.service.notification.StatusBarNotification n : nm.getActiveNotifications()) {
            if (n.getId() != SignInService.NOTIFICATION_ID) continue;
            require("ChatGPT-Anmeldung läuft".contentEquals(n.getNotification().extras.getCharSequence(Notification.EXTRA_TITLE)) && (n.getNotification().flags & Notification.FLAG_ONGOING_EVENT) != 0,
                "REQ-044.5 ongoing notification 'ChatGPT-Anmeldung läuft'");
            require(n.getNotification().extras.getCharSequence(Notification.EXTRA_TEXT) == null, "REQ-043.2 the notification carries no account data");
        }
    }
}
