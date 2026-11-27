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
    static void runShared(Instrumentation inst) throws Exception {
        AgentAuditAcceptance check = new AgentAuditAcceptance(inst);
        String previous = AgentAuditStorage.folder(check.target);
        AgentAuditStorage.allowTestProvider(check.authority);
        check.tree = DocumentsContract.buildTreeDocumentUri(check.authority, AuditTestProvider.ROOT_ID);
        try { check.grant(); check.fresh(); check.sharedDiagnosis(); }
        finally {
            try { check.clear(); } catch (Exception ignored) { }
            AgentAuditStorage.allowTestProvider("");
            check.target.getSharedPreferences("agent-audit", 0).edit().putString("folder", previous).commit();
        }
    }

    private void all() throws Exception {
        String previous = AgentAuditStorage.folder(target);
        AgentAuditStorage.allowTestProvider(authority);
        tree = DocumentsContract.buildTreeDocumentUri(authority, AuditTestProvider.ROOT_ID);
        try {
            selection();
            fresh();
            sessionAndRequest();
            fresh(); turnCorrelation();
            fresh(); tools();
            fresh(); stopRunningTool();
            fresh(); disclosure();
            fresh(); persistenceFailureAndInterruption();
            fresh(); outcomes();
            fresh(); secrets();
            fresh(); exportPreview();
            fresh(); retentionAndOutsideChanges();
            fresh(); interruptedAppendRecovery();
            fresh(); confirmedFolderRecovery();
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
        s.runModelRequest(request, "turn1", id, json -> {
            quiet(() -> persistedBefore.set(json.equals(request.toString()) && intentOf(s, id) != null && intentOf(s, id).getJSONObject("data").getJSONObject("request").toString().equals(json)));
            quiet(() -> {
                s.recordModelEvent(id, "response_delta", new JSONObject().put("text", "first"));
                s.recordModelEvent(id, "response_delta", new JSONObject().put("text", "second"));
            });
        });
        JSONArray events = events("request_id", id);
        require(persistedBefore.get(), "REQ-045.2 and REQ-045.10: the exact request is stored (write and sync) before the client receives it");
        require(events.length() == 5 && type(events, 0).equals("request_intent") && type(events, 1).equals("response_delta") && type(events, 2).equals("response_delta")
            && type(events, 3).equals("request_dispatched") && type(events, 4).equals("request_end")
            && "first".equals(events.getJSONObject(1).getJSONObject("data").optString("text")) && "second".equals(events.getJSONObject(2).getJSONObject("data").optString("text")),
            "REQ-045.3 deltas stay in arrival order between the request and its end");
        AtomicBoolean reused = new AtomicBoolean();
        require(refused(() -> s.runModelRequest(new JSONObject(), "turn1", id, json -> reused.set(true))) != null && !reused.get(), "REQ-045.14 a retry cannot reuse a request identity");
        JSONArray all = events("session_id", s.sessionId());
        long last = 0;
        for (int i = 0; i < all.length(); i++) {
            require(all.getJSONObject(i).getLong("seq") == i + 1 && all.getJSONObject(i).getLong("mono_ns") >= last && all.getJSONObject(i).has("wall_ms"), "REQ-045.1 sequence and times are monotone");
            last = all.getJSONObject(i).getLong("mono_ns");
        }
        s.finish("success");
    }

    /** REQ-045.1: two turns, a retry and tool calls preserve all correlation identities. */
    private void turnCorrelation() throws Exception {
        AgentAuditSession session = begin(); session.disclosure(true);
        java.lang.reflect.Method request;
        java.lang.reflect.Method tool;
        try {
            request = AgentAuditSession.class.getMethod("runModelRequest", JSONObject.class, String.class, String.class, AgentAuditSession.ModelDispatch.class);
            tool = AgentAuditSession.class.getMethod("runTool", String.class, String.class, String.class, String.class, String.class,
                boolean.class, String.class, java.util.concurrent.Callable.class);
        } catch (NoSuchMethodException e) { throw new AssertionError("REQ-045.1 turn correlation API missing", e); }
        for (String[] pair : new String[][]{{"turn1", "request1"}, {"turn1", "retry1"}, {"turn2", "request2"}}) {
            request.invoke(session, new JSONObject(), pair[0], pair[1], (AgentAuditSession.ModelDispatch) json -> {});
            JSONArray entries = events("request_id", pair[1]);
            require(entries.length() == 3, "request lifecycle complete");
            for (int i = 0; i < entries.length(); i++) require(pair[0].equals(entries.getJSONObject(i).optString("turn_id")), "request turn identity");
        }
        for (int i = 0; i < 2; i++) {
            String turn = "turn" + (i + 1), req = "request" + (i + 1), call = "tool" + (i + 1);
            tool.invoke(session, turn, req, call, "synthetic", "{}", true, "allowed",
                (java.util.concurrent.Callable<AgentAuditSession.ToolResult>) () -> new AgentAuditSession.ToolResult(new JSONObject(), null, null));
            JSONArray entries = events("tool_call_id", call);
            require(entries.length() == 5, "tool lifecycle complete");
            for (int j = 0; j < entries.length(); j++) {
                JSONObject e = entries.getJSONObject(j);
                require(turn.equals(e.optString("turn_id")) && req.equals(e.optString("request_id")) && call.equals(e.optString("tool_call_id")), "tool correlation identity");
            }
        }
        session.finish("success");
    }

    /** REQ-045.4, .5, .6, .7: one tool lifecycle for every tool; denied calls never execute; failures never look successful. */
    private void tools() throws Exception {
        AgentAuditSession s = begin();
        s.disclosure(true);
        AtomicBoolean ran = new AtomicBoolean();
        String denied = id();
        s.runTool("turn1", "request1", denied, "synthetic_tool", "{\"raw\": 1}", false, "denied by policy", () -> { ran.set(true); return new AgentAuditSession.ToolResult(new JSONObject(), null, null); });
        JSONArray d = events("tool_call_id", denied);
        require(!ran.get() && d.length() == 2 && type(d, 0).equals("tool_proposal") && type(d, 1).equals("tool_decision")
            && d.getJSONObject(0).getJSONObject("data").getString("arguments").equals("{\"raw\": 1}"), "REQ-045.5 a denied call keeps its original arguments and has no execution");
        String allowed = id();
        java.lang.reflect.Constructor<AgentAuditSession.ToolResult> withEvidence;
        try { withEvidence = AgentAuditSession.ToolResult.class.getConstructor(JSONObject.class, JSONObject.class, JSONObject.class); }
        catch (NoSuchMethodException e) { throw new AssertionError("REQ-045.4 missing evidence reference", e); }
        s.runTool("turn1", "request1", allowed, "synthetic_tool", "{\"x\":1}", true, "allowed", () ->
            withEvidence.newInstance(new JSONObject().put("value", "synthetic"), null,
                new JSONObject().put("recording_ref", "synthetic.jsonl").put("offset", 42)));
        JSONArray a = events("tool_call_id", allowed);
        require(a.length() == 5 && type(a, 0).equals("tool_proposal") && type(a, 1).equals("tool_decision") && type(a, 2).equals("tool_intent")
            && type(a, 3).equals("tool_dispatched") && type(a, 4).equals("tool_end")
            && "synthetic".equals(a.getJSONObject(4).getJSONObject("data").getJSONObject("result").optString("value"))
            && a.getJSONObject(4).getJSONObject("data").getJSONObject("evidence_ref").optLong("offset") == 42,
            "REQ-045.4 proposal, decision, start, end, result and exact evidence reference");
        String failing = id();
        String message = refused(() -> s.runTool("turn1", "request1", failing, "synthetic_tool", "{}", true, "allowed", () -> { throw new IllegalStateException("boom"); }));
        JSONArray f = events("tool_call_id", failing);
        require(message != null && f.length() == 4 && type(f, 2).equals("tool_intent") && "error".equals(f.getJSONObject(3).getJSONObject("data").optString("status"))
            && !f.getJSONObject(3).getJSONObject("data").has("result"), "REQ-045.6 an allowed tool that fails records its start and a terminal error, not a success");
        for (String kind : new String[]{"memo_read", "memo_write", "knowledge_lookup", "recording_query"}) {
            String call = id();
            s.runTool("turn1", "request1", call, kind, "{}", true, "allowed", () -> new AgentAuditSession.ToolResult(
                new JSONObject().put("complete_local", kind + "-everything").put("mutated", kind.equals("memo_write")), new JSONObject().put("subset", kind), null));
            JSONArray k = events("tool_call_id", call);
            JSONObject end = k.getJSONObject(k.length() - 1).getJSONObject("data");
            require(k.length() == 5 && type(k, 2).equals("tool_intent") && type(k, 3).equals("tool_dispatched") && type(k, 4).equals("tool_end")
                && end.getJSONObject("result").getString("complete_local").equals(kind + "-everything") && end.getJSONObject("sent_external").getString("subset").equals(kind),
                "REQ-045.7 " + kind + " uses the same lifecycle and separates the local outcome from the subset sent out");
        }
        s.finish("success");
    }

    /** REQ-045.11/.16/.17: stop must not wait behind an in-flight tool. */
    private void stopRunningTool() throws Exception {
        for (String reason : new String[]{"cancelled", "disconnected"}) {
            AgentAuditSession session = begin();
            String call = id();
            java.util.concurrent.CountDownLatch entered = new java.util.concurrent.CountDownLatch(1);
            java.util.concurrent.CountDownLatch release = new java.util.concurrent.CountDownLatch(1);
            Thread worker = new Thread(() -> {
                try {
                    session.runTool("turn1", "request1", call, "synthetic", "{}", true, "allowed", () -> {
                        entered.countDown();
                        release.await();
                        return new AgentAuditSession.ToolResult(new JSONObject(), null, null);
                    });
                } catch (Exception ignored) { /* stopped work is expected to fail */ }
            });
            worker.start();
            require(entered.await(5, java.util.concurrent.TimeUnit.SECONDS), "tool started");
            java.util.concurrent.ExecutorService executor = java.util.concurrent.Executors.newSingleThreadExecutor();
            java.util.concurrent.Future<?> stopped = executor.submit(() -> { try { session.stop(reason); } catch (IOException e) { throw new IllegalStateException(e); } });
            boolean timely;
            try { stopped.get(2, java.util.concurrent.TimeUnit.SECONDS); timely = true; }
            catch (java.util.concurrent.TimeoutException e) { timely = false; }
            finally { release.countDown(); executor.shutdown(); worker.join(5000); }
            require(timely, "REQ-045.16/.17 stop must not wait for a running tool");
            JSONArray logged = events("tool_call_id", call);
            JSONObject end = last(logged).getJSONObject("data");
            require("tool_end".equals(type(logged, logged.length() - 1)) && reason.equals(end.optString("status")) && !end.has("result"),
                "stopped tool records reason without a result");
        }
    }

    /** REQ-045.8: initial and renewed denial are recorded, earlier exchanges stay, nothing more is sent. */
    private void disclosure() throws Exception {
        AgentAuditSession first = begin();
        first.disclosure(false);
        AtomicBoolean sent = new AtomicBoolean();
        require(refused(() -> first.runModelRequest(new JSONObject(), "turn1", id(), json -> sent.set(true))) != null && !sent.get(), "REQ-045.8 initial denial: zero external requests");
        require(events("session_id", first.sessionId()).getJSONObject(1).getJSONObject("data").optBoolean("approved", true) == false, "REQ-045.8 the denial is recorded");
        first.finish("denied");
        AgentAuditSession second = begin();
        second.disclosure(true);
        String approvedId = id();
        second.runModelRequest(new JSONObject().put("message", "approved"), "turn1", approvedId, json -> sent.set(true));
        require(sent.get(), "REQ-045.8 approved exchange runs");
        sent.set(false);
        second.disclosure(false);
        require(refused(() -> second.runModelRequest(new JSONObject().put("message", "later"), "turn2", id(), json -> sent.set(true))) != null && !sent.get(), "REQ-045.8 renewed denial: the later request is never dispatched");
        require(events("request_id", approvedId).length() == 3, "REQ-045.8 renewed denial keeps the earlier approved exchange");
        second.finish("denied");
    }

    /** REQ-045.9, .11, .12, .18: inspection triggers nothing; a persistence failure stops everything; a killed process leaves a prefix. */
    private void persistenceFailureAndInterruption() throws Exception {
        AgentAuditSession s = begin();
        s.disclosure(true);
        String id = id();
        Uri file = sessionFile(s.sessionId());
        String message = refused(() -> s.runModelRequest(new JSONObject().put("message", "m"), "turn1", id, json -> quiet(() -> { // a third party changes the folder: the next write cannot be trusted
            try (OutputStream out = resolver.openOutputStream(file, "wa")) { out.write(' '); }
        })));
        AtomicBoolean ran = new AtomicBoolean();
        require(message != null && s.failed(), "REQ-045.11 a failed persistence ends the request");
        require(refused(() -> s.runModelRequest(new JSONObject(), "turn1", id(), json -> ran.set(true))) != null && !ran.get(), "REQ-045.11 no later call and no retry starts");
        require(refused(() -> s.finish("success")) != null && !text(file).contains("\"session_end\""), "REQ-045.11 the audit is never reported as complete");
        clear();
        AgentAuditSession listed = begin();
        listed.finish("success");
        String before = text(sessionFile(listed.sessionId()));
        AgentAuditSession.storageWarning(target);
        AgentAuditExport.preview(target, listed.sessionId());
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
            require(refused(() -> s.runModelRequest(new JSONObject(), "turn1", id, json -> {
                quiet(() -> s.recordModelEvent(id, "response_delta", new JSONObject().put("text", "partial")));
                throw new AgentAuditSession.ModelFailure(outcome);
            })) != null, "REQ-045." + outcome + " the request fails");
            JSONArray e = events("request_id", id);
            require(outcome.equals(e.getJSONObject(e.length() - 1).getJSONObject("data").optString("status")) && "partial".equals(e.getJSONObject(1).getJSONObject("data").optString("text"))
                && !e.getJSONObject(e.length() - 1).getJSONObject("data").has("usage"),
                "REQ-045.13, REQ-045.14, REQ-045.15: " + outcome + " keeps the received evidence, is no success and invents no usage");
            String retry = id();
            s.runModelRequest(new JSONObject(), "turn1", retry, json -> { });
            require(!retry.equals(id) && events("request_id", retry).length() == 3, "REQ-045.14 a permitted retry has its own identity");
            s.finish("failed");
        }
        for (String stop : new String[]{"cancelled", "disconnected"}) {
            AgentAuditSession s = begin();
            s.disclosure(true);
            s.stop(stop);
            AtomicBoolean ran = new AtomicBoolean();
            require(refused(() -> s.runModelRequest(new JSONObject(), "turn1", id(), json -> ran.set(true))) != null && !ran.get()
                && stop.equals(last(events("session_id", s.sessionId())).getJSONObject("data").optString("diagnosis_status")), "REQ-045.16 and REQ-045.17: " + stop + " ends the diagnosis without further work");
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
            String m1 = refused(() -> s.runModelRequest(new JSONObject().put("message", "x " + token.substring(4, 24) + " y"), "turn1", id, json -> ran.set(true)));
            require(m1 != null && !ran.get() && events("request_id", id).length() == 0, "REQ-045.22 an outgoing payload with a current secret substring is rejected before sending");
            require(!m1.contains("marker"), "REQ-045.21 the error output carries no secret");
            AgentAuditSession bearer = begin();
            String m2 = refused(() -> bearer.append("note", new JSONObject().put("authorization", "Bearer synthetic-marker-value"), null, null, null));
            require(m2 != null && !m2.contains("marker") && !text(sessionFile(bearer.sessionId())).contains("marker"), "REQ-045.21 Bearer shapes never reach the JSONL");
            require(!text(sessionFile(s.sessionId())).contains("marker"), "REQ-045.21 the private JSONL holds no marker");
            AgentAuditSession ok = begin();
            ok.append("note", new JSONObject().put("vin", "SYNTHETICVIN0000000").put("odometer", 123456), null, null, null);
            require(text(sessionFile(ok.sessionId())).contains("SYNTHETICVIN0000000"), "REQ-045.22 vehicle identifiers are not secret material");
            JSONObject preview = AgentAuditExport.preview(target, s.sessionId());
            require(!preview.getString("text").contains("marker"), "REQ-045.21 the export holds no marker");
            ok.finish("success");
        } finally {
            store.deleteAll();
            ChatGptAccount.install(null);
        }
    }

    /** REQ-045.24, .25: selection, warning, preview of every known match; cancelling creates nothing; the source is unchanged. */
    private void exportPreview() throws Exception {
        java.io.File isolated = new java.io.File(target.getCacheDir(), "audit-export-records");
        if (!isolated.isDirectory() && !isolated.mkdirs()) throw new IOException("fixture directory");
        Context records = new android.content.ContextWrapper(target) { @Override public java.io.File getFilesDir() { return isolated; } };
        VehicleStore vehicles = new VehicleStore(new java.io.File(isolated, "vehicles.json"));
        String vin = "TESTVIN0000000001", serial = "SYNTH-SERIAL", email = "audit@example.test", subject = "subject-test";
        android.content.SharedPreferences adapter = target.getSharedPreferences("adapter", 0);
        String previous = adapter.getString("known_address", "");
        AccountStore accountStore = new AccountStore(target, "audit-export-fixture");
        ChatGptAccount.Config config = ChatGptAccount.Config.defaults(target); config.store = accountStore;
        ChatGptAccount.install(new ChatGptAccount(config));
        try {
            AgentAuditSession s = begin();
            s.append("note", new JSONObject().put("text", vin + " " + serial + " 654321.0 AA:BB:CC:DD:EE:12 " + email + " " + subject), null, null, null);
            s.finish("success");
            String source = text(sessionFile(s.sessionId()));
            java.lang.reflect.Method preview;
            try { preview = AgentAuditExport.class.getMethod("preview", Context.class, String.class); }
            catch (NoSuchMethodException e) { throw new AssertionError("REQ-045.24 store-derived export preview missing", e); }
            JSONObject empty = (JSONObject) preview.invoke(null, records, s.sessionId());
            require(empty.getJSONArray("matches").length() == 0 && empty.getString("warning").contains("Erkannt werden nur Werte, die die App kennt."), "empty stores do not guess");
            require("created".equals(vehicles.observe(new JSONObject().put("vin", vin), 1)), "fixture vehicle saved");
            vehicles.odometer(vin, 654321.0, 2);
            JSONObject identity = new JSONObject().put("value", serial).put("semantic", "ecu.serial");
            vehicles.setDiagnostic(vin, "ecu_discovery", new JSONObject().put("targets", new JSONArray().put(new JSONObject().put("identity", new JSONArray().put(identity)))));
            adapter.edit().putString("known_address", "AA:BB:CC:DD:EE:12").commit();
            accountStore.save(new AccountStore.Record("synthetic-client", "access-test", "refresh-test", "id-test", ChatGptAccount.SCOPE,
                email, subject, System.currentTimeMillis() + 3600000, 0, false));
            ChatGptAccount.install(new ChatGptAccount(config));
            JSONObject redacted = (JSONObject) preview.invoke(null, records, s.sessionId());
            require(redacted.getJSONArray("matches").length() == 6, "REQ-045.24 every known value is preselected and matched");
            String text = redacted.getString("text");
            require(!text.contains(vin) && !text.contains(serial) && !text.contains("654321.0")
                && !text.contains("AA:BB:CC:DD:EE:12") && !text.contains(email) && !text.contains(subject), "known values redacted");
            int before = children().size();
            require(AgentAuditExport.confirm(target, redacted, false) == null && children().size() == before, "REQ-045.25 cancelled export creates nothing");
            Uri exported = AgentAuditExport.confirm(target, redacted, true);
            require(exported != null && !text(exported).contains(vin) && source.equals(text(sessionFile(s.sessionId()))), "confirmed export redacts known values without changing source");
        } finally {
            accountStore.deleteAll(); ChatGptAccount.install(null);
            if (previous.isEmpty()) adapter.edit().remove("known_address").commit(); else adapter.edit().putString("known_address", previous).commit();
            new java.io.File(isolated, "vehicles.json").delete(); isolated.delete();
        }
    }

    /** REQ-045.26 and .30: retention and appends fail closed on every detectable outside change. */
    private void retentionAndOutsideChanges() throws Exception {
        Uri complete = oldAudit(true), incomplete = oldAudit(false);
        AgentAuditCatalog.remember(target, tree);
        long old = 31L * 86400000L;
        Uri intruder = create("intruder.txt");
        resetCatalogMemory();
        expectNoDeletion(old, complete, "REQ-045.26 an unknown file");
        delete(intruder);
        AgentAuditCatalog.remember(target, tree);
        try (OutputStream out = resolver.openOutputStream(incomplete, "wa")) { out.write('x'); }
        resetCatalogMemory();
        expectNoDeletion(old, complete, "REQ-045.26 another size");
        AgentAuditCatalog.remember(target, tree);
        delete(incomplete);
        incomplete = oldAudit(false, 2, false);
        resetCatalogMemory();
        expectNoDeletion(old, complete, "REQ-045.26 another file identity");
        AgentAuditCatalog.remember(target, tree);
        AgentAuditSession.expire(target, old);
        require(!exists(complete) && exists(incomplete), "REQ-045.26 only the verified completed session expires; incomplete sessions stay");
        AgentAuditSession.expire(target, 10L * 86400000L);
        require(exists(incomplete), "REQ-045.26 nothing expires before 30 days");
        AgentAuditSession s = begin();
        Uri outsider = create("outsider.txt");
        resetCatalogMemory();
        require(refused(() -> s.disclosure(true)) != null && s.failed(), "REQ-045.30 an outside change stops every further append");
        delete(outsider);
        AgentAuditCatalog.remember(target, tree);
        AgentAuditSession running = begin();
        require(AgentAuditSession.storageWarning(target) == false, "REQ-045.28 a valid destination needs no warning");
        running.finish("success");
    }

    /** REQ-045.26/.30: a crash or sync error after the write must not permanently block the folder. */
    private void interruptedAppendRecovery() throws Exception {
        java.lang.reflect.Method prepare;
        try { prepare = AgentAuditCatalog.class.getMethod("prepareAppend", Context.class, Uri.class, Uri.class, long.class, long.class); }
        catch (NoSuchMethodException e) { throw new AssertionError("REQ-045.30 durable pending append missing", e); }
        for (int scenario = 0; scenario < 3; scenario++) {
            AgentAuditSession s = begin();
            Uri file = sessionFile(s.sessionId());
            if (scenario == 2) {
                AgentAuditSession.sync = fd -> { fd.sync(); throw new IOException("synthetic sync failure"); };
                try {
                    require(refused(() -> s.append("note", new JSONObject(), null, null, null)) != null && s.failed(),
                        "REQ-045.28 a throwing sync stops the session");
                } finally { AgentAuditSession.sync = java.io.FileDescriptor::sync; }
            } else {
                byte[] line = "{\"type\":\"synthetic\"}\n".getBytes(StandardCharsets.UTF_8);
                long before = size(file);
                prepare.invoke(null, target, tree, file, before, before + line.length);
                if (scenario == 1) {
                    try (java.io.FileOutputStream out = new ParcelFileDescriptor.AutoCloseOutputStream(resolver.openFileDescriptor(file, "wa"))) {
                        out.write(line); out.getFD().sync();
                    }
                }
                java.lang.reflect.Field active = AgentAuditSession.class.getDeclaredField("active");
                active.setAccessible(true); active.set(null, null);
            }
            resetCatalogMemory();
            AgentAuditSession recovered = begin();
            recovered.finish("success");
        }
    }

    /** REQ-045.26: a deliberate folder re-selection preserves everything already there. */
    private void confirmedFolderRecovery() throws Exception {
        Uri complete = oldAudit(true);
        AgentAuditCatalog.remember(target, tree);
        Uri foreign = create("foreign.txt");
        resetCatalogMemory();
        require(refused(() -> AgentAuditSession.begin(target, metadata())) != null, "REQ-045.26 outside change blocks work before confirmation");
        java.lang.reflect.Method adopt;
        try { adopt = AgentAuditCatalog.class.getMethod("adopt", Context.class, Uri.class); }
        catch (NoSuchMethodException e) { throw new AssertionError("REQ-045.26 explicit folder adoption missing", e); }
        adopt.invoke(null, target, tree);
        AgentAuditSession recovered = begin();
        recovered.finish("success");
        AgentAuditSession.expire(target, System.currentTimeMillis() + 31L * 86400000L);
        require(exists(complete) && exists(foreign), "REQ-045.26 adoption never deletes pre-existing files");
    }

    /** REQ-045.26/.30: simulate a fresh process without deleting the persisted catalog. */
    private static void resetCatalogMemory() throws Exception {
        java.lang.reflect.Field expected = AgentAuditCatalog.class.getDeclaredField("expected");
        java.lang.reflect.Field tree = AgentAuditCatalog.class.getDeclaredField("expectedTree");
        expected.setAccessible(true); tree.setAccessible(true);
        expected.set(null, null); tree.set(null, "");
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
        require(refused(() -> w.runModelRequest(new JSONObject(), "turn1", id(), json -> sent.set(true))) != null && !sent.get(), "REQ-045.32 work that needs events beyond the cap is not begun");
        AgentAuditSession.resetLimits();
        w.finish("success");
    }

    // ------------------------------------------------------------------------------------------------ helpers
    /** REQ-046.23: real JNI and audit around the shared synthetic vehicle. */
    private void sharedDiagnosis() throws Exception {
        ObdService service = ObdService.current;
        ObdService.Connector connector = service.connector;
        VehicleStore vehicles = service.vehicles;
        Boolean permission = service.permissionOverride, bluetooth = service.bluetoothOverride;
        boolean autoRecord = ObdService.autoRecordEnabled(target);
        SharedFixtureLink link = new SharedFixtureLink(own, service);
        AgentAuditSession audit = null;
        try {
            service.setAutoConnect(false); service.disconnect();
            awaitShared(() -> "disconnected".equals(service.snapshot().optString("phase")));
            service.permissionOverride = true; service.bluetoothOverride = true;
            service.vehicles = new VehicleStore(new java.io.File(target.getCacheDir(), "shared-diagnosis-" + System.nanoTime() + ".json"));
            service.connector = address -> link;
            service.setAutoRecord(true); service.connect("AA:BB:CC:00:00:62");
            awaitShared(() -> "ready".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"));
            service.setSignals(new JSONArray());
            java.lang.reflect.Field field = ObdService.class.getDeclaredField("worker"); field.setAccessible(true);
            java.util.concurrent.ExecutorService worker = (java.util.concurrent.ExecutorService) field.get(service);
            java.lang.reflect.Method execute = ObdService.class.getDeclaredMethod("execute", String.class, String.class); execute.setAccessible(true);
            require(sharedCore(worker, execute, service, "ecu.discover", "").optBoolean("ok"), "fixture ECU discovery");
            require(sharedCore(worker, execute, service, "assistant.tools", "").optBoolean("ok"), "fixture tools");
            require(sharedCore(worker, execute, service, "assistant.begin", "").optBoolean("ok"), "fixture begin");
            String recording = service.snapshot().getString("autoRecording");
            java.io.File source = new java.io.File(target.getFilesDir(), "recordings/" + recording);
            require(source.isFile(), "fixture recording exists");
            audit = begin(); audit.disclosure(true);
            AgentAuditSession session = audit;
            session.runModelRequest(new JSONObject().put("case", "synthetic EGR/MAF"), "turn1", "request1", request -> {
                try { session.recordModelEvent("request1", "response_item", new JSONObject().put("text", "Read signals and stored codes.")); }
                catch (Exception e) { throw new IllegalStateException(e); }
            });
            JSONObject listing = sharedTool(session, worker, execute, service, source, recording, "list", new JSONObject().put("tool", "signal_list"));
            String rpm = sharedSignalId(listing, "engine.rpm"), maf = sharedSignalId(listing, "engine.maf");
            String knowledge = sharedSignalId(listing, "air.charge_limit_overcharge");
            JSONObject rpmResult = sharedTool(session, worker, execute, service, source, recording, "rpm", sharedRead(rpm));
            JSONObject scan = sharedTool(session, worker, execute, service, source, recording, "dtc", new JSONObject().put("tool", "dtc_scan"));
            require(scan.toString().contains("P0401"), "fixture EGR code drives follow-up");
            session.runModelRequest(new JSONObject().put("finding", "P0401").put("follow_up", "MAF and Knowledge"), "turn2", "request2", request -> {
                try { session.recordModelEvent("request2", "response_item", new JSONObject().put("text", "Read MAF and Knowledge; EGR position is missing.")); }
                catch (Exception e) { throw new IllegalStateException(e); }
            });
            JSONObject mafResult = sharedTool(session, worker, execute, service, source, recording, "maf", sharedRead(maf));
            JSONObject knowledgeResult = sharedTool(session, worker, execute, service, source, recording, "knowledge", sharedRead(knowledge));
            JSONObject egrResult = sharedTool(session, worker, execute, service, source, recording, "egr", sharedRead("engine.egr.commanded"));
            require(rpmResult.optDouble("value") == 1726 && mafResult.optDouble("value") == 10
                && "ok".equals(knowledgeResult.optString("outcome")) && "unknown_signal".equals(egrResult.optString("reason")),
                "fixture measured values and missing EGR fact");
            int before = link.commands.size();
            session.runTool("turn2", "request2", "denied", "raw", "{\"command\":\"04\"}", false, "not allowed",
                () -> { throw new AssertionError("denied tool dispatched"); });
            require(link.commands.size() == before, "REQ-046.23 denied tool emits zero TX");
            session.finish("unknown");
            java.io.File destination = new java.io.File(target.getFilesDir(), "req-046-23");
            if (!destination.isDirectory() && !destination.mkdirs()) throw new IOException("fixture output directory");
            java.nio.file.Files.write(new java.io.File(destination, "audit.jsonl").toPath(), text(sessionFile(session.sessionId())).getBytes(StandardCharsets.UTF_8));
            service.disconnect();
            awaitShared(() -> "disconnected".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"));
            java.nio.file.Files.copy(source.toPath(), new java.io.File(destination, recording).toPath(), java.nio.file.StandardCopyOption.REPLACE_EXISTING);
        } finally {
            if (audit != null) try { audit.stop("cancelled"); } catch (IOException ignored) { }
            service.disconnect(); service.connector = connector; service.vehicles = vehicles;
            service.permissionOverride = permission; service.bluetoothOverride = bluetooth;
            service.setAutoRecord(autoRecord); service.setDemo();
            awaitShared(() -> "ready".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"));
        }
    }
    private interface SharedReady { boolean get() throws Exception; }
    private static void awaitShared(SharedReady ready) throws Exception {
        long end = android.os.SystemClock.elapsedRealtime() + 60000;
        while (android.os.SystemClock.elapsedRealtime() < end) {
            if (ready.get()) return;
            android.os.SystemClock.sleep(100);
        }
        throw new AssertionError("shared diagnosis timed out");
    }
    private static JSONObject sharedCore(java.util.concurrent.ExecutorService worker, java.lang.reflect.Method execute, ObdService service, String action, String argument) throws Exception {
        return worker.submit(() -> (JSONObject) execute.invoke(service, action, argument)).get(60, java.util.concurrent.TimeUnit.SECONDS);
    }
    private static JSONObject sharedRead(String id) throws Exception {
        return new JSONObject().put("tool", "signal_read").put("arguments", new JSONObject().put("signal", id));
    }
    private static String sharedSignalId(JSONObject listing, String semantic) throws Exception {
        JSONArray signals = listing.getJSONArray("signals");
        for (int i = 0; i < signals.length(); i++) if (semantic.equals(signals.getJSONObject(i).optString("semantic"))) return signals.getJSONObject(i).getString("id");
        throw new AssertionError("missing signal " + semantic);
    }
    private static JSONObject sharedTool(AgentAuditSession audit, java.util.concurrent.ExecutorService worker, java.lang.reflect.Method execute,
        ObdService service, java.io.File source, String recording, String id, JSONObject request) throws Exception {
        JSONObject[] result = new JSONObject[1];
        String turn = id.equals("maf") || id.equals("knowledge") || id.equals("egr") ? "turn2" : "turn1";
        audit.runTool(turn, turn.equals("turn1") ? "request1" : "request2", id, request.getString("tool"), request.toString(), true, "allowed", () -> {
            long offset = source.length();
            JSONObject envelope = sharedCore(worker, execute, service, "assistant.tool", request.toString());
            require(envelope.optBoolean("ok"), "assistant tool failed: " + envelope);
            result[0] = envelope.getJSONObject("data");
            return new AgentAuditSession.ToolResult(result[0], null, new JSONObject().put("recording_ref", recording).put("offset", offset));
        });
        return result[0];
    }


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
