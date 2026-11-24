package de.obdentic.spike378;

import android.content.BroadcastReceiver;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.os.SystemClock;
import android.provider.DocumentsContract;
import android.system.Os;
import android.system.OsConstants;
import android.util.Log;

import java.io.File;
import java.io.FileDescriptor;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.channels.FileChannel;
import java.util.ArrayList;
import java.util.List;
import java.util.zip.CRC32;

/**
 * Throwaway spike #378 prototype (not product code). Driven by `am broadcast`.
 * dest:  saf | path | nobackup | files | cache      (saf and path are the user-selected shared internal directory)
 * strat: wa (O_APPEND + fsync) | wfd (O_APPEND + FileChannel.force(false)) | pre (preallocated, positional write, force(false))
 */
public class Cmd extends BroadcastReceiver {
  static final String T = "Spike378";
  static final String DIRNAME = "obdentic-spike378-audit";
  static final String PATHDIR = "/storage/emulated/0/Documents/" + DIRNAME;
  static final long PREALLOC = 8L << 20;
  static final int EXISTING_LOGICAL = 256 << 10;

  Context ctx;
  ContentResolver cr;
  Uri tree;
  String run, dest, strat;
  FileWriter out;
  long lastAcked = -1;
  int dispatches = 0;
  String killPoint = "";
  long killSeq = -1;
  String preMethod = "";
  boolean noExtra;

  @Override public void onReceive(Context c, Intent i) {
    final PendingResult pr = goAsync();
    ctx = c.getApplicationContext();
    cr = ctx.getContentResolver();
    final Intent in = i;
    new Thread(new Runnable() { public void run() {
      try { go(in); } catch (Throwable t) { Log.i(T, "FATAL " + t); }
      pr.finish();
    }}).start();
  }

  void go(Intent in) throws Exception {
    String cmd = in.getStringExtra("cmd");
    run = in.getStringExtra("run");
    if (run == null) run = "r" + System.currentTimeMillis();
    String t = ctx.getSharedPreferences("s", 0).getString("tree", null);
    tree = t == null ? null : Uri.parse(t);
    dest = in.hasExtra("dest") ? in.getStringExtra("dest") : "saf";
    strat = in.hasExtra("strat") ? in.getStringExtra("strat") : "wa";
    out = new FileWriter(new File(ctx.getExternalFilesDir(null), (in.hasExtra("csv") ? in.getStringExtra("csv") : run) + ".csv"), true);
    int size = in.getIntExtra("size", 4096);
    int n = in.getIntExtra("n", 100);
    killPoint = in.hasExtra("kill") ? in.getStringExtra("kill") : "";
    killSeq = in.getIntExtra("killseq", -1);
    noExtra = in.getBooleanExtra("noextra", false);
    Log.i(T, "CMD " + cmd + " run=" + run + " dest=" + dest + " strat=" + strat + " size=" + size + " n=" + n
        + " procAgeMs=" + (SystemClock.elapsedRealtime() - Process.getStartElapsedRealtime()));
    switch (cmd) {
      case "probe": probe(); break;
      case "cold": cold(size, in.getBooleanExtra("existing", false)); break;
      case "mkexisting": mkexisting(); break;
      case "gc": gc(size, n, in.getIntExtra("k", 1), in.getIntExtra("win", 0)); break;
      case "idle": idle(size, n, in.getIntExtra("interval", 0)); break;
      case "clean": clean(in.getStringExtra("prefix")); break;
      case "seq": seq(size, n, in.getIntExtra("interval", 0)); break;
      default: Log.i(T, "unknown cmd");
    }
    out.close();
    Log.i(T, "DONE " + cmd + " run=" + run);
  }

  boolean isPre() { return strat.startsWith("pre"); }

  // ---------- destination ----------
  Uri treeDir() { return DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree)); }

  File fileDir() {
    switch (dest) {
      case "path": return new File(PATHDIR);
      case "nobackup": return ctx.getNoBackupFilesDir();
      case "files": return ctx.getFilesDir();
      case "cache": return ctx.getCacheDir();
      default: throw new IllegalStateException("not a file dest " + dest);
    }
  }

  /** One open audit file. */
  class H {
    Object ref; // Uri (saf) or File
    ParcelFileDescriptor pfd;
    FileDescriptor fd;
    FileOutputStream os;
    java.io.RandomAccessFile raf;
    FileChannel ch;
    long pos;      // logical end (pre) / bytes written (wa)
    long createNs, openNs, scanNs, preNs, dirNs;

    void create(String name) throws Exception {
      long t0 = System.nanoTime();
      if (dest.equals("saf")) {
        ref = DocumentsContract.createDocument(cr, treeDir(), "application/octet-stream", name);
        if (ref == null) throw new IOException("createDocument returned null");
      } else {
        File f = new File(fileDir(), name);
        if (!f.createNewFile()) throw new IOException("exists " + f);
        ref = f;
      }
      createNs = System.nanoTime() - t0;
    }

    void resolve(String name) {
      if (dest.equals("saf")) ref = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree) + "/" + name);
      else ref = new File(fileDir(), name);
    }

    void open() throws Exception {
      long t0 = System.nanoTime();
      boolean pre = isPre();
      if (dest.equals("saf")) {
        pfd = cr.openFileDescriptor((Uri) ref, pre ? "rw" : "wa");
        fd = pfd.getFileDescriptor();
        os = new FileOutputStream(fd);
      } else {
        if (pre) { raf = new java.io.RandomAccessFile((File) ref, "rw"); fd = raf.getFD(); ch = raf.getChannel(); }
        else { os = new FileOutputStream((File) ref, true); fd = os.getFD(); }
      }
      if (ch == null) ch = os.getChannel();
      openNs = System.nanoTime() - t0;
    }

    /** Preallocate and make the size durable (full fsync). Method recorded in preMethod. */
    void preallocate() throws Exception {
      long t0 = System.nanoTime();
      try { Os.posix_fallocate(fd, 0, PREALLOC); preMethod = "fallocate"; }
      catch (Throwable e) {
        preMethod = "zerofill(" + e.getClass().getSimpleName() + ")";
        byte[] z = new byte[1 << 20];
        for (long p = 0; p < PREALLOC; p += z.length) ch.write(ByteBuffer.wrap(z), p);
      }
      ch.force(true);
      preNs = System.nanoTime() - t0;
    }

    /**
     * Find the logical end of a preallocated file: end of the last complete newline-terminated record before the first NUL.
     * strat "prescan": sequential read of the whole prefix (array loop). strat "pre": binary search for the first all-zero 4 KiB block
     * (valid because acknowledged data is a gap-free prefix), then a scan of the block before it only.
     */
    void scan() throws Exception {
      long t0 = System.nanoTime();
      FileChannel rc = new FileInputStream(fd).getChannel();
      final int BLK = 4096;
      long start = 0;
      if (!strat.equals("prescan")) {
        long lo = 0, hi = PREALLOC / BLK;      // first all-zero block index in [lo, hi]
        ByteBuffer b = ByteBuffer.allocate(1);
        while (lo < hi) {
          long mid = (lo + hi) >>> 1;
          b.clear(); int r = rc.read(b, mid * BLK);
          boolean zero = r <= 0 || b.get(0) == 0;   // data blocks start with a JSON byte; zero tail starts with NUL
          if (zero) hi = mid; else lo = mid + 1;
        }
        start = Math.max(0, lo - 1) * BLK;
      }
      ByteBuffer b = ByteBuffer.allocate(64 << 10);
      long off = start, lastNl = start; boolean done = false;
      byte[] a = b.array();
      while (!done) {
        b.clear();
        int r = rc.read(b, off);
        if (r <= 0) break;
        for (int k = 0; k < r; k++) {
          byte x = a[k];
          if (x == 0) { done = true; break; }
          if (x == '\n') lastNl = off + k + 1;
        }
        off += r;
      }
      pos = lastNl;
      scanNs = System.nanoTime() - t0;
    }

    void scanWa() throws Exception { // wa/wfd: logical end = file size
      pos = pfd != null ? Os.fstat(fd).st_size : ((File) ref).length();
    }

    /** write + sync per strategy; returns [write_ns, sync_ns]. */
    long[] append(byte[] rec) throws Exception {
      long t1 = System.nanoTime();
      if (isPre()) {
        ByteBuffer b = ByteBuffer.wrap(rec);
        long p = pos;
        while (b.hasRemaining()) p += ch.write(b, p);
      } else {
        os.write(rec);
      }
      killAt("after_write", rec.length);
      long t2 = System.nanoTime();
      if (strat.equals("wa")) fd.sync(); else ch.force(false);
      long t3 = System.nanoTime();
      pos += rec.length;
      return new long[] {t2 - t1, t3 - t2};
    }

    void dirSync() throws Exception {
      long t0 = System.nanoTime();
      if (dest.equals("saf")) {
        try (ParcelFileDescriptor d = cr.openFileDescriptor(treeDir(), "r")) { Os.fsync(d.getFileDescriptor()); }
      } else {
        FileDescriptor d = Os.open(fileDir().getPath(), OsConstants.O_RDONLY, 0);
        try { Os.fsync(d); } finally { Os.close(d); }
      }
      dirNs = System.nanoTime() - t0;
    }

    void close() { try { if (raf != null) raf.close(); } catch (Throwable e) { } try { if (os != null) os.close(); } catch (Throwable e) { } try { if (pfd != null) pfd.close(); } catch (Throwable e) { } os = null; pfd = null; }
  }

  // ---------- records ----------
  byte[] record(long seq, int size) {
    String head = "{\"v\":1,\"run\":\"" + run + "\",\"seq\":" + seq + ",\"sz\":" + size + ",\"crc\":\"";
    String mid = "\",\"pad\":\"";
    String tail = "\"}\n";
    int padLen = size - head.length() - 8 - mid.length() - tail.length();
    if (padLen < 0) padLen = 0;
    char[] pad = new char[padLen];
    long x = seq * 2654435761L + 12345;
    for (int k = 0; k < padLen; k++) { x = x * 6364136223846793005L + 1442695040888963407L; pad[k] = (char) ('a' + ((x >>> 33) % 26)); }
    String p = new String(pad);
    CRC32 c = new CRC32(); c.update(p.getBytes());
    return (head + String.format("%08x", c.getValue()) + mid + p + tail).getBytes();
  }

  void killAt(String point, long seq) {
    // after_write passes the record length as seq; only the kill sequence number gate uses lastAcked+1
    if (point.equals("after_write")) seq = lastAcked + 1;
    if (point.equals(killPoint) && seq == killSeq) {
      Log.i(T, "KILL run=" + run + " point=" + point + " seq=" + seq);
      Process.killProcess(Process.myPid());
      try { Thread.sleep(10000); } catch (Exception e) { }
    }
  }

  void csv(String label, int size, int i, long total, long[] c, String ok) throws Exception {
    StringBuilder sb = new StringBuilder(label).append(',').append(size).append(',').append(i).append(',').append(total);
    for (int k = 0; k < 9; k++) sb.append(',').append(c != null && k < c.length ? c[k] : -1);
    out.write(sb.append(',').append(ok).append('\n').toString());
  }

  // ---------- commands ----------
  void probe() throws Exception {
    String[] ds = {"saf", "path", "nobackup", "files", "cache"};
    for (String d : ds) {
      dest = d;
      try {
        H h = new H(); strat = "pre";
        h.create("probe-" + d + ".bin"); h.open();
        String m;
        try { Os.posix_fallocate(h.fd, 0, 1 << 20); m = "ok"; } catch (Throwable e) { m = e.toString(); }
        Log.i(T, "PROBE dest=" + d + " fallocate=" + m + " size=" + Os.fstat(h.fd).st_size + " blocks=" + Os.fstat(h.fd).st_blocks);
        try { h.ch.force(true); Log.i(T, "PROBE dest=" + d + " force(true) ok"); } catch (Throwable e) { Log.i(T, "PROBE dest=" + d + " force " + e); }
        try { h.dirSync(); Log.i(T, "PROBE dest=" + d + " dirsync ok ms=" + h.dirNs / 1e6); } catch (Throwable e) { Log.i(T, "PROBE dest=" + d + " dirsync " + e); }
        Log.i(T, "PROBE dest=" + d + " fdlink=" + Os.readlink("/proc/self/fd/" + fdInt(h.fd)));
        h.close();
      } catch (Throwable e) { Log.i(T, "PROBE dest=" + d + " failed " + e); }
    }
  }

  static int fdInt(FileDescriptor f) throws Exception {
    java.lang.reflect.Method m = FileDescriptor.class.getDeclaredMethod("getInt$");
    return (Integer) m.invoke(f);
  }

  /** Warm process, held descriptor, fixed pause between appends (hypothesis test: is the first-sync cost process- or idle-related?). */
  void idle(int size, int n, int interval) throws Exception {
    H h = new H(); h.create("idle-" + run + ".jsonl"); h.open();
    if (isPre()) h.preallocate();
    h.dirSync();
    for (int i = -3; i < n; i++) {
      byte[] rec = record(i + 3, size);
      long t0 = System.nanoTime();
      long[] tm = h.append(rec);
      long tot = System.nanoTime() - t0;
      csv("idle-" + dest + "-" + strat + "-i" + interval + (i < 0 ? "-warmup" : ""), size, i, tot, new long[] {tm[0], tm[1]}, "ok");
      if (interval > 0) Thread.sleep(interval);
    }
    h.close();
  }

  void clean(String prefix) {
    File[] fs = fileDir().listFiles();
    int n = 0;
    if (fs != null) for (File f : fs) if (f.getName().startsWith(prefix) && f.delete()) n++;
    Log.i(T, "CLEAN " + dest + " " + prefix + " deleted=" + n);
  }

  String existingName() { return "cold-existing-" + dest + "-" + strat + ".jsonl"; }

  /** Untimed: create the "existing" file with EXISTING_LOGICAL bytes of complete records. */
  void mkexisting() throws Exception {
    H g = new H(); g.create(existingName()); g.open();
    if (isPre()) g.preallocate();
    int per = 16 << 10;
    for (int i = 0; i < EXISTING_LOGICAL / per; i++) g.append(record(1000 + i, per));
    g.dirSync();
    g.close();
    Log.i(T, "MKEXISTING " + existingName() + " logical=" + EXISTING_LOGICAL);
  }

  void cold(int size, boolean existing) throws Exception {
    byte[] rec = record(0, size);   // encoded before the timer (protocol)
    byte[] r2 = record(1, size), r3 = record(2, size);
    long age = SystemClock.elapsedRealtime() - Process.getStartElapsedRealtime();
    String label = "cold-" + dest + "-" + strat + "-" + (existing ? "existing" : "new");
    H h = new H();
    long t0 = System.nanoTime();
    long[] tm;
    try {
      if (existing) { h.resolve(existingName()); h.open(); if (isPre()) h.scan(); else h.scanWa(); }
      else { h.create("cold-" + run + ".jsonl"); h.open(); if (isPre()) h.preallocate(); }
      tm = h.append(rec);
      if (!existing) h.dirSync();   // new file: the name must be durable too
      long total = System.nanoTime() - t0;
      lastAcked = 0;
      Log.i(T, "ACK run=" + run + " seq=0 sz=" + size + " ms=" + String.format("%.3f", total / 1e6));
      // c: create, open, scan, prealloc, write, sync, dirsync, a2, a3 ... (verdict uses total only)
      long[] c = {h.createNs, h.openNs, h.scanNs, h.preNs, tm[0], tm[1], h.dirNs, 0, 0};
      if (!noExtra) {   // noextra: keeps a preallocated 8 MiB file from overflowing at 64 KiB (3 x 64 KiB x 100 > 8 MiB)
        long a = System.nanoTime(); h.append(r2); c[7] = System.nanoTime() - a;
        a = System.nanoTime(); h.append(r3); c[8] = System.nanoTime() - a;
      }
      csv(label, size, 0, total, c, "ok:" + preMethod + ":age=" + age);
    } catch (Throwable e) {
      csv(label, size, 0, -1, null, "ERR " + e.getClass().getSimpleName());
    }
    h.close();
  }

  // ---------- group commit (warm) ----------
  class Req { byte[] rec; long t0; volatile boolean done; volatile Throwable err; int batch; }

  void gc(final int size, final int n, final int k, final int win) throws Exception {
    final H h = new H();
    h.create("gc-" + run + ".jsonl"); h.open();
    if (isPre()) h.preallocate();
    h.dirSync();
    final ArrayList<Req> q = new ArrayList<Req>();
    final boolean[] stop = {false};
    Thread w = new Thread(new Runnable() { public void run() {
      while (true) {
        List<Req> b;
        synchronized (q) {
          while (q.isEmpty() && !stop[0]) { try { q.wait(); } catch (InterruptedException e) { } }
          if (q.isEmpty() && stop[0]) return;
        }
        if (win > 0) { try { Thread.sleep(win); } catch (InterruptedException e) { } }
        synchronized (q) { b = new ArrayList<Req>(q); q.clear(); }
        Throwable err = null;
        try {
          if (isPre()) {
            for (Req r : b) { ByteBuffer bb = ByteBuffer.wrap(r.rec); long p = h.pos; while (bb.hasRemaining()) p += h.ch.write(bb, p); h.pos = p; }
            h.ch.force(false);
          } else {
            for (Req r : b) h.os.write(r.rec);
            if (strat.equals("wa")) h.fd.sync(); else h.ch.force(false);
          }
        } catch (Throwable e) { err = e; }
        for (Req r : b) { r.batch = b.size(); r.err = err; r.done = true; }
        synchronized (q) { q.notifyAll(); }
      }
    }});
    w.start();
    final ArrayList<String> rows = new ArrayList<String>();
    Thread[] ts = new Thread[k];
    final String label = "gc-" + dest + "-" + strat + "-k" + k + "-w" + win;
    for (int a = 0; a < k; a++) {
      final int id = a;
      ts[a] = new Thread(new Runnable() { public void run() {
        for (int i = -5; i < n; i++) {   // 5 warm-up appends per writer, flagged
          Req r = new Req(); r.rec = record(id * 100000L + i + 5, size);
          r.t0 = System.nanoTime();
          synchronized (q) { q.add(r); q.notifyAll(); }
          synchronized (q) { while (!r.done) { try { q.wait(); } catch (InterruptedException e) { } } }
          long tot = System.nanoTime() - r.t0;
          synchronized (rows) { rows.add(label + (i < 0 ? "-warmup" : "") + "," + size + "," + (id * 1000 + i) + "," + (r.err == null ? tot : -1)
              + "," + r.batch + ",-1,-1,-1,-1,-1,-1,-1,-1," + (r.err == null ? "ok" : "ERR " + r.err.getClass().getSimpleName())); }
        }
      }});
      ts[a].start();
    }
    for (Thread t : ts) t.join();
    synchronized (q) { stop[0] = true; q.notifyAll(); }
    w.join();
    for (String r : rows) out.write(r + "\n");
    h.close();
  }

  // ---------- sequenced appends with optional kill point (failure injection) ----------
  void seq(int size, int n, int interval) throws Exception {
    H h = new H();
    try { h.create("seq-" + run + ".jsonl"); h.open(); if (isPre()) h.preallocate(); h.dirSync(); }
    catch (Throwable e) { Log.i(T, "REFUSE run=" + run + " " + e + " dispatches=" + dispatches); return; }
    killAt("after_create", 0);
    for (int i = 0; i < n; i++) {
      try {
        long[] tm = h.append(record(i, size));
        killAt("after_sync", i);
        lastAcked = i;
        Log.i(T, "ACK run=" + run + " seq=" + i + " sz=" + size);
        killAt("after_ack", i);
        dispatches++;
        Log.i(T, "DISPATCH run=" + run + " seq=" + i);
      } catch (Throwable e) {
        Log.i(T, "ERR run=" + run + " seq=" + i + " " + e.getClass().getSimpleName() + ": " + e.getMessage());
        break; // fail closed
      }
      if (interval > 0) Thread.sleep(interval);
    }
    Log.i(T, "SUMMARY run=" + run + " dest=" + dest + " strat=" + strat + " last-acked=" + lastAcked + " dispatches=" + dispatches);
    h.close();
  }
}
