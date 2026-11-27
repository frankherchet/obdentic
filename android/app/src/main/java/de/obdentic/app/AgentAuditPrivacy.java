package de.obdentic.app;

import org.json.JSONArray;
import org.json.JSONObject;
import java.util.Iterator;

/** Deterministic secret boundary; vehicle identifiers are not classified as credentials. */
public final class AgentAuditPrivacy {
    private AgentAuditPrivacy() {}

    public static boolean containsSecret(String text, String[] secrets) {
        if (text == null) return false;
        if (AppLog.containsCredentialShape(text)) return true;
        for (String secret : secrets) {
            if (secret == null || secret.isEmpty()) continue;
            if (text.contains(secret)) return true;
            for (int i = 0; i + 16 <= secret.length(); i++)
                if (text.contains(secret.substring(i, i + 16))) return true;
        }
        return false;
    }

    /** Check decoded strings and object keys, not only their JSON-escaped representation. */
    public static boolean containsSecret(Object value, String[] secrets) {
        if (value instanceof String) return containsSecret((String) value, secrets);
        if (value instanceof JSONObject) {
            JSONObject object = (JSONObject) value;
            Iterator<String> keys = object.keys();
            while (keys.hasNext()) {
                String key = keys.next();
                if (containsSecret(key, secrets) || containsSecret(object.opt(key), secrets)) return true;
            }
        } else if (value instanceof JSONArray) {
            JSONArray array = (JSONArray) value;
            for (int i = 0; i < array.length(); i++) if (containsSecret(array.opt(i), secrets)) return true;
        }
        return false;
    }

    /** Redact selected, known values only; the preview records every occurrence, including overlaps. */
    public static JSONObject redact(String text, JSONObject known, JSONArray selected) throws org.json.JSONException {
        boolean[] hidden = new boolean[text.length()];
        JSONArray matches = new JSONArray();
        for (int i = 0; i < selected.length(); i++) {
            String category = selected.optString(i);
            JSONArray values = known.optJSONArray(category);
            if (values == null) continue;
            for (int j = 0; j < values.length(); j++) {
                String value = values.optString(j);
                if (value.isEmpty()) continue;
                for (int start = text.indexOf(value); start >= 0; start = text.indexOf(value, start + 1)) {
                    matches.put(new JSONObject().put("category", category).put("value", value).put("offset", start));
                    java.util.Arrays.fill(hidden, start, start + value.length(), true);
                }
            }
        }
        StringBuilder redacted = new StringBuilder(text.length());
        for (int i = 0; i < text.length();) {
            if (!hidden[i]) { redacted.append(text.charAt(i++)); continue; }
            redacted.append("[redacted]");
            do { i++; } while (i < text.length() && hidden[i]);
        }
        return new JSONObject().put("text", redacted.toString()).put("matches", matches);
    }
}
