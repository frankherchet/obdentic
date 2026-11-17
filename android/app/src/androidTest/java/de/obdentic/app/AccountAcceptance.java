package de.obdentic.app;

import static de.obdentic.app.AccountFixtures.*;

import android.content.Context;
import android.os.SystemClock;
import java.io.File;
import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.NetworkInterface;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.SocketTimeoutException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.KeyStore;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.Enumeration;
import java.util.List;
import java.util.Map;
import java.util.TreeSet;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicReference;

/**
 * #327 REQ-042, REQ-043, REQ-044 against the real engine with a fake authorization server and a fake browser (no real OpenAI traffic, no
 * real account). Nothing here counts as phone evidence (REQ-044.8).
 */
final class AccountAcceptance {
    private static final java.util.Set<String> AUTHORIZE_NAMES = new TreeSet<>(Arrays.asList("client_id", "agent_name_hint", "ext_agent_host_id", "response_type",
        "redirect_uri", "scope", "resource", "state", "nonce", "code_challenge_method", "code_challenge", "prompt"));
    private static final List<String> secrets = Collections.synchronizedList(new ArrayList<>());

    private AccountAcceptance() {}

    static void run(Context context) throws Exception {
        File logs = new File(context.getCacheDir(), "account-log-" + System.nanoTime());
        File original = new File(context.getFilesDir(), "logs");
        require(logs.mkdirs(), "test setup");
        AppLog.setDirectory(logs);
        try {
            authorizationRequest(context);
            loopbackListener(context);
            invalidCallbacks(context);
            waitingCancelTimeout(context);
            callbackPage(context);
            codeExchange(context);
            idTokenValidation(context);
            planMissing(context);
            statusDialogAndOneAccount(context);
            noBrowserNoPort(context);
            credentialStorage(context);
            unreadableRecord(context);
            accessTokenAndRefresh(context);
            refreshSerialized(context);
            terminalRefresh(context);
            temporaryRefresh(context);
            invalidClient(context);
            disconnect(context);
            raceWithDisconnect(context);
            nothingWithoutAccount(context);
            requestsAreMinimal(context);
            manifestPermissions(context);
            noPathToTheVehicle();
            logMasking();
            logsCarryNoSecrets(logs);
        } finally {
            AppLog.setDirectory(original);
            deleteTree(logs);
        }
    }

    // ---------------------------------------------------------------- helpers

    private static void collect(Fixture f) {
        secrets.addAll(f.auth.secrets);
        secrets.add(f.auth.email);
        secrets.add(f.auth.subject);
        synchronized (f.browser.authorizations) {
            for (Map<String, String> q : f.browser.authorizations) { secrets.add(q.get("state")); secrets.add(q.get("nonce")); }
        }
        synchronized (f.auth.requests) {
            for (Req r : f.auth.requests) {
                for (String k : new String[]{"code", "code_verifier"}) if (r.form.get(k) != null) secrets.add(r.form.get(k));
            }
        }
    }

    private static int port(Fixture f, int index) {
        return Integer.parseInt(f.browser.authorizations.get(index).get("redirect_uri").replaceAll(".*:(\\d+)/auth/callback", "$1"));
    }

    private static boolean canBind(int port) {
        try (ServerSocket s = new ServerSocket()) {
            s.setReuseAddress(true);
            s.bind(new InetSocketAddress(InetAddress.getByAddress(new byte[]{127, 0, 0, 1}), port));
            return true;
        } catch (IOException e) {
            return false;
        }
    }

    private static void deleteTree(File file) {
        File[] children = file.listFiles();
        if (children != null) for (File c : children) deleteTree(c);
        file.delete();
    }

    private static String keyAlias(Fixture f) { return "obdentic-" + f.store.directory().getName(); }

    private static boolean keyExists(Fixture f) throws Exception {
        KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
        ks.load(null);
        return ks.containsAlias(keyAlias(f));
    }

    /** REQ-043.12: after any deletion only the host id (and the client id unless removed) remain. */
    private static void assertDeleted(Fixture f, boolean keepClient, String why) throws Exception {
        File[] files = f.store.directory().listFiles();
        require(files == null || files.length == 0, why + ": no file left in " + f.store.directory().getName());
        require(!keyExists(f), why + ": Keystore key deleted");
        Map<String, ?> prefs = f.context.getSharedPreferences(f.store.directory().getName(), Context.MODE_PRIVATE).getAll();
        java.util.Set<String> keys = new TreeSet<>(prefs.keySet());
        require(keys.equals(new TreeSet<>(keepClient ? Arrays.asList("host_id", "client_id") : Arrays.asList("host_id"))), why + ": only host id" + (keepClient ? " and client id" : "") + " remain: " + keys);
        require(f.store.load() == null, why + ": no record");
    }

    private static String text(String file) throws IOException { return new String(Files.readAllBytes(new File(file).toPath()), StandardCharsets.UTF_8); }

    // ---------------------------------------------------------------- REQ-042

    private static void authorizationRequest(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            f.signInAndWait();
            Map<String, String> q = f.browser.authorizations.get(0);
            String url = f.browser.opened.get(0);
            require(url.startsWith("https://auth.openai.com/api/accounts/authorize?"), "REQ-042.2 authorize endpoint: " + url);
            require("dynamic_agent_client".equals(q.get("client_id")) && "OBDentic".equals(q.get("agent_name_hint")), "REQ-042.2 first registration: " + q);
            require(q.get("ext_agent_host_id").matches("urn:uuid:[0-9a-f-]{36}"), "REQ-042.2 random host id: " + q.get("ext_agent_host_id"));
            require("code".equals(q.get("response_type")) && ChatGptAccount.SCOPE.equals(q.get("scope")) && "https://api.openai.com/v1".equals(q.get("resource")), "REQ-042.2 response type, scope, resource: " + q);
            require(q.get("redirect_uri").matches("http://127\\.0\\.0\\.1:\\d+/auth/callback"), "REQ-042.2 redirect uri on 127.0.0.1, never localhost: " + q.get("redirect_uri"));
            require("S256".equals(q.get("code_challenge_method")) && !q.get("state").isEmpty() && !q.get("nonce").isEmpty() && !q.get("code_challenge").isEmpty(), "REQ-042.2 state, nonce, PKCE S256");
            require(AUTHORIZE_NAMES.containsAll(q.keySet()) && !q.containsKey("prompt") && !q.containsKey("id_token_hint") && !q.containsKey("login_hint") && !q.containsKey("api_key"),
                "REQ-042.2 no API key, login_hint, id_token_hint, vehicle data, no prompt on the first sign-in: " + q.keySet());
            require(f.browser.listeningAtOpen, "REQ-042.2 the listener accepts connections before the browser is opened");
            Req exchange = f.auth.to(TOKEN_URL).get(0);
            require(Oidc.challenge(exchange.form.get("code_verifier")).equals(q.get("code_challenge")), "REQ-042.2 the challenge is the S256 of the verifier sent at the exchange");
            require(f.keepalive.starts.get() == 1 && f.keepalive.stops.get() == 1, "REQ-044.5 the keep-alive is started by the tap and stopped when the attempt ends");

            f.account.disconnect();
            f.waitIdle();
            f.signInAndWait();
            Map<String, String> q2 = f.browser.authorizations.get(1);
            require(f.auth.clientId.equals(q2.get("client_id")) && !q2.containsKey("agent_name_hint"), "REQ-042.2 later sign-in uses the saved client id and no agent name: " + q2);
            require(q.get("ext_agent_host_id").equals(q2.get("ext_agent_host_id")), "REQ-042.2 the host id is reused");
            require(!q.get("state").equals(q2.get("state")) && !q.get("nonce").equals(q2.get("nonce")) && !q.get("code_challenge").equals(q2.get("code_challenge")), "REQ-042.2 fresh state, nonce and challenge per attempt");
        } finally { collect(f); f.close(); }
    }

    private static void loopbackListener(Context ctx) throws Exception {
        LoopbackListener l = LoopbackListener.open(0, 60_000, true);
        int port = l.port();
        try {
            // REQ-042.3: only 127.0.0.1
            Enumeration<NetworkInterface> nics = NetworkInterface.getNetworkInterfaces();
            while (nics != null && nics.hasMoreElements()) {
                for (InetAddress a : Collections.list(nics.nextElement().getInetAddresses())) {
                    if (a.isLoopbackAddress() || a instanceof java.net.Inet6Address) continue;
                    try (Socket s = new Socket()) { s.connect(new InetSocketAddress(a, port), 1000); require(false, "REQ-042.3 the listener must not accept on " + a); }
                    catch (IOException expected) { /* refused or unreachable */ }
                }
            }
            String other = get(port, "/other?code=SECRETCODE");
            String post = new String(rawRequest(port, "POST /auth/callback HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"), StandardCharsets.UTF_8);
            String sub = get(port, "/auth/callback/extra?code=SECRETCODE");
            require(other.startsWith("HTTP/1.1 404") && post.startsWith("HTTP/1.1 404") && sub.startsWith("HTTP/1.1 404"), "REQ-042.3 every other path or method gets 404");
            require(!l.result().isDone(), "REQ-042.3 a 404 does not end the attempt");
            String ok = get(port, "/auth/callback?code=SECRETCODE&state=SECRETSTATE&error=SECRETERROR");
            require(ok.startsWith("HTTP/1.1 200") && !ok.contains("SECRET"), "REQ-042.3 the page never echoes code, state or error: " + ok);
            Map<String, String> result = l.result().get(1, java.util.concurrent.TimeUnit.SECONDS);
            require("SECRETCODE".equals(result.get("code")) && "SECRETSTATE".equals(result.get("state")), "REQ-042.3 the first callback is the result");
            waitUntil(() -> canBind(port), 1000, "REQ-042.3 the port is free within 1 s after the first callback");
        } finally { l.close(); }

        // REQ-042.3: port 1455 first, another free port only when it is taken
        try (ServerSocket held = new ServerSocket()) {
            boolean free = true;
            try { held.setReuseAddress(true); held.bind(new InetSocketAddress(InetAddress.getByAddress(new byte[]{127, 0, 0, 1}), 1455)); } catch (IOException e) { free = false; }
            if (free) {
                LoopbackListener fallback = LoopbackListener.open(1455, 60_000, true);
                try { require(fallback.port() != 1455, "REQ-042.3 a taken port 1455 falls back to another free port"); } finally { fallback.close(); }
                held.close();
                LoopbackListener preferred = LoopbackListener.open(1455, 60_000, true);
                try { require(preferred.port() == 1455, "REQ-042.3 port 1455 is used when free: " + preferred.port()); } finally { preferred.close(); }
                waitUntil(() -> canBind(1455), 1000, "REQ-042.3 1455 can be bound again after cancel");
            }
        }

        // REQ-042.3: closed after 300 s without callback (shortened by the seam) and on cancel
        LoopbackListener timed = LoopbackListener.open(0, 600, true);
        int timedPort = timed.port();
        try { timed.result().get(5, java.util.concurrent.TimeUnit.SECONDS); require(false, "REQ-042.3 the timeout fails the result"); }
        catch (java.util.concurrent.ExecutionException e) { require(e.getCause() instanceof java.util.concurrent.TimeoutException, "REQ-042.3 timeout: " + e.getCause()); }
        waitUntil(() -> canBind(timedPort), 1000, "REQ-042.3 port free after the timeout");
        LoopbackListener cancelled = LoopbackListener.open(0, 60_000, true);
        int cancelledPort = cancelled.port();
        long t0 = SystemClock.elapsedRealtime();
        cancelled.close();
        waitUntil(() -> canBind(cancelledPort), 1000, "REQ-042.3 port free after cancel");
        require(cancelled.result().isCompletedExceptionally() && SystemClock.elapsedRealtime() - t0 < 1000, "REQ-042.3 cancel ends the result within 1 s");
    }

    private static byte[] rawRequest(int port, String request) throws IOException {
        try (Socket s = new Socket("127.0.0.1", port)) {
            s.setSoTimeout(5000);
            s.getOutputStream().write(request.getBytes(StandardCharsets.ISO_8859_1));
            s.getOutputStream().flush();
            return readAll(s.getInputStream()).getBytes(StandardCharsets.UTF_8);
        }
    }

    private static void invalidCallbacks(Context ctx) throws Exception {
        String[][] cases = {
            {"wrong state", "code=c1&state=forged&client_id=issued-x", "Antwort ungültig"},
            {"access denied", "error=access_denied&state=@", "Anmeldung abgebrochen"},
            {"other error", "error=server_error&state=@", "server_error"},
            {"unknown error code", "error=%3Cscript%3E&state=@", "unbekannter Fehler"},
            {"no code", "state=@&client_id=issued-x", "Antwort ungültig"},
            {"no client id", "code=c1&state=@", "Registrierung unvollständig"},
            {"dynamic client id", "code=c1&state=@&client_id=dynamic_agent_client", "Registrierung unvollständig"}};
        for (String[] c : cases) {
            Fixture f = new Fixture(ctx);
            try {
                f.browser.callback = (q, a) -> c[1].replace("@", FakeBrowser.enc(q.get("state")));
                f.signInAndWait();
                ChatGptAccount.Snapshot s = f.account.snapshot();
                require(f.auth.count(TOKEN_URL) == 0, "REQ-042.4 " + c[0] + ": no code exchange");
                require(s.message.contains(c[2]) && !s.message.contains("<script>"), "REQ-042.4 " + c[0] + " names its cause '" + c[2] + "': " + s.message);
                require(s.status == ChatGptAccount.Status.NOT_CONNECTED && s.phase == ChatGptAccount.Phase.IDLE, "REQ-042.4 " + c[0] + ": 'Continue with ChatGPT' offered again");
                require(f.store.load() == null && f.store.clientId() == null, "REQ-042.4 " + c[0] + ": nothing stored");
                require(f.keepalive.stops.get() == 1, "REQ-044.5 " + c[0] + ": the keep-alive stops");
            } finally { collect(f); f.close(); }
        }
    }

    private static void waitingCancelTimeout(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            f.browser.callback = null;
            require(f.account.signIn(), "sign-in accepted");
            waitUntil(() -> f.browser.opened.size() == 1, 5000, "browser opened");
            ChatGptAccount.Snapshot s = f.account.snapshot();
            require(s.phase == ChatGptAccount.Phase.WAITING && f.store.load() == null, "REQ-042.5 waiting state, nothing stored yet");
            require(f.restarted().snapshot().status == ChatGptAccount.Status.NOT_CONNECTED, "REQ-042.5 an app killed while waiting starts as not connected");
            int port = port(f, 0);
            long t0 = SystemClock.elapsedRealtime();
            f.account.cancel();
            waitUntil(() -> f.account.snapshot().phase == ChatGptAccount.Phase.IDLE, 1000, "REQ-042.5 cancel ends the attempt within 1 s");
            require(SystemClock.elapsedRealtime() - t0 < 1000 && f.account.snapshot().message.contains("abgebrochen"), "REQ-042.5 cancel message: " + f.account.snapshot().message);
            waitUntil(() -> canBind(port), 1000, "REQ-042.3 the cancelled attempt released its port");
            require(f.auth.count(TOKEN_URL) == 0 && f.store.load() == null, "REQ-042.5 cancel stores nothing and sends nothing");
        } finally { collect(f); f.close(); }
        Fixture t = new Fixture(ctx);
        try {
            t.config.callbackTimeoutMs = 700;
            t.browser.callback = null;
            t.signInAndWait();
            require(t.account.snapshot().message.equals("Zeitüberschreitung · erneut versuchen"), "REQ-042.5 timeout message: " + t.account.snapshot().message);
            require(t.store.load() == null && t.keepalive.stops.get() == 1, "REQ-042.5/REQ-044.5 timeout stores nothing and stops the keep-alive");
        } finally { collect(t); t.close(); }
    }

    private static void callbackPage(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            f.connect();
            String page = f.browser.callbackResponse;
            require(page.contains("href=\"obdentic://account\"") && page.contains("Zurück zu OBDentic") && !page.contains("code=") && !page.contains("access-"), "REQ-042.6 the page links back without parameters: " + page);
            android.content.Intent view = new android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse("obdentic://account"));
            boolean resolved = false;
            for (android.content.pm.ResolveInfo ri : ctx.getPackageManager().queryIntentActivities(view, 0)) resolved |= ri.activityInfo.name.equals(ReturnActivity.class.getName());
            require(resolved && !LoopbackListener.RETURN_LINK.contains("?"), "REQ-042.6 the link resolves to the return activity");
        } finally { collect(f); f.close(); }
        Fixture n = new Fixture(ctx);
        try {
            n.config.returnLink = false;
            n.connect();
            String page = n.browser.callbackResponse;
            require(!page.contains("href=") && page.contains("Wechsle jetzt zurück"), "REQ-044.7 without the link the page tells the user to switch back: " + page);
        } finally { collect(n); n.close(); }
    }

    private static void codeExchange(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            f.connect();
            Req ex = f.auth.to(TOKEN_URL).get(0);
            require(names(ex).equals(new TreeSet<>(Arrays.asList("grant_type", "client_id", "code", "code_verifier", "redirect_uri", "resource"))), "REQ-042.7 exchange fields: " + names(ex));
            require("authorization_code".equals(ex.form.get("grant_type")) && f.auth.clientId.equals(ex.form.get("client_id")) && ex.form.get("redirect_uri").equals(f.browser.authorizations.get(0).get("redirect_uri"))
                && "https://api.openai.com/v1".equals(ex.form.get("resource")) && "POST".equals(ex.method), "REQ-042.7 exchange values");
            require(ex.body.matches("[A-Za-z0-9_.~%&=-]*"), "REQ-042.7 form encoded: " + ex.body);
        } finally { collect(f); f.close(); }

        Fixture net = new Fixture(ctx);
        try {
            net.auth.token = r -> { throw new IOException("down"); };
            net.signInAndWait();
            require(net.account.snapshot().message.equals("Keine Verbindung zu ChatGPT · erneut versuchen") && net.store.load() == null, "REQ-042.7 network failure: " + net.account.snapshot().message);
        } finally { collect(net); net.close(); }

        String longCode = "x".repeat(70);
        String[][] errors = {{"invalid_grant", "HTTP 400 invalid_grant"}, {longCode, "HTTP 400"}, {"bad code!", "HTTP 400"}};
        for (String[] e : errors) {
            Fixture http = new Fixture(ctx);
            try {
                http.auth.token = r -> json(400, "{\"error\":\"" + e[0] + "\",\"error_description\":\"SECRET-BODY\",\"detail\":\"SECRET-DETAIL\"}");
                http.signInAndWait();
                String m = http.account.snapshot().message;
                require(m.contains(e[1]) && !m.contains("SECRET") && !m.contains(longCode) && !m.contains("bad code"), "REQ-042.7 HTTP error shows status and a valid code only: " + m);
                require(http.store.load() == null && http.store.clientId() == null, "REQ-042.7 an HTTP error stores nothing");
            } finally { collect(http); http.close(); }
        }
        Fixture big = new Fixture(ctx);
        try {
            big.auth.token = r -> new AccountHttp.Response(200, "{\"access_token\":\"", true);
            big.signInAndWait();
            require(big.account.snapshot().message.equals("Antwort ungültig") && big.store.load() == null, "REQ-042.7 an oversized response is not used: " + big.account.snapshot().message);
        } finally { collect(big); big.close(); }
    }

    private static void idTokenValidation(Context ctx) throws Exception {
        List<Object[]> cases = new ArrayList<>();
        cases.add(new Object[]{"bad signature", (java.util.function.Consumer<Fixture>) f -> f.auth.badSignature = true, "signature"});
        cases.add(new Object[]{"wrong issuer", (java.util.function.Consumer<Fixture>) f -> f.auth.issuer = "https://evil.example", "issuer"});
        cases.add(new Object[]{"wrong audience", (java.util.function.Consumer<Fixture>) f -> f.auth.audience = "someone-else", "audience"});
        cases.add(new Object[]{"expired", (java.util.function.Consumer<Fixture>) f -> f.auth.idExpiresInSec = -90, "expired"});
        cases.add(new Object[]{"wrong nonce", (java.util.function.Consumer<Fixture>) f -> f.browser.callback = (q, a) -> { a.nonce = "other-nonce"; return FakeBrowser.approve(q, a); }, "nonce"});
        cases.add(new Object[]{"HS256", (java.util.function.Consumer<Fixture>) f -> f.auth.alg = "HS256", "algorithm"});
        cases.add(new Object[]{"alg none", (java.util.function.Consumer<Fixture>) f -> f.auth.alg = "none", "algorithm"});
        cases.add(new Object[]{"unknown key", (java.util.function.Consumer<Fixture>) f -> f.auth.jwks = r -> json(200, "{\"keys\":[]}"), "key"});
        for (Object[] c : cases) {
            Fixture f = new Fixture(ctx);
            try {
                @SuppressWarnings("unchecked") java.util.function.Consumer<Fixture> setup = (java.util.function.Consumer<Fixture>) c[1];
                setup.accept(f);
                f.signInAndWait();
                String m = f.account.snapshot().message;
                require(m.contains("nicht verifizierbar") && m.contains((String) c[2]), "REQ-042.8 " + c[0] + " shows its cause: " + m);
                require(f.store.load() == null && f.store.clientId() == null && f.account.snapshot().status == ChatGptAccount.Status.NOT_CONNECTED, "REQ-042.8 " + c[0] + " stores nothing");
                List<Req> revoked = f.auth.to(REVOKE_URL);
                require(revoked.size() == 1 && f.auth.latest("refresh-").equals(revoked.get(0).form.get("token")), "REQ-042.8 " + c[0] + " revokes the obtained refresh token");
            } finally { collect(f); f.close(); }
        }
        // leeway of 60 seconds
        Fixture leeway = new Fixture(ctx);
        try {
            leeway.auth.idExpiresInSec = -30;
            leeway.connect();
        } finally { collect(leeway); leeway.close(); }
        // missing parts
        for (String missing : new String[]{"id", "refresh", "access"}) {
            Fixture f = new Fixture(ctx);
            try {
                if (missing.equals("id")) f.auth.omitIdToken = true; else if (missing.equals("refresh")) f.auth.omitRefresh = true; else f.auth.omitAccess = true;
                f.signInAndWait();
                require(f.account.snapshot().message.equals("Antwort ungültig") && f.store.load() == null, "REQ-042.8 missing " + missing + " token stores nothing: " + f.account.snapshot().message);
            } finally { collect(f); f.close(); }
        }
        // happy path
        Fixture ok = new Fixture(ctx);
        try {
            ok.connect();
            AccountStore.Record r = ok.store.load();
            require(r != null && r.clientId.equals(ok.auth.clientId) && r.email.equals("tester@example.test") && r.subject.equals("subject-0001") && r.hasPlan(), "REQ-042.8 a valid sign-in is stored");
            require(ok.store.clientId().equals(ok.auth.clientId), "REQ-042.8 the issued client id is kept");
            require(ok.auth.count(DISCOVERY_URL) >= 1 && ok.auth.count(JWKS_URL) == 1, "REQ-042.8 key set taken from the discovery document");
        } finally { collect(ok); ok.close(); }
    }

    private static void planMissing(Context ctx) throws Exception {
        String noPlan = "openid profile email offline_access resource.invoke";
        Fixture f = new Fixture(ctx);
        try {
            f.auth.scope = noPlan;
            f.signInAndWait();
            ChatGptAccount.Snapshot s = f.account.snapshot();
            require(s.status == ChatGptAccount.Status.PLAN_MISSING && s.email.equals("tester@example.test") && s.status.label.equals("ChatGPT-Plan-Nutzung nicht freigegeben"), "REQ-042.9 sign-in kept, inactive, e-mail shown: " + s.status);
            AccountStore.Record kept = f.store.load();
            require(kept != null && !kept.hasPlan() && f.store.clientId() != null, "REQ-042.9 the record is stored with its granted scope");
            require(f.store.firstUse().isEmpty() && !f.account.firstUseDue(), "REQ-042.11 no one-time dialog for a sign-in without the plan permission");
            int before = f.auth.requests.size();
            ChatGptAccount.Token t = f.account.accessToken();
            require(t.value == null && t.failure == ChatGptAccount.Failure.PLAN_NOT_ENABLED && f.auth.requests.size() == before, "REQ-042.9/REQ-043.4 no access token and no request while inactive");

            // a failed repeat leaves the record unchanged and asks with prompt=consent
            f.browser.callback = (q, a) -> "error=access_denied&state=" + FakeBrowser.enc(q.get("state"));
            require(f.account.signIn(), "REQ-042.9 'Continue with ChatGPT' stays available when inactive");
            f.waitIdle();
            Map<String, String> q = f.browser.authorizations.get(1);
            require("consent".equals(q.get("prompt")) && f.auth.clientId.equals(q.get("client_id")) && ChatGptAccount.SCOPE.equals(q.get("scope")) && !q.containsKey("agent_name_hint"), "REQ-042.9 repeat uses the saved client id, the complete scope and prompt=consent: " + q);
            require(f.account.snapshot().status == ChatGptAccount.Status.PLAN_MISSING && f.store.load().refreshToken.equals(kept.refreshToken), "REQ-042.9 a failed repeat leaves the inactive record unchanged");

            // a successful repeat activates the plan and revokes the replaced refresh token
            f.auth.scope = ChatGptAccount.SCOPE;
            f.browser.callback = FakeBrowser::approve;
            f.signInAndWait();
            require(f.account.snapshot().status == ChatGptAccount.Status.CONNECTED && f.store.load().hasPlan(), "REQ-042.9 a successful repeat activates the plan");
            waitUntil(() -> f.auth.count(REVOKE_URL) == 1, 5000, "REQ-042.9 the replaced refresh token is revoked");
            require(f.auth.to(REVOKE_URL).get(0).form.get("token").equals(kept.refreshToken), "REQ-042.9 the replaced refresh token is the one revoked");
            require(f.store.firstUse().equals("due"), "REQ-042.11 the dialog becomes due once the plan is active");
        } finally { collect(f); f.close(); }
    }

    private static void statusDialogAndOneAccount(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            require(f.account.snapshot().status == ChatGptAccount.Status.NOT_CONNECTED && f.account.snapshot().status.label.equals("Nicht verbunden"), "REQ-042.10 not connected");
            f.connect();
            ChatGptAccount.Snapshot s = f.account.snapshot();
            require(s.status.label.equals("Verbunden") && s.email.equals("tester@example.test"), "REQ-042.10 connected shows the e-mail address");
            require(f.account.openUsage() && f.usageOpened.equals(Collections.singletonList("https://chatgpt.com/settings/usage")), "REQ-042.10 Manage usage opens the usage settings: " + f.usageOpened);
            f.browser.noBrowser = true;
            require(!f.account.openUsage(), "REQ-042.10 Manage usage reports a missing browser");
            f.browser.noBrowser = false;

            // REQ-042.11
            require(f.account.firstUseDue() && f.store.firstUse().equals("due"), "REQ-042.11 the dialog is due after the first connection");
            f.account.firstUseAcknowledged();
            require(!f.account.firstUseDue() && !f.restarted().firstUseDue(), "REQ-042.11 'Got it' is remembered across a restart");
            f.account.disconnect();
            f.waitIdle();
            f.connect();
            require(!f.account.firstUseDue(), "REQ-042.11 not shown again on later sign-ins, also after sign-out");

            // REQ-042.12: connected -> no second attempt
            int opened = f.browser.opened.size();
            require(!f.account.signIn() && f.browser.opened.size() == opened, "REQ-042.12 while connected 'Continue with ChatGPT' starts nothing");
            f.account.disconnect();
            f.waitIdle();
            f.browser.callback = null;
            require(f.account.signIn(), "REQ-042.12 first tap");
            require(!f.account.signIn(), "REQ-042.12 a second tap during an attempt is ignored");
            waitUntil(() -> f.browser.opened.size() == opened + 1, 5000, "browser opened once");
            SystemClock.sleep(300);
            require(f.browser.opened.size() == opened + 1 && f.keepalive.starts.get() == f.keepalive.stops.get() + 1, "REQ-042.12 only one attempt and one keep-alive");
            f.account.cancel();
            f.waitIdle();
        } finally { collect(f); f.close(); }
    }

    private static void noBrowserNoPort(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            AtomicReference<LoopbackListener> listener = new AtomicReference<>();
            f.config.opener = (p, t, r) -> { LoopbackListener l = LoopbackListener.open(p, t, r); listener.set(l); return l; };
            f.browser.noBrowser = true;
            f.signInAndWait();
            require(f.account.snapshot().message.contains("Kein Browser") && f.store.load() == null && f.auth.requests.isEmpty(), "REQ-042.13 no browser: says why, nothing stored, nothing sent: " + f.account.snapshot().message);
            int port = listener.get().port();
            waitUntil(() -> canBind(port), 1000, "REQ-042.13 the listener is closed when no browser opens");
            require(f.keepalive.stops.get() == 1, "REQ-044.5 the keep-alive stops");
        } finally { collect(f); f.close(); }
        Fixture p = new Fixture(ctx);
        try {
            p.config.opener = (port, t, r) -> { throw new IOException("no port"); };
            p.signInAndWait();
            require(p.account.snapshot().message.contains("Kein freier lokaler Port") && p.browser.opened.isEmpty() && p.store.load() == null, "REQ-042.13 no port: nothing opened, nothing stored");
        } finally { collect(p); p.close(); }
    }

    // ---------------------------------------------------------------- REQ-043

    private static void credentialStorage(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            f.connect();
            File file = new File(f.store.directory(), "credential.bin");
            byte[] bytes = Files.readAllBytes(file.toPath());
            String raw = new String(bytes, StandardCharsets.ISO_8859_1);
            for (String secret : f.auth.secrets) require(!raw.contains(secret), "REQ-043.1 the file holds no plaintext token");
            require(!raw.contains("tester@example.test") && !raw.contains("subject-0001") && !raw.contains(f.auth.clientId), "REQ-043.1 no plaintext e-mail, subject or client id");
            require(f.store.directory().getParentFile().equals(ctx.getFilesDir()) && Arrays.equals(f.store.directory().list(), new String[]{"credential.bin"}), "REQ-043.1 one app-private file, no temporary file left: " + Arrays.toString(f.store.directory().list()));
            require((ctx.getApplicationInfo().flags & android.content.pm.ApplicationInfo.FLAG_ALLOW_BACKUP) == 0, "REQ-043.1 not part of the app backup");
            AccountStore.Record r = f.store.load();
            byte[] iv1 = Arrays.copyOfRange(bytes, 1, 13);
            f.store.save(r);
            byte[] iv2 = Arrays.copyOfRange(Files.readAllBytes(file.toPath()), 1, 13);
            f.store.save(r);
            byte[] iv3 = Arrays.copyOfRange(Files.readAllBytes(file.toPath()), 1, 13);
            require(!Arrays.equals(iv1, iv2) && !Arrays.equals(iv2, iv3), "REQ-043.1 a fresh IV for each write");
            // atomic: a failed write leaves the previous record
            File blocker = new File(f.store.directory(), "credential.bin.tmp");
            require(blocker.mkdir(), "test setup");
            try { f.store.save(r.refreshed("changed", "changed", r.scope, 1, 0)); require(false, "REQ-043.1 the blocked write must fail"); } catch (IOException expected) { /* failed write */ }
            require(f.store.load().accessToken.equals(r.accessToken), "REQ-043.1 a failed write leaves the previous record");
            blocker.delete();
            // readable again after a restart of the process (new store object over the same file and Keystore key)
            AccountStore again = new AccountStore(ctx, f.store.directory().getName());
            require(again.load().refreshToken.equals(r.refreshToken) && f.restarted().snapshot().status == ChatGptAccount.Status.CONNECTED, "REQ-043.1 readable again after a restart");
        } finally { collect(f); f.close(); }
    }

    private static void unreadableRecord(Context ctx) throws Exception {
        String[] kinds = {"corrupt file", "unknown format", "key deleted"};
        for (String kind : kinds) {
            Fixture f = new Fixture(ctx);
            try {
                f.connect();
                File file = new File(f.store.directory(), "credential.bin");
                byte[] bytes = Files.readAllBytes(file.toPath());
                if (kind.equals("corrupt file")) { for (int i = 20; i < bytes.length; i++) bytes[i] ^= 0x5a; Files.write(file.toPath(), bytes); }
                else if (kind.equals("unknown format")) { bytes[0] = 9; Files.write(file.toPath(), bytes); }
                else { KeyStore ks = KeyStore.getInstance("AndroidKeyStore"); ks.load(null); ks.deleteEntry(keyAlias(f)); }
                f.account.reload();
                ChatGptAccount.Snapshot s = f.account.snapshot();
                require(s.status == ChatGptAccount.Status.NOT_CONNECTED && s.message.contains("erneut mit ChatGPT verbinden"), "REQ-043.3 " + kind + ": not connected with a hint: " + s.message);
                assertDeleted(f, true, "REQ-043.3 " + kind);
                f.signInAndWait();
                require(f.account.snapshot().status == ChatGptAccount.Status.CONNECTED, "REQ-043.3 " + kind + ": the app keeps working and connects again");
            } finally { collect(f); f.close(); }
        }
    }

    private static void accessTokenAndRefresh(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            f.connect();
            AccountStore.Record first = f.store.load();
            int requests = f.auth.requests.size();
            ChatGptAccount.Token t = f.account.accessToken();
            require(first.accessToken.equals(t.value) && f.auth.requests.size() == requests, "REQ-043.4 a token valid for 300 s or more is returned without a request");
            f.auth.clock.addAndGet((3600 - 299) * 1000L);
            t = f.account.accessToken();
            require(t.value != null && !t.value.equals(first.accessToken) && f.auth.count(TOKEN_URL) == 2, "REQ-043.4 below 300 s the token is refreshed first: " + t.failure);
            Req refresh = f.auth.to(TOKEN_URL).get(1);
            require(names(refresh).equals(new TreeSet<>(Arrays.asList("grant_type", "client_id", "refresh_token", "resource"))), "REQ-043.4 refresh fields, no scope: " + names(refresh));
            require("refresh_token".equals(refresh.form.get("grant_type")) && f.auth.clientId.equals(refresh.form.get("client_id")) && !"dynamic_agent_client".equals(refresh.form.get("client_id"))
                && first.refreshToken.equals(refresh.form.get("refresh_token")) && "https://api.openai.com/v1".equals(refresh.form.get("resource")), "REQ-043.4 refresh values");
            AccountStore.Record second = f.store.load();
            require(second.accessToken.equals(t.value) && !second.refreshToken.equals(first.refreshToken), "REQ-043.5 the rotated record is written before the token is returned");
            require(f.account.snapshot().status == ChatGptAccount.Status.CONNECTED, "REQ-043.5 still connected");
        } finally { collect(f); f.close(); }

        // earliest_refresh_at is never undercut
        Fixture e = new Fixture(ctx);
        try {
            e.auth.earliest = e.auth.now() / 1000 + 7200;
            e.connect();
            e.auth.clock.addAndGet(3500 * 1000L);
            int before = e.auth.requests.size();
            ChatGptAccount.Token held = e.account.accessToken();
            require(held.value != null && e.auth.requests.size() == before, "REQ-043.4 no refresh before earliest_refresh_at");
            e.auth.earliest = null;
            e.auth.clock.addAndGet(3800 * 1000L);
            ChatGptAccount.Token after = e.account.accessToken();
            require(after.value != null && e.auth.count(TOKEN_URL) == 2, "REQ-043.4 refresh once earliest_refresh_at has passed");
        } finally { collect(e); e.close(); }
        require(Oidc.earliestRefreshMs(1_800_000_000L) == 1_800_000_000_000L && Oidc.earliestRefreshMs(1_800_000_000_000L) == 1_800_000_000_000L
            && Oidc.earliestRefreshMs("1800000000") == 1_800_000_000_000L && Oidc.earliestRefreshMs("2027-01-15T08:00:00Z") == 1_800_000_000_000L
            && Oidc.earliestRefreshMs("nonsense") == 0 && Oidc.earliestRefreshMs(null) == 0, "REQ-043.4 earliest_refresh_at as seconds, milliseconds, digits or ISO instant");

        // REQ-043.5 responses that fail the check
        Fixture incomplete = new Fixture(ctx);
        try {
            incomplete.connect();
            incomplete.auth.clock.addAndGet(3500 * 1000L);
            incomplete.auth.omitRefresh = true;
            ChatGptAccount.Token t = incomplete.account.accessToken();
            require(t.value == null && t.failure == ChatGptAccount.Failure.EXPIRED && incomplete.account.snapshot().status == ChatGptAccount.Status.EXPIRED, "REQ-043.5 a refresh without a replacement refresh token is a terminal error");
            assertDeleted(incomplete, true, "REQ-043.5 incomplete refresh");
        } finally { collect(incomplete); incomplete.close(); }
        Fixture scope = new Fixture(ctx);
        try {
            scope.connect();
            scope.auth.clock.addAndGet(3500 * 1000L);
            scope.auth.scope = "openid profile email offline_access resource.invoke";
            ChatGptAccount.Token t = scope.account.accessToken();
            require(t.value == null && t.failure == ChatGptAccount.Failure.PLAN_NOT_ENABLED && scope.account.snapshot().status == ChatGptAccount.Status.PLAN_MISSING, "REQ-043.5 a refresh that loses the plan scope keeps the sign-in inactive: " + t.failure);
            AccountStore.Record r = scope.store.load();
            require(r != null && !r.hasPlan() && r.refreshToken.equals(scope.auth.latest("refresh-")), "REQ-043.5 the rotated tokens are stored even without the plan scope");
        } finally { collect(scope); scope.close(); }
    }

    private static void refreshSerialized(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            f.connect();
            f.auth.clock.addAndGet(3500 * 1000L);
            CountDownLatch release = latch();
            f.auth.token = r -> { try { release.await(); } catch (InterruptedException e) { throw new IOException(e); } return f.auth.issue(r); };
            ChatGptAccount.Token[] out = new ChatGptAccount.Token[2];
            Thread a = new Thread(() -> out[0] = f.account.accessToken()), b = new Thread(() -> out[1] = f.account.accessToken());
            a.start(); b.start();
            waitUntil(() -> f.auth.count(TOKEN_URL) == 2, 5000, "the refresh request arrives");
            SystemClock.sleep(400);
            require(f.auth.count(TOKEN_URL) == 2, "REQ-043.6 the second caller waits, no second refresh request");
            release.countDown();
            a.join(10000); b.join(10000);
            require(out[0].value != null && out[0].value.equals(out[1].value) && f.auth.count(TOKEN_URL) == 2, "REQ-043.6 both callers get the one new access token");
        } finally { collect(f); f.close(); }
    }

    private static void terminalRefresh(Context ctx) throws Exception {
        Object[][] cases = {{400, "invalid_grant"}, {401, "invalid_refresh_token"}, {400, "token_expired"}, {400, "refresh_token_expired"}, {401, "refresh_token_invalidated"}, {400, "refresh_token_reused"}};
        for (Object[] c : cases) {
            Fixture f = new Fixture(ctx);
            try {
                f.connect();
                f.auth.clock.addAndGet(3500 * 1000L);
                String body = c[1].equals("token_expired") ? "{\"error\":{\"code\":\"token_expired\"}}" : "{\"error\":\"" + c[1] + "\"}";
                f.auth.token = r -> json((Integer) c[0], body);
                ChatGptAccount.Token t = f.account.accessToken();
                require(t.value == null && t.failure == ChatGptAccount.Failure.EXPIRED, "REQ-043.7 " + c[1] + " is terminal");
                ChatGptAccount.Snapshot s = f.account.snapshot();
                require(s.status == ChatGptAccount.Status.EXPIRED && s.status.label.equals("Anmeldung abgelaufen · bitte erneut mit ChatGPT anmelden") && s.email.isEmpty(), "REQ-043.7 " + c[1] + " shows the expired state");
                assertDeleted(f, true, "REQ-043.7 " + c[1]);
                require(f.restarted().snapshot().status == ChatGptAccount.Status.NOT_CONNECTED, "REQ-043.7 nothing left after a restart");
                int before = f.auth.requests.size();
                require(f.account.accessToken().failure == ChatGptAccount.Failure.EXPIRED && f.auth.requests.size() == before, "REQ-043.12 no further request with the old tokens");
            } finally { collect(f); f.close(); }
        }
    }

    private static void temporaryRefresh(Context ctx) throws Exception {
        String[] kinds = {"network error", "timeout", "429", "500", "503", "unknown 4xx"};
        for (String kind : kinds) {
            Fixture f = new Fixture(ctx);
            try {
                f.connect();
                AccountStore.Record before = f.store.load();
                f.auth.clock.addAndGet(3500 * 1000L);
                f.auth.token = r -> {
                    switch (kind) {
                        case "network error": throw new IOException("down");
                        case "timeout": throw new SocketTimeoutException("slow");
                        case "429": return json(429, "{\"error\":\"rate_limited\"}");
                        case "500": return json(500, "oops");
                        case "503": return json(503, "{\"error\":\"unavailable\"}");
                        default: return json(400, "{\"error\":\"temporarily_unavailable\"}");
                    }
                };
                ChatGptAccount.Token t = f.account.accessToken();
                require(t.value == null && t.failure == ChatGptAccount.Failure.UNREACHABLE, "REQ-043.8 " + kind + ": no token, unreachable");
                require(f.account.snapshot().status == ChatGptAccount.Status.UNREACHABLE && f.account.snapshot().status.label.equals("ChatGPT nicht erreichbar"), "REQ-043.8 " + kind + " shows 'ChatGPT nicht erreichbar'");
                require(f.store.load().refreshToken.equals(before.refreshToken), "REQ-043.8 " + kind + " keeps every credential");
                SystemClock.sleep(300);
                require(f.auth.count(TOKEN_URL) == 2, "REQ-043.8 " + kind + " starts no background retry");
                f.auth.token = null;
                require(f.account.accessToken().value != null && f.account.snapshot().status == ChatGptAccount.Status.CONNECTED, "REQ-043.8 " + kind + ": the next request tries again and recovers");
            } finally { collect(f); f.close(); }
        }
    }

    private static void invalidClient(Context ctx) throws Exception {
        // at a refresh
        Fixture f = new Fixture(ctx);
        try {
            f.connect();
            f.auth.clock.addAndGet(3500 * 1000L);
            f.auth.token = r -> json(401, "{\"error\":\"invalid_client\"}");
            require(f.account.accessToken().failure == ChatGptAccount.Failure.EXPIRED, "REQ-043.9 invalid_client at a refresh is terminal");
            require(f.account.snapshot().message.contains("Registrierung ungültig"), "REQ-043.9 says the registration is invalid: " + f.account.snapshot().message);
            assertDeleted(f, false, "REQ-043.9 refresh");
            f.auth.token = null;
            f.signInAndWait();
            Map<String, String> q = f.browser.authorizations.get(1);
            require("dynamic_agent_client".equals(q.get("client_id")) && "OBDentic".equals(q.get("agent_name_hint")), "REQ-043.9 the next sign-in registers a new client: " + q);
        } finally { collect(f); f.close(); }
        // in an authorization callback with a plan-less sign-in kept
        Fixture a = new Fixture(ctx);
        try {
            a.auth.scope = "openid profile email offline_access resource.invoke";
            a.signInAndWait();
            a.browser.callback = (q, x) -> "error=invalid_client&state=" + FakeBrowser.enc(q.get("state"));
            a.signInAndWait();
            require(a.account.snapshot().message.contains("Registrierung ungültig") && a.account.snapshot().status == ChatGptAccount.Status.NOT_CONNECTED, "REQ-043.9 invalid_client at the authorization");
            assertDeleted(a, false, "REQ-043.9 authorization");
        } finally { collect(a); a.close(); }
        // at the code exchange
        Fixture x = new Fixture(ctx);
        try {
            x.auth.token = r -> json(400, "{\"error\":\"invalid_client\"}");
            x.signInAndWait();
            require(x.account.snapshot().message.contains("Registrierung ungültig") && x.store.clientId() == null, "REQ-043.9 invalid_client at the code exchange");
        } finally { collect(x); x.close(); }
    }

    private static void disconnect(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            f.connect();
            AccountStore.Record r = f.store.load();
            f.account.disconnect();
            f.waitIdle();
            Req revoke = f.auth.to(REVOKE_URL).get(0);
            require(names(revoke).equals(new TreeSet<>(Arrays.asList("token", "token_type_hint", "client_id"))) && r.refreshToken.equals(revoke.form.get("token"))
                && "refresh_token".equals(revoke.form.get("token_type_hint")) && f.auth.clientId.equals(revoke.form.get("client_id")) && "POST".equals(revoke.method), "REQ-043.10 revocation request: " + revoke.body);
            require(f.account.snapshot().message.equals("Abgemeldet · Zugriff widerrufen") && f.account.snapshot().status == ChatGptAccount.Status.NOT_CONNECTED, "REQ-043.10 confirmed sign-out message");
            assertDeleted(f, true, "REQ-043.10/REQ-043.12 sign-out");
            int before = f.auth.requests.size();
            require(f.account.accessToken().failure == ChatGptAccount.Failure.NOT_CONNECTED && f.auth.requests.size() == before, "REQ-043.12 no further request with the old tokens");
        } finally { collect(f); f.close(); }

        // REQ-043.11: three attempts, 1 s and 3 s apart, then local deletion anyway
        Fixture g = new Fixture(ctx);
        try {
            g.connect();
            int[] n = {0};
            g.auth.revoke = q -> { n[0]++; if (n[0] == 1) throw new IOException("down"); return json(n[0] == 2 ? 503 : 500, "{}"); };
            g.account.disconnect();
            g.waitIdle();
            List<Req> attempts = g.auth.to(REVOKE_URL);
            require(attempts.size() == 3, "REQ-043.11 at most three attempts: " + attempts.size());
            long gap1 = attempts.get(1).at - attempts.get(0).at, gap2 = attempts.get(2).at - attempts.get(1).at;
            require(gap1 >= 950 && gap1 < 2500 && gap2 >= 2950 && gap2 < 5000, "REQ-043.11 attempts 1 s and 3 s apart: " + gap1 + " / " + gap2);
            require(g.account.snapshot().message.contains("nicht bestätigt") && g.account.snapshot().message.contains("Manage usage"), "REQ-043.11 unconfirmed message: " + g.account.snapshot().message);
            assertDeleted(g, true, "REQ-043.11");
        } finally { collect(g); g.close(); }
        Fixture h = new Fixture(ctx);
        try {
            h.connect();
            h.auth.revoke = q -> json(400, "{\"error\":\"invalid_request\"}");
            h.account.disconnect();
            h.waitIdle();
            require(h.auth.count(REVOKE_URL) == 1 && h.account.snapshot().message.contains("nicht bestätigt"), "REQ-043.11 a 4xx is not retried and stays unconfirmed");
            assertDeleted(h, true, "REQ-043.11 4xx");
        } finally { collect(h); h.close(); }
        Fixture i = new Fixture(ctx);
        try {
            i.connect();
            int[] n = {0};
            i.auth.revoke = q -> { n[0]++; return n[0] == 1 ? json(503, "{}") : json(200, "{}"); };
            i.account.disconnect();
            i.waitIdle();
            require(i.auth.count(REVOKE_URL) == 2 && i.account.snapshot().message.equals("Abgemeldet · Zugriff widerrufen"), "REQ-043.11 a retry that succeeds confirms the revocation");
        } finally { collect(i); i.close(); }
    }

    private static void raceWithDisconnect(Context ctx) throws Exception {
        // while waiting for the browser
        Fixture w = new Fixture(ctx);
        try {
            w.browser.callback = null;
            require(w.account.signIn(), "sign-in accepted");
            waitUntil(() -> w.browser.opened.size() == 1, 5000, "browser opened");
            w.account.disconnect();
            w.waitIdle();
            waitUntil(() -> w.keepalive.stops.get() == 1, 3000, "REQ-044.5 keep-alive stopped");
            require(w.store.load() == null && w.account.snapshot().status == ChatGptAccount.Status.NOT_CONNECTED && w.auth.count(TOKEN_URL) == 0, "REQ-043.13 disconnect while waiting: nothing stored");
        } finally { collect(w); w.close(); }

        // during the code exchange
        Fixture x = new Fixture(ctx);
        try {
            CountDownLatch release = latch();
            x.auth.token = r -> { try { release.await(); } catch (InterruptedException e) { throw new IOException(e); } return x.auth.issue(r); };
            require(x.account.signIn(), "sign-in accepted");
            waitUntil(() -> x.auth.count(TOKEN_URL) == 1, 5000, "exchange in flight");
            x.account.disconnect();
            release.countDown();
            x.waitIdle();
            waitUntil(() -> x.keepalive.stops.get() == 1, 5000, "sign-in thread finished");
            SystemClock.sleep(300);
            require(x.store.load() == null && x.account.snapshot().status == ChatGptAccount.Status.NOT_CONNECTED && x.store.clientId() == null, "REQ-043.13 disconnect during the exchange: no token stored afterwards");
            waitUntil(() -> x.auth.count(REVOKE_URL) == 1, 5000, "the discarded refresh token is revoked");
        } finally { collect(x); x.close(); }

        // during a refresh
        Fixture y = new Fixture(ctx);
        try {
            y.connect();
            y.auth.clock.addAndGet(3500 * 1000L);
            CountDownLatch release = latch();
            y.auth.token = r -> { try { release.await(); } catch (InterruptedException e) { throw new IOException(e); } return y.auth.issue(r); };
            ChatGptAccount.Token[] out = new ChatGptAccount.Token[1];
            Thread t = new Thread(() -> out[0] = y.account.accessToken());
            t.start();
            waitUntil(() -> y.auth.count(TOKEN_URL) == 2, 5000, "refresh in flight");
            y.account.disconnect();
            y.waitIdle();
            release.countDown();
            t.join(10000);
            SystemClock.sleep(300);
            require(out[0].value == null && y.store.load() == null && y.account.snapshot().status == ChatGptAccount.Status.NOT_CONNECTED, "REQ-043.13 disconnect during a refresh: the new token is never stored or returned");
        } finally { collect(y); y.close(); }
    }

    // ---------------------------------------------------------------- REQ-044

    private static void nothingWithoutAccount(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            f.account.snapshot();
            f.account.firstUseDue();
            f.account.reload();
            f.account.snapshot();
            require(f.auth.requests.isEmpty(), "REQ-044.1 no request at start, for the state or while a tab is open");
            require(f.account.accessToken().failure == ChatGptAccount.Failure.NOT_CONNECTED && f.auth.requests.isEmpty(), "REQ-044.1 a caller without an account causes no request");
            require(!SignInService.running && f.keepalive.starts.get() == 0, "REQ-044.1 no service without an account or sign-in");
            f.auth.any = r -> { throw new IOException("airplane mode"); };
            f.signInAndWait();
            require(f.account.snapshot().message.equals("Keine Verbindung zu ChatGPT · erneut versuchen") || f.account.snapshot().message.contains("Antwort"), "REQ-044.1 an attempt without network ends with a message: " + f.account.snapshot().message);
        } finally { collect(f); f.close(); }
    }

    private static void requestsAreMinimal(Context ctx) throws Exception {
        Fixture f = new Fixture(ctx);
        try {
            f.connect();
            f.auth.clock.addAndGet(3500 * 1000L);
            f.account.accessToken();
            f.account.disconnect();
            f.waitIdle();
            java.util.Set<String> hostIdNames = new TreeSet<>();
            synchronized (f.auth.requests) {
                for (Req r : f.auth.requests) {
                    require(r.url.startsWith("https://auth.openai.com/"), "REQ-044.2 HTTPS to auth.openai.com only: " + r.url);
                    require(!r.body.matches("(?s).*(WVW|VIN|[0-9A-F]{2}(:[0-9A-F]{2}){5}|\\.jsonl|bluetooth).*"), "REQ-044.2 no vehicle, adapter or recording content in " + r.url);
                    if (r.method.equals("GET")) require(r.body.isEmpty(), "REQ-044.2 key and discovery fetches carry no body");
                    hostIdNames.addAll(r.form.keySet());
                }
            }
            require(new TreeSet<>(Arrays.asList("grant_type", "client_id", "code", "code_verifier", "redirect_uri", "resource", "refresh_token", "token", "token_type_hint")).containsAll(hostIdNames), "REQ-044.2 only the named fields: " + hostIdNames);
        } finally { collect(f); f.close(); }

        // endpoints from the discovery document must be HTTPS on the account host
        String[] bad = {"http://auth.openai.com/api/accounts/jwks", "https://evil.example/jwks", "https://auth.openai.com:8443/jwks", "https://user@auth.openai.com/jwks", "javascript:alert(1)"};
        for (String endpoint : bad) {
            Fixture d = new Fixture(ctx);
            try {
                d.auth.discovery = r -> json(200, "{\"issuer\":\"https://auth.openai.com\",\"jwks_uri\":\"" + endpoint + "\",\"revocation_endpoint\":\"" + endpoint + "\"}");
                d.signInAndWait();
                require(d.store.load() == null && d.auth.requests.size() == 2, "REQ-044.2 discovery endpoint '" + endpoint + "' is not used (requests: " + d.auth.requests.size() + ")");
            } finally { collect(d); d.close(); }
        }
        require(Oidc.sameHostHttps("https://auth.openai.com/a", "auth.openai.com") && Oidc.sameHostHttps("https://auth.openai.com:443/a", "auth.openai.com") && !Oidc.sameHostHttps("https://auth.openai.com.evil.example/a", "auth.openai.com"), "REQ-044.2 same-host rule");
    }

    /** REQ-044.3 (with REQ-018.8): the installed app asks for INTERNET and the dataSync service permission, no storage permission, and no cleartext traffic. */
    private static void manifestPermissions(Context ctx) throws Exception {
        java.util.List<String> p = Arrays.asList(ctx.getPackageManager().getPackageInfo(ctx.getPackageName(), android.content.pm.PackageManager.GET_PERMISSIONS).requestedPermissions);
        require(p.contains("android.permission.INTERNET") && p.contains("android.permission.FOREGROUND_SERVICE_DATA_SYNC") && p.contains("android.permission.FOREGROUND_SERVICE"), "REQ-044.3 INTERNET and the dataSync service permissions are declared: " + p);
        require(Arrays.asList("android.permission.BLUETOOTH", "android.permission.BLUETOOTH_ADMIN", "android.permission.ACCESS_FINE_LOCATION", "android.permission.BLUETOOTH_SCAN", "android.permission.BLUETOOTH_CONNECT",
            "android.permission.POST_NOTIFICATIONS", "android.permission.FOREGROUND_SERVICE", "android.permission.FOREGROUND_SERVICE_CONNECTED_DEVICE", "android.permission.FOREGROUND_SERVICE_DATA_SYNC", "android.permission.INTERNET",
            "android.permission.WAKE_LOCK").containsAll(p), "REQ-044.3 no permission beyond INTERNET and the dataSync service permission was added to the existing ones: " + p);
        for (String s : p) require(!s.contains("STORAGE"), "REQ-018.8 no storage permission: " + s);
        android.content.pm.ApplicationInfo info = ctx.getApplicationInfo();
        require((info.flags & android.content.pm.ApplicationInfo.FLAG_USES_CLEARTEXT_TRAFFIC) == 0, "REQ-044.3 cleartext traffic stays disabled");
    }

    private static void noPathToTheVehicle() {
        Class<?>[] account = {ChatGptAccount.class, AccountStore.class, AccountStore.Record.class, AccountHttp.class, AccountHttp.HttpsTransport.class, AccountHttp.Request.class, AccountHttp.Response.class,
            Oidc.class, LoopbackListener.class, SignInService.class, PlanIndicatorView.class, ChatGptAccount.Config.class, ChatGptAccount.Snapshot.class};
        List<String> forbidden = Arrays.asList("ObdService", "BleTransport", "NativeCore", "VehicleStore", "RecordingExporter", "PollSchedule", "ReconnectPolicy", "MainActivity");
        for (Class<?> c : account) {
            List<Class<?>> types = new ArrayList<>();
            for (java.lang.reflect.Field f : c.getDeclaredFields()) types.add(f.getType());
            for (java.lang.reflect.Method m : c.getDeclaredMethods()) { types.add(m.getReturnType()); types.addAll(Arrays.asList(m.getParameterTypes())); }
            for (java.lang.reflect.Constructor<?> k : c.getDeclaredConstructors()) types.addAll(Arrays.asList(k.getParameterTypes()));
            for (Class<?> t : types) require(!forbidden.contains(t.getSimpleName().replaceAll("\\$.*", "")), "REQ-044.4 " + c.getSimpleName() + " has no path to the vehicle session: " + t);
        }
    }

    private static void logMasking() {
        String jwt = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxMjMifQ.c2lnbmF0dXJl";
        String scrubbed = AppLog.scrub("token " + jwt + " and Authorization: Bearer abc123.def-456_xyz; done");
        require(!scrubbed.contains("eyJ") && !scrubbed.contains("abc123") && scrubbed.contains("[jwt]") && scrubbed.contains("Bearer [token]") && scrubbed.endsWith("; done"), "REQ-043.2 JWT-shaped text and Bearer values are masked: " + scrubbed);
        require(AppLog.scrub("vin WVWZZZ1KZAW123456").contains("[VIN]"), "REQ-029.4 still masks VINs");
    }

    private static void logsCarryNoSecrets(File logs) throws Exception {
        StringBuilder all = new StringBuilder();
        File[] files = logs.listFiles();
        if (files != null) for (File f : files) all.append(text(f.getPath()));
        String log = all.toString();
        require(log.contains("account signin") && log.contains("account disconnect"), "REQ-043.2 account events are logged: " + log.length());
        int checked = 0;
        for (String s : secrets) {
            if (s == null || s.length() < 6) continue;
            require(!log.contains(s), "REQ-043.2 the app log never holds a token, code, state, nonce, verifier, e-mail or subject: " + s.substring(0, 6) + "…");
            checked++;
        }
        require(checked > 50 && !log.contains("Bearer ") && !log.contains("eyJ") && !log.contains("authorize?") && !log.contains("@example.test"), "REQ-043.2 no URL, header or address in the log (" + checked + " values checked)");
    }
}
