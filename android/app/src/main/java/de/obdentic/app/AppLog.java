package de.obdentic.app;

import android.content.Context;
import android.util.Log;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.text.SimpleDateFormat;
import java.util.*;
import java.util.regex.Pattern;

/** REQ-029: operational debug log, separate from the replayable vehicle evidence. Never throws, never carries VINs or raw adapter traffic. */
public final class AppLog {
    private static final Pattern VIN = Pattern.compile("\\b[A-HJ-NPR-Z0-9]{17}\\b"), HEX = Pattern.compile("\\b(?:[0-9A-Fa-f]{2} ){5,}[0-9A-Fa-f]{2}\\b"),
        ADDRESS = Pattern.compile("\\b(?:[0-9A-Fa-f]{2}:){4}([0-9A-Fa-f]{2}:[0-9A-Fa-f]{2})\\b");
    private static final int KEEP = 50;
    public static volatile long maxBytes = 1024 * 1024;
    private static File dir;
    private static boolean handlerInstalled;
    private AppLog() {}

    public static synchronized void init(Context context) {
        dir = new File(context.getFilesDir(), "logs");
        if (handlerInstalled) return;
        handlerInstalled = true; Thread.UncaughtExceptionHandler previous = Thread.getDefaultUncaughtExceptionHandler();
        Thread.setDefaultUncaughtExceptionHandler((thread, error) -> { logUncaught(thread, error); if (previous != null) previous.uncaughtException(thread, error); });
    }
    /** Test seam: where the log is written. */
    public static synchronized void setDirectory(File directory) { dir = directory; }
    public static void i(String tag, String message) { write("INFO", tag, message, null); }
    public static void w(String tag, String message) { write("WARN", tag, message, null); }
    public static void e(String tag, String message) { write("ERROR", tag, message, null); }
    public static void e(String tag, String message, Throwable error) { write("ERROR", tag, message, error); }
    public static void logUncaught(Thread thread, Throwable error) { write("FATAL", "uncaught", "thread " + thread.getName(), error); }

    /** REQ-029.4 */
    static String scrub(String text) {
        if (text == null) return "";
        return ADDRESS.matcher(HEX.matcher(VIN.matcher(text).replaceAll("[VIN]")).replaceAll("[hex]")).replaceAll("…$1");
    }
    private static synchronized void write(String level, String tag, String message, Throwable error) {
        StringBuilder line = new StringBuilder(new SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSSZ", Locale.ROOT).format(new Date())).append(' ').append(level).append(' ').append(tag).append(' ').append(scrub(message));
        if (error != null) { StringWriter trace = new StringWriter(); error.printStackTrace(new PrintWriter(trace)); line.append('\n').append(scrub(trace.toString().trim())); }
        try { Log.println(level.equals("INFO") ? Log.INFO : level.equals("WARN") ? Log.WARN : Log.ERROR, "OBDentic/" + tag, line.toString()); } catch (RuntimeException ignored) { /* plain JVM */ }
        try {
            File directory = dir; if (directory == null || (!directory.isDirectory() && !directory.mkdirs())) return;
            File active = new File(directory, "app.log");
            try (Writer out = new OutputStreamWriter(new FileOutputStream(active, true), StandardCharsets.UTF_8)) { out.write(line.append('\n').toString()); }
            if (active.length() >= maxBytes) seal();
        } catch (IOException | RuntimeException ignored) { /* REQ-029.5: logging never disturbs the app */ }
    }
    /** Renames the active log to its unique sealed name (if it has content) and drops sealed files beyond the newest 50. */
    public static synchronized void seal() {
        try {
            File directory = dir; if (directory == null) return;
            File active = new File(directory, "app.log");
            if (active.isFile() && active.length() > 0) active.renameTo(new File(directory, "app-" + new SimpleDateFormat("yyyyMMdd-HHmmss-SSS", Locale.ROOT).format(new Date()) + ".log"));
            List<File> sealed = sealed();
            for (int i = KEEP; i < sealed.size(); i++) sealed.get(i).delete();
        } catch (RuntimeException ignored) { /* REQ-029.5 */ }
    }
    /** Sealed log files, newest first. */
    public static synchronized List<File> sealed() {
        List<File> files = new ArrayList<>(); File[] list = dir == null ? null : dir.listFiles();
        if (list != null) for (File file : list) if (file.isFile() && file.getName().matches("app-[0-9]{8}-[0-9]{6}-[0-9]{3}\\.log")) files.add(file);
        Collections.sort(files, (a, b) -> b.getName().compareTo(a.getName()));
        return files;
    }
}
