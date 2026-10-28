package de.obdentic.app;

/** REQ-020: when automatic connection to the known adapter may run and how long it waits. No Android dependencies, no timers: the service drives it. */
public final class ReconnectPolicy {
    public enum State { IDLE, WAITING, CONNECTING, BLOCKED, STOPPED }
    private static final long[] PAUSES_MS = {2000, 5000, 10000};
    private static final long REPEAT_MS = 30000;
    private boolean enabled;
    private State state = State.IDLE;
    private int attempts;

    public ReconnectPolicy(boolean enabled) { this.enabled = enabled; }

    /** REQ-020.3: pause before the attempt with this zero-based number: 2, 5, 10 s, then every 30 s. */
    public static long delayMs(int attempt) { return attempt < PAUSES_MS.length ? PAUSES_MS[attempt] : REPEAT_MS; }

    /** REQ-020.8: the profile to start again after reconnection; "" for no profile or no recording. */
    public static String resumeProfile(String profile) {
        return profile == null ? "" : profile;
    }

    public synchronized boolean enabled() { return enabled; }
    public synchronized State state() { return state; }
    /** An automatic attempt is running, waiting for its pause or waiting for Bluetooth. */
    public synchronized boolean pending() { return state == State.WAITING || state == State.CONNECTING || state == State.BLOCKED; }

    /** REQ-020.6: switching off ends everything pending. */
    public synchronized void setEnabled(boolean on) { enabled = on; if (!on) { attempts = 0; if (state != State.STOPPED) state = State.IDLE; } }
    /** REQ-020.2: app start (or switching on); the first attempt is immediate. False when switched off. */
    public synchronized boolean startup() { if (!enabled) return false; state = State.CONNECTING; attempts = 0; return true; }
    /** An established session was lost. Pause before the retry in ms, or -1 when no automatic attempt may follow (REQ-020.4/.6). */
    public synchronized long lost() { if (!enabled || state == State.STOPPED) return -1; attempts = 0; state = State.WAITING; return delayMs(attempts++); }
    /** An automatic attempt failed or was postponed. Pause before the next one, or -1 when it is not an automatic attempt. */
    public synchronized long failed() { if (!enabled || state != State.CONNECTING) return -1; state = State.WAITING; return delayMs(attempts++); }
    /** The pause is over; true when the attempt may start now. */
    public synchronized boolean attemptDue() { if (!enabled || state != State.WAITING) return false; state = State.CONNECTING; return true; }
    /** REQ-020.9: Bluetooth off or permission missing; no attempt until {@link #unblock}. */
    public synchronized void block() { if (enabled && (state == State.WAITING || state == State.CONNECTING)) state = State.BLOCKED; }
    public synchronized boolean unblock() { if (!enabled || state != State.BLOCKED) return false; state = State.CONNECTING; attempts = 0; return true; }
    /** REQ-020.4: manual disconnect, demo, replay or the stop button; nothing starts until {@link #userConnect} or the next app start. */
    public synchronized void stop() { state = State.STOPPED; attempts = 0; }
    /** The user connects by hand: attempts may follow a later loss again. */
    public synchronized void userConnect() { state = State.IDLE; attempts = 0; }
    /** Connected, or nothing to attempt. */
    public synchronized void success() { state = State.IDLE; attempts = 0; }
}
