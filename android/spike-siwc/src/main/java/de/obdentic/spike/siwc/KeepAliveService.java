package de.obdentic.spike.siwc;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.IBinder;

/**
 * Foreground service held while a network action runs. While the system browser is in front the app is a cached
 * background process and Android blocks its network access (DNS fails with UnknownHostException); a foreground
 * service keeps the process in a network-allowed state until the sign-in and the requests after it are done.
 */
public final class KeepAliveService extends Service {
    private static final String CHANNEL = "siwc";
    private static int holds;

    /** Call from the foreground activity before the work starts. */
    static synchronized void acquire(Context ctx) {
        if (holds++ > 0) return;
        try {
            ctx.startForegroundService(new Intent(ctx, KeepAliveService.class));
            Report.line("keepalive", "INFO", "started=true");
        } catch (RuntimeException e) {
            Report.line("keepalive", "INFO", "started=false exception=" + Oauth.safe(e.getClass().getSimpleName()));
        }
    }

    /** Call when the work has finished, whatever its outcome. */
    static synchronized void release(Context ctx) {
        if (holds > 0 && --holds == 0) ctx.stopService(new Intent(ctx, KeepAliveService.class));
    }

    @Override
    public void onCreate() {
        super.onCreate();
        NotificationManager nm = getSystemService(NotificationManager.class);
        nm.createNotificationChannel(new NotificationChannel(CHANNEL, "SIWC spike", NotificationManager.IMPORTANCE_LOW));
        Notification n = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_notify_sync)
                .setContentTitle("OBDentic SIWC Spike")
                .setContentText("Sign-in test is running")
                .setOngoing(true)
                .build();
        startForeground(1, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC);
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        return START_NOT_STICKY;
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
