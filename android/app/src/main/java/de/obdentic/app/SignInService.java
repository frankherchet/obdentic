package de.obdentic.app;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.IBinder;
import java.io.File;

/**
 * REQ-044.5: while a sign-in waits for the browser the app is a cached background process and Android may block its network
 * (UnknownHostException on the code exchange, seen on the real phone). This foreground service of type dataSync keeps the process in a
 * network-allowed state. It is started only by the tap on 'Continue with ChatGPT', stopped when the attempt ends, and never runs otherwise.
 */
public final class SignInService extends Service {
    public static final String CHANNEL = "account";
    public static final String TITLE = "ChatGPT-Anmeldung läuft";
    public static final int NOTIFICATION_ID = 2;
    /** Debug builds only: with this file in the app's files directory the service is not started (phone measurement of REQ-044.6). */
    public static final String DEBUG_SWITCH = "debug-no-signin-service";
    public static volatile boolean running;

    /** Starts the service from the visible app. */
    public static void start(Context app) {
        if (disabledForMeasurement(app)) return;
        try {
            app.startForegroundService(new Intent(app, SignInService.class));
        } catch (RuntimeException e) {
            AppLog.w("account", "sign-in service not started " + e.getClass().getSimpleName());
        }
    }

    public static void stop(Context app) {
        app.stopService(new Intent(app, SignInService.class));
    }

    public static boolean disabledForMeasurement(Context context) {
        return (context.getApplicationInfo().flags & ApplicationInfo.FLAG_DEBUGGABLE) != 0 && new File(context.getFilesDir(), DEBUG_SWITCH).exists();
    }

    @Override public void onCreate() {
        super.onCreate();
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel(CHANNEL, "ChatGPT-Anmeldung", NotificationManager.IMPORTANCE_LOW));
        Notification.Builder builder = new Notification.Builder(this, CHANNEL).setContentTitle(TITLE).setSmallIcon(android.R.drawable.stat_notify_sync).setOngoing(true);
        if (Build.VERSION.SDK_INT >= 31) builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE); // Android 12 would otherwise show it after 10 s
        Notification notification = builder.build();
        if (Build.VERSION.SDK_INT >= 29) startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC);
        else startForeground(NOTIFICATION_ID, notification);
        running = true;
    }

    @Override public int onStartCommand(Intent intent, int flags, int startId) { return START_NOT_STICKY; }

    @Override public void onDestroy() {
        running = false;
        super.onDestroy();
    }

    @Override public IBinder onBind(Intent intent) { return null; }
}
