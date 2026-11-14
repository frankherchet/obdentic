package de.obdentic.spike.siwc;

import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.net.Uri;
import java.io.BufferedReader;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.math.BigInteger;
import java.net.BindException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.SocketTimeoutException;
import java.net.URL;
import java.net.URLDecoder;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.security.AlgorithmParameters;
import java.security.KeyFactory;
import java.security.MessageDigest;
import java.security.SecureRandom;
import java.security.Signature;
import java.security.interfaces.ECPublicKey;
import java.security.spec.ECGenParameterSpec;
import java.security.spec.ECParameterSpec;
import java.security.spec.ECPoint;
import java.security.spec.ECPublicKeySpec;
import java.security.spec.RSAPublicKeySpec;
import java.util.Arrays;
import java.util.Base64;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import javax.net.ssl.HttpsURLConnection;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

/** Sign in with ChatGPT (open-source direct flow): loopback callback, PKCE, token calls, ID-token validation. */
final class Oauth {
    static final String AUTHORIZE = "https://auth.openai.com/api/accounts/authorize";
    static final String TOKEN = "https://auth.openai.com/api/accounts/oauth/token";
    static final String DISCOVERY = "https://auth.openai.com/.well-known/openid-configuration";
    static final String RESOURCE = "https://api.openai.com/v1";
    static final String SCOPE = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
    static final String PLAN_SCOPE = "chatgpt.tokens.use.direct";
    static final String CALLBACK_PATH = "/auth/callback";
    static final String AGENT_NAME = "OBDentic";
    static final String DYNAMIC_CLIENT = "dynamic_agent_client";
    static final String VARIANT = "A";

    private static final SecureRandom RNG = new SecureRandom();
    private static final Base64.Encoder B64URL = Base64.getUrlEncoder().withoutPadding();
    private static JSONObject discoveryDoc;

    private Oauth() {}

    // ---------------------------------------------------------------- HTTP

    static final class Http {
        final int status;
        final String body;
        String errCode = "";
        String errParam = "";
        boolean detail;

        Http(int status, String body) {
            this.status = status;
            this.body = body == null ? "" : body;
            if (status < 200 || status >= 300) parseError();
        }

        boolean ok() {
            return status >= 200 && status < 300;
        }

        private void parseError() {
            try {
                JSONObject o = new JSONObject(body);
                detail = o.has("detail");
                Object e = o.opt("error");
                if (e instanceof JSONObject) {
                    errCode = safe(((JSONObject) e).optString("code", ""));
                    errParam = safe(((JSONObject) e).optString("param", ""));
                } else if (e instanceof String) {
                    errCode = safe((String) e);
                }
            } catch (JSONException ignored) {
                errCode = "unparseable";
            }
        }

        String summary() {
            StringBuilder sb = new StringBuilder("http=").append(status);
            if (!errCode.isEmpty()) sb.append(" error=").append(errCode);
            if (!errParam.isEmpty()) sb.append(" param=").append(errParam);
            if (detail) sb.append(" detail=present");
            return sb.toString();
        }
    }

    /** Keep only characters of fixed error vocabularies; bounded length. */
    static String safe(String s) {
        if (s == null) return "";
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < s.length() && i < 64; i++) {
            char ch = s.charAt(i);
            boolean okc = (ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || (ch >= '0' && ch <= '9')
                    || ch == '_' || ch == '.' || ch == ':' || ch == '-' || ch == '[' || ch == ']';
            sb.append(okc ? ch : '?');
        }
        return sb.toString();
    }

    static String readAll(InputStream in) throws IOException {
        if (in == null) return "";
        try (InputStream is = in) {
            ByteArrayOutputStream bo = new ByteArrayOutputStream();
            byte[] buf = new byte[8192];
            int n;
            while ((n = is.read(buf)) > 0 && bo.size() < (1 << 20)) bo.write(buf, 0, n);
            return bo.toString("UTF-8");
        }
    }

    /** Opens the connection and sends the optional body; caller reads the response. */
    static HttpsURLConnection open(String method, String url, String contentType, byte[] body, String bearer,
            String accept) throws IOException {
        if (!url.startsWith("https://")) throw new IOException("not_https");
        HttpsURLConnection c = (HttpsURLConnection) new URL(url).openConnection();
        c.setConnectTimeout(30000);
        c.setReadTimeout(30000);
        c.setRequestMethod(method);
        c.setInstanceFollowRedirects(false);
        if (bearer != null) c.setRequestProperty("Authorization", "Bearer " + bearer);
        if (accept != null) c.setRequestProperty("Accept", accept);
        if (body != null) {
            c.setDoOutput(true);
            c.setRequestProperty("Content-Type", contentType);
            c.setFixedLengthStreamingMode(body.length);
            try (OutputStream o = c.getOutputStream()) {
                o.write(body);
            }
        }
        return c;
    }

    static Http request(String method, String url, String contentType, byte[] body, String bearer, String accept)
            throws IOException {
        HttpsURLConnection c = open(method, url, contentType, body, bearer, accept);
        try {
            int status = c.getResponseCode();
            String text = readAll(status >= 400 ? c.getErrorStream() : c.getInputStream());
            return new Http(status, text);
        } finally {
            c.disconnect();
        }
    }

    static String form(Map<String, String> params) throws IOException {
        StringBuilder sb = new StringBuilder();
        for (Map.Entry<String, String> e : params.entrySet()) {
            if (sb.length() > 0) sb.append('&');
            sb.append(URLEncoder.encode(e.getKey(), "UTF-8")).append('=')
                    .append(URLEncoder.encode(e.getValue(), "UTF-8").replace("+", "%20"));
        }
        return sb.toString();
    }

    static Http postForm(String url, Map<String, String> params) throws IOException {
        return request("POST", url, "application/x-www-form-urlencoded",
                form(params).getBytes(StandardCharsets.UTF_8), null, "application/json");
    }

    static synchronized JSONObject discovery() throws IOException, JSONException {
        if (discoveryDoc == null) {
            Http h = request("GET", DISCOVERY, null, null, null, "application/json");
            if (!h.ok()) throw new IOException("discovery_http_" + h.status);
            discoveryDoc = new JSONObject(h.body);
        }
        return discoveryDoc;
    }

    // ---------------------------------------------------------------- host id (S1)

    static String hostId(Context ctx) {
        SharedPreferences sp = ctx.getSharedPreferences("siwc", Context.MODE_PRIVATE);
        String id = sp.getString("host_id", null);
        boolean created = false;
        if (id == null) {
            id = "urn:uuid:" + UUID.randomUUID();
            created = sp.edit().putString("host_id", id).commit();
        }
        boolean persisted = id.equals(sp.getString("host_id", null));
        Report.line("S1", persisted ? "PASS" : "FAIL", "created=" + created + " persisted=" + persisted
                + " length=" + id.length());
        return id;
    }

    // ---------------------------------------------------------------- attempt state

    static final class Attempt {
        final String state = rand32();
        final String nonce = rand32();
        final String verifier = rand32();
        final String challenge;
        String redirectUri;
        final String expectedClientId; // null for new registration

        Attempt(String expectedClientId) throws java.security.GeneralSecurityException {
            this.expectedClientId = expectedClientId;
            this.challenge = B64URL.encodeToString(
                    MessageDigest.getInstance("SHA-256").digest(verifier.getBytes(StandardCharsets.US_ASCII)));
        }
    }

    private static String rand32() {
        byte[] b = new byte[32];
        RNG.nextBytes(b);
        return B64URL.encodeToString(b);
    }

    // ---------------------------------------------------------------- loopback listener (variant A)

    /** Process-level singleton: one daemon thread accepting on 127.0.0.1 until the callback arrives or 300 s pass. */
    static final class CallbackListener {
        private static ServerSocket server;

        private CallbackListener() {}

        static synchronized int start(CompletableFuture<Map<String, String>> result) throws IOException {
            stop();
            InetAddress lo = InetAddress.getByName("127.0.0.1");
            ServerSocket s;
            try {
                s = bind(lo, 1455);
            } catch (BindException e) {
                s = bind(lo, 0);
            }
            server = s;
            final ServerSocket fs = s;
            Thread t = new Thread(() -> serve(fs, result), "siwc-callback");
            t.setDaemon(true);
            t.start();
            return s.getLocalPort();
        }

        private static ServerSocket bind(InetAddress lo, int port) throws IOException {
            ServerSocket s = new ServerSocket();
            try {
                s.setReuseAddress(true);
                s.bind(new InetSocketAddress(lo, port), 1);
            } catch (IOException e) {
                s.close();
                throw e;
            }
            return s;
        }

        static synchronized void stop() {
            if (server != null) {
                try {
                    server.close();
                } catch (IOException ignored) {
                    // already closed
                }
                server = null;
            }
        }

        private static void serve(ServerSocket s, CompletableFuture<Map<String, String>> result) {
            long deadline = System.currentTimeMillis() + 300_000L;
            try {
                while (true) {
                    long left = deadline - System.currentTimeMillis();
                    if (left <= 0) {
                        result.completeExceptionally(new TimeoutException());
                        return;
                    }
                    s.setSoTimeout((int) left);
                    Socket c;
                    try {
                        c = s.accept();
                    } catch (SocketTimeoutException e) {
                        result.completeExceptionally(new TimeoutException());
                        return;
                    }
                    try (Socket cs = c) {
                        if (handle(cs, result)) return;
                    } catch (IOException ignored) {
                        // speculative or broken connection: keep listening
                    }
                }
            } catch (IOException e) {
                result.completeExceptionally(e);
            } finally {
                try {
                    s.close();
                } catch (IOException ignored) {
                    // closing
                }
            }
        }

        /** Returns true when the callback request was served. */
        private static boolean handle(Socket c, CompletableFuture<Map<String, String>> result) throws IOException {
            c.setSoTimeout(5000);
            BufferedReader br = new BufferedReader(
                    new InputStreamReader(c.getInputStream(), StandardCharsets.ISO_8859_1));
            String reqLine = br.readLine();
            if (reqLine == null) return false;
            String h;
            int guard = 0;
            while ((h = br.readLine()) != null && !h.isEmpty() && guard++ < 100) {
                // drain headers
            }
            String[] parts = reqLine.split(" ");
            OutputStream out = c.getOutputStream();
            if (parts.length < 2 || !parts[0].equals("GET")) {
                respond(out, 404, "text/plain; charset=utf-8", "not found");
                return false;
            }
            String target = parts[1];
            int q = target.indexOf('?');
            String path = q < 0 ? target : target.substring(0, q);
            if (!path.equals(CALLBACK_PATH)) {
                respond(out, 404, "text/plain; charset=utf-8", "not found");
                return false;
            }
            Map<String, String> params = new HashMap<>();
            if (q >= 0) {
                for (String kv : target.substring(q + 1).split("&")) {
                    if (kv.isEmpty()) continue;
                    int eq = kv.indexOf('=');
                    String k = eq < 0 ? kv : kv.substring(0, eq);
                    String v = eq < 0 ? "" : kv.substring(eq + 1);
                    try {
                        params.put(URLDecoder.decode(k, "UTF-8"), URLDecoder.decode(v, "UTF-8"));
                    } catch (IllegalArgumentException ignored) {
                        // skip malformed pair
                    }
                }
            }
            respond(out, 200, "text/html; charset=utf-8",
                    "<p>Anmeldung \u00fcbermittelt.</p><p><a href=\"obdentic-siwc-spike://done\">"
                            + "Zur\u00fcck zu OBDentic SIWC Spike</a></p>");
            result.complete(params);
            return true;
        }

        private static void respond(OutputStream out, int status, String type, String body) throws IOException {
            byte[] b = body.getBytes(StandardCharsets.UTF_8);
            String head = "HTTP/1.1 " + status + (status == 200 ? " OK" : " Not Found") + "\r\n"
                    + "Content-Type: " + type + "\r\n"
                    + "Content-Length: " + b.length + "\r\n"
                    + "Connection: close\r\n\r\n";
            out.write(head.getBytes(StandardCharsets.ISO_8859_1));
            out.write(b);
            out.flush();
        }
    }

    // ---------------------------------------------------------------- sign-in (S1, S3, S4, S5)

    private static String authorizeUrl(Attempt a, String hostId, JSONObject stored) throws IOException, JSONException {
        Map<String, String> q = new LinkedHashMap<>();
        if (stored == null) {
            q.put("client_id", DYNAMIC_CLIENT);
            q.put("agent_name_hint", AGENT_NAME);
        } else {
            q.put("client_id", stored.getString("client_id"));
        }
        q.put("ext_agent_host_id", hostId);
        if (stored != null) q.put("id_token_hint", stored.getString("id_token"));
        q.put("response_type", "code");
        q.put("redirect_uri", a.redirectUri);
        q.put("scope", SCOPE);
        q.put("resource", RESOURCE);
        q.put("state", a.state);
        q.put("nonce", a.nonce);
        q.put("code_challenge_method", "S256");
        q.put("code_challenge", a.challenge);
        return AUTHORIZE + "?" + form(q);
    }

    /**
     * Runs one sign-in attempt. {@code stored == null}: new registration; otherwise returning sign-in with the
     * stored session. Returns the new credential record (already persisted) or null on failure. A returning
     * sign-in whose ID token does not validate (including {@code sub} match) returns null and persists nothing.
     */
    static JSONObject signIn(Context ctx, JSONObject stored) {
        boolean returning = stored != null;
        int port = -1;
        try {
            String hostId = hostId(ctx);
            Attempt a = new Attempt(returning ? stored.optString("client_id") : null);
            CompletableFuture<Map<String, String>> fut = new CompletableFuture<>();
            port = CallbackListener.start(fut); // listening before the browser is opened
            a.redirectUri = "http://127.0.0.1:" + port + CALLBACK_PATH;
            String url = authorizeUrl(a, hostId, stored);
            try {
                ctx.startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            } catch (ActivityNotFoundException e) {
                Report.line("S3", "FAIL", "no_browser port=" + port);
                return null;
            }
            Map<String, String> cb;
            try {
                cb = fut.get(310, TimeUnit.SECONDS);
            } catch (ExecutionException e) {
                Throwable c = e.getCause();
                Report.line("S3", "FAIL", (c instanceof TimeoutException ? "timeout" : c.getClass().getSimpleName())
                        + " port=" + port + " variant=" + VARIANT);
                return null;
            } catch (TimeoutException e) {
                Report.line("S3", "FAIL", "timeout port=" + port + " variant=" + VARIANT);
                return null;
            }
            String cbState = cb.get("state") == null ? "" : cb.get("state");
            if (!MessageDigest.isEqual(cbState.getBytes(StandardCharsets.UTF_8),
                    a.state.getBytes(StandardCharsets.UTF_8))) {
                Report.line("S3", "FAIL", "state_mismatch port=" + port + " variant=" + VARIANT);
                return null;
            }
            if (cb.containsKey("error")) {
                Report.line("S3", "FAIL", "error=" + safe(cb.get("error")) + " port=" + port + " variant=" + VARIANT);
                return null;
            }
            String cbClient = cb.get("client_id");
            String clientId;
            if (!returning) {
                if (cbClient == null || cbClient.isEmpty() || cbClient.equals(DYNAMIC_CLIENT)) {
                    Report.line("S3", "FAIL", "no_issued_client_id port=" + port + " variant=" + VARIANT);
                    return null;
                }
                clientId = cbClient;
            } else {
                if (cbClient != null && !cbClient.equals(a.expectedClientId)) {
                    Report.line("S3", "FAIL", "client_id_mismatch port=" + port + " variant=" + VARIANT);
                    return null;
                }
                clientId = a.expectedClientId;
            }
            String code = cb.get("code");
            if (code == null || code.isEmpty()) {
                Report.line("S3", "FAIL", "no_code port=" + port + " variant=" + VARIANT);
                return null;
            }
            Report.line("S3", "PASS", "port=" + port + " variant=" + VARIANT + " returning=" + returning);

            // S4: code exchange
            Map<String, String> f = new LinkedHashMap<>();
            f.put("grant_type", "authorization_code");
            f.put("client_id", clientId);
            f.put("code", code);
            f.put("code_verifier", a.verifier);
            f.put("redirect_uri", a.redirectUri);
            f.put("resource", RESOURCE);
            Http h = postForm(TOKEN, f);
            if (!h.ok()) {
                Report.line("S4", "FAIL", h.summary());
                return null;
            }
            JSONObject t = new JSONObject(h.body);
            boolean planScope = Arrays.asList(t.optString("scope", "").split(" ")).contains(PLAN_SCOPE);
            String detail = "http=" + h.status + " refresh_token=" + hasStr(t, "refresh_token")
                    + " id_token=" + hasStr(t, "id_token") + " earliest_refresh_at=" + t.has("earliest_refresh_at")
                    + " plan_scope=" + planScope;
            if (!planScope) {
                Report.line("S4", "FAIL", "plan_scope_missing " + detail);
                return null;
            }
            Report.line("S4", "PASS", detail);

            // S5: ID-token validation
            String idToken = t.optString("id_token", "");
            String requiredSub = returning ? stored.optString("subject", "") : null;
            boolean valid = validateIdToken(idToken, clientId, a.nonce, requiredSub);
            if (returning && !valid) return null;

            JSONObject claims = unverifiedClaims(idToken);
            JSONObject rec = new JSONObject();
            rec.put("issuer", claims.optString("iss", ""));
            rec.put("subject", claims.optString("sub", ""));
            rec.put("email", claims.optString("email", ""));
            rec.put("client_id", clientId);
            rec.put("ext_agent_host_id", hostId);
            rec.put("id_token", idToken);
            rec.put("access_token", t.optString("access_token", ""));
            rec.put("refresh_token", t.optString("refresh_token", ""));
            rec.put("token_type", t.optString("token_type", ""));
            rec.put("expires_in", t.optLong("expires_in", 0));
            rec.put("scopes", t.optString("scope", ""));
            rec.put("saved_at", System.currentTimeMillis());
            CredentialStore.save(ctx, rec);
            Report.line("store", "PASS", "access_len=" + rec.getString("access_token").length()
                    + " refresh_len=" + rec.getString("refresh_token").length()
                    + " id_len=" + idToken.length() + " expires_in=" + rec.getLong("expires_in"));
            return rec;
        } catch (Exception e) {
            Report.line("signin", "ERR", e.getClass().getSimpleName() + " port=" + port);
            return null;
        } finally {
            CallbackListener.stop();
        }
    }

    private static boolean hasStr(JSONObject o, String k) {
        return !o.optString(k, "").isEmpty();
    }

    // ---------------------------------------------------------------- ID token (S5)

    private static byte[] b64d(String s) {
        return Base64.getUrlDecoder().decode(s);
    }

    private static JSONObject unverifiedClaims(String jwt) {
        try {
            String[] p = jwt.split("\\.");
            return new JSONObject(new String(b64d(p[1]), StandardCharsets.UTF_8));
        } catch (Exception e) {
            return new JSONObject();
        }
    }

    static boolean validateIdToken(String jwt, String clientId, String nonce, String requiredSub) {
        String alg = "?";
        try {
            String[] p = jwt.split("\\.");
            if (p.length != 3) {
                Report.line("S5", "FAIL", "malformed_jwt");
                return false;
            }
            JSONObject header = new JSONObject(new String(b64d(p[0]), StandardCharsets.UTF_8));
            alg = safe(header.optString("alg", "?"));
            String kid = header.optString("kid", "");
            JSONObject disc = discovery();
            Http jh = request("GET", disc.getString("jwks_uri"), null, null, null, "application/json");
            if (!jh.ok()) {
                Report.line("S5", "FAIL", "jwks_" + jh.summary());
                return false;
            }
            JSONArray keys = new JSONObject(jh.body).getJSONArray("keys");
            JSONObject jwk = null;
            for (int i = 0; i < keys.length(); i++) {
                JSONObject k = keys.getJSONObject(i);
                if (k.optString("kid", "").equals(kid)) {
                    jwk = k;
                    break;
                }
            }
            if (jwk == null) {
                Report.line("S5", "FAIL", "kid_not_found alg=" + alg);
                return false;
            }
            byte[] signed = (p[0] + "." + p[1]).getBytes(StandardCharsets.US_ASCII);
            byte[] sig = b64d(p[2]);
            boolean sigOk;
            if (alg.equals("RS256")) {
                RSAPublicKeySpec spec = new RSAPublicKeySpec(
                        new BigInteger(1, b64d(jwk.getString("n"))), new BigInteger(1, b64d(jwk.getString("e"))));
                Signature s = Signature.getInstance("SHA256withRSA");
                s.initVerify(KeyFactory.getInstance("RSA").generatePublic(spec));
                s.update(signed);
                sigOk = s.verify(sig);
            } else if (alg.equals("ES256")) {
                AlgorithmParameters ap = AlgorithmParameters.getInstance("EC");
                ap.init(new ECGenParameterSpec("secp256r1"));
                ECParameterSpec ps = ap.getParameterSpec(ECParameterSpec.class);
                ECPoint pt = new ECPoint(new BigInteger(1, b64d(jwk.getString("x"))),
                        new BigInteger(1, b64d(jwk.getString("y"))));
                ECPublicKey pk = (ECPublicKey) KeyFactory.getInstance("EC").generatePublic(new ECPublicKeySpec(pt, ps));
                Signature s = Signature.getInstance("SHA256withECDSA");
                s.initVerify(pk);
                s.update(signed);
                sigOk = s.verify(joseToDer(sig));
            } else {
                Report.line("S5", "FAIL", "alg_unsupported:" + alg);
                return false;
            }
            JSONObject claims = new JSONObject(new String(b64d(p[1]), StandardCharsets.UTF_8));
            boolean issOk = claims.optString("iss", "").equals(disc.optString("issuer", "\u0000"));
            boolean audOk = false;
            Object aud = claims.opt("aud");
            if (aud instanceof String) audOk = aud.equals(clientId);
            else if (aud instanceof JSONArray) {
                for (int i = 0; i < ((JSONArray) aud).length(); i++) {
                    if (clientId.equals(((JSONArray) aud).optString(i))) audOk = true;
                }
            }
            boolean expOk = claims.optLong("exp", 0) * 1000L > System.currentTimeMillis();
            boolean nonceOk = nonce.equals(claims.optString("nonce", "\u0000"));
            boolean all = sigOk && issOk && audOk && expOk && nonceOk;
            String detail = "alg=" + alg + " sig=" + sigOk + " iss=" + issOk + " aud=" + audOk + " exp=" + expOk
                    + " nonce=" + nonceOk;
            if (requiredSub != null) {
                boolean subOk = !requiredSub.isEmpty() && requiredSub.equals(claims.optString("sub", "\u0000"));
                detail += " sub=" + subOk;
                all &= subOk;
            }
            Report.line("S5", all ? "PASS" : "FAIL", detail);
            return all;
        } catch (Exception e) {
            Report.line("S5", "FAIL", e.getClass().getSimpleName() + " alg=" + alg);
            return false;
        }
    }

    /** JOSE r||s (64 bytes) to DER SEQUENCE { INTEGER r, INTEGER s }. */
    private static byte[] joseToDer(byte[] sig) {
        int h = sig.length / 2;
        byte[] r = new BigInteger(1, Arrays.copyOfRange(sig, 0, h)).toByteArray();
        byte[] s = new BigInteger(1, Arrays.copyOfRange(sig, h, sig.length)).toByteArray();
        ByteArrayOutputStream bo = new ByteArrayOutputStream();
        bo.write(0x30);
        bo.write(4 + r.length + s.length);
        bo.write(0x02);
        bo.write(r.length);
        bo.write(r, 0, r.length);
        bo.write(0x02);
        bo.write(s.length);
        bo.write(s, 0, s.length);
        return bo.toByteArray();
    }

    // ---------------------------------------------------------------- refresh (S12) and sign-out (S13)

    static Http refreshCall(JSONObject rec) throws IOException {
        Map<String, String> f = new LinkedHashMap<>();
        f.put("grant_type", "refresh_token");
        f.put("client_id", rec.optString("client_id", ""));
        f.put("refresh_token", rec.optString("refresh_token", ""));
        f.put("resource", RESOURCE);
        return postForm(TOKEN, f);
    }

    /** New record with access token, refresh token, scopes and expiry replaced together. */
    static JSONObject applyRefresh(JSONObject rec, JSONObject resp) throws JSONException {
        JSONObject n = new JSONObject(rec.toString());
        n.put("access_token", resp.getString("access_token"));
        n.put("refresh_token", resp.optString("refresh_token", rec.optString("refresh_token", "")));
        n.put("scopes", resp.optString("scope", rec.optString("scopes", "")));
        n.put("expires_in", resp.optLong("expires_in", 0));
        if (!resp.optString("id_token", "").isEmpty()) n.put("id_token", resp.getString("id_token"));
        n.put("saved_at", System.currentTimeMillis());
        return n;
    }

    static Http revoke(JSONObject rec) throws IOException, JSONException {
        String endpoint = discovery().getString("revocation_endpoint");
        Map<String, String> f = new LinkedHashMap<>();
        f.put("token", rec.optString("refresh_token", ""));
        f.put("token_type_hint", "refresh_token");
        f.put("client_id", rec.optString("client_id", ""));
        return postForm(endpoint, f);
    }
}
