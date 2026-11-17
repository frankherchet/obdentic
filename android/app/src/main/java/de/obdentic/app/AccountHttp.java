package de.obdentic.app;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.URL;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import javax.net.ssl.HostnameVerifier;
import javax.net.ssl.HttpsURLConnection;
import javax.net.ssl.SSLSocketFactory;
import org.json.JSONException;
import org.json.JSONObject;

/** REQ-042.7, REQ-044.2: the only network code of the ChatGPT account. Platform HTTPS only, no redirects, bounded reads. */
public final class AccountHttp {
    /** REQ-042.7: responses are read up to 1 MiB. */
    public static final int LIMIT = 1 << 20;
    public static final int LONG_TIMEOUT_MS = 30_000;

    private AccountHttp() {}

    /** One request; {@code body} is form encoded, null for a GET. */
    public static final class Request {
        public final String method, url, body;
        public final int timeoutMs;

        public Request(String method, String url, String body, int timeoutMs) {
            this.method = method;
            this.url = url;
            this.body = body;
            this.timeoutMs = timeoutMs;
        }

        public static Request get(String url, int timeoutMs) { return new Request("GET", url, null, timeoutMs); }

        public static Request post(String url, Map<String, String> form, int timeoutMs) throws IOException {
            return new Request("POST", url, form(form), timeoutMs);
        }
    }

    public static final class Response {
        public final int status;
        public final String body;
        /** The server sent more than {@link #LIMIT} bytes; {@link #body} holds the first part only. */
        public final boolean truncated;

        public Response(int status, String body, boolean truncated) {
            this.status = status;
            this.body = body == null ? "" : body;
            this.truncated = truncated;
        }

        public boolean ok() { return status >= 200 && status < 300 && !truncated; }

        /** The JSON body, or null when it is not a complete JSON object. */
        public JSONObject json() {
            if (truncated) return null;
            try { return new JSONObject(body); } catch (JSONException e) { return null; }
        }

        /** Error code of an error response ("error":"x" or "error":{"code":"x"}); "" when absent or not a valid code. */
        public String errorCode() {
            JSONObject o = json();
            if (o == null) return "";
            Object e = o.opt("error");
            String code = e instanceof JSONObject ? ((JSONObject) e).optString("code", "") : e instanceof String ? (String) e : "";
            return validCode(code) ? code : "";
        }
    }

    /** REQ-042.7: at most 64 characters from letters, digits and '_.:-'. */
    public static boolean validCode(String code) {
        if (code == null || code.isEmpty() || code.length() > 64) return false;
        for (int i = 0; i < code.length(); i++) {
            char c = code.charAt(i);
            if (!((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_' || c == '.' || c == ':' || c == '-')) return false;
        }
        return true;
    }

    public interface Transport {
        /** Throws an IOException for every network level failure (no connection, timeout, TLS, not HTTPS). */
        Response send(Request request) throws IOException;
    }

    public static String form(Map<String, String> params) throws IOException {
        StringBuilder sb = new StringBuilder();
        for (Map.Entry<String, String> e : params.entrySet()) {
            if (sb.length() > 0) sb.append('&');
            sb.append(URLEncoder.encode(e.getKey(), "UTF-8")).append('=').append(URLEncoder.encode(e.getValue(), "UTF-8").replace("+", "%20"));
        }
        return sb.toString();
    }

    /** HttpsURLConnection transport. The two optional arguments exist only for the local test server of the acceptance run. */
    public static final class HttpsTransport implements Transport {
        private final SSLSocketFactory sockets;
        private final HostnameVerifier names;

        public HttpsTransport() { this(null, null); }

        public HttpsTransport(SSLSocketFactory sockets, HostnameVerifier names) {
            this.sockets = sockets;
            this.names = names;
        }

        @Override public Response send(Request r) throws IOException {
            if (!r.url.startsWith("https://")) throw new IOException("not_https");
            HttpsURLConnection c = (HttpsURLConnection) new URL(r.url).openConnection();
            try {
                if (sockets != null) c.setSSLSocketFactory(sockets);
                if (names != null) c.setHostnameVerifier(names);
                c.setConnectTimeout(r.timeoutMs);
                c.setReadTimeout(r.timeoutMs);
                c.setInstanceFollowRedirects(false); // REQ-042.7, REQ-044.2
                c.setUseCaches(false);
                c.setRequestMethod(r.method);
                // The platform default would carry the device model and Android version (REQ-044.2).
                c.setRequestProperty("User-Agent", "OBDentic");
                c.setRequestProperty("Accept", "application/json");
                if (r.body != null) {
                    byte[] bytes = r.body.getBytes(StandardCharsets.UTF_8);
                    c.setDoOutput(true);
                    c.setRequestProperty("Content-Type", "application/x-www-form-urlencoded");
                    c.setFixedLengthStreamingMode(bytes.length);
                    try (OutputStream out = c.getOutputStream()) { out.write(bytes); }
                }
                int status = c.getResponseCode();
                InputStream in = status >= 400 ? c.getErrorStream() : c.getInputStream();
                return read(status, in);
            } finally {
                c.disconnect();
            }
        }

        private static Response read(int status, InputStream in) throws IOException {
            if (in == null) return new Response(status, "", false);
            try (InputStream is = in) {
                ByteArrayOutputStream bytes = new ByteArrayOutputStream();
                byte[] buffer = new byte[8192];
                boolean truncated = false;
                int n;
                while ((n = is.read(buffer)) > 0) {
                    if (bytes.size() + n > LIMIT) { bytes.write(buffer, 0, LIMIT - bytes.size()); truncated = true; break; }
                    bytes.write(buffer, 0, n);
                }
                return new Response(status, bytes.toString("UTF-8"), truncated);
            }
        }
    }
}
