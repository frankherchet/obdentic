package de.obdentic.app;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.function.BiPredicate;
import org.json.JSONObject;

/**
 * REQ-021: the buttons whose action was tapped and has not yet shown its result. Owned by ObdService, so it outlives
 * Activity recreation (REQ-021.6). Time and snapshots are passed in, so the logic runs without a clock or a UI.
 */
public final class PendingActions {
    /** What an action looks like to the user. {@code target} gets (current snapshot, snapshot at the tap) and says whether the result arrived. */
    public static final class Spec {
        public final String key, name, pending; public final long timeoutMs; final BiPredicate<JSONObject,JSONObject> target;
        public Spec(String key, String name, String pending, long timeoutMs, BiPredicate<JSONObject,JSONObject> target) { this.key=key; this.name=name; this.pending=pending; this.timeoutMs=timeoutMs; this.target=target; }
    }
    public static final class Entry {
        public final String id, name, label; public final JSONObject baseline; final long startedAt, timeoutMs; final BiPredicate<JSONObject,JSONObject> target;
        Entry(String id, Spec spec, JSONObject baseline, long now) { this.id=id; name=spec.name; label=spec.pending; this.baseline=baseline; startedAt=now; timeoutMs=spec.timeoutMs; target=spec.target; }
    }
    private final LinkedHashMap<String,Entry> entries = new LinkedHashMap<>();
    private String notice = ""; private long noticeSeq = -1;

    /** REQ-021.4: false when this action is already pending, so the caller starts nothing. */
    public synchronized boolean start(Spec spec, String suffix, JSONObject baseline, long nowMs) {
        String id = spec.key + suffix; if(entries.containsKey(id)) return false;
        entries.put(id,new Entry(id,spec,baseline,nowMs)); return true;
    }
    public synchronized boolean isPending(String id) { return entries.containsKey(id); }
    public synchronized Entry resolve(String id) { return entries.remove(id); }
    /** REQ-021.2: ends every entry whose target state is visible in {@code now}. */
    public synchronized List<Entry> observe(JSONObject now) {
        List<Entry> done = new ArrayList<>();
        for(Entry e : new ArrayList<>(entries.values())) if(e.target.test(now,e.baseline)) { entries.remove(e.id); done.add(e); }
        return done;
    }
    /** REQ-021.3: ends every entry older than its limit. */
    public synchronized List<Entry> expire(long nowMs) {
        List<Entry> late = new ArrayList<>();
        for(Entry e : new ArrayList<>(entries.values())) if(nowMs - e.startedAt >= e.timeoutMs) { entries.remove(e.id); late.add(e); }
        return late;
    }
    /** Changes whenever the set of pending buttons changes; part of the Activity's render key. */
    public synchronized String signature() { return entries.keySet().toString(); }
    /** The "no response" text stays the status message until the next service message (a late result replaces it). */
    public synchronized void setNotice(String text, long messageSeq) { notice = text; noticeSeq = messageSeq; }
    public synchronized String notice(long messageSeq) { return messageSeq == noticeSeq ? notice : ""; }
}
