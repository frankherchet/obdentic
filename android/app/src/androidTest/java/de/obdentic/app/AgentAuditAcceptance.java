package de.obdentic.app;

import android.app.Instrumentation;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Scanner;
import java.util.UUID;
import java.util.concurrent.atomic.AtomicBoolean;
import org.json.JSONArray;
import org.json.JSONObject;

/**
 * REQ-045 acceptance of the synthetic-client audit against a file-backed test document provider (no manual folder choice, no model,
 * no vehicle, no adapter). Every check names the criterion it covers. Process death is simulated by the file a killed process leaves.
 */
final class AgentAuditAcceptance {
    private interface Action { void run() throws Exception; }

    private final Instrumentation inst;
    private final Context target, own;
    private final ContentResolver resolver;
    private Uri tree;
    private final String authority = AuditTestProvider.AUTHORITY;

    private AgentAuditAcceptance(Instrumentation inst) {
        this.inst = inst; target = inst.getTargetContext(); own = inst.getContext(); resolver = target.getContentResolver();
    }

    static void run(Instrumentation inst) throws Exception {
        new AgentAuditAcceptance(inst).all();
    }

    private void all() throws Exception {
        String previous = AgentAuditStorage.folder(target);
        AgentAuditStorage.allowTestProvider(authority);
        tree = DocumentsContract.buildTreeDocumentUri(authority, AuditTestProvider.ROOT_ID);
        try {
            selection();
            fresh();
            sessionAndRequest();
            fresh(); tools();
            fresh(); disclosure();
            fresh(); persistenceFailureAndInterruption();
            fresh(); outcomes();
            fresh(); secrets();
            fresh(); exportPreview();
            fresh(); retentionAndOutsideChanges();
            fresh(); storage();
            fresh(); boundaries();
        } finally {
            AgentAuditSession.resetLimits();
            ChatGptAccount.install(null);
            try { clear(); } catch (Exception ignored) { /* best effort */ }
            AgentAuditStorage.allowTestProvider("");
            target.getSharedPreferences("agent-audit", 0).edit().putString("folder", previous).commit();
        }
    }

    // ------------------------------------------------------------------------------------------------ checks

    /** REQ-045.23, .28: only an explicitly selected, local, granted tree is used; nothing falls back. */
    private void selection() throws Exception {
        target.getSharedPreferences("agent-audit", 0).edit().remove("folder").commit();
        require(refused(() -> AgentAuditSession.begin(target, metadata())) != null, "REQ-045.28 no folder selected: no session");
        Uri cloud = Uri.parse("content://com.example.cloud.documents/tree/primary%3Aaudit");
        require(!AgentAuditStorage.isLocalTree(cloud) && refused(() -> AgentAuditStorage.select(target, cloud)) != null, "REQ-045.28 a non-local provider is rejected");
        require(AgentAuditStorage.folder(target).isEmpty(), "REQ-045.28 a rejected provider never replaces the destination");
        grant();
        require(tree.toString().equals(AgentAuditStorage.folder(target)), "REQ-045.23 the user-selected tree is stored");
        resolver.releasePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
        require(refused(() -> AgentAuditSession.begin(target, metadata())) != null, "REQ-045.28 a revoked grant stops the session");
        grant();
    }

    /** REQ-045.1, .2, .3, .10, .14: context, exact request persisted before dispatch, ordered stream, distinct retry identity. */
    private void sessionAndRequest() throws Exception {
        require(refused(() -> AgentAuditSession.begin(target, new JSONObject())) != null, "REQ-045.1 the start needs versions, pin, vehicle reference, configuration and budgets");
        AgentAuditSession s = begin();
        s.disclosure(true);
        String id = id();
        JSONObject request = new JSONObject("{\"message\":\"synthetic\",\"items\":[3,1,2],\"text\":\"exact \\u00e4 string\"}");
        AtomicBoolean persistedBefore = new AtomicBoolean();
        s.runModelRequest(request, id, json -> {
            quiet(() -> persistedBefore.set(json.equals(request.toString()) && intentOf(s, id) != null && intentOf(s, id).getJSONObject("data").getJSONObject("request").toString().equals(json)));
            quiet(() -> {
                s.recordModelEvent(id, "response_delta", new JSONObject().put("text", "first"));
                s.recordModelEvent(id, "response_delta", new JSONObject().put("text", "second"));
            });
        });
        JSONArray events = events("request_id", id);
        require(persistedBefore.get(), "REQ-045.2/.10 the exact request is stored (write and sync) before the client receives it");
        require(events.length() == 5 && type(events, 0).equals("request_intent") && type(events, 1).equals("response_delta") && type(events, 2).equals("response_delta")
            && type(events, 3).equals("request_dispatched") && type(events, 4).equals("request_end")
            && "first".equals(events.getJSONObject(1).getJSONObject("data").optString("text")) && "second".equals(events.getJSONObject(2).getJSONObject("data").optString("text")),
            "REQ-045.3 deltas stay in arrival order between the request and its end");
        AtomicBoolean reused = new AtomicBoolean();
        require(refused(() -> s.runModelRequest(new JSONObject(), id, json -> reused.set(true))) != null && !reused.get(), "REQ-045.14 a retry cannot reuse a request identity");
        JSONArray all = events("session_id", s.sessionId());
        long last = 0;
        for (int i = 0; i < all.length(); i++) {
            require(all.getJSONObject(i).getLong("seq") == i + 1 && all.getJSONObject(i).getLong("mono_ns") >= last && all.getJSONObject(i).has("wall_ms"), "REQ-045.1 sequence and times are monotone");
            last = all.getJSONObject(i).getLong("mono_ns");
        }
        s.finish("success");
    }

    /** REQ-045.4, .5, .6, .7: one tool lifecycle for every tool; denied calls never execute; failures never look successful. */
    private void tools() throws Exception {
        AgentAuditSession s = begin();
        s.disclosure(true);
        AtomicBoolean ran = new AtomicBoolean();
        String denied = id();
        s.runTool(denied, "synthetic_tool", "{\"raw\": 1}", false, "denied by policy", () -> { ran.set(true); return new AgentAuditSession.ToolResult(new JSONObject(), null); });
        JSONArray d = events("tool_call_id", denied);
        require(!ran.get() && d.length() == 2 && type(d, 0).equals("tool_proposal") && type(d, 1).equals("tool_decision")
            && d.getJSONObject(0).getJSONObject("data").getString("arguments").equals("{\"raw\": 1}"), "REQ-045.5 a denied call keeps its original arguments and has no execution");
        String allowed = id();
        s.runTool(allowed, "synthetic_tool", "{\"x\":1}", true, "allowed", () -> new AgentAuditSession.ToolResult(new JSONObject().put("value", "synthetic"), null));
        JSONArray a = events("tool_call_id", allowed);
        require(a.length() == 5 && type(a, 0).equals("tool_proposal") && type(a, 1).equals("tool_decision") && type(a, 2).equals("tool_intent")
            && type(a, 3).equals("tool_dispatched") && type(a, 4).equals("tool_end")
            && "synthetic".equals(a.getJSONObject(4).getJSONObject("data").getJSONObject("result").optString("value")), "REQ-045.4 proposal, decision, start, end and result");
        String failing = id();
        String message = refused(() -> s.runTool(failing, "synthetic_tool", "{}", true, "allowed", () -> { throw new IllegalStateException("boom"); }));
        JSONArray f = events("tool_call_id", failing);
        require(message != null && f.length() == 4 && type(f, 2).equals("tool_intent") && "error".equals(f.getJSONObject(3).getJSONObject("data").optString("status"))
            && !f.getJSONObject(3).getJSONObject("data").has("result"), "REQ-045.6 an allowed tool that fails records its start and a terminal error, not a success");
        for (String kind : new String[]{"memo_read", "memo_write", "knowledge_lookup", "recording_query"}) {
            String call = id();
            s.runTool(call, kind, "{}", true, "allowed", () -> new AgentAuditSession.ToolResult(
                new JSONObject().put("complete_local", kind + "-everything").put("mutated", kind.equals("memo_write")), new JSONObject().put("subset", kind)));
            JSONArray k = events("tool_call_id", call);
            JSONObject end = k.getJSONObject(k.length() - 1).getJSONObject("data");
            require(k.length() == 5 && type(k, 2).equals("tool_intent") && type(k, 3).equals("tool_dispatched") && type(k, 4).equals("tool_end")
                && end.getJSONObject("result").getString("complete_local").equals(kind + "-everything") && end.getJSONObject("sent_external").getString("subset").equals(kind),
                "REQ-045.7 " + kind + " uses the same lifecycle and separates the local outcome from the subset sent out");
        }
        s.finish("success");
    }

    /** REQ-045.8: initial and renewed denial are recorded, earlier exchanges stay, nothing more is sent. */
    private void disclosure() throws Exception {
        AgentAuditSession first = begin();
        first.disclosure(false);
        AtomicBoolean sent = new AtomicBoolean();
        require(refused(() -> first.runModelRequest(new JSONObject(), id(), json -> sent.set(true))) != null && !sent.get(), "REQ-045.8 initial denial: zero external requests");
        require(events("session_id", first.sessionId()).getJSONObject(1).getJSONObject("data").optBoolean("approved", true) == false, "REQ-045.8 the denial is recorded");
        first.finish("denied");
        AgentAuditSession second = begin();
        second.disclosure(true);
        String approvedId = id();
        second.runModelRequest(new JSONObject().put("message", "approved"), approvedId, json -> sent.set(true));
        require(sent.get(), "REQ-045.8 approved exchange runs");
        sent.set(false);
        second.disclosure(false);
        require(refused(() -> second.runModelRequest(new JSONObject().put("message", "later"), id(), json -> sent.set(true))) != null && !sent.get(), "REQ-045.8 renewed denial: the later request is never dispatched");
        require(events("request_id", approvedId).length() == 3, "REQ-045.8 renewed denial keeps the earlier approved exchange");
        second.finish("denied");
    }

    /** REQ-045.9, .11, .12, .18: inspection triggers nothing; a persistence failure stops everything; a killed process leaves a prefix. */
    private void persistenceFailureAndInterruption() throws Exception {
        AgentAuditSession s = begin();
        s.disclosure(true);
        String id = id();
        Uri file = sessionFile(s.sessionId());
        String message = refused(() -> s.runModelRequest(new JSONObject().put("message", "m"), id, json -> quiet(() -> { // a third party changes the folder: the next write cannot be trusted
            try (OutputStream out = resolver.openOutputStream(file, "wa")) { out.write(' '); }
        })));
        AtomicBoolean ran = new AtomicBoolean();
        require(message != null && s.failed(), "REQ-045.11 a failed persistence ends the request");
        require(refused(() -> s.runModelRequest(new JSONObject(), id(), json -> ran.set(true))) != null && !ran.get(), "REQ-045.11 no later call and no retry starts");
        require(refused(() -> s.finish("success")) != null && !text(file).contains("\"session_end\""), "REQ-045.11 the audit is never reported as complete");
        clear();
        AgentAuditSession listed = begin();
        listed.finish("success");
        String before = text(sessionFile(listed.sessionId()));
        AgentAuditSession.storageWarning(target);
        AgentAuditExport.preview(target, listed.sessionId(), new JSONObject(), new JSONArray());
        require(before.equals(text(sessionFile(listed.sessionId()))) && ran.get() == false, "REQ-045.9 reading an audit changes nothing and starts no work");
        // REQ-045.18: the prefix a killed process leaves (request, partial stream, torn line) is never expired or completed.
        String killed = UUID.randomUUID().toString();
        writeAudit("obdentic-agent-" + killed + ".jsonl", line(killed, 1, "session_start", "") + line(killed, 2, "request_intent", ",\"request_id\":\"r\"")
            + line(killed, 3, "response_delta", ",\"request_id\":\"r\"") + "{\"schema\":\"obdentic-agent-aud");
        AgentAuditSession.expire(target, 400L * 86400000L);
        require(sessionFile(killed) != null, "REQ-045.18 an interrupted audit is retained and stays incomplete");
    }

    /** REQ-045.13, .14, .15, .16, .17: typed outcomes are recorded as such; cancel and disconnect stop all work. */
    private void outcomes() throws Exception {
        for (String outcome : new String[]{"timeout", "network_error", "usage_limit"}) {
            AgentAuditSession s = begin();
            s.disclosure(true);
            String id = id();
            require(refused(() -> s.runModelRequest(new JSONObject(), id, json -> {
                quiet(() -> s.recordModelEvent(id, "response_delta", new JSONObject().put("text", "partial")));
                throw new AgentAuditSession.ModelFailure(outcome);
            })) != null, "REQ-045." + outcome + " the request fails");
            JSONArray e = events("request_id", id);
            require(outcome.equals(e.getJSONObject(e.length() - 1).getJSONObject("data").optString("status")) && "partial".equals(e.getJSONObject(1).getJSONObject("data").optString("text"))
                && !e.getJSONObject(e.length() - 1).getJSONObject("data").has("usage"),
                "REQ-045.13/.14/.15 " + outcome + " keeps the received evidence, is no success and invents no usage");
            String retry = id();
            s.runModelRequest(new JSONObject(), retry, json -> { });
            require(!retry.equals(id) && events("request_id", retry).length() == 3, "REQ-045.14 a permitted retry has its own identity");
            s.finish("failed");
        }
        for (String stop : new String[]{"cancelled", "disconnected"}) {
            AgentAuditSession s = begin();
            s.disclosure(true);
            s.stop(stop);
            AtomicBoolean ran = new AtomicBoolean();
            require(refused(() -> s.runModelRequest(new JSONObject(), id(), json -> ran.set(true))) != null && !ran.get()
                && stop.equals(last(events("session_id", s.sessionId())).getJSONObject("data").optString("diagnosis_status")), "REQ-045.16/.17 " + stop + " ends the diagnosis without further work");
        }
    }

    /** REQ-045.21, .22: secrets never reach the JSONL, the export or an error message; the outgoing payload is rejected unsent. */
    private void secrets() throws Exception {
        String token = "access-marker-0123456789abcdef", refresh = "refresh-marker-fedcba9876543210";
        AccountStore store = new AccountStore(target, "audit-marker-test");
        store.save(new AccountStore.Record("client-marker-abcdefghijkl", token, refresh, "id-marker-abcdefghijklmnop", ChatGptAccount.SCOPE,
            "someone@example.test", "subject-marker", System.currentTimeMillis() + 3600000, 0, false));
        ChatGptAccount.Config config = ChatGptAccount.Config.defaults(target);
        config.store = store;
        ChatGptAccount.install(new ChatGptAccount(config));
        try {
            AgentAuditSession s = begin();
            s.disclosure(true);
            AtomicBoolean ran = new AtomicBoolean();
            String id = id();
            String m1 = refused(() -> s.runModelRequest(new JSONObject().put("message", "x " + token.substring(4, 24) + " y"), id, json -> ran.set(true)));
            require(m1 != null && !ran.get() && events("request_id", id).length() == 0, "REQ-045.22 an outgoing payload with a current secret substring is rejected before sending");
            require(!m1.contains("marker"), "REQ-045.21 the error output carries no secret");
            AgentAuditSession bearer = begin();
            String m2 = refused(() -> bearer.append("note", new JSONObject().put("authorization", "Bearer synthetic-marker-value"), null, null, null));
            require(m2 != null && !m2.contains("marker") && !text(sessionFile(bearer.sessionId())).contains("marker"), "REQ-045.21 Bearer shapes never reach the JSONL");
            require(!text(sessionFile(s.sessionId())).contains("marker"), "REQ-045.21 the private JSONL holds no marker");
            AgentAuditSession ok = begin();
            ok.append("note", new JSONObject().put("vin", "SYNTHETICVIN0000000").put("odometer", 123456), null, null, null);
            require(text(sessionFile(ok.sessionId())).contains("SYNTHETICVIN0000000"), "REQ-045.22 vehicle identifiers are not secret material");
            JSONObject preview = AgentAuditExport.preview(target, s.sessionId(), new JSONObject(), new JSONArray());
            require(!preview.getString("text").contains("marker"), "REQ-045.21 the export holds no marker");
            ok.finish("success");
        } finally {
            store.deleteAll();
            ChatGptAccount.install(null);
        }
    }

    /** REQ-045.24, .25: selection, warning, preview of every known match; cancelling creates nothing; the source is unchanged. */
    private void exportPreview() throws Exception {
        AgentAuditSession s = begin();
        s.append("note", new JSONObject().put("text", "SYNTHETIC-VIN and SYNTHETIC-VIN again"), null, null, null);
        s.finish("success");
        String source = text(sessionFile(s.sessionId()));
        JSONObject known = new JSONObject().put("vin", new JSONArray().put("SYNTHETIC-VIN"));
        JSONArray categories = new JSONArray().put("vin");
        JSONObject preview = AgentAuditExport.preview(target, s.sessionId(), known, categories);
        require(preview.getJSONArray("matches").length() == 2 && preview.getString("warning").contains("Erkannt werden nur Werte, die die App kennt."), "REQ-045.24 the preview shows every match and the warning");
        int before = children().size();
        require(AgentAuditExport.confirm(target, preview, false) == null && children().size() == before, "REQ-045.25 a cancelled export creates nothing");
        Uri exported = AgentAuditExport.confirm(target, preview, true);
        require(exported != null && !text(exported).contains("SYNTHETIC-VIN") && source.equals(text(sessionFile(s.sessionId()))), "REQ-045.24 the confirmed copy is redacted and the source is unchanged");
    }

    /** REQ-045.26 and .30: retention and appends fail closed on every detectable outside change. */
    private void retentionAndOutsideChanges() throws Exception {
        Uri complete = oldAudit(true), incomplete = oldAudit(false);
        AgentAuditCatalog.remember(target, tree);
        long old = 31L * 86400000L;
        Uri intruder = create("intruder.txt");
        expectNoDeletion(old, complete, "REQ-045.26 an unknown file");
        delete(intruder);
        AgentAuditCatalog.remember(target, tree);
        try (OutputStream out = resolver.openOutputStream(incomplete, "wa")) { out.write('x'); }
        expectNoDeletion(old, complete, "REQ-045.26 another size");
        AgentAuditCatalog.remember(target, tree);
        delete(incomplete);
        incomplete = oldAudit(false, 2, false);
        expectNoDeletion(old, complete, "REQ-045.26 another file identity");
        AgentAuditCatalog.remember(target, tree);
        AgentAuditSession.expire(target, old);
        require(!exists(complete) && exists(incomplete), "REQ-045.26 only the verified completed session expires; incomplete sessions stay");
        AgentAuditSession.expire(target, 10L * 86400000L);
        require(exists(incomplete), "REQ-045.26 nothing expires before 30 days");
        AgentAuditSession s = begin();
        Uri outsider = create("outsider.txt");
        require(refused(() -> s.disclosure(true)) != null && s.failed(), "REQ-045.30 an outside change stops every further append");
        delete(outsider);
        AgentAuditCatalog.remember(target, tree);
        AgentAuditSession running = begin();
        require(AgentAuditSession.storageWarning(target) == false, "REQ-045.28 a valid destination needs no warning");
        running.finish("success");
    }

    /** REQ-045.27, .29: warning from 80 % of the budget; a full folder stops the run without deleting unexpired history. */
    private void storage() throws Exception {
        AgentAuditSession.resetLimits();
        long limit = AgentAuditSession.TOTAL_LIMIT, threshold = (limit * 4 + 4) / 5;
        Uri filler = create("obdentic-agent-" + UUID.randomUUID() + ".jsonl");
        resize(filler, threshold - 1);
        AgentAuditCatalog.remember(target, tree);
        require(!AgentAuditSession.storageWarning(target), "REQ-045.29 no warning just below 80 %");
        resize(filler, threshold);
        AgentAuditCatalog.remember(target, tree);
        require(AgentAuditSession.storageWarning(target), "REQ-045.29 the warning starts at 80 % of the total budget (filler " + size(filler) + " of " + threshold + ", files " + children().size() + ")");
        Uri history = oldAudit(true, System.currentTimeMillis(), true);
        AgentAuditCatalog.remember(target, tree);
        resize(filler, limit);
        AgentAuditCatalog.remember(target, tree);
        String message = refused(() -> AgentAuditSession.begin(target, metadata()));
        require(message != null && message.contains("voll") && exists(filler) && exists(history), "REQ-045.27 a full folder reports exhaustion and discards no history");
    }

    /** REQ-045.30, .31, .32: exactly at a cap is allowed, the first byte or event over it is refused without truncation. */
    private void boundaries() throws Exception {
        long big = AgentAuditSession.DEFAULT_TOTAL_LIMIT;
        int bigCount = AgentAuditSession.DEFAULT_COUNT_LIMIT;
        JSONObject probe = new JSONObject().put("p", "xxxxxxxx");
        // measure one event of this shape
        AgentAuditSession m = begin();
        long s0 = size(sessionFile(m.sessionId()));
        m.append("probe", probe, null, null, null);
        long len = size(sessionFile(m.sessionId())) - s0;
        m.finish("success");
        clear();
        // REQ-045.31 event cap
        for (int delta : new int[]{0, -1}) {
            AgentAuditSession s = begin();
            long before = size(sessionFile(s.sessionId()));
            AgentAuditSession.setLimits(big, big, (int) len + delta, bigCount);
            String m31 = refused(() -> s.append("probe", probe, null, null, null));
            require((delta == 0) == (m31 == null) && size(sessionFile(s.sessionId())) == (delta == 0 ? before + len : before),
                "REQ-045.31 event of " + (len + delta) + " bytes " + (delta == 0 ? "at" : "one byte over") + " the cap");
            AgentAuditSession.resetLimits();
            if (!s.failed()) s.finish("success");
            clear();
        }
        // REQ-045.30 per-session cap and total cap
        for (int mode = 0; mode < 2; mode++) {
            AgentAuditSession s = begin();
            long start = size(sessionFile(s.sessionId()));
            AgentAuditSession.setLimits(mode == 1 ? start + len : big, mode == 0 ? start + len : big, AgentAuditSession.DEFAULT_EVENT_LIMIT, bigCount);
            s.append("probe", probe, null, null, null);
            long fitted = size(sessionFile(s.sessionId()));
            require(fitted == start + len, "REQ-045.30 an append that fits exactly is allowed (" + (mode == 0 ? "session" : "total") + ")");
            require(refused(() -> s.append("probe", probe, null, null, null)) != null && size(sessionFile(s.sessionId())) == fitted && s.failed(),
                "REQ-045.30 the first append over the " + (mode == 0 ? "32 MiB session" : "256 MiB total") + " cap is refused without truncation");
            AgentAuditSession.resetLimits();
            clear();
        }
        // REQ-045.32 event count: the last event fits, work that needs more is not begun, the missing end marker stays honest
        AgentAuditSession s = begin();
        AgentAuditSession.setLimits(big, big, AgentAuditSession.DEFAULT_EVENT_LIMIT, 4);
        s.append("probe", probe, null, null, null);
        s.append("probe", probe, null, null, null);
        s.append("probe", probe, null, null, null);
        require(events("session_id", s.sessionId()).length() == 4, "REQ-045.32 the last event within the cap can be written");
        require(refused(() -> s.append("probe", probe, null, null, null)) != null && events("session_id", s.sessionId()).length() == 4, "REQ-045.32 the next event is refused");
        require(refused(() -> s.finish("success")) != null && !text(sessionFile(s.sessionId())).contains("\"session_end\""), "REQ-045.32 an interrupted audit stays without an end marker");
        AgentAuditSession.resetLimits();
        clear();
        AgentAuditSession w = begin();
        w.disclosure(true);
        AgentAuditSession.setLimits(big, big, AgentAuditSession.DEFAULT_EVENT_LIMIT, 4);
        AtomicBoolean sent = new AtomicBoolean();
        require(refused(() -> w.runModelRequest(new JSONObject(), id(), json -> sent.set(true))) != null && !sent.get(), "REQ-045.32 work that needs events beyond the cap is not begun");
        AgentAuditSession.resetLimits();
        w.finish("success");
    }

    // ------------------------------------------------------------------------------------------------ helpers

    private static void require(boolean ok, String what) {
        if (!ok) throw new AssertionError(what);
    }

    private static String refused(Action action) {
        try { action.run(); return null; } catch (Exception e) { return String.valueOf(e.getMessage()); }
    }

    private static void quiet(Action action) {
        try { action.run(); } catch (Exception e) { throw new IllegalStateException(e); }
    }

    private static String id() { return UUID.randomUUID().toString(); }

    private static JSONObject metadata() throws Exception {
        return new JSONObject().put("runtime_version", "synthetic").put("build_version", "synthetic").put("model_version", "synthetic")
            .put("knowledge_pin", "synthetic").put("vehicle_ref", JSONObject.NULL).put("configuration", new JSONObject()).put("budgets", new JSONObject());
    }

    private AgentAuditSession begin() throws Exception { return AgentAuditSession.begin(target, metadata()); }

    private static String type(JSONArray events, int i) throws Exception { return events.getJSONObject(i).getString("type"); }
    private static JSONObject last(JSONArray events) throws Exception { return events.getJSONObject(events.length() - 1); }

    private void grant() throws Exception {
        int flags = Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION;
        // The instrumentation runs as the app under test and cannot grant a tree it does not own: the receiver of the test APK (the owner) does.
        java.util.concurrent.CountDownLatch done = new java.util.concurrent.CountDownLatch(1);
        int[] code = {-1};
        target.sendOrderedBroadcast(new Intent().setComponent(new android.content.ComponentName("de.obdentic.app.debug.test", AuditGrantReceiver.class.getName()))
            .putExtra("package", target.getPackageName()), null, new android.content.BroadcastReceiver() {
                @Override public void onReceive(Context context, Intent intent) { code[0] = getResultCode(); done.countDown(); }
            }, null, 0, null, null);
        if (!done.await(20, java.util.concurrent.TimeUnit.SECONDS) || code[0] != 1) throw new AssertionError("the test provider could not grant its tree: " + code[0]);
        resolver.takePersistableUriPermission(tree, flags & ~Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
        AgentAuditStorage.select(target, tree);
    }

    private void fresh() throws Exception {
        AgentAuditSession.resetLimits();
        clear();
    }

    private Uri document(String documentId) { return DocumentsContract.buildDocumentUriUsingTree(tree, documentId); }
    private Uri parent() { return document(DocumentsContract.getTreeDocumentId(tree)); }

    private List<String[]> children() throws Exception {
        List<String[]> out = new ArrayList<>();
        Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
        try (Cursor c = resolver.query(children, new String[]{DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME}, null, null, null)) {
            while (c != null && c.moveToNext()) out.add(new String[]{c.getString(0), c.getString(1)});
        }
        return out;
    }

    private void clear() throws Exception {
        for (String[] child : children()) DocumentsContract.deleteDocument(resolver, document(child[0]));
        AgentAuditCatalog.remember(target, tree);
    }

    private Uri create(String name) throws Exception {
        Uri uri = DocumentsContract.createDocument(resolver, parent(), "application/x-ndjson", name);
        if (uri == null) throw new AssertionError("could not create " + name);
        return uri;
    }

    private void delete(Uri uri) throws Exception { DocumentsContract.deleteDocument(resolver, uri); }

    private boolean exists(Uri uri) {
        try (Cursor c = resolver.query(uri, new String[]{DocumentsContract.Document.COLUMN_DOCUMENT_ID}, null, null, null)) {
            return c != null && c.moveToFirst();
        } catch (IllegalArgumentException missing) { return false; }
    }

    private long size(Uri uri) throws Exception {
        try (Cursor c = resolver.query(uri, new String[]{DocumentsContract.Document.COLUMN_SIZE}, null, null, null)) {
            if (c == null || !c.moveToFirst()) throw new AssertionError("no size");
            return c.getLong(0);
        }
    }

    /** Grows a file to the given length by appending zeros through the provider (a truncate on the descriptor of another app's file does not take effect). */
    private void resize(Uri uri, long length) throws Exception {
        long have = size(uri);
        if (length < have) throw new AssertionError("the filler only grows");
        byte[] zeros = new byte[1 << 20];
        try (OutputStream out = resolver.openOutputStream(uri, "wa")) {
            for (long left = length - have; left > 0; left -= zeros.length) out.write(zeros, 0, (int) Math.min(zeros.length, left));
        }
    }

    private Uri sessionFile(String sessionId) throws Exception {
        for (String[] child : children()) if (child[1].equals("obdentic-agent-" + sessionId + ".jsonl")) return document(child[0]);
        return null;
    }

    private String text(Uri uri) throws Exception {
        try (InputStream in = resolver.openInputStream(uri)) {
            Scanner scanner = new Scanner(in, "UTF-8").useDelimiter("\\A");
            return scanner.hasNext() ? scanner.next() : "";
        }
    }

    private void writeAudit(String name, String content) throws Exception { writeAudit(name, content, true); }

    private void writeAudit(String name, String content, boolean remember) throws Exception {
        Uri uri = create(name);
        try (OutputStream out = resolver.openOutputStream(uri, "w")) { out.write(content.getBytes(StandardCharsets.UTF_8)); }
        if (remember) AgentAuditCatalog.remember(target, tree);
    }

    private static String line(String session, int seq, String type, String extra) {
        return "{\"schema\":\"obdentic-agent-audit\",\"version\":1,\"session_id\":\"" + session + "\",\"seq\":" + seq + ",\"wall_ms\":" + seq + ",\"mono_ns\":" + seq
            + ",\"type\":\"" + type + "\"" + extra + ",\"data\":{}}\n";
    }

    private Uri oldAudit(boolean complete) throws Exception { return oldAudit(complete, 2, true); }

    private Uri oldAudit(boolean complete, long endWallMs, boolean remember) throws Exception {
        String s = UUID.randomUUID().toString();
        writeAudit("obdentic-agent-" + s + ".jsonl", line(s, 1, "session_start", "")
            + (complete ? line(s, 2, "session_end", "").replace("\"wall_ms\":2", "\"wall_ms\":" + endWallMs) : ""), remember);
        return sessionFile(s);
    }

    private void expectNoDeletion(long now, Uri survivor, String change) throws Exception {
        require(refused(() -> AgentAuditSession.expire(target, now)) != null && exists(survivor), change + " fails closed and deletes nothing");
    }

    private JSONArray events(String field, String value) throws Exception {
        JSONArray found = new JSONArray();
        for (String[] child : children()) {
            if (!child[1].startsWith("obdentic-agent-")) continue;
            for (String l : text(document(child[0])).split("\n")) {
                if (l.isEmpty()) continue;
                try {
                    JSONObject e = new JSONObject(l);
                    if (value.equals(e.optString(field))) found.put(e);
                } catch (org.json.JSONException torn) { /* a torn tail is not an event */ }
            }
        }
        return found;
    }

    private JSONObject intentOf(AgentAuditSession s, String requestId) throws Exception {
        JSONArray e = events("request_id", requestId);
        return e.length() > 0 && type(e, 0).equals("request_intent") ? e.getJSONObject(0) : null;
    }
}
