package de.obdentic.app;

public final class NativeCore {
    static { System.loadLibrary("obdentic_mobile"); }
    private NativeCore() {}
    public interface Exchange { String exchange(String command, long timeoutMs) throws Exception; }
    public static native long create(Exchange exchange);
    public static native String execute(long handle, String action, String argument);
    public static native void destroy(long handle);
}
