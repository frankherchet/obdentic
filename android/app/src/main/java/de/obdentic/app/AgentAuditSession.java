package de.obdentic.app;

import android.content.ContentResolver;
import android.content.Context;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.os.SystemClock;
import android.provider.DocumentsContract;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.UUID;
import org.json.JSONException;
import org.json.JSONObject;

/** One append-only agent audit in the explicitly chosen local SAF tree. */
public final class AgentAuditSession {
    public static final long DEFAULT_TOTAL_LIMIT = 256L * 1024 * 1024, DEFAULT_SESSION_LIMIT = 32L * 1024 * 1024;
    public static final int DEFAULT_EVENT_LIMIT = 2 * 1024 * 1024, DEFAULT_COUNT_LIMIT = 10000;
    /** The REQ-045.29/.30/.31/.32 budgets; only the acceptance tests change them, to hit the exact boundaries cheaply. */
    public static volatile long TOTAL_LIMIT = DEFAULT_TOTAL_LIMIT, SESSION_LIMIT = DEFAULT_SESSION_LIMIT;
    public static volatile int EVENT_LIMIT = DEFAULT_EVENT_LIMIT, COUNT_LIMIT = DEFAULT_COUNT_LIMIT;

    public static void setLimits(long total, long session, int event, int count) { TOTAL_LIMIT = total; SESSION_LIMIT = session; EVENT_LIMIT = event; COUNT_LIMIT = count; }
    public static void resetLimits() { setLimits(DEFAULT_TOTAL_LIMIT, DEFAULT_SESSION_LIMIT, DEFAULT_EVENT_LIMIT, DEFAULT_COUNT_LIMIT); }

    /** REQ-045.29: from 80 % of the total budget on, a warning is due before more work. Counts every audit byte, active and incomplete included. */
    public static boolean storageWarning(Context context) throws IOException {
        Context app = context.getApplicationContext();
        synchronized (LOCK) { return bytes(app.getContentResolver(), AgentAuditStorage.selected(app)) >= (TOTAL_LIMIT * 4 + 4) / 5; }
    }
    public boolean failed() { synchronized (LOCK) { return failed; } }
    private static final String PREFIX = "obdentic-agent-", MIME = "application/x-ndjson";
    public static final String MIME_TYPE = "application/x-ndjson";
    private static final Object LOCK = new Object();
    private static AgentAuditSession active;
    private final Context context;
    private final Uri tree, document;
    private final String id;
    private long sequence, size;
    private boolean failed, ended, disclosureApproved, disclosureDenied;
    private String inFlightRequest;
    private final java.util.HashSet<String> requests = new java.util.HashSet<>(), tools = new java.util.HashSet<>();

    private AgentAuditSession(Context context, Uri tree, Uri document, String id) {
        this.context = context; this.tree = tree; this.document = document; this.id = id;
    }

    /** Creates a session only in the selected tree and syncs the start before returning. */
    public static AgentAuditSession begin(Context supplied, JSONObject metadata) throws IOException {
        synchronized (LOCK) {
            if (active != null) throw new IOException("Ein Agentenprotokoll läuft bereits");
            if (metadata == null || metadata.optString("runtime_version").isEmpty()
                    || metadata.optString("build_version").isEmpty() || metadata.optString("model_version").isEmpty()
                    || metadata.optString("knowledge_pin").isEmpty() || !metadata.has("vehicle_ref")
                    || metadata.optJSONObject("configuration") == null || metadata.optJSONObject("budgets") == null)
                throw new IOException("Protokollkontext ist unvollständig");
            Context context = supplied.getApplicationContext();
            Uri tree = AgentAuditStorage.selected(context);
            expire(context, System.currentTimeMillis());
            long total = bytes(context.getContentResolver(), tree);
            if (total >= TOTAL_LIMIT) throw new IOException("Protokollspeicher voll");
            String id = UUID.randomUUID().toString();
            try {
                Uri parent = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
                Uri document = DocumentsContract.createDocument(context.getContentResolver(), parent, MIME, PREFIX + id + ".jsonl");
                if (document == null) throw new IOException("Protokolldatei konnte nicht angelegt werden");
                AgentAuditCatalog.remember(context, tree); // the app's own new file is part of the expected folder
                AgentAuditSession session = new AgentAuditSession(context, tree, document, id);
                active = session;
                try { session.append("session_start", metadata, null, null, null); }
                catch (IOException e) { active = null; throw e; }
                return session;
            } catch (SecurityException | IllegalArgumentException e) {
                throw new IOException("Protokollordner nicht verfügbar");
            }
        }
    }
    /** Persist each disclosure decision; a denial permanently stops this diagnosis. */
    public void disclosure(boolean approved) throws IOException {
        synchronized (LOCK) {
            if (disclosureDenied) throw new IOException("Offenlegung wurde abgelehnt");
            try {
                append("disclosure", new JSONObject().put("approved", approved), null, null, null);
                disclosureApproved = approved;
                disclosureDenied = !approved;
            } catch (JSONException e) { throw new IOException("Offenlegung konnte nicht protokolliert werden"); }
        }
    }
    /** A typed failure of the model request: timeout, network_error, usage_limit. Received events stay in the audit. */
    public static final class ModelFailure extends RuntimeException {
        public final String outcome;
        public ModelFailure(String outcome) { super(outcome); this.outcome = outcome; }
    }

    /** Synchronized end marker; the diagnosis outcome is separate from the audit being complete. A failed end leaves the audit incomplete. */
    public void finish(String diagnosisStatus) throws IOException {
        synchronized (LOCK) {
            try { append("session_end", new JSONObject().put("diagnosis_status", diagnosisStatus), null, null, null); }
            catch (JSONException e) { throw new IOException("Ende konnte nicht protokolliert werden"); }
        }
    }

    /** Cancel or disconnect: no further work for this diagnosis; the open request, if any, is ended with the reason. */
    public void stop(String reason) throws IOException {
        synchronized (LOCK) {
            try {
                if (inFlightRequest != null) append("request_end", new JSONObject().put("status", reason), null, inFlightRequest, null);
            } catch (JSONException e) { throw new IOException("Abbruch konnte nicht protokolliert werden"); }
            finish(reason);
        }
    }

    public String sessionId() { return id; }
    public interface ModelDispatch { void send(String requestJson); }
    /** Audit the actual request before the synchronous synthetic client receives it. */
    public void runModelRequest(JSONObject request, String requestId, ModelDispatch dispatch) throws IOException {
        String exact;
        synchronized (LOCK) {
            if (active != this || failed || ended || inFlightRequest != null || !disclosureApproved || disclosureDenied
                    || requestId == null || requestId.isEmpty() || requests.contains(requestId) || sequence > COUNT_LIMIT - 3)
                throw new IOException("Protokoll oder Offenlegung erlaubt keinen weiteren Auftrag");
            try {
                JSONObject copy = new JSONObject(request.toString());
                append("request_intent", new JSONObject().put("request", copy), null, requestId, null);
                requests.add(requestId);
                exact = copy.toString();
                inFlightRequest = requestId;
            } catch (JSONException e) {
                failed = true; active = null;
                throw new IOException("Anfrage konnte nicht protokolliert werden");
            }
        }
        try {
            dispatch.send(exact);
            append("request_dispatched", new JSONObject(), null, requestId, null);
            append("request_end", new JSONObject().put("status", "completed"), null, requestId, null);
        } catch (JSONException e) {
            synchronized (LOCK) { failed = true; active = null; }
            throw new IOException("Anfrage konnte nicht protokolliert werden");
        } catch (RuntimeException e) {
            String status = e instanceof ModelFailure ? ((ModelFailure) e).outcome : "error";
            try { append("request_end", new JSONObject().put("status", status), null, requestId, null); }
            catch (JSONException impossible) { throw new IOException("Protokoll fehlgeschlagen"); }
            throw new IOException("Anfrage fehlgeschlagen");
        } finally {
            synchronized (LOCK) { inFlightRequest = null; }
        }
    }

    /** Persist each received stream item in arrival order while its request is active. */
    public void recordModelEvent(String requestId, String type, JSONObject data) throws IOException {
        synchronized (LOCK) {
            if (inFlightRequest == null || !inFlightRequest.equals(requestId)
                    || !("response_delta".equals(type) || "response_item".equals(type)))
                throw new IOException("Kein passender Modellauftrag aktiv");
            append(type, data, null, requestId, null);
        }
    }
    /** Record a proposal and policy decision before starting the permitted operation. */
    /** What a tool produced locally and the exact subset that went out of the app (REQ-045.7); the subset is null when nothing was sent. */
    public static final class ToolResult {
        public final JSONObject local, sentExternally;
        public ToolResult(JSONObject local, JSONObject sentExternally) { this.local = local; this.sentExternally = sentExternally; }
    }

    public void runTool(String callId, String name, String arguments, boolean allowed, String reason, java.util.concurrent.Callable<ToolResult> work) throws IOException {
        synchronized (LOCK) {
            if (active != this || failed || ended || disclosureDenied || callId == null || callId.isEmpty()
                    || tools.contains(callId) || sequence > COUNT_LIMIT - (allowed ? 5 : 2))
                throw new IOException("Protokoll erlaubt keinen weiteren Auftrag");
            try {
                append("tool_proposal", new JSONObject().put("name", name).put("arguments", arguments), null, null, callId);
                tools.add(callId);
                append("tool_decision", new JSONObject().put("allowed", allowed).put("reason", reason), null, null, callId);
                if (!allowed) return;
                append("tool_intent", new JSONObject().put("operation", name), null, null, callId);
                ToolResult result;
                try {
                    result = work.call();
                    if (result == null || result.local == null) throw new IllegalStateException("tool returned no result");
                } catch (Exception e) {
                    append("tool_end", new JSONObject().put("status", "error"), null, null, callId);
                    throw new IOException("Werkzeug fehlgeschlagen");
                }
                append("tool_dispatched", new JSONObject(), null, null, callId);
                JSONObject end = new JSONObject().put("status", "completed").put("result", result.local);
                if (result.sentExternally != null) end.put("sent_external", result.sentExternally);
                append("tool_end", end, null, null, callId);
            } catch (JSONException e) {
                failed = true; active = null;
                throw new IOException("Werkzeug konnte nicht protokolliert werden");
            }
        }
    }

    /** Persist an entire event and synchronize its file descriptor before returning to the caller. */
    public void append(String type, JSONObject data, String turnId, String requestId, String toolCallId) throws IOException {
        synchronized (LOCK) {
            if (active != this || failed || ended) throw new IOException("Protokoll nicht mehr beschreibbar");
            try {
                if (!tree.equals(AgentAuditStorage.selected(context))) throw new IOException("Protokollordner wurde geändert");
                AgentAuditCatalog.verify(context, tree); // REQ-045.30: a third-party change stops every further append
                if (sequence >= COUNT_LIMIT) throw new IOException("Protokoll-Ereignisgrenze erreicht");
                JSONObject event = new JSONObject().put("schema", "obdentic-agent-audit").put("version", 1)
                    .put("session_id", id).put("seq", sequence + 1).put("wall_ms", System.currentTimeMillis())
                    .put("mono_ns", SystemClock.elapsedRealtimeNanos()).put("type", type).put("data", data);
                if (turnId != null) event.put("turn_id", turnId);
                if (requestId != null) event.put("request_id", requestId);
                if (toolCallId != null) event.put("tool_call_id", toolCallId);
                String[] secrets = ChatGptAccount.get(context).auditSecrets();
                if (AgentAuditPrivacy.containsSecret(event, secrets) || AgentAuditPrivacy.containsSecret(event.toString(), secrets))
                    throw new IOException("Protokoll enthält unzulässige Zugangsdaten");
                byte[] line = (event.toString() + "\n").getBytes(StandardCharsets.UTF_8);
                long total = bytes(context.getContentResolver(), tree);
                if (line.length > EVENT_LIMIT || size + line.length > SESSION_LIMIT || total + line.length > TOTAL_LIMIT)
                    throw new IOException("Protokollspeichergrenze erreicht");
                ParcelFileDescriptor fd = context.getContentResolver().openFileDescriptor(document, "wa");
                if (fd == null) throw new IOException("Protokolldatei nicht verfügbar");
                try (FileOutputStream output = new ParcelFileDescriptor.AutoCloseOutputStream(fd)) {
                    output.write(line); output.flush(); output.getFD().sync();
                }
                sequence++; size += line.length;
                AgentAuditCatalog.remember(context, tree);
                if ("session_end".equals(type)) { ended = true; active = null; }
            } catch (IOException | JSONException | RuntimeException e) {
                failed = true; active = null;
                throw new IOException("Protokoll konnte nicht dauerhaft gespeichert werden");
            }
        }
    }
    /** Expire only whole, structurally verified completed sessions; never the active file. */
    public static void expire(Context context, long nowMs) throws IOException {
        synchronized (LOCK) {
            Uri tree = AgentAuditStorage.selected(context);
            ContentResolver resolver = context.getContentResolver();
            long cutoff = nowMs - 30L * 86400000L;
            if (cutoff < 0) return;
            AgentAuditCatalog.verify(context, tree); // REQ-045.26: a third-party change deletes nothing
            java.util.ArrayList<Uri> candidates = new java.util.ArrayList<>();
            java.util.ArrayList<String> ids = new java.util.ArrayList<>();
            try {
                Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
                try (Cursor c = resolver.query(children, new String[]{DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME, DocumentsContract.Document.COLUMN_SIZE}, null, null, null)) {
                    if (c == null) throw new IOException("Protokollordner nicht lesbar");
                    while (c.moveToNext()) {
                        String name = c.getString(1);
                        if (name == null || !name.startsWith(PREFIX) || !name.endsWith(".jsonl") || c.isNull(2) || c.getLong(2) > SESSION_LIMIT) continue;
                        String id = name.substring(PREFIX.length(), name.length() - 6);
                        try { if (!UUID.fromString(id).toString().equals(id)) continue; }
                        catch (IllegalArgumentException e) { continue; }
                        Uri file = DocumentsContract.buildDocumentUriUsingTree(tree, c.getString(0));
                        if (active != null && active.document.equals(file)) continue;
                        candidates.add(file); ids.add(id);
                    }
                }
                for (int i = 0; i < candidates.size(); i++) {
                    long end = verifiedEnd(resolver, candidates.get(i), ids.get(i));
                    if (end >= 0 && end <= cutoff && !DocumentsContract.deleteDocument(resolver, candidates.get(i)))
                        throw new IOException("Altes Protokoll konnte nicht gelöscht werden");
                }
            } catch (SecurityException | IllegalArgumentException | UnsupportedOperationException e) {
                throw new IOException("Protokollordner nicht verfügbar");
            }
            AgentAuditCatalog.remember(context, tree);
        }
    }

    private static long verifiedEnd(ContentResolver resolver, Uri file, String id) throws IOException {
        java.util.HashSet<String> open = new java.util.HashSet<>();
        int seq = 0, ch;
        long lastMono = -1, end = -1;
        java.io.ByteArrayOutputStream line = new java.io.ByteArrayOutputStream();
        try (java.io.InputStream raw = resolver.openInputStream(file)) {
            if (raw == null) return -1;
            java.io.BufferedInputStream input = new java.io.BufferedInputStream(raw);
            while ((ch = input.read()) != -1) {
                if (ch != '\n') {
                    if (line.size() >= EVENT_LIMIT - 1) return -1;
                    line.write(ch); continue;
                }
                if (++seq > COUNT_LIMIT || end >= 0) return -1;
                String text;
                try { text = StandardCharsets.UTF_8.newDecoder().decode(java.nio.ByteBuffer.wrap(line.toByteArray())).toString(); }
                catch (java.nio.charset.CharacterCodingException e) { return -1; }
                line.reset();
                try {
                    org.json.JSONTokener tokener = new org.json.JSONTokener(text);
                    Object parsed = tokener.nextValue();
                    if (!(parsed instanceof JSONObject) || tokener.nextClean() != 0) return -1;
                    JSONObject event = (JSONObject) parsed;
                    String type = event.optString("type");
                    long mono = event.optLong("mono_ns", -1);
                    if (!"obdentic-agent-audit".equals(event.optString("schema")) || event.optInt("version") != 1
                            || !id.equals(event.optString("session_id")) || event.optLong("seq") != seq
                            || mono < 0 || mono < lastMono || event.optLong("wall_ms", -1) < 0 || event.optJSONObject("data") == null
                            || (seq == 1) != "session_start".equals(type)) return -1;
                    if (!java.util.Arrays.asList("session_start", "session_end", "disclosure", "request_intent", "request_dispatched",
                            "request_end", "response_item", "response_delta", "tool_proposal", "tool_decision",
                            "tool_intent", "tool_dispatched", "tool_end").contains(type)) return -1;
                    lastMono = mono;
                    if ("request_intent".equals(type) || "tool_intent".equals(type)) {
                        String key = type.startsWith("request") ? "r:" + event.optString("request_id") : "t:" + event.optString("tool_call_id");
                        if (key.length() == 2 || !open.add(key)) return -1;
                    } else if ("request_end".equals(type) || "tool_end".equals(type)) {
                        String key = type.startsWith("request") ? "r:" + event.optString("request_id") : "t:" + event.optString("tool_call_id");
                        if (!open.remove(key)) return -1;
                    } else if ("session_end".equals(type)) {
                        if (!open.isEmpty()) return -1;
                        end = event.optLong("wall_ms", -1);
                    }
                } catch (JSONException e) { return -1; }
            }
            return line.size() == 0 ? end : -1;
        }
    }

    /** Includes active and incomplete audit files; an unknown provider size fails closed. */
    private static long bytes(ContentResolver resolver, Uri tree) throws IOException {
        try {
            Uri root = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
            try (Cursor cursor = resolver.query(root, new String[]{DocumentsContract.Document.COLUMN_DOCUMENT_ID}, null, null, null)) {
                if (cursor == null || !cursor.moveToFirst()) throw new IOException("Protokollordner fehlt");
            }
            Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
            long total = 0;
            try (Cursor cursor = resolver.query(children, new String[]{DocumentsContract.Document.COLUMN_DISPLAY_NAME, DocumentsContract.Document.COLUMN_SIZE}, null, null, null)) {
                if (cursor == null) throw new IOException("Protokollordner nicht lesbar");
                while (cursor.moveToNext()) {
                    String name = cursor.getString(0);
                    if (name == null || !name.startsWith(PREFIX) || !name.endsWith(".jsonl")) continue;
                    if (cursor.isNull(1) || cursor.getLong(1) < 0) throw new IOException("Protokollgröße unbekannt");
                    long length = cursor.getLong(1);
                    if (total > Long.MAX_VALUE - length) throw new IOException("Protokollgröße ungültig");
                    total += length;
                }
            }
            return total;
        } catch (SecurityException | IllegalArgumentException | UnsupportedOperationException e) {
            throw new IOException("Protokollordner nicht verfügbar");
        }
    }
}
