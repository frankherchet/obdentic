package de.obdentic.app;

import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import java.io.IOException;
import java.util.Arrays;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CancellationException;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.locks.ReentrantLock;
import java.util.function.LongSupplier;
import org.json.JSONObject;

/**
 * The optional ChatGPT account (REQ-042, REQ-043, REQ-044): sign-in through the system browser (loopback callback, PKCE), ID-token
 * validation, Keystore-protected credential, serialized refresh rotation and revocation. Platform APIs only. It has no path to the
 * vehicle session, the adapter transport or the diagnostic jobs (REQ-044.4), and it makes no request unless a user action or a caller
 * asking for an access token needs one (REQ-044.1).
 */
public final class ChatGptAccount {
    public static final String PLAN_SCOPE = "chatgpt.tokens.use.direct";
    public static final String SCOPE = "openid profile email offline_access resource.invoke " + PLAN_SCOPE;
    public static final String DYNAMIC_CLIENT = "dynamic_agent_client";
    public static final String AGENT_NAME = "OBDentic";
    /** REQ-043.4: an access token valid for at least this long is used without a refresh. */
    public static final long REFRESH_MARGIN_MS = 300_000;

    public static final String MSG_EXPIRED = "Anmeldung abgelaufen · bitte erneut mit ChatGPT anmelden";
    public static final String MSG_OFFLINE = "Keine Verbindung zu ChatGPT · erneut versuchen";
    public static final String MSG_REVOKED = "Abgemeldet · Zugriff widerrufen";
    public static final String MSG_UNCONFIRMED = "Abgemeldet · Widerruf nicht bestätigt. Der Zugriff lässt sich in den ChatGPT-Einstellungen unter „Manage usage“ entfernen.";
    public static final String MSG_UNREADABLE = "Nicht verbunden · die gespeicherte Anmeldung war nicht lesbar, bitte erneut mit ChatGPT verbinden";
    public static final String MSG_INVALID_CLIENT = "Registrierung ungültig · beim nächsten Anmelden registriert sich die App neu";

    /** REQ-043.7 */
    private static final Set<String> TERMINAL = new HashSet<>(Arrays.asList("invalid_grant", "invalid_refresh_token", "token_expired",
        "refresh_token_expired", "refresh_token_invalidated", "refresh_token_reused"));
    /** REQ-042.4: error codes of an authorization callback that may be shown. */
    private static final Set<String> CALLBACK_ERRORS = new HashSet<>(Arrays.asList("invalid_request", "unauthorized_client", "unsupported_response_type",
        "invalid_scope", "server_error", "temporarily_unavailable", "interaction_required", "login_required", "consent_required", "account_selection_required"));

    public enum Status {
        NOT_CONNECTED("Nicht verbunden"), CONNECTED("Verbunden"), EXPIRED(MSG_EXPIRED), UNREACHABLE("ChatGPT nicht erreichbar"),
        PLAN_MISSING("ChatGPT-Plan-Nutzung nicht freigegeben");
        public final String label;
        Status(String label) { this.label = label; }
    }

    public enum Phase { IDLE, WAITING, EXCHANGING, DISCONNECTING }

    /** Why a caller got no access token. */
    public enum Failure { NOT_CONNECTED, PLAN_NOT_ENABLED, EXPIRED, UNREACHABLE }

    public static final class Snapshot {
        public final Status status;
        public final Phase phase;
        public final String email, message;
        /** Changes whenever anything shown changes. */
        public final long version;
        public final boolean revocationUnconfirmed;

        public Snapshot(Status status, Phase phase, String email, String message, long version, boolean revocationUnconfirmed) {
            this.status = status;
            this.phase = phase;
            this.email = email;
            this.message = message;
            this.version = version;
            this.revocationUnconfirmed = revocationUnconfirmed;
        }
    }

    /** Either a token or the reason there is none. */
    public static final class Token {
        public final String value;
        public final Failure failure;

        private Token(String value, Failure failure) {
            this.value = value;
            this.failure = failure;
        }
    }

    public interface Launcher { void open(String url) throws ActivityNotFoundException; }

    /** Holds the process in a network-allowed state while a sign-in waits for the browser (REQ-044.5). */
    public interface Keepalive { void start(); void stop(); }

    public interface Opener { LoopbackListener open(int port, long timeoutMs, boolean returnLink) throws IOException; }

    public static final class Config {
        public String origin = "https://auth.openai.com";
        public String resource = "https://api.openai.com/v1";
        public String usageUrl = "https://chatgpt.com/settings/usage";
        public int preferredPort = 1455;
        public long callbackTimeoutMs = 300_000;
        /** REQ-043.11: pauses between the three revocation attempts. */
        public long[] revokeDelaysMs = {1000, 3000};
        public int revokeTimeoutMs = 10_000;
        /** Callback page link back to the app; removed when the phone measurement of REQ-044.7 says it does not work. */
        public boolean returnLink = true;
        public LongSupplier clock = System::currentTimeMillis;
        public AccountHttp.Transport transport;
        public Launcher launcher;
        public Keepalive keepalive;
        public Opener opener = LoopbackListener::open;
        public AccountStore store;

        public static Config defaults(Context context) {
            Config c = new Config();
            Context app = context.getApplicationContext();
            c.transport = new AccountHttp.HttpsTransport();
            c.launcher = url -> app.startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            c.keepalive = new Keepalive() {
                @Override public void start() { SignInService.start(app); }
                @Override public void stop() { SignInService.stop(app); }
            };
            c.store = new AccountStore(app, "account");
            return c;
        }
    }

    private static ChatGptAccount instance;

    public static synchronized ChatGptAccount get(Context context) {
        if (instance == null) instance = new ChatGptAccount(Config.defaults(context));
        return instance;
    }

    /** Test seam: replaces the process-wide account. */
    public static synchronized void install(ChatGptAccount account) { instance = account; }

    private final Config config;
    private final AccountStore store;
    private final String host;
    private final Object state = new Object();
    private final ReentrantLock refreshLock = new ReentrantLock();
    private Phase phase = Phase.IDLE;
    private long epoch, signInId, version;
    /** Set by {@link #cancel()} for the running attempt, also while its listener does not exist yet. */
    private boolean cancelRequested;
    private String message = "";
    private boolean expired, unreachable, loaded;
    /** True while {@code record} is a rotated credential that storage does not hold yet: the one valid refresh token, retried on every access. */
    private boolean unsaved;
    private boolean revocationUnconfirmed;
    /** The attempt that started the sign-in service and has not stopped it; only that attempt may stop it. */
    private long keepaliveOwner;
    private AccountStore.Record record;
    private LoopbackListener listener;
    private volatile JSONObject discovery;

    public ChatGptAccount(Config config) {
        this.config = config;
        this.store = config.store;
        this.host = java.net.URI.create(config.origin).getHost();
    }

    private static void log(String line) { AppLog.i("account", line); }

    // ---------------------------------------------------------------- state

    /** Callers hold {@code state}. A record that cannot be read is deleted with its key (REQ-043.3). */
    private AccountStore.Record current() {
        if (!loaded) {
            try {
                record = store.load();
            } catch (Exception e) {
                log("record unreadable " + e.getClass().getSimpleName());
                store.deleteRecord();
                record = null;
                message = MSG_UNREADABLE;
                version++;
            }
            loaded = true;
        }
        return record;
    }

    /** Forgets the cached record; the next call reads the file again. A rotated record that is not stored yet is kept. */
    public void reload() {
        synchronized (state) { if (!unsaved) loaded = false; version++; }
    }

    public Snapshot snapshot() {
        synchronized (state) {
            AccountStore.Record r = current();
            Status s = r == null ? (expired ? Status.EXPIRED : Status.NOT_CONNECTED)
                : !r.hasPlan() ? Status.PLAN_MISSING : unreachable ? Status.UNREACHABLE : Status.CONNECTED;
            return new Snapshot(s, phase, r == null ? "" : r.email, message, version, revocationUnconfirmed);
        }
    }

    private void changed() { version++; }

    // ---------------------------------------------------------------- one-time dialog (REQ-042.11)

    public boolean firstUseDue() { return "due".equals(store.firstUse()); }

    public void firstUseAcknowledged() {
        store.setFirstUse("done");
        synchronized (state) { changed(); }
    }

    /** REQ-042.10: opens the usage settings in the system browser; false when there is none. */
    public boolean openUsage() {
        try {
            config.launcher.open(config.usageUrl);
            return true;
        } catch (ActivityNotFoundException e) {
            return false;
        }
    }

    // ---------------------------------------------------------------- sign-in (REQ-042)

    private static final class Stop extends Exception {
        final String text, code;

        Stop(String text, String code) {
            super(code);
            this.text = text;
            this.code = code;
        }
    }

    /**
     * 'Continue with ChatGPT'. Returns false when ignored: an attempt or a disconnect is running, or the account is connected
     * (REQ-042.12). An inactive sign-in without the plan permission is repeated with prompt=consent (REQ-042.9).
     */
    public boolean signIn() {
        final long id, startEpoch;
        final boolean reauth;
        synchronized (state) {
            if (phase != Phase.IDLE) return false;
            AccountStore.Record existing = current();
            if (existing != null && existing.hasPlan()) return false;
            reauth = existing != null;
            phase = Phase.WAITING;
            message = "";
            id = ++signInId;
            cancelRequested = false;
            keepaliveOwner = id;
            startEpoch = epoch;
            changed();
        }
        log("signin start reauth=" + reauth);
        config.keepalive.start(); // REQ-044.5: started by the tap, before anything is opened
        Thread t = new Thread(() -> runSignIn(id, startEpoch, reauth), "account-signin");
        t.setDaemon(true);
        t.start();
        return true;
    }

    /** REQ-042.5: ends a waiting attempt. */
    public void cancel() {
        synchronized (state) {
            if (phase != Phase.WAITING) return;
            cancelRequested = true; // the attempt checks this before it publishes a listener and before it opens the browser
            if (listener != null) listener.close();
        }
    }

    private void runSignIn(long id, long startEpoch, boolean reauth) {
        String result;
        String text = null;
        try {
            attempt(id, startEpoch, reauth);
            result = "ok";
        } catch (Stop s) {
            result = s.code;
            text = s.text;
        } catch (CancellationException e) {
            result = "cancel";
            text = "Anmeldung abgebrochen";
        } catch (RuntimeException e) {
            result = "exception " + e.getClass().getSimpleName();
            text = "Anmeldung fehlgeschlagen";
        } finally {
            closeListener(id);
            synchronized (state) {
                if (keepaliveOwner == id) { // a newer attempt owns the service when the user disconnected and signed in again meanwhile
                    keepaliveOwner = 0;
                    config.keepalive.stop(); // REQ-044.5: within 2 s after the attempt ends
                }
            }
        }
        synchronized (state) {
            if (signInId == id) {
                phase = Phase.IDLE;
                message = text == null ? "" : text;
                changed();
            }
        }
        log("signin end result=" + result);
    }

    private void closeListener(long id) {
        synchronized (state) {
            if (listener != null && signInId == id) { listener.close(); listener = null; }
        }
    }

    private void attempt(long id, long startEpoch, boolean reauth) throws Stop {
        String hostId = store.hostId();
        String saved = store.clientId();
        String oauthState = Oidc.random(), nonce = Oidc.random(), verifier = Oidc.random();
        LoopbackListener l;
        try {
            l = config.opener.open(config.preferredPort, config.callbackTimeoutMs, config.returnLink);
        } catch (IOException e) {
            throw new Stop("Kein freier lokaler Port für die Anmeldung", "no_port");
        }
        synchronized (state) {
            if (signInId != id || cancelRequested) { l.close(); throw new CancellationException(); }
            listener = l;
        }
        String redirect = "http://127.0.0.1:" + l.port() + LoopbackListener.PATH;
        try {
            synchronized (state) {
                if (signInId != id || cancelRequested) throw new CancellationException(); // REQ-042.5: no browser after a cancel
            }
            config.launcher.open(authorizeUrl(saved, hostId, redirect, oauthState, nonce, Oidc.challenge(verifier), reauth)); // REQ-042.2: the listener already accepts
        } catch (ActivityNotFoundException e) {
            throw new Stop("Kein Browser gefunden · Anmeldung nicht möglich", "no_browser");
        } catch (IOException e) {
            throw new Stop("Anmeldung fehlgeschlagen", "url");
        }
        Map<String, String> callback;
        try {
            callback = l.result().get();
        } catch (ExecutionException e) {
            if (e.getCause() instanceof TimeoutException) throw new Stop("Zeitüberschreitung · erneut versuchen", "timeout");
            throw new Stop("Anmeldung fehlgeschlagen", "listener");
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new CancellationException();
        }
        synchronized (state) {
            if (signInId != id || cancelRequested) throw new CancellationException(); // a cancel after the callback arrived still wins (REQ-042.5)
            phase = Phase.EXCHANGING;
            changed();
        }
        String clientId = checkCallback(callback, oauthState, saved);
        AccountHttp.Response r = post(config.origin + "/api/accounts/oauth/token", exchangeForm(clientId, callback.get("code"), verifier, redirect), AccountHttp.LONG_TIMEOUT_MS, "network");
        if (r.truncated) throw new Stop("Antwort ungültig", "too_large");
        if (!r.ok()) {
            String code = r.errorCode();
            if ((r.status == 400 || r.status == 401) && code.equals("invalid_client")) { invalidClient(id); throw new Stop(MSG_INVALID_CLIENT, "invalid_client"); }
            throw new Stop("Anmeldung fehlgeschlagen · HTTP " + r.status + (code.isEmpty() ? "" : " " + code), "http_" + r.status);
        }
        JSONObject t = r.json();
        if (t == null) throw new Stop("Antwort ungültig", "json");
        String access = t.optString("access_token", ""), refresh = t.optString("refresh_token", ""), idToken = t.optString("id_token", "");
        if (access.isEmpty() || refresh.isEmpty() || idToken.isEmpty()) {
            revokeQuietly(refresh, clientId);
            throw new Stop("Antwort ungültig", "tokens");
        }
        Oidc.IdToken claims;
        try {
            claims = Oidc.validateIdToken(idToken, jwks(), config.origin, clientId, nonce, config.clock.getAsLong());
        } catch (Oidc.Invalid e) {
            revokeQuietly(refresh, clientId);
            throw new Stop("Anmeldung nicht verifizierbar · " + e.cause, "idtoken_" + e.cause);
        } catch (Stop e) {
            revokeQuietly(refresh, clientId);
            throw e;
        }
        long now = config.clock.getAsLong();
        AccountStore.Record rec = new AccountStore.Record(clientId, access, refresh, idToken, t.optString("scope", ""), claims.email, claims.subject,
            now + t.optLong("expires_in", 0) * 1000L, Oidc.earliestRefreshMs(t.opt("earliest_refresh_at")), false);
        AccountStore.Record replaced;
        synchronized (state) {
            if (signInId != id || epoch != startEpoch) { // REQ-043.13: disconnected meanwhile
                discard(refresh, clientId);
                throw new CancellationException();
            }
            replaced = current();
            if (replaced != null && !replaced.subject.equals(claims.subject)) { // REQ-042.12: a browser account switch is not a re-authorization
                discard(refresh, clientId);
                throw new Stop("Anderes ChatGPT-Konto · die bestehende Anmeldung bleibt, bitte mit dem verbundenen Konto bestätigen", "subject_changed");
            }
            try {
                store.save(rec);
            } catch (Exception e) {
                log("store write failed " + e.getClass().getSimpleName());
                if (replaced == null) store.deleteRecord(); // no remnant of the failed write: file artifacts and the key created for it
                discard(refresh, clientId); // REQ-042.8: the grant that cannot be kept is revoked
                throw new Stop("Anmeldung konnte nicht gespeichert werden", "store");
            }
            store.saveClientId(clientId);
            record = rec;
            loaded = true;
            unsaved = false;
            epoch++;
            expired = false;
            unreachable = false;
            if (rec.hasPlan() && store.firstUse().isEmpty()) store.setFirstUse("due");
            changed();
        }
        log("signin stored plan=" + rec.hasPlan());
        if (replaced != null && !replaced.refreshToken.equals(refresh)) revokeQuietly(replaced.refreshToken, clientId); // REQ-042.9
    }

    /** Revokes a token of a sign-in that must not be kept, off the state lock. */
    private void discard(String refresh, String clientId) {
        Thread t = new Thread(() -> revokeQuietly(refresh, clientId), "account-revoke");
        t.setDaemon(true);
        t.start();
    }

    private String authorizeUrl(String saved, String hostId, String redirect, String oauthState, String nonce, String challenge, boolean reauth) throws IOException {
        Map<String, String> q = new LinkedHashMap<>();
        q.put("client_id", saved == null ? DYNAMIC_CLIENT : saved);
        if (saved == null) q.put("agent_name_hint", AGENT_NAME);
        q.put("ext_agent_host_id", hostId);
        q.put("response_type", "code");
        q.put("redirect_uri", redirect);
        q.put("scope", SCOPE);
        q.put("resource", config.resource);
        q.put("state", oauthState);
        q.put("nonce", nonce);
        q.put("code_challenge_method", "S256");
        q.put("code_challenge", challenge);
        if (reauth) q.put("prompt", "consent");
        return config.origin + "/api/accounts/authorize?" + AccountHttp.form(q);
    }

    /** REQ-042.4: validates the callback and returns the client id to use. */
    private String checkCallback(Map<String, String> cb, String oauthState, String saved) throws Stop {
        if (!Oidc.same(oauthState, cb.get("state"))) throw new Stop("Antwort ungültig", "state");
        String error = cb.get("error");
        if (error != null) {
            if (error.equals("access_denied")) throw new Stop("Anmeldung abgebrochen", "denied");
            if (error.equals("invalid_client")) { invalidClientNow(); throw new Stop(MSG_INVALID_CLIENT, "invalid_client"); }
            throw new Stop("Anmeldung fehlgeschlagen · " + (CALLBACK_ERRORS.contains(error) ? error : "unbekannter Fehler"), "error");
        }
        String code = cb.get("code"), issued = cb.get("client_id");
        if (code == null || code.isEmpty()) throw new Stop("Antwort ungültig", "no_code");
        if (saved == null) {
            if (issued == null || issued.isEmpty() || issued.equals(DYNAMIC_CLIENT)) throw new Stop("Registrierung unvollständig", "no_client");
            return issued;
        }
        if (issued != null && !issued.equals(saved)) throw new Stop("Antwort ungültig", "client_mismatch");
        return saved;
    }

    private Map<String, String> exchangeForm(String clientId, String code, String verifier, String redirect) {
        Map<String, String> f = new LinkedHashMap<>();
        f.put("grant_type", "authorization_code");
        f.put("client_id", clientId);
        f.put("code", code);
        f.put("code_verifier", verifier);
        f.put("redirect_uri", redirect);
        f.put("resource", config.resource);
        return f;
    }

    private AccountHttp.Response post(String url, Map<String, String> form, int timeoutMs, String networkCode) throws Stop {
        try {
            return config.transport.send(AccountHttp.Request.post(url, form, timeoutMs));
        } catch (IOException e) {
            throw new Stop(MSG_OFFLINE, networkCode);
        }
    }

    /** The discovery document (REQ-044.2: fetched only when a sign-in or sign-out needs it). */
    private JSONObject discovery(int timeoutMs) throws IOException {
        JSONObject d = discovery;
        if (d != null) return d;
        AccountHttp.Response r = config.transport.send(AccountHttp.Request.get(config.origin + "/.well-known/openid-configuration", timeoutMs));
        d = r.ok() ? r.json() : null;
        if (d == null) throw new IOException("discovery");
        discovery = d;
        return d;
    }

    private JSONObject jwks() throws Stop {
        try {
            String uri = discovery(AccountHttp.LONG_TIMEOUT_MS).optString("jwks_uri", "");
            if (!Oidc.sameHostHttps(uri, host)) throw new Stop("Antwort ungültig", "jwks_endpoint");
            AccountHttp.Response r = config.transport.send(AccountHttp.Request.get(uri, AccountHttp.LONG_TIMEOUT_MS));
            JSONObject keys = r.ok() ? r.json() : null;
            if (keys == null) throw new Stop("Antwort ungültig", "jwks");
            return keys;
        } catch (IOException e) {
            throw new Stop(MSG_OFFLINE, "network");
        }
    }

    // ---------------------------------------------------------------- access token and refresh (REQ-043.4 to .9)

    /**
     * The access token for a caller (the assistant, #328), refreshed first when it is valid for less than 300 seconds. Blocking; call it
     * off the main thread. Without the plan permission it answers PLAN_NOT_ENABLED without any request (REQ-042.9).
     */
    public Token accessToken() {
        Held h = held();
        if (h == null) return failure();
        Token t = usable(h.record);
        if (t != null) return t;
        refreshLock.lock(); // REQ-043.6: one refresh at a time; a waiting caller re-checks the record afterwards
        try {
            h = held();
            if (h == null) return failure();
            t = usable(h.record);
            return t != null ? t : refresh(h.record, h.epoch);
        } finally {
            refreshLock.unlock();
        }
    }

    /** A record with the epoch it was read in, taken under one lock so that work on it can tell when it was superseded. */
    private static final class Held {
        final AccountStore.Record record;
        final long epoch;

        Held(AccountStore.Record record, long epoch) {
            this.record = record;
            this.epoch = epoch;
        }
    }

    private Held held() {
        synchronized (state) {
            AccountStore.Record r = phase == Phase.DISCONNECTING ? null : current();
            if (r != null && unsaved) persistPending(r);
            return r == null ? null : new Held(r, epoch);
        }
    }

    /** Callers hold {@code state}. Retries the write of a rotated record whose first write failed (REQ-043.5). */
    private void persistPending(AccountStore.Record r) {
        try {
            store.save(r);
            unsaved = false;
            log("pending record stored");
        } catch (Exception e) {
            log("pending record write failed " + e.getClass().getSimpleName());
        }
    }

    private Token failure() {
        synchronized (state) { return new Token(null, expired ? Failure.EXPIRED : Failure.NOT_CONNECTED); }
    }

    /** The stored token when it can be handed out without a refresh, a refusal when none can be, else null (a refresh is due and allowed). */
    private Token usable(AccountStore.Record r) {
        if (!r.hasPlan()) return new Token(null, Failure.PLAN_NOT_ENABLED);
        long now = config.clock.getAsLong();
        if (r.expiresAtMs - now >= REFRESH_MARGIN_MS) return new Token(r.accessToken, null);
        if (r.earliestRefreshMs > now) { // REQ-043.4: never refresh before earliest_refresh_at, not even after the access token expired
            return r.expiresAtMs > now ? new Token(r.accessToken, null) : new Token(null, Failure.UNREACHABLE);
        }
        return null;
    }

    private Token refresh(AccountStore.Record r, long startEpoch) {
        synchronized (state) {
            if (epoch != startEpoch) return new Token(null, Failure.NOT_CONNECTED); // REQ-043.13: disconnected or replaced since the record was read; nothing is sent
            try {
                // The old token may be spent as soon as the request leaves, even if its response is lost.
                AccountStore.Record pending = r.pendingRotation();
                store.save(pending);
                record = pending;
            } catch (Exception e) {
                return temporary(startEpoch, "rotation_marker_storage");
            }
        }
        Map<String, String> f = new LinkedHashMap<>();
        f.put("grant_type", "refresh_token");
        f.put("client_id", r.clientId);
        f.put("refresh_token", r.refreshToken);
        f.put("resource", config.resource);
        AccountHttp.Response resp;
        try {
            resp = config.transport.send(AccountHttp.Request.post(config.origin + "/api/accounts/oauth/token", f, AccountHttp.LONG_TIMEOUT_MS));
        } catch (IOException e) {
            return temporary(startEpoch, "network");
        }
        String code = resp.errorCode();
        if (resp.ok()) {
            JSONObject t = resp.json();
            String access = t == null ? "" : t.optString("access_token", ""), refresh = t == null ? "" : t.optString("refresh_token", "");
            if (access.isEmpty() || refresh.isEmpty()) return terminal(startEpoch, "refresh_incomplete", false);
            long now = config.clock.getAsLong();
            AccountStore.Record n = r.refreshed(access, refresh, t.optString("scope", r.scope), now + t.optLong("expires_in", 0) * 1000L,
                Oidc.earliestRefreshMs(t.opt("earliest_refresh_at")));
            synchronized (state) {
                if (epoch != startEpoch) { // REQ-043.13: disconnected meanwhile
                    discard(refresh, r.clientId);
                    return new Token(null, Failure.NOT_CONNECTED);
                }
                boolean stored = true;
                try {
                    store.save(n); // REQ-043.5: complete before the old refresh token is dropped
                } catch (Exception e) {
                    log("store write failed " + e.getClass().getSimpleName());
                    stored = false; // the old refresh token is spent now: the rotated record is the only valid one and stays in memory until it is stored
                }
                unsaved = !stored;
                record = n;
                unreachable = false;
                changed();
            }
            log("refresh http=" + resp.status + " plan=" + n.hasPlan());
            return n.hasPlan() ? new Token(n.accessToken, null) : new Token(null, Failure.PLAN_NOT_ENABLED);
        }
        log("refresh http=" + resp.status + (code.isEmpty() ? "" : " code=" + code));
        if ((resp.status == 400 || resp.status == 401) && code.equals("invalid_client")) return terminal(startEpoch, "invalid_client", true);
        if ((resp.status == 400 || resp.status == 401) && TERMINAL.contains(code)) return terminal(startEpoch, code, false);
        return temporary(startEpoch, "http_" + resp.status); // 429, 5xx and anything unexpected: keep the credentials (REQ-043.8)
    }

    private Token temporary(long startEpoch, String why) {
        synchronized (state) {
            if (epoch == startEpoch) { unreachable = true; changed(); }
        }
        log("refresh failed temporarily " + why);
        return new Token(null, Failure.UNREACHABLE);
    }

    /** REQ-043.7, REQ-043.9: the tokens are useless; delete them at once. */
    private Token terminal(long startEpoch, String why, boolean dropClientId) {
        synchronized (state) {
            if (epoch == startEpoch) {
                revocationUnconfirmed |= record != null && record.rotationPending;
                epoch++;
                store.deleteRecord();
                if (dropClientId) store.deleteClientId();
                record = null;
                loaded = true;
                unsaved = false;
                expired = true;
                unreachable = false;
                message = (dropClientId ? MSG_INVALID_CLIENT : MSG_EXPIRED) + (revocationUnconfirmed ? " · " + MSG_UNCONFIRMED : "");
                changed();
            }
        }
        log("refresh terminal " + why);
        synchronized (state) { return new Token(null, epoch == startEpoch || expired ? Failure.EXPIRED : Failure.NOT_CONNECTED); }
    }

    /** REQ-043.9 for an invalid_client answer during a sign-in. */
    private void invalidClient(long id) {
        synchronized (state) {
            if (signInId == id) invalidClientNow();
        }
    }

    private void invalidClientNow() {
        synchronized (state) {
            epoch++;
            store.deleteRecord();
            store.deleteClientId();
            record = null;
            loaded = true;
            unsaved = false;
            expired = false;
            unreachable = false;
            changed();
        }
    }

    // ---------------------------------------------------------------- sign-out (REQ-043.10 to .13)

    /**
     * Disconnects: cancels a sign-in or refresh in progress, revokes the refresh token (three attempts), then deletes the credentials
     * locally in any case. Returns false when a disconnect is already running.
     */
    public boolean disconnect() {
        final AccountStore.Record r;
        synchronized (state) {
            if (phase == Phase.DISCONNECTING) return false;
            epoch++; // REQ-043.13: nothing of work in progress is stored from here on
            signInId++;
            if (listener != null) { listener.close(); listener = null; }
            r = current();
            phase = Phase.DISCONNECTING;
            message = "";
            changed();
        }
        log("disconnect start connected=" + (r != null));
        Thread t = new Thread(() -> runDisconnect(r), "account-disconnect");
        t.setDaemon(true);
        t.start();
        return true;
    }

    private void runDisconnect(AccountStore.Record r) {
        boolean revoked = r != null && revoke(r.refreshToken, r.clientId);
        boolean confirmed = revoked && !r.rotationPending;
        synchronized (state) {
            store.deleteRecord();
            record = null;
            loaded = true;
            unsaved = false;
            epoch++;
            expired = false;
            unreachable = false;
            phase = Phase.IDLE;
            revocationUnconfirmed |= r != null && !confirmed;
            message = revocationUnconfirmed ? MSG_UNCONFIRMED : r == null ? "Nicht verbunden" : MSG_REVOKED;
            changed();
        }
        log("disconnect end revoked=" + confirmed);
    }

    /** Up to three attempts after a network error, timeout or 5xx, the pauses of {@link Config#revokeDelaysMs} apart. */
    private boolean revoke(String refreshToken, String clientId) {
        for (int attempt = 0; ; attempt++) {
            int status = -1;
            try {
                String endpoint = discovery(config.revokeTimeoutMs).optString("revocation_endpoint", "");
                if (!Oidc.sameHostHttps(endpoint, host)) return false;
                AccountHttp.Response resp = config.transport.send(AccountHttp.Request.post(endpoint, revocationForm(refreshToken, clientId), config.revokeTimeoutMs));
                status = resp.status;
                if (status == 200) return true;
            } catch (IOException e) {
                status = -1;
            }
            log("revoke attempt " + (attempt + 1) + " http=" + status);
            if ((status != -1 && status < 500) || attempt >= config.revokeDelaysMs.length) return false;
            try {
                Thread.sleep(config.revokeDelaysMs[attempt]);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return false;
            }
        }
    }

    private Map<String, String> revocationForm(String refreshToken, String clientId) {
        Map<String, String> f = new LinkedHashMap<>();
        f.put("token", refreshToken);
        f.put("token_type_hint", "refresh_token");
        f.put("client_id", clientId);
        return f;
    }

    /** One attempt, failures ignored: for a refresh token that was obtained but must not be kept (REQ-042.8, REQ-043.13). */
    private void revokeQuietly(String refreshToken, String clientId) {
        if (refreshToken == null || refreshToken.isEmpty()) return;
        try {
            String endpoint = discovery(config.revokeTimeoutMs).optString("revocation_endpoint", "");
            if (!Oidc.sameHostHttps(endpoint, host)) return;
            AccountHttp.Response r = config.transport.send(AccountHttp.Request.post(endpoint, revocationForm(refreshToken, clientId), config.revokeTimeoutMs));
            log("revoke best effort http=" + r.status);
        } catch (IOException | RuntimeException e) {
            log("revoke best effort failed");
        }
    }
}
