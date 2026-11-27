package de.obdentic.app;

import android.content.Context;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import org.json.JSONArray;
import org.json.JSONObject;

/** Test-only ELM link backed by the same synthetic vehicle data as the Rust virtual adapter. */
final class SharedFixtureLink implements ObdService.Link {
    private final JSONObject fixture;
    private final ObdService service;
    private final Method wire;
    final List<String> commands = new ArrayList<>();
    private boolean open = true, targeted, broken;

    SharedFixtureLink(Context testContext, ObdService service) throws Exception {
        try (java.io.InputStream in = testContext.getAssets().open("virtual-vehicle.json")) {
            java.io.ByteArrayOutputStream bytes = new java.io.ByteArrayOutputStream();
            byte[] buffer = new byte[4096];
            for (int count; (count = in.read(buffer)) != -1;) bytes.write(buffer, 0, count);
            fixture = new JSONObject(bytes.toString(StandardCharsets.UTF_8.name()));
        }
        this.service = service;
        wire = ObdService.class.getDeclaredMethod("wire", String.class, String.class);
        wire.setAccessible(true);
    }
    public boolean isOpen() { return open; }
    public void close() { open = false; }
    public synchronized String exchange(String command, long timeoutMs) throws Exception {
        if (!open) throw new java.io.IOException("link closed");
        commands.add(command);
        wire.invoke(service, "TX", command);
        String response = respond(command.trim().toUpperCase(Locale.ROOT));
        wire.invoke(service, "RX", response);
        return response;
    }
    private String respond(String command) throws Exception {
        if (command.equals("ATZ")) { targeted = false; broken = false; return "ELM327 v1.5\r>"; }
        if (command.equals("ATI")) return "ELM327 v1.5\r>";
        if (command.equals("AT@1")) return "CARLY-UNIVERSAL\r>";
        if (command.startsWith("ATSH ")) {
            targeted = !command.substring(5).equals("7DF");
            if (targeted) broken = true;
            return "OK\r>";
        }
        if (command.startsWith("AT")) return "OK\r>";
        if (!targeted && broken) return "NO DATA\r>";
        String payload = null;
        if (command.equals("0100")) {
            long bitmap = 0;
            java.util.Iterator<String> keys = fixture.getJSONObject("pids").keys();
            while (keys.hasNext()) { int pid = Integer.parseInt(keys.next(), 16); if (pid > 0 && pid <= 32) bitmap |= 1L << (32 - pid); }
            payload = String.format(Locale.ROOT, "4100%08X", bitmap);
        } else if (command.startsWith("01") && command.length() == 4) {
            String value = fixture.getJSONObject("pids").optString(command.substring(2), "");
            if (!value.isEmpty()) payload = "41" + command.substring(2) + value;
        } else if (command.equals("03") || command.equals("07")) {
            JSONArray codes = fixture.getJSONArray(command.equals("03") ? "stored_dtcs" : "pending_dtcs");
            StringBuilder value = new StringBuilder(command.equals("03") ? "43" : "47");
            if (codes.length() == 0) value.append("00");
            for (int i = 0; i < codes.length(); i++) value.append(codes.getString(i));
            payload = value.toString();
        } else if (command.equals("0902")) {
            StringBuilder value = new StringBuilder("490201");
            for (byte b : fixture.getString("vin").getBytes(StandardCharsets.US_ASCII)) value.append(String.format(Locale.ROOT, "%02X", b));
            payload = value.toString();
        } else if (command.startsWith("22") && command.length() == 6) {
            String did = command.substring(2);
            String value = fixture.getJSONObject("dids").optString(did, did.startsWith("F1") ? fixture.getString("identification") : "");
            if (!value.isEmpty()) payload = "62" + did + value;
        }
        return payload == null ? "NO DATA\r>" : frames(fixture.getString("response_id"), payload);
    }
    private static String frames(String responder, String hex) {
        int bytes = hex.length() / 2;
        List<String> frames = new ArrayList<>();
        if (bytes <= 7) frames.add(String.format(Locale.ROOT, "%02X", bytes) + hex);
        else {
            frames.add(String.format(Locale.ROOT, "%04X", 0x1000 | bytes) + hex.substring(0, 12));
            int seq = 1;
            for (int start = 12; start < hex.length(); start += 14) frames.add(String.format(Locale.ROOT, "%02X", 0x20 | (seq++ & 15)) + hex.substring(start, Math.min(start + 14, hex.length())));
        }
        StringBuilder result = new StringBuilder();
        for (String frame : frames) {
            result.append(responder);
            for (int i = 0; i < frame.length(); i += 2) result.append(' ').append(frame, i, i + 2);
            result.append('\r');
        }
        return result.append('>').toString();
    }
}
