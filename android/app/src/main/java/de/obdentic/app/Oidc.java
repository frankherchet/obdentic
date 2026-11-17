package de.obdentic.app;

import java.math.BigInteger;
import java.net.URI;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.KeyFactory;
import java.security.MessageDigest;
import java.security.SecureRandom;
import java.security.Signature;
import java.security.spec.RSAPublicKeySpec;
import java.util.Base64;
import java.util.HashMap;
import java.util.Map;
import org.json.JSONArray;
import org.json.JSONObject;

/** Pure OAuth/OIDC helpers (REQ-042.2, REQ-042.8): random values, PKCE, callback parsing and ID-token validation. No I/O. */
public final class Oidc {
    private static final SecureRandom RNG = new SecureRandom();
    private static final Base64.Encoder B64URL = Base64.getUrlEncoder().withoutPadding();
    /** REQ-042.8: clock leeway for the ID-token expiry. */
    public static final long LEEWAY_MS = 60_000;
    private static final int MIN_RSA_BITS = 2048;

    private Oidc() {}

    /** 32 random bytes, base64url: state, nonce, PKCE verifier and the host id suffix. */
    public static String random() {
        byte[] b = new byte[32];
        RNG.nextBytes(b);
        return B64URL.encodeToString(b);
    }

    /** PKCE S256. */
    public static String challenge(String verifier) {
        try {
            return B64URL.encodeToString(MessageDigest.getInstance("SHA-256").digest(verifier.getBytes(StandardCharsets.US_ASCII)));
        } catch (GeneralSecurityException e) {
            throw new IllegalStateException(e);
        }
    }

    public static boolean same(String a, String b) {
        return a != null && b != null && MessageDigest.isEqual(a.getBytes(StandardCharsets.UTF_8), b.getBytes(StandardCharsets.UTF_8));
    }

    /** First value of every key of a query string; malformed pairs are skipped. */
    public static Map<String, String> query(String raw) {
        Map<String, String> out = new HashMap<>();
        if (raw == null) return out;
        int pairs = 0;
        for (String kv : raw.split("&")) {
            if (kv.isEmpty() || ++pairs > 50) continue;
            int eq = kv.indexOf('=');
            try {
                String k = URLDecoder.decode(eq < 0 ? kv : kv.substring(0, eq), "UTF-8");
                String v = eq < 0 ? "" : URLDecoder.decode(kv.substring(eq + 1), "UTF-8");
                if (!out.containsKey(k)) out.put(k, v);
            } catch (Exception ignored) {
                // malformed percent encoding: skip the pair
            }
        }
        return out;
    }

    /** REQ-044.2: an endpoint taken from the discovery document must be HTTPS on the account host (default port, no user info). */
    public static boolean sameHostHttps(String url, String host) {
        try {
            URI u = new URI(url);
            return "https".equals(u.getScheme()) && host.equals(u.getHost()) && (u.getPort() == -1 || u.getPort() == 443) && u.getUserInfo() == null;
        } catch (Exception e) {
            return false;
        }
    }

    /** Parses {@code earliest_refresh_at}: epoch seconds or milliseconds, or an ISO-8601 instant; 0 when absent or unreadable. */
    public static long earliestRefreshMs(Object value) {
        try {
            if (value instanceof Number) {
                long n = ((Number) value).longValue();
                return n > 100_000_000_000L ? n : n * 1000L;
            }
            if (value instanceof String && !((String) value).isEmpty()) {
                String s = (String) value;
                if (s.matches("\\d+")) return earliestRefreshMs(Long.parseLong(s));
                return java.time.Instant.parse(s).toEpochMilli();
            }
        } catch (RuntimeException ignored) {
            // unreadable: no restriction
        }
        return 0;
    }

    /** Claims taken from a validated ID token. */
    public static final class IdToken {
        public final String subject, email;

        public IdToken(String subject, String email) {
            this.subject = subject;
            this.email = email;
        }
    }

    /** Thrown with a cause from a fixed vocabulary: format, algorithm, key, signature, issuer, audience, expired, nonce, subject. */
    public static final class Invalid extends Exception {
        public final String cause;

        public Invalid(String cause) {
            super(cause);
            this.cause = cause;
        }
    }

    private static byte[] b64(String s) throws Invalid {
        try { return Base64.getUrlDecoder().decode(s); } catch (IllegalArgumentException e) { throw new Invalid("format"); }
    }

    /**
     * REQ-042.8: RS256 signature against a key of {@code jwks}, issuer, audience containing the client id, expiry in the
     * future (60 s leeway), nonce and a subject.
     */
    public static IdToken validateIdToken(String jwt, JSONObject jwks, String issuer, String clientId, String nonce, long nowMs) throws Invalid {
        if (jwt == null) throw new Invalid("format");
        String[] p = jwt.split("\\.", -1);
        if (p.length != 3 || p[2].isEmpty()) throw new Invalid("format");
        JSONObject header, claims;
        try {
            header = new JSONObject(new String(b64(p[0]), StandardCharsets.UTF_8));
            claims = new JSONObject(new String(b64(p[1]), StandardCharsets.UTF_8));
        } catch (org.json.JSONException e) {
            throw new Invalid("format");
        }
        if (!"RS256".equals(header.optString("alg", ""))) throw new Invalid("algorithm");
        JSONObject jwk = key(jwks, header.optString("kid", ""));
        try {
            RSAPublicKeySpec spec = new RSAPublicKeySpec(new BigInteger(1, b64(jwk.getString("n"))), new BigInteger(1, b64(jwk.getString("e"))));
            if (spec.getModulus().bitLength() < MIN_RSA_BITS) throw new Invalid("key");
            Signature s = Signature.getInstance("SHA256withRSA");
            s.initVerify(KeyFactory.getInstance("RSA").generatePublic(spec));
            s.update((p[0] + "." + p[1]).getBytes(StandardCharsets.US_ASCII));
            if (!s.verify(b64(p[2]))) throw new Invalid("signature");
        } catch (GeneralSecurityException | org.json.JSONException e) {
            throw new Invalid("signature");
        }
        if (!same(issuer, claims.optString("iss", null))) throw new Invalid("issuer");
        boolean aud = false;
        Object a = claims.opt("aud");
        if (a instanceof String) aud = clientId.equals(a);
        else if (a instanceof JSONArray) for (int i = 0; i < ((JSONArray) a).length(); i++) aud |= clientId.equals(((JSONArray) a).optString(i, null));
        if (!aud) throw new Invalid("audience");
        if (!claims.has("exp") || claims.optLong("exp", 0) * 1000L + LEEWAY_MS <= nowMs) throw new Invalid("expired");
        if (!same(nonce, claims.optString("nonce", null))) throw new Invalid("nonce");
        String sub = claims.optString("sub", "");
        if (sub.isEmpty()) throw new Invalid("subject");
        return new IdToken(sub, claims.optString("email", ""));
    }

    private static JSONObject key(JSONObject jwks, String kid) throws Invalid {
        JSONArray keys = jwks == null ? null : jwks.optJSONArray("keys");
        if (keys == null) throw new Invalid("key");
        JSONObject only = null;
        int rsa = 0;
        for (int i = 0; i < keys.length(); i++) {
            JSONObject k = keys.optJSONObject(i);
            if (k == null || !"RSA".equals(k.optString("kty")) || !(k.optString("use", "sig").equals("sig"))) continue;
            rsa++;
            only = k;
            if (!kid.isEmpty() && kid.equals(k.optString("kid", null))) return k;
        }
        if (kid.isEmpty() && rsa == 1) return only; // a token without kid is accepted only when the key set has exactly one candidate
        throw new Invalid("key");
    }
}
