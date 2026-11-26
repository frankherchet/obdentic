package de.obdentic.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.os.Bundle;
import android.widget.Button;

/** Focused, simulated acceptance for the agent audit; never connects to a vehicle or model. */
public final class AgentAuditAcceptanceRunner extends Instrumentation {
    @Override public void onCreate(Bundle arguments) { super.onCreate(arguments); start(); }
    @Override public void onStart() {
        Bundle result = new Bundle();
        ObdService.autoScan = false;
        ObdService.autoSurvey = false;
        android.content.SharedPreferences prefs = getTargetContext().getSharedPreferences("adapter", 0);
        boolean automatic = prefs.getBoolean("auto", true);
        prefs.edit().putBoolean("auto", false).commit();
        try {
            // REQ-045.26/.30: unknown files, changed size and changed file identity all fail closed.
            java.lang.reflect.Method verify;
            try { verify = Class.forName("de.obdentic.app.AgentAuditCatalog").getMethod("unchanged", org.json.JSONObject.class, org.json.JSONObject.class); }
            catch (ClassNotFoundException | NoSuchMethodException e) { throw new AssertionError("audit folder integrity boundary missing"); }
            org.json.JSONObject expectedFolder = new org.json.JSONObject().put("synthetic-file", new org.json.JSONObject().put("id", "id1").put("size", 3).put("device", 1).put("inode", 2));
            org.json.JSONObject extraFile = new org.json.JSONObject(expectedFolder.toString()).put("unknown", new org.json.JSONObject());
            org.json.JSONObject changedSize = new org.json.JSONObject(expectedFolder.toString());
            changedSize.getJSONObject("synthetic-file").put("size", 4);
            org.json.JSONObject changedIdentity = new org.json.JSONObject(expectedFolder.toString());
            changedIdentity.getJSONObject("synthetic-file").put("inode", 3);
            if (!Boolean.TRUE.equals(verify.invoke(null, expectedFolder, new org.json.JSONObject(expectedFolder.toString())))
                    || !Boolean.FALSE.equals(verify.invoke(null, expectedFolder, extraFile))
                    || !Boolean.FALSE.equals(verify.invoke(null, expectedFolder, changedSize))
                    || !Boolean.FALSE.equals(verify.invoke(null, expectedFolder, changedIdentity)))
                throw new AssertionError("audit folder changes must stop retention and append");
            // REQ-045.10/.28: no session can start without an explicitly selected, accessible destination.
            android.content.SharedPreferences audit = getTargetContext().getSharedPreferences("agent-audit", 0);
            String previous = audit.getString("folder", "");
            audit.edit().remove("folder").commit();
            try {
                boolean refused = false;
                try {
                    Class<?> session = Class.forName("de.obdentic.app.AgentAuditSession");
                    session.getMethod("begin", android.content.Context.class, org.json.JSONObject.class)
                        .invoke(null, getTargetContext(), metadata());
                } catch (java.lang.reflect.InvocationTargetException e) {
                    refused = e.getCause() instanceof java.io.IOException;
                } catch (ClassNotFoundException | NoSuchMethodException e) {
                    // A missing audit boundary does not count as a safe refusal.
                }
                if (!refused) throw new AssertionError("session must refuse a missing audit destination");
            } finally {
                if (previous.isEmpty()) audit.edit().remove("folder").commit();
                else audit.edit().putString("folder", previous).commit();
            }
            // REQ-045.22: exact current secrets and their 16-character substrings are rejected.
            boolean protects = false;
            try {
                Class<?> privacy = Class.forName("de.obdentic.app.AgentAuditPrivacy");
                java.lang.reflect.Method contains = privacy.getMethod("containsSecret", String.class, String[].class);
                String marker = "synthetic-marker-abcdefghijklmnop";
                protects = Boolean.TRUE.equals(contains.invoke(null, "prefix " + marker + " suffix", new String[]{marker}))
                    && Boolean.TRUE.equals(contains.invoke(null, marker.substring(3, 19), new String[]{marker}))
                    && Boolean.FALSE.equals(contains.invoke(null, "ordinary vehicle data", new String[]{marker}))
                    && Boolean.TRUE.equals(contains.invoke(null, "Bearer synthetic", new String[0]));
            } catch (ClassNotFoundException | NoSuchMethodException e) { /* no protection */ }
            if (!protects) throw new AssertionError("outgoing payload secret protection missing");
            // REQ-045.24: every occurrence of known selected values is previewed, including inside free text.
            boolean redactionWorks = false;
            try {
                java.lang.reflect.Method redact = AgentAuditPrivacy.class.getMethod("redact", String.class, org.json.JSONObject.class, org.json.JSONArray.class);
                org.json.JSONObject known = new org.json.JSONObject().put("vin", new org.json.JSONArray().put("SYNTHETIC-VALUE"));
                org.json.JSONObject preview = (org.json.JSONObject) redact.invoke(null, "xSYNTHETIC-VALUEy SYNTHETIC-VALUE unknown", known, new org.json.JSONArray().put("vin"));
                redactionWorks = "x[redacted]y [redacted] unknown".equals(preview.optString("text"))
                    && preview.getJSONArray("matches").length() == 2;
            } catch (NoSuchMethodException e) { /* export cannot claim a preview */ }
            if (!redactionWorks) throw new AssertionError("redaction preview must show each known occurrence");
            // REQ-045.1: a session without version, configuration and budget context cannot begin.
            boolean missingMetadataRejected = false;
            AgentAuditSession invalid = null;
            try { invalid = AgentAuditSession.begin(getTargetContext(), new org.json.JSONObject()); }
            catch (java.io.IOException expected) { missingMetadataRejected = true; }
            finally { if (invalid != null) invalid.append("session_end", new org.json.JSONObject(), null, null, null); }
            if (!missingMetadataRejected) throw new AssertionError("session metadata is mandatory");
            // REQ-045.2/.10: the exact request is persisted before the synthetic client is dispatched.
            AgentAuditSession active = AgentAuditSession.begin(getTargetContext(), metadata());
            try {
                active.disclosure(true);
                java.util.concurrent.atomic.AtomicBoolean sent = new java.util.concurrent.atomic.AtomicBoolean();
                String requestId = java.util.UUID.randomUUID().toString();
                // REQ-045.3: ordered stream deltas and the final status remain inspectable.
                java.lang.reflect.Method modelEvent;
                try { modelEvent = active.getClass().getMethod("recordModelEvent", String.class, String.class, org.json.JSONObject.class); }
                catch (NoSuchMethodException e) { throw new AssertionError("model stream audit missing"); }
                active.runModelRequest(new org.json.JSONObject().put("message", "synthetic"), requestId, json -> {
                    sent.set("{\"message\":\"synthetic\"}".equals(json) && requestIsPersisted(getTargetContext(), requestId));
                    try {
                        modelEvent.invoke(active, requestId, "response_delta", new org.json.JSONObject().put("text", "first"));
                        modelEvent.invoke(active, requestId, "response_delta", new org.json.JSONObject().put("text", "second"));
                    } catch (Exception e) { throw new IllegalStateException(e); }
                });
                org.json.JSONArray requestEvents = eventsBy(getTargetContext(), "request_id", requestId);
                if (!sent.get() || requestEvents.length() != 5
                        || !"first".equals(requestEvents.getJSONObject(1).getJSONObject("data").optString("text"))
                        || !"second".equals(requestEvents.getJSONObject(2).getJSONObject("data").optString("text")))
                    throw new AssertionError("ordered model stream was not persisted");
                // REQ-045.14: a retry cannot reuse an earlier request identity.
                java.util.concurrent.atomic.AtomicBoolean reused = new java.util.concurrent.atomic.AtomicBoolean();
                boolean duplicateRejected = false;
                try { active.runModelRequest(new org.json.JSONObject(), requestId, json -> reused.set(true)); }
                catch (java.io.IOException expected) { duplicateRejected = true; }
                if (!duplicateRejected || reused.get()) throw new AssertionError("request identities must not be reused");
                // REQ-045.5: policy-denied tool proposals never execute.
                String deniedId = java.util.UUID.randomUUID().toString();
                java.util.concurrent.atomic.AtomicBoolean toolRan = new java.util.concurrent.atomic.AtomicBoolean();
                try {
                    active.getClass().getMethod("runTool", String.class, String.class, String.class, boolean.class, String.class, java.util.concurrent.Callable.class)
                        .invoke(active, deniedId, "synthetic_tool", "{}", false, "denied by policy",
                            (java.util.concurrent.Callable<org.json.JSONObject>) () -> { toolRan.set(true); return new org.json.JSONObject(); });
                } catch (NoSuchMethodException e) { throw new AssertionError("tool lifecycle boundary missing"); }
                org.json.JSONArray deniedEvents = toolEvents(getTargetContext(), deniedId);
                if (toolRan.get() || deniedEvents.length() != 2
                        || !"tool_proposal".equals(deniedEvents.getJSONObject(0).optString("type"))
                        || !"tool_decision".equals(deniedEvents.getJSONObject(1).optString("type")))
                    throw new AssertionError("denied tool must record proposal and decision without execution");
                // REQ-045.4: the complete result belongs to the allowed tool lifecycle.
                String allowedId = java.util.UUID.randomUUID().toString();
                active.getClass().getMethod("runTool", String.class, String.class, String.class, boolean.class, String.class, java.util.concurrent.Callable.class)
                    .invoke(active, allowedId, "synthetic_tool", "{\"x\":1}", true, "allowed",
                        (java.util.concurrent.Callable<org.json.JSONObject>) () -> new org.json.JSONObject().put("result", "synthetic"));
                org.json.JSONArray allowedEvents = toolEvents(getTargetContext(), allowedId);
                if (allowedEvents.length() != 5 || !"synthetic".equals(allowedEvents.getJSONObject(4).getJSONObject("data").getJSONObject("result").optString("result")))
                    throw new AssertionError("allowed tool result must be persisted");
            } finally {
                active.append("session_end", new org.json.JSONObject().put("diagnosis_status", "success"), null, null, null);
            }
            // REQ-045.30: an outside change stops every further append of the running session.
            AgentAuditSession intruded = AgentAuditSession.begin(getTargetContext(), metadata());
            android.net.Uri treeNow = AgentAuditStorage.selected(getTargetContext());
            android.content.ContentResolver resolver = getTargetContext().getContentResolver();
            android.net.Uri intruderFile = android.provider.DocumentsContract.createDocument(resolver, android.provider.DocumentsContract.buildDocumentUriUsingTree(treeNow, android.provider.DocumentsContract.getTreeDocumentId(treeNow)), "text/plain", "outsider.txt");
            boolean appendRefused = false;
            try { intruded.disclosure(true); } catch (java.io.IOException expected) { appendRefused = true; }
            android.provider.DocumentsContract.deleteDocument(resolver, intruderFile);
            AgentAuditCatalog.remember(getTargetContext(), treeNow);
            if (!appendRefused) throw new AssertionError("append must fail closed after an outside change");
            // REQ-045.13/.14/.15/.16/.17: typed outcomes are recorded, a cancel or disconnect stops all further work.
            for (String outcome : new String[]{"timeout", "network_error", "usage_limit"}) {
                AgentAuditSession failing = AgentAuditSession.begin(getTargetContext(), metadata());
                failing.disclosure(true);
                String failId = java.util.UUID.randomUUID().toString();
                boolean failed = false;
                try { failing.runModelRequest(new org.json.JSONObject(), failId, json -> { throw new AgentAuditSession.ModelFailure(outcome); }); }
                catch (java.io.IOException expected) { failed = true; }
                org.json.JSONArray failEvents = eventsBy(getTargetContext(), "request_id", failId);
                if (!failed || !outcome.equals(failEvents.getJSONObject(failEvents.length() - 1).getJSONObject("data").optString("status")))
                    throw new AssertionError("outcome must be recorded: " + outcome);
                failing.finish("failed");
            }
            for (String stop : new String[]{"cancelled", "disconnected"}) {
                AgentAuditSession stopping = AgentAuditSession.begin(getTargetContext(), metadata());
                stopping.disclosure(true);
                stopping.stop(stop);
                java.util.concurrent.atomic.AtomicBoolean ran = new java.util.concurrent.atomic.AtomicBoolean();
                boolean refusedAfterStop = false;
                try { stopping.runModelRequest(new org.json.JSONObject(), java.util.UUID.randomUUID().toString(), json -> ran.set(true)); }
                catch (java.io.IOException expected) { refusedAfterStop = true; }
                if (!refusedAfterStop || ran.get()) throw new AssertionError("no work after " + stop);
            }
            // REQ-045.24/.25: export needs confirmation; cancelling creates nothing and the source stays unchanged.
            AgentAuditSession exporting = AgentAuditSession.begin(getTargetContext(), metadata());
            exporting.append("disclosure", new org.json.JSONObject().put("approved", true).put("note", "SYNTHETIC-VIN"), null, null, null);
            String exportId = exporting.sessionId();
            exporting.finish("success");
            org.json.JSONObject knownValues = new org.json.JSONObject().put("vin", new org.json.JSONArray().put("SYNTHETIC-VIN"));
            org.json.JSONArray categories = new org.json.JSONArray().put("vin");
            org.json.JSONObject exportPreview = AgentAuditExport.preview(getTargetContext(), exportId, knownValues, categories);
            if (exportPreview.getJSONArray("matches").length() != 1 || !exportPreview.getString("warning").contains("Erkannt werden nur Werte, die die App kennt."))
                throw new AssertionError("export preview must show matches and warning");
            int before = documentCount(getTargetContext());
            if (AgentAuditExport.confirm(getTargetContext(), exportPreview, false) != null || documentCount(getTargetContext()) != before)
                throw new AssertionError("cancelled export must create nothing");
            android.net.Uri exported = AgentAuditExport.confirm(getTargetContext(), exportPreview, true);
            if (exported == null || exportedText(getTargetContext(), exported).contains("SYNTHETIC-VIN"))
                throw new AssertionError("confirmed export must be redacted");
            // REQ-045.8: initial denial is recorded and prevents the request from starting.
            AgentAuditSession denied = AgentAuditSession.begin(getTargetContext(), metadata());
            try {
                boolean protectedByConsent = false;
                java.util.concurrent.atomic.AtomicBoolean sent = new java.util.concurrent.atomic.AtomicBoolean();
                try {
                    denied.getClass().getMethod("disclosure", boolean.class).invoke(denied, false);
                    try {
                        denied.runModelRequest(new org.json.JSONObject().put("message", "synthetic"), "denied", json -> sent.set(true));
                    } catch (java.io.IOException expected) { protectedByConsent = true; }
                } catch (NoSuchMethodException e) { /* consent must be recorded, not assumed */ }
                if (!protectedByConsent || sent.get()) throw new AssertionError("denied disclosure must prevent dispatch");
            } finally {
                denied.append("session_end", new org.json.JSONObject().put("diagnosis_status", "denied"), null, null, null);
            }
            // REQ-045.26: retention deletes only verified completed sessions after 30 days.
            retentionCheck(getTargetContext());
            Activity activity = startActivitySync(new Intent(getTargetContext(), MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            runOnMainSync(() -> UiNav.openAdapter(activity));
            waitForIdleSync();
            runOnMainSync(() -> {
                Button account = UiNav.button(activity.getWindow().getDecorView(), "Konto öffnen");
                if (account == null) throw new AssertionError("account navigation missing");
                account.performClick();
            });
            waitForIdleSync();
            runOnMainSync(() -> {
                // REQ-045.23: selecting a local audit folder is an explicit user action.
                if (UiNav.button(activity.getWindow().getDecorView(), "Protokollordner auswählen") == null)
                    throw new AssertionError("audit folder selection missing");
            });
            result.putString("stream", "AGENT AUDIT ACCEPTANCE PASSED\n");
            finish(Activity.RESULT_OK, result);
        } catch (Throwable error) {
            result.putString("stream", "FAILED: " + error + "\n");
            finish(Activity.RESULT_CANCELED, result);
        } finally { prefs.edit().putBoolean("auto", automatic).commit(); }
    }
    private static boolean requestIsPersisted(android.content.Context context, String requestId) {
        try {
            android.net.Uri tree = AgentAuditStorage.selected(context);
            android.net.Uri children = android.provider.DocumentsContract.buildChildDocumentsUriUsingTree(tree, android.provider.DocumentsContract.getTreeDocumentId(tree));
            try (android.database.Cursor c = context.getContentResolver().query(children, new String[]{android.provider.DocumentsContract.Document.COLUMN_DOCUMENT_ID}, null, null, null)) {
                if (c == null) return false;
                while (c.moveToNext()) {
                    android.net.Uri file = android.provider.DocumentsContract.buildDocumentUriUsingTree(tree, c.getString(0));
                    try (java.io.InputStream in = context.getContentResolver().openInputStream(file)) {
                        if (in == null) continue;
                        java.io.BufferedReader lines = new java.io.BufferedReader(new java.io.InputStreamReader(in, java.nio.charset.StandardCharsets.UTF_8));
                        String line;
                        while ((line = lines.readLine()) != null) {
                            org.json.JSONObject event = new org.json.JSONObject(line);
                            if ("request_intent".equals(event.optString("type")) && requestId.equals(event.optString("request_id"))
                                    && "synthetic".equals(event.getJSONObject("data").getJSONObject("request").optString("message"))) return true;
                        }
                    }
                }
            }
        } catch (Exception e) { return false; }
        return false;
    }
    private static org.json.JSONArray toolEvents(android.content.Context context, String callId) throws Exception {
        return eventsBy(context, "tool_call_id", callId);
    }
    private static org.json.JSONArray eventsBy(android.content.Context context, String field, String id) throws Exception {
        org.json.JSONArray events = new org.json.JSONArray();
        android.net.Uri tree = AgentAuditStorage.selected(context);
        android.net.Uri children = android.provider.DocumentsContract.buildChildDocumentsUriUsingTree(tree, android.provider.DocumentsContract.getTreeDocumentId(tree));
        try (android.database.Cursor c = context.getContentResolver().query(children, new String[]{android.provider.DocumentsContract.Document.COLUMN_DOCUMENT_ID}, null, null, null)) {
            if (c == null) throw new AssertionError("audit directory unavailable");
            while (c.moveToNext()) {
                android.net.Uri file = android.provider.DocumentsContract.buildDocumentUriUsingTree(tree, c.getString(0));
                try (java.io.InputStream in = context.getContentResolver().openInputStream(file)) {
                    if (in == null) continue;
                    java.io.BufferedReader lines = new java.io.BufferedReader(new java.io.InputStreamReader(in, java.nio.charset.StandardCharsets.UTF_8));
                    String line;
                    while ((line = lines.readLine()) != null) {
                        org.json.JSONObject event = new org.json.JSONObject(line);
                        if (id.equals(event.optString(field))) events.put(event);
                    }
                }
            }
        }
        return events;
    }
    private static int documentCount(android.content.Context context) throws Exception {
        android.net.Uri tree = AgentAuditStorage.selected(context);
        android.net.Uri children = android.provider.DocumentsContract.buildChildDocumentsUriUsingTree(tree, android.provider.DocumentsContract.getTreeDocumentId(tree));
        try (android.database.Cursor c = context.getContentResolver().query(children, new String[]{android.provider.DocumentsContract.Document.COLUMN_DOCUMENT_ID}, null, null, null)) {
            return c == null ? -1 : c.getCount();
        }
    }
    private static String exportedText(android.content.Context context, android.net.Uri uri) throws Exception {
        try (java.io.InputStream in = context.getContentResolver().openInputStream(uri)) {
            return new java.util.Scanner(in, "UTF-8").useDelimiter("\\A").next();
        }
    }
    private static void retentionCheck(android.content.Context context) throws Exception {
        android.net.Uri tree = AgentAuditStorage.selected(context);
        android.net.Uri parent = android.provider.DocumentsContract.buildDocumentUriUsingTree(tree, android.provider.DocumentsContract.getTreeDocumentId(tree));
        android.net.Uri complete = null, incomplete = null;
        try {
            complete = makeOldAudit(context, parent, true);
            incomplete = makeOldAudit(context, parent, false);
            AgentAuditCatalog.remember(context, tree); // the synthetic old sessions stand for sessions the app itself left
            long old = 31L * 86400000L;
            // REQ-045.26/.30: every detectable third-party change stops retention and deletes nothing.
            android.net.Uri intruder = android.provider.DocumentsContract.createDocument(context.getContentResolver(), parent, "text/plain", "intruder.txt");
            try { expectRefused(context, old, complete, "unknown file"); } finally { android.provider.DocumentsContract.deleteDocument(context.getContentResolver(), intruder); }
            AgentAuditCatalog.remember(context, tree);
            try (android.os.ParcelFileDescriptor fd = context.getContentResolver().openFileDescriptor(incomplete, "wa")) {
                new java.io.FileOutputStream(fd.getFileDescriptor()).write("x".getBytes());
            }
            expectRefused(context, old, complete, "changed size");
            AgentAuditCatalog.remember(context, tree);
            android.provider.DocumentsContract.deleteDocument(context.getContentResolver(), incomplete);
            incomplete = makeOldAudit(context, parent, false); // same role, another file identity
            expectRefused(context, old, complete, "replaced file");
            AgentAuditCatalog.remember(context, tree);
            try { AgentAuditSession.class.getMethod("expire", android.content.Context.class, long.class).invoke(null, context, old); }
            catch (NoSuchMethodException e) { throw new AssertionError("audit retention missing"); }
            if (documentExists(context, complete) || !documentExists(context, incomplete))
                throw new AssertionError("retention must preserve incomplete sessions");
        } finally {
            if (complete != null) try { android.provider.DocumentsContract.deleteDocument(context.getContentResolver(), complete); } catch (Exception ignored) {}
            if (incomplete != null) try { android.provider.DocumentsContract.deleteDocument(context.getContentResolver(), incomplete); } catch (Exception ignored) {}
        }
    }
    private static void expectRefused(android.content.Context context, long now, android.net.Uri survivor, String change) throws Exception {
        boolean refused = false;
        try { AgentAuditSession.expire(context, now); } catch (java.io.IOException expected) { refused = true; }
        if (!refused || !documentExists(context, survivor)) throw new AssertionError("third-party change must fail closed: " + change);
    }
    private static android.net.Uri makeOldAudit(android.content.Context context, android.net.Uri parent, boolean complete) throws Exception {
        String id = java.util.UUID.randomUUID().toString();
        android.net.Uri uri = android.provider.DocumentsContract.createDocument(context.getContentResolver(), parent, "application/x-ndjson", "obdentic-agent-" + id + ".jsonl");
        if (uri == null) throw new AssertionError("could not create synthetic audit");
        org.json.JSONObject first = new org.json.JSONObject().put("schema", "obdentic-agent-audit").put("version", 1).put("session_id", id)
            .put("seq", 1).put("wall_ms", 1).put("mono_ns", 1).put("type", "session_start").put("data", new org.json.JSONObject());
        org.json.JSONObject end = new org.json.JSONObject().put("schema", "obdentic-agent-audit").put("version", 1).put("session_id", id)
            .put("seq", 2).put("wall_ms", 2).put("mono_ns", 2).put("type", "session_end").put("data", new org.json.JSONObject());
        try (android.os.ParcelFileDescriptor fd = context.getContentResolver().openFileDescriptor(uri, "w")) {
            if (fd == null) throw new AssertionError("could not write synthetic audit");
            java.io.FileOutputStream out = new java.io.FileOutputStream(fd.getFileDescriptor());
            out.write((first.toString() + "\n" + (complete ? end.toString() + "\n" : "")).getBytes(java.nio.charset.StandardCharsets.UTF_8));
            out.flush(); fd.getFileDescriptor().sync();
        }
        return uri;
    }
    private static boolean documentExists(android.content.Context context, android.net.Uri uri) {
        try (android.database.Cursor c = context.getContentResolver().query(uri, new String[]{android.provider.DocumentsContract.Document.COLUMN_DOCUMENT_ID}, null, null, null)) {
            return c != null && c.moveToFirst();
        } catch (IllegalArgumentException missing) { return false; }
    }
    private static org.json.JSONObject metadata() throws org.json.JSONException {
        return new org.json.JSONObject().put("runtime_version", "synthetic").put("build_version", "synthetic")
            .put("model_version", "synthetic").put("knowledge_pin", "synthetic").put("vehicle_ref", org.json.JSONObject.NULL)
            .put("configuration", new org.json.JSONObject()).put("budgets", new org.json.JSONObject());
    }
}
