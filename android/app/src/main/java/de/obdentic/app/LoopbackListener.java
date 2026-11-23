package de.obdentic.app;

import android.system.ErrnoException;
import android.system.OsConstants;
import java.io.ByteArrayOutputStream;
import java.io.Closeable;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.BindException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.SocketTimeoutException;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CancellationException;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeoutException;

/**
 * REQ-042.3: the OAuth redirect target. Binds only 127.0.0.1, serves one static page for {@code GET /auth/callback},
 * answers everything else with 404 without ending the attempt, and closes after the first callback, the timeout or {@link #close()}.
 */
public final class LoopbackListener implements Closeable {
    public static final String PATH = "/auth/callback";
    /** The link of the callback page carries no parameters; the app takes no value from it (REQ-042.6). */
    public static final String RETURN_LINK = "obdentic://account";
    private static final int HEAD_LIMIT = 8192;
    /** Connections served at the same time; more are closed at once (a browser needs one, speculative connections a few). */
    public static final int MAX_HANDLERS = 8;
    /** Absolute time one request may take from accept to the end of its head. */
    public static final long REQUEST_DEADLINE_MS = 3000;

    private final ServerSocket server;
    private final CompletableFuture<Map<String, String>> result = new CompletableFuture<>();
    private final boolean returnLink;
    private final Set<Socket> accepted = new HashSet<>();
    private boolean done;

    private LoopbackListener(ServerSocket server, boolean returnLink) {
        this.server = server;
        this.returnLink = returnLink;
    }

    /** Opens the listener on {@code preferredPort}, or on another free port when that one is taken. */
    public static LoopbackListener open(int preferredPort, long timeoutMs, boolean returnLink) throws IOException {
        InetAddress lo = InetAddress.getByAddress(new byte[]{127, 0, 0, 1});
        ServerSocket s;
        try {
            s = bind(lo, preferredPort);
        } catch (BindException e) {
            if (preferredPort == 0 || socketErrno(e) != OsConstants.EADDRINUSE) throw e;
            s = bind(lo, 0);
        }
        LoopbackListener l = new LoopbackListener(s, returnLink);
        Thread t = new Thread(() -> l.accept(timeoutMs), "account-callback");
        t.setDaemon(true);
        t.start();
        return l;
    }

    private static ServerSocket bind(InetAddress lo, int port) throws IOException {
        ServerSocket s = new ServerSocket();
        try {
            s.setReuseAddress(true);
            s.bind(new InetSocketAddress(lo, port), 8);
        } catch (IOException | SecurityException e) {
            try { s.close(); } catch (IOException closeError) { e.addSuppressed(closeError); }
            throw e;
        }
        return s;
    }

    /** Android wraps the native socket error in a Java exception. Do not classify by the exception's text. */
    static int socketErrno(Throwable error) {
        for (Throwable cause = error; cause != null; cause = cause.getCause()) {
            if (cause instanceof ErrnoException) return ((ErrnoException) cause).errno;
        }
        return 0;
    }

    public int port() { return server.getLocalPort(); }

    /** The first callback's parameters (code, state, error, client_id); fails with TimeoutException or CancellationException. */
    public CompletableFuture<Map<String, String>> result() { return result; }

    /** Test synchronization: a TCP handshake alone does not mean accept() has registered its socket. */
    public int acceptedCount() { synchronized (accepted) { return accepted.size(); } }

    /** Ends the attempt: the port is released and a pending result is cancelled. */
    @Override public void close() {
        synchronized (this) { done = true; }
        closeServer();
        result.completeExceptionally(new CancellationException());
    }

    /** Closes the listening socket and every accepted connection that is still open. */
    private void closeServer() {
        try { server.close(); } catch (IOException ignored) { /* already closed */ }
        Set<Socket> open;
        synchronized (accepted) { open = new HashSet<>(accepted); }
        for (Socket s : open) {
            try { s.close(); } catch (IOException ignored) { /* already closed */ }
        }
    }

    /** Registers a connection unless the handlers are at their bound or the attempt has ended; false: the socket was closed. */
    private boolean admit(Socket c) {
        boolean ok;
        synchronized (accepted) { ok = accepted.size() < MAX_HANDLERS && !server.isClosed() && accepted.add(c); }
        if (!ok) {
            try { c.close(); } catch (IOException ignored) { /* nothing to close */ }
            return false;
        }
        if (server.isClosed()) { // closed between the check and the registration: closeServer may have missed this socket
            try { c.close(); } catch (IOException ignored) { /* nothing to close */ }
        }
        return true;
    }

    private void accept(long timeoutMs) {
        long deadline = System.currentTimeMillis() + timeoutMs;
        try {
            while (true) {
                long left = deadline - System.currentTimeMillis();
                if (left <= 0) throw new SocketTimeoutException();
                server.setSoTimeout((int) Math.min(left, Integer.MAX_VALUE));
                Socket c = server.accept();
                if (!admit(c)) continue;
                Thread t = new Thread(() -> serve(c), "account-callback-connection");
                t.setDaemon(true);
                t.start();
            }
        } catch (SocketTimeoutException e) {
            closeServer();
            result.completeExceptionally(new TimeoutException());
        } catch (IOException e) {
            closeServer(); // closed by close() or after the callback
        }
    }

    private void serve(Socket socket) {
        try (Socket c = socket) {
            String head = head(c, System.currentTimeMillis() + REQUEST_DEADLINE_MS);
            OutputStream out = c.getOutputStream();
            String[] line = head == null ? new String[0] : head.split("\r?\n", 2)[0].split(" ");
            String path = line.length < 2 ? "" : line[1].split("\\?", 2)[0];
            if (line.length < 3 || !"GET".equals(line[0]) || !PATH.equals(path)) {
                respond(out, 404, "not found", "text/plain; charset=utf-8");
                return;
            }
            synchronized (this) {
                if (done) { respond(out, 404, "not found", "text/plain; charset=utf-8"); return; }
                done = true;
            }
            Map<String, String> all = Oidc.query(line[1].contains("?") ? line[1].substring(line[1].indexOf('?') + 1) : "");
            Map<String, String> keep = new HashMap<>();
            for (String k : new String[]{"code", "state", "error", "client_id"}) if (all.containsKey(k)) keep.put(k, all.get(k));
            respond(out, 200, page(), "text/html; charset=utf-8");
            result.complete(keep);
            closeServer(); // REQ-042.3: closed right after the first callback
        } catch (IOException ignored) {
            // a speculative, slow or broken connection does not end the attempt
        } finally {
            synchronized (accepted) { accepted.remove(socket); }
        }
    }

    /** Reads the request head up to its blank line, bounded in size and by one absolute deadline (a timeout is an IOException). */
    private static String head(Socket c, long deadline) throws IOException {
        InputStream in = c.getInputStream();
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        int b;
        while (bytes.size() < HEAD_LIMIT) {
            long left = deadline - System.currentTimeMillis();
            if (left <= 0) throw new SocketTimeoutException();
            c.setSoTimeout((int) left);
            if ((b = in.read()) < 0) break;
            bytes.write(b);
            int n = bytes.size();
            if (b == '\n' && n >= 2) {
                byte[] a = bytes.toByteArray();
                if (a[n - 2] == '\n' || (n >= 4 && a[n - 2] == '\r' && a[n - 3] == '\n' && a[n - 4] == '\r')) break;
            }
        }
        return bytes.size() == 0 ? null : bytes.toString("ISO-8859-1");
    }

    /** Static page: never echoes the code, state or error (REQ-042.3). */
    public String page() {
        return "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">"
            + "<title>OBDentic</title></head><body><p>Anmeldung \u00fcbermittelt.</p>"
            + (returnLink ? "<p><a href=\"" + RETURN_LINK + "\">Zur\u00fcck zu OBDentic</a></p>" : "<p>Wechsle jetzt zur\u00fcck zur App OBDentic.</p>")
            + "</body></html>";
    }

    private static void respond(OutputStream out, int status, String body, String type) throws IOException {
        byte[] b = body.getBytes(StandardCharsets.UTF_8);
        String head = "HTTP/1.1 " + status + (status == 200 ? " OK" : " Not Found") + "\r\nContent-Type: " + type + "\r\nContent-Length: " + b.length
            + "\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n";
        out.write(head.getBytes(StandardCharsets.ISO_8859_1));
        out.write(b);
        out.flush();
    }
}
