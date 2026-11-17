package de.obdentic.app;

import android.content.ActivityNotFoundException;
import android.content.Context;
import android.os.SystemClock;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.Signature;
import java.security.interfaces.RSAPublicKey;
import java.util.ArrayList;
import java.util.Base64;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import org.json.JSONArray;
import org.json.JSONObject;

/**
 * Test doubles for the ChatGPT account (#327): an in-process fake authorization server behind the transport seam (never OpenAI), a fake
 * browser that answers the loopback callback through a real socket, and a counting keep-alive. All data is synthetic.
 */
final class AccountFixtures {
    static final String ORIGIN = "https://auth.openai.com";
    static final String TOKEN_URL = ORIGIN + "/api/accounts/oauth/token", REVOKE_URL = ORIGIN + "/api/accounts/oauth/revoke",
        JWKS_URL = ORIGIN + "/.well-known/jwks.json", DISCOVERY_URL = ORIGIN + "/.well-known/openid-configuration";
    private static KeyPair sharedKey;

    private AccountFixtures() {}

    static synchronized KeyPair key() throws Exception {
        if (sharedKey == null) {
            KeyPairGenerator g = KeyPairGenerator.getInstance("RSA");
            g.initialize(2048);
            sharedKey = g.generateKeyPair();
        }
        return sharedKey;
    }

    interface Handler { AccountHttp.Response handle(Req r) throws IOException; }

    /** One request the app sent. */
    static final class Req {
        final String method, url, body;
        final Map<String, String> form;
        final long at = SystemClock.elapsedRealtime();

        Req(AccountHttp.Request r) {
            method = r.method;
            url = r.url;
            body = r.body == null ? "" : r.body;
            form = Oidc.query(body);
        }

        boolean to(String u) { return url.equals(u); }
    }

    static AccountHttp.Response json(int status, String body) { return new AccountHttp.Response(status, body, false); }

    /** The fake authorization server. Defaults answer a perfect sign-in, refresh and revocation. */
    static final class FakeAuth implements AccountHttp.Transport {
        final List<Req> requests = Collections.synchronizedList(new ArrayList<>());
        final AtomicLong clock = new AtomicLong(1_800_000_000_000L);
        final AtomicInteger counter = new AtomicInteger();
        final KeyPair key;
        volatile String nonce = "", clientId = "client-issued-1", scope = ChatGptAccount.SCOPE;
        volatile String issuer = ORIGIN, audience = null, email = "tester@example.test", subject = "subject-0001", alg = "RS256", kid = "key-1";
        volatile long expiresIn = 3600, idExpiresInSec = 3600;
        volatile Object earliest = null;
        volatile boolean badSignature, omitRefresh, omitIdToken, omitAccess;
        volatile Handler token, revoke, jwks, discovery, any;
        /** Everything secret that was handed out, for the "never logged" checks. */
        final List<String> secrets = Collections.synchronizedList(new ArrayList<>());
        /** The refresh tokens that were issued and not yet used or revoked. */
        final java.util.Set<String> live = Collections.synchronizedSet(new java.util.HashSet<>());

        FakeAuth() throws Exception { key = AccountFixtures.key(); }

        long now() { return clock.get(); }

        List<Req> to(String url) {
            List<Req> out = new ArrayList<>();
            synchronized (requests) { for (Req r : requests) if (r.to(url)) out.add(r); }
            return out;
        }

        int count(String url) { return to(url).size(); }

        String lastRefreshToken() { return secrets.isEmpty() ? "" : latest("refresh-"); }

        String latest(String prefix) {
            synchronized (secrets) { for (int i = secrets.size() - 1; i >= 0; i--) if (secrets.get(i).startsWith(prefix)) return secrets.get(i); }
            return "";
        }

        @Override public AccountHttp.Response send(AccountHttp.Request request) throws IOException {
            Req r = new Req(request);
            requests.add(r);
            Handler h = any;
            if (h != null) { AccountHttp.Response o = h.handle(r); if (o != null) return o; }
            if (r.to(DISCOVERY_URL)) {
                if (discovery != null) return discovery.handle(r);
                try {
                    return json(200, new JSONObject().put("issuer", ORIGIN).put("authorization_endpoint", ORIGIN + "/api/accounts/authorize")
                        .put("token_endpoint", TOKEN_URL).put("jwks_uri", JWKS_URL).put("revocation_endpoint", REVOKE_URL).toString());
                } catch (Exception e) { throw new IOException(e); }
            }
            if (r.to(JWKS_URL)) {
                if (jwks != null) return jwks.handle(r);
                try {
                    RSAPublicKey pub = (RSAPublicKey) key.getPublic();
                    Base64.Encoder b = Base64.getUrlEncoder().withoutPadding();
                    JSONObject k = new JSONObject().put("kty", "RSA").put("kid", kid).put("use", "sig").put("alg", "RS256")
                        .put("n", b.encodeToString(unsigned(pub.getModulus().toByteArray()))).put("e", b.encodeToString(unsigned(pub.getPublicExponent().toByteArray())));
                    return json(200, new JSONObject().put("keys", new JSONArray().put(k)).toString());
                } catch (Exception e) { throw new IOException(e); }
            }
            if (r.to(TOKEN_URL)) {
                if (token != null) return token.handle(r);
                return issue(r);
            }
            if (r.to(REVOKE_URL)) { live.remove(r.form.get("token")); return revoke != null ? revoke.handle(r) : json(200, "{}"); }
            throw new IOException("unexpected request " + r.method + " " + r.url);
        }

        private static byte[] unsigned(byte[] v) {
            if (v.length > 1 && v[0] == 0) { byte[] o = new byte[v.length - 1]; System.arraycopy(v, 1, o, 0, o.length); return o; }
            return v;
        }

        /** The default token endpoint: authorization_code and refresh_token grants. */
        AccountHttp.Response issue(Req r) throws IOException {
            try {
                if ("refresh_token".equals(r.form.get("grant_type")) && !live.remove(r.form.get("refresh_token"))) { // single-use rotation: a spent or unknown token is refused
                    return json(400, new JSONObject().put("error", "refresh_token_reused").toString());
                }
                int n = counter.incrementAndGet();
                JSONObject o = new JSONObject();
                if (!omitAccess) { String a = "access-" + n; secrets.add(a); o.put("access_token", a); }
                if (!omitRefresh) { String rt = "refresh-" + n; secrets.add(rt); live.add(rt); o.put("refresh_token", rt); }
                if (!omitIdToken && "authorization_code".equals(r.form.get("grant_type"))) {
                    String id = idToken(r.form.get("client_id"));
                    secrets.add(id);
                    o.put("id_token", id);
                }
                o.put("token_type", "Bearer").put("expires_in", expiresIn).put("scope", scope);
                if (earliest != null) o.put("earliest_refresh_at", earliest);
                return json(200, o.toString());
            } catch (Exception e) { throw new IOException(e); }
        }

        String idToken(String client) throws Exception {
            Base64.Encoder b = Base64.getUrlEncoder().withoutPadding();
            JSONObject header = new JSONObject().put("alg", alg).put("typ", "JWT");
            if (kid != null) header.put("kid", kid);
            JSONObject claims = new JSONObject().put("iss", issuer).put("sub", subject).put("aud", audience == null ? client : audience)
                .put("exp", now() / 1000 + idExpiresInSec).put("iat", now() / 1000).put("nonce", nonce).put("email", email);
            String signed = b.encodeToString(header.toString().getBytes(StandardCharsets.UTF_8)) + "." + b.encodeToString(claims.toString().getBytes(StandardCharsets.UTF_8));
            Signature s = Signature.getInstance("SHA256withRSA");
            s.initSign(key.getPrivate());
            s.update(signed.getBytes(StandardCharsets.US_ASCII));
            byte[] sig = s.sign();
            if (badSignature) sig[sig.length - 1] ^= 1;
            return signed + "." + b.encodeToString(sig);
        }
    }

    /** What the fake browser sends back to the loopback listener. */
    interface Callback { String query(Map<String, String> authorize, FakeAuth auth); }

    /** Fake system browser: proves the listener already accepts, then answers the callback through a real socket. */
    static final class FakeBrowser implements ChatGptAccount.Launcher {
        final List<String> opened = Collections.synchronizedList(new ArrayList<>());
        final List<Map<String, String>> authorizations = Collections.synchronizedList(new ArrayList<>());
        final FakeAuth auth;
        volatile Callback callback;
        volatile boolean noBrowser, listeningAtOpen;
        volatile String callbackResponse = "";
        volatile int delayMs;

        FakeBrowser(FakeAuth auth) {
            this.auth = auth;
            this.callback = FakeBrowser::approve;
        }

        /** Approves: state echoed, a code, and the issued client id on first registration. */
        static String approve(Map<String, String> q, FakeAuth auth) {
            String s = "code=code-" + auth.counter.get() + "&state=" + enc(q.get("state"));
            return "dynamic_agent_client".equals(q.get("client_id")) ? s + "&client_id=" + enc(auth.clientId) : s;
        }

        static String enc(String v) { try { return java.net.URLEncoder.encode(v, "UTF-8"); } catch (Exception e) { throw new IllegalStateException(e); } }

        @Override public void open(String url) {
            if (noBrowser) throw new ActivityNotFoundException("no browser");
            opened.add(url);
            Map<String, String> q = Oidc.query(url.substring(url.indexOf('?') + 1));
            authorizations.add(q);
            auth.nonce = q.get("nonce");
            int port = Integer.parseInt(q.get("redirect_uri").replaceAll(".*:(\\d+)/auth/callback", "$1"));
            try (Socket probe = new Socket("127.0.0.1", port)) { listeningAtOpen = probe.isConnected(); } catch (IOException e) { listeningAtOpen = false; }
            Callback cb = callback;
            if (cb == null) return;
            Thread t = new Thread(() -> {
                try {
                    if (delayMs > 0) Thread.sleep(delayMs);
                    String query = cb.query(q, auth);
                    if (query != null) callbackResponse = get(port, "/auth/callback?" + query);
                } catch (Exception ignored) { /* listener may already be closed by the test */ }
            }, "fake-browser");
            t.setDaemon(true);
            t.start();
        }
    }

    /** One raw HTTP GET to the loopback listener; returns the whole response. */
    static String get(int port, String target) throws IOException {
        try (Socket s = new Socket("127.0.0.1", port)) {
            s.setSoTimeout(5000);
            s.getOutputStream().write(("GET " + target + " HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").getBytes(StandardCharsets.ISO_8859_1));
            s.getOutputStream().flush();
            return readAll(s.getInputStream());
        }
    }

    static String readAll(InputStream in) throws IOException {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        byte[] b = new byte[4096];
        int n;
        while ((n = in.read(b)) > 0) out.write(b, 0, n);
        return out.toString("UTF-8");
    }

    static final class CountingKeepalive implements ChatGptAccount.Keepalive {
        final AtomicInteger starts = new AtomicInteger(), stops = new AtomicInteger();
        volatile long lastStop;

        @Override public void start() { starts.incrementAndGet(); }
        @Override public void stop() { lastStop = SystemClock.elapsedRealtime(); stops.incrementAndGet(); }
    }

    /** An account over its own store, fake server, fake browser and fake keep-alive. */
    static final class Fixture {
        final Context context;
        final FlakyStore store;
        final FakeAuth auth;
        final FakeBrowser browser;
        final CountingKeepalive keepalive = new CountingKeepalive();
        final ChatGptAccount.Config config;
        final ChatGptAccount account;
        final List<String> usageOpened = Collections.synchronizedList(new ArrayList<>());

        Fixture(Context context) throws Exception {
            this.context = context;
            store = new FlakyStore(context, "account-test-" + System.nanoTime());
            auth = new FakeAuth();
            browser = new FakeBrowser(auth);
            config = new ChatGptAccount.Config();
            config.transport = auth;
            config.store = store;
            config.keepalive = keepalive;
            config.preferredPort = 0;
            config.clock = auth.clock::get;
            config.launcher = url -> { if (url.startsWith("https://chatgpt.com")) { usageOpened.add(url); if (browser.noBrowser) throw new ActivityNotFoundException("none"); } else browser.open(url); };
            account = new ChatGptAccount(config);
        }

        /** A second engine over the same store, as after a restart of the app. */
        ChatGptAccount restarted() { return new ChatGptAccount(config); }

        void signInAndWait() throws Exception {
            require(account.signIn(), "sign-in accepted");
            waitIdle();
        }

        void waitIdle() throws Exception { waitUntil(() -> account.snapshot().phase == ChatGptAccount.Phase.IDLE, 15000, "sign-in or disconnect ends"); }

        /** A completed first sign-in with the plan permission. */
        void connect() throws Exception {
            signInAndWait();
            require(account.snapshot().status == ChatGptAccount.Status.CONNECTED, "connected after sign-in: " + account.snapshot().status + " " + account.snapshot().message);
        }

        void close() {
            account.disconnect();
            try { waitIdle(); } catch (Exception ignored) { /* cleanup */ }
            store.deleteAll();
        }
    }

    /** A store whose writes can be made to fail. */
    static final class FlakyStore extends AccountStore {
        volatile boolean failSave;

        FlakyStore(Context context, String name) { super(context, name); }

        @Override synchronized void save(Record record) throws java.security.GeneralSecurityException, IOException, org.json.JSONException {
            if (failSave) throw new IOException("injected write failure");
            super.save(record);
        }
    }

    interface Check { boolean ok() throws Exception; }

    static void waitUntil(Check check, long timeoutMs, String what) throws Exception {
        long until = SystemClock.elapsedRealtime() + timeoutMs;
        while (SystemClock.elapsedRealtime() < until) { if (check.ok()) return; SystemClock.sleep(20); }
        throw new AssertionError("Timeout: " + what);
    }

    static void require(boolean ok, String message) { if (!ok) throw new AssertionError(message); }

    static CountDownLatch latch() { return new CountDownLatch(1); }

    static void await(CountDownLatch l) throws InterruptedException { l.await(15, TimeUnit.SECONDS); }

    /** Request parameter names of a form body. */
    static java.util.Set<String> names(Req r) { return new java.util.TreeSet<>(r.form.keySet()); }

    static Map<String, String> map(String... kv) {
        Map<String, String> m = new HashMap<>();
        for (int i = 0; i < kv.length; i += 2) m.put(kv[i], kv[i + 1]);
        return m;
    }
}
