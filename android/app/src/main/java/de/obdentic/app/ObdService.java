package de.obdentic.app;

import android.app.*;
import android.bluetooth.*;
import android.bluetooth.le.*;
import android.content.*;
import android.content.pm.PackageManager;
import android.os.*;
import org.json.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.text.SimpleDateFormat;
import java.util.*;
import java.util.concurrent.*;

/** Owns adapter lifetime, the serialized Rust session and private evidence files. */
public final class ObdService extends Service {
    public static volatile ObdService current;
    public interface Listener { void changed(); }
    private volatile Listener listener;
    private final Object stateLock = new Object(), recordLock = new Object();
    private final JSONObject state = new JSONObject();
    private final Handler main = new Handler(Looper.getMainLooper());
    private final ScheduledExecutorService worker = Executors.newSingleThreadScheduledExecutor();
    private final ExecutorService exporter = Executors.newSingleThreadExecutor(); // REQ-018.9: copying files never holds up the adapter thread
    private volatile int generation;
    private volatile Link transport;
    /** The live adapter link: BLE in the app, a fake in acceptance tests (REQ-020). */
    public interface Link extends NativeCore.Exchange { boolean isOpen(); void close(); }
    public interface Connector { Link open(String address) throws Exception; }
    /** Test seams (REQ-020): replace BLE as the live link and the Bluetooth/permission state. All null in the app. */
    public volatile Connector connector;
    public volatile Boolean permissionOverride, bluetoothOverride;
    private static final String PREFS = "adapter", KNOWN = "known_address", AUTO = "auto", AUTO_REC = "auto_recording";
    /** REQ-028: header profile of the automatic session recording, and the point where its file rotates. Fields are test seams. */
    static final String AUTO_PROFILE = "session-auto";
    public volatile long rotateBytes = 10L * 1024 * 1024, rotateMs = 30L * 60 * 1000;
    private long recordingBytes, recordingStartedAt;
    private ReconnectPolicy policy = new ReconnectPolicy(true);
    private volatile String sessionAddress = "", pendingProfile = "", retryNote = "";
    private volatile long nextAttemptAt;
    private final Runnable retryTick = this::retryTick;
    private final BroadcastReceiver bluetoothReceiver = new BroadcastReceiver() {
        public void onReceive(Context context, Intent intent) { if (intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, -1) == BluetoothAdapter.STATE_ON) resumeIfBlocked(); }
    };
    private volatile boolean busy;
    private volatile boolean knowledgeJob;
    /** REQ-030.4: jobs queued behind the running one (ECU identification, then the availability check). Test seam: runners switch the automatic survey off. */
    public static volatile boolean autoSurvey = true;
    private final java.util.Deque<String> chain = new java.util.concurrent.ConcurrentLinkedDeque<>();
    private volatile boolean udsJob;
    private long core;
    private BluetoothLeScanner scanner;
    private ScanCallback scanCallback;
    private volatile String mode = "live";
    private volatile String replayFilename = "";
    private BufferedWriter writer;
    private boolean chained; // worker thread only: a back-to-back poll task is already queued (#231)
    private long lastWakeRefresh, recordingOriginNanos;
    private int scanGeneration;
    private android.os.PowerManager.WakeLock wakeLock;
    private volatile String recordingProfile = "";
    private final Map<String, Long> due = new HashMap<>(), intervals = new HashMap<>();
    private final Map<String, JSONObject> values = new LinkedHashMap<>();
    public volatile VehicleStore vehicles;
    public void onCreate() {
        super.onCreate(); current = this; AppLog.init(this); AppLog.i("service","create");
        policy = new ReconnectPolicy(autoConnectEnabled(this));
        IntentFilter bluetoothState = new IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED);
        if (Build.VERSION.SDK_INT >= 33) registerReceiver(bluetoothReceiver, bluetoothState, Context.RECEIVER_NOT_EXPORTED); else registerReceiver(bluetoothReceiver, bluetoothState);
        vehicles = new VehicleStore(new File(getFilesDir(), "vehicles.json"));
        synchronized (stateLock) {
            put(state,"phase","idle"); put(state,"mode","live"); put(state,"busy",false);
            put(state,"message","Carly verbinden oder Demo starten");
            for (String key : new String[]{"devices","signals","profiles","values","debug","selected","records"}) put(state,key,new JSONArray());
            put(state,"recording",""); put(state,"autoRecording",""); put(state,"vehicle",new JSONObject()); put(state,"diagnostics",new JSONObject());
        }
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel("connection", "OBD-Verbindung", NotificationManager.IMPORTANCE_LOW));
        worker.scheduleWithFixedDelay(() -> { if(!chained) poll(); }, 500, 500, TimeUnit.MILLISECONDS);
        worker.execute(() -> { try { long temporary = NativeCore.create((c,t) -> { throw new IOException("Not connected"); });
            try { applyCatalog(new JSONObject(NativeCore.execute(temporary,"catalog",""))); } finally { NativeCore.destroy(temporary); }
            restoreSelection(selectedVin()); // REQ-013.6/REQ-014.1: after the catalog, so unknown semantics are filtered
        } catch (Throwable e) { message("Rust-Kern: " + e.getMessage()); } });
        autoStart(); // REQ-020.2
    }
    public int onStartCommand(Intent intent, int flags, int startId) { return START_NOT_STICKY; }
    public IBinder onBind(Intent intent) { return null; }
    public void setListener(Listener value) { listener = value; }
    public void clearListener(Listener value) { if (listener == value) listener = null; }
    private void changed() { Listener value = listener; if(value != null) main.post(() -> { if(listener == value) value.changed(); }); }
    private static void put(JSONObject object, String key, Object value) { try { object.put(key,value); } catch (JSONException e) { throw new IllegalArgumentException(e); } }
    private void set(String key,Object value) { synchronized(stateLock) { put(state,key,value); } changed(); }
    private String phase() { synchronized(stateLock) { return state.optString("phase"); } }
    /** REQ-021: counts messages, so a button can tell that a (possibly identical) message for its action arrived. */
    private void message(String value) { message(value,true); }
    private void message(String value,boolean log) { if(log) AppLog.i("msg",value); synchronized(stateLock) { put(state,"messageSeq",state.optLong("messageSeq")+1); put(state,"message",value); } changed(); }
    /** REQ-021: pending buttons live here, not in the Activity, so they survive its recreation. */
    public final PendingActions pending = new PendingActions();
    public JSONObject snapshot() {
        synchronized(stateLock) { put(state,"busy",busy); put(state,"records",listRecords()); put(state,"vehicles",vehicles.snapshot());
            put(state,"autoExport",RecordingExporter.auto(this)); put(state,"autoRecord",autoRecordEnabled(this)); put(state,"autoConnect",policy.enabled()); put(state,"autoPending",policy.pending()); put(state,"knownAdapter",maskedAdapter());
            try { return new JSONObject(state.toString()); } catch(JSONException e) { return new JSONObject(); } }
    }
    private boolean permitted() {
        Boolean forced = permissionOverride; if (forced != null) return forced;
        return Build.VERSION.SDK_INT < 31 || (checkSelfPermission("android.permission.BLUETOOTH_SCAN") == PackageManager.PERMISSION_GRANTED
            && checkSelfPermission("android.permission.BLUETOOTH_CONNECT") == PackageManager.PERMISSION_GRANTED);
    }
    private BluetoothAdapter adapter() { BluetoothManager m = getSystemService(BluetoothManager.class); return m == null ? null : m.getAdapter(); }
    public void scan() {
        stopScan();
        if (!permitted()) { message("Bluetooth-Berechtigungen fehlen"); return; }
        BluetoothAdapter adapter = adapter();
        if (adapter == null || !adapter.isEnabled()) { message("Bitte Bluetooth einschalten"); return; }
        if (busy || phase().equals("ready")) { message("Vor der Suche Verbindung trennen"); return; }
        try {
            scanner = adapter.getBluetoothLeScanner(); if(scanner == null) { message("BLE-Suche nicht verfügbar"); return; }
            resetSessionData(); mode="live"; set("mode",mode);
            set("devices",new JSONArray()); set("phase","scanning"); message("Suche Carly-Adapter …");
            int scanToken = ++scanGeneration; scanCallback=scanCallback(scanToken);
            scanner.startScan(null,new ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build(),scanCallback);
            main.postDelayed(() -> { if(scanToken == scanGeneration && phase().equals("scanning")) { stopScan(); set("phase","idle"); message("Suche beendet"); } },12000);
        } catch(SecurityException | IllegalStateException e) { set("phase","error"); message(e.getMessage()); }
    }
    private ScanCallback scanCallback(int token) { return new ScanCallback() {
        public void onScanResult(int type, ScanResult result) {
            if(token!=scanGeneration || !phase().equals("scanning")) return;
            ScanRecord record = result.getScanRecord(); String name = record == null ? null : record.getDeviceName();
            boolean service = record != null && record.getServiceUuids() != null && record.getServiceUuids().contains(new ParcelUuid(BleTransport.SERVICE));
            if (!service && (name == null || !name.toLowerCase(Locale.ROOT).contains("carly"))) return;
            synchronized(stateLock) {
                JSONArray devices = state.optJSONArray("devices"); String address = result.getDevice().getAddress();
                JSONObject row = null; for(int i=0;i<devices.length();i++) if(address.equals(devices.optJSONObject(i).optString("address"))) row=devices.optJSONObject(i);
                if(row == null) { row = new JSONObject(); devices.put(row); }
                put(row,"address",address); put(row,"name",name == null ? "Carly FFE0" : name); put(row,"rssi",result.getRssi());
            } changed();
        }
        public void onScanFailed(int code) { if(token!=scanGeneration || !phase().equals("scanning")) return; set("phase","error"); message("BLE-Suche fehlgeschlagen: " + code); }
    }; }
    private void stopScan() { ++scanGeneration; if(scanner != null) { try { scanner.stopScan(scanCallback); } catch(SecurityException | IllegalStateException ignored) {} scanner=null; } }
    private boolean foreground() {
        try {
            Intent launch = getPackageManager().getLaunchIntentForPackage(getPackageName());
            PendingIntent pending = PendingIntent.getActivity(this,0,launch,PendingIntent.FLAG_IMMUTABLE|PendingIntent.FLAG_UPDATE_CURRENT);
            Notification notification = new Notification.Builder(this,"connection").setContentTitle(policy.pending() ? "OBDentic verbindet automatisch" : "OBDentic verbunden")
                .setContentText(policy.pending() ? "Wartet auf den bekannten Adapter" : "Diagnose und lokale Aufzeichnung laufen").setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
                .setContentIntent(pending).setOngoing(true).build();
            if(Build.VERSION.SDK_INT >= 29) startForeground(1,notification,android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE);
            else startForeground(1,notification);
            if(wakeLock == null) wakeLock = getSystemService(android.os.PowerManager.class).newWakeLock(android.os.PowerManager.PARTIAL_WAKE_LOCK, "obdentic:connection");
            wakeLock.setReferenceCounted(false); wakeLock.acquire(10*60*1000L); lastWakeRefresh=SystemClock.elapsedRealtime();
            return true;
        } catch(RuntimeException e) { message("Hintergrundverbindung: " + e.getMessage()); return false; }
    }
    public void connect(String address) {
        String blocked=connectBlocker(); if(blocked!=null) { message(blocked); return; }
        main.removeCallbacks(retryTick); policy.userConnect(); pendingProfile=""; // REQ-020.4/.8: a manual connection is the user's own
        connectNow(address);
    }
    /** The German reason why nothing may connect now (REQ-020.9), or null. */
    private String environmentBlocker() {
        if (!permitted()) return "Bluetooth-Berechtigungen fehlen";
        Boolean forced=bluetoothOverride; BluetoothAdapter adapter=adapter();
        return (forced != null ? forced : adapter != null && adapter.isEnabled()) ? null : "Bitte Bluetooth einschalten";
    }
    private String connectBlocker() {
        String blocked=environmentBlocker();
        return blocked == null && (busy || phase().equals("ready")) ? "Zuerst aktuelle Verbindung trennen" : blocked;
    }
    /** Starts the live connection (guards already checked); false when the foreground service could not start. */
    private boolean connectNow(String address) {
        AppLog.i("conn","attempt "+(connector!=null?"simulated ":"ble ")+address);
        stopScan(); resetSessionData(); sessionAddress=""; int token=++generation; busy=true; mode="live"; set("mode",mode); set("phase","connecting");
        if(!foreground()) { busy=false; set("phase","error"); return false; }
        worker.execute(() -> {
            try {
                linked=false; destroyCore(); Link next;
                if(connector != null) { next=connector.open(address); if(token != generation) { next.close(); return; } transport=next; }
                else {
                    BleTransport ble=new BleTransport(this::debug); transport=ble;
                    if(token != generation) { ble.close(); return; }
                    BluetoothAdapter adapter=adapter(); if(adapter == null) throw new IOException("Bluetooth nicht verfügbar");
                    ble.connect(this,adapter.getRemoteDevice(address)); next=ble; // direct GATT connect to the exact address, no scan needed
                }
                if(token != generation) return;
                linked=true; startAutoRecording(); // REQ-028.1: before the first adapter byte; REQ-028.12: only once the link is up
                core=NativeCore.create(next); initialize(token);
                if(token == generation && phase().equals("ready")) connected(address);
            } catch(Throwable e) { fail(token,e); } finally { if(token==generation) { busy=false; changed(); } }
        });
        return true;
    }
    /** REQ-020.1/.8: worker thread, right after initialize and inventory. Remembers the adapter; continues an interrupted recording as a new one. */
    private void connected(String address) {
        AppLog.i("conn","ready "+address);
        sessionAddress=address; getSharedPreferences(PREFS,MODE_PRIVATE).edit().putString(KNOWN,address).commit();
        policy.success(); String resume=pendingProfile; pendingProfile="";
        foreground();
        if(!resume.isEmpty()) startRecording(resume); // queued behind this task, so busy is clear by then
    }
    /** REQ-028.5: "Dauer-Aufzeichnung", on by default. */
    public static boolean autoRecordEnabled(Context context) { return context.getSharedPreferences(PREFS,MODE_PRIVATE).getBoolean(AUTO_REC,true); }
    public void setAutoRecord(boolean on) {
        getSharedPreferences(PREFS,MODE_PRIVATE).edit().putBoolean(AUTO_REC,on).commit();
        message(on ? "Dauer-Aufzeichnung an" : "Dauer-Aufzeichnung aus");
        if(!on && !worker.isShutdown()) worker.execute(() -> { if(recordingProfile.equals(AUTO_PROFILE)) closeRecording("stopped","",false); });
    }
    public static boolean autoConnectEnabled(Context context) { return context.getSharedPreferences(PREFS,MODE_PRIVATE).getBoolean(AUTO,true); }
    /** The last successfully connected adapter's address, or "". App-private; never exported or shown in full. */
    public String knownAdapter() { return getSharedPreferences(PREFS,MODE_PRIVATE).getString(KNOWN,""); }
    private String maskedAdapter() { String address=knownAdapter(); return address.length() <= 5 ? address : "…"+address.substring(address.length()-5); }
    /** REQ-020.6 */
    public void setAutoConnect(boolean on) {
        getSharedPreferences(PREFS,MODE_PRIVATE).edit().putBoolean(AUTO,on).commit();
        boolean pending=policy.pending(); policy.setEnabled(on);
        if(!on && pending) disconnect();
        message(on ? "Automatische Verbindung an" : "Automatische Verbindung aus");
        String p=phase();
        if(on && !pending && !busy && mode.equals("live") && (p.equals("idle") || p.equals("error") || p.equals("disconnected"))) autoStart();
    }
    /** REQ-020.4: the stop button; ends pending or running automatic attempts. */
    public void stopAutoConnect() { if(policy.pending()) { disconnect(); message("Automatische Verbindung gestoppt"); } }
    /** REQ-020.2: app start (and switching on). Connects the known adapter when allowed; otherwise says why. */
    public void autoStart() {
        main.post(() -> { main.removeCallbacks(retryTick); if(!knownAdapter().isEmpty() && mode.equals("live") && policy.startup()) attempt(); });
    }
    /** REQ-020.9: Bluetooth turned on, or the app came back to the foreground (permission may have been granted). */
    public void resumeIfBlocked() {
        main.post(() -> {
            if(policy.state() == ReconnectPolicy.State.BLOCKED && environmentBlocker() == null && policy.unblock()) attempt();
            else if(policy.wake()) { main.removeCallbacks(retryTick); attempt(); } // REQ-020.10
        });
    }
    /** Main thread; the policy is CONNECTING. Only ever connects the exact known address (REQ-020.5). */
    private void attempt() {
        String blocked=environmentBlocker();
        if(blocked != null) { policy.block(); message(blocked+" · Automatische Verbindung wartet"); return; }
        String known=knownAdapter(), p=phase();
        if(known.isEmpty() || !mode.equals("live") || p.equals("ready")) { policy.success(); return; } // demo, replay or already connected
        if(busy || p.equals("scanning") || p.equals("connecting")) { long delay=policy.failed(); if(delay >= 0) schedule(delay,"Adapter-Suche läuft"); return; }
        message("Automatische Verbindung zum bekannten Adapter …");
        if(!connectNow(known)) { long delay=policy.failed(); if(delay >= 0) schedule(delay,"Adapter nicht erreichbar"); }
    }
    private void schedule(long delayMs,String note) {
        AppLog.i("reconnect",note+" · next attempt in "+delayMs/1000+" s");
        nextAttemptAt=SystemClock.elapsedRealtime()+delayMs; retryNote=note; main.removeCallbacks(retryTick); main.post(retryTick);
    }
    /** REQ-020.3: once a second until the pause is over; shows when the next attempt follows. */
    private void retryTick() {
        long left=nextAttemptAt-SystemClock.elapsedRealtime();
        if(left > 0) {
            if(phase().equals("error")) message(retryNote+" · neuer Versuch in "+(left+999)/1000+" s"+(pendingProfile.isEmpty() ? "" : " · Aufnahme wird fortgesetzt"),false); // REQ-029.7: the countdown is not logged
            main.postDelayed(retryTick,Math.min(1000,left));
        } else if(policy.attemptDue()) attempt();
    }
    /** Test seam: a working link answering like the demo, but in live mode. */
    public Link simulatedLink() { return new Demo("live"); }
    public void setDemo() {
        disconnect(); resetSessionData(); int token=++generation; busy=true; mode="demo"; set("mode",mode); set("phase","connecting");
        if(permitted()) foreground();
        worker.execute(() -> { try { destroyCore(); core=NativeCore.create(new Demo("demo")); initialize(token); }
            catch(Throwable e) { fail(token,e); } finally { if(token==generation) { busy=false; changed(); } } });
    }
    private void resetSessionData() {
        replayFilename="";
        synchronized(stateLock) { values.clear(); put(state,"values",new JSONArray()); put(state,"debug",new JSONArray());
            put(state,"uds",new JSONObject()); put(state,"udsCandidates",0);
            put(state,"replayFile","");
            put(state,"vehicle",new JSONObject()); put(state,"diagnostics",new JSONObject()); put(state,"jobResult",new JSONObject()); put(state,"throughput",new JSONObject()); put(state,"ecuDiscovery",new JSONObject()); put(state,"knowledgeLearning",new JSONObject()); put(state,"availability",new JSONObject()); put(state,"replayPartial",false);
            JSONArray signals=state.optJSONArray("signals"); for(int i=0;i<signals.length();i++) signals.optJSONObject(i).remove("status");
        }
        synchronized(due) { due.clear(); }
    }
    private void initialize(int token) throws Exception {
        resetSessionData();
        JSONObject result=execute("initialize","");
        if(token!=generation) return;
        if(!result.optBoolean("ok")) throw new IOException(result.optString("error","Initialisierung fehlgeschlagen"));
        applyCatalog(execute("catalog",""));
        JSONObject support=result.optJSONObject("data"); if(support != null) { set("vehicle",support); applySupport(support); }
        JSONObject inventory=execute("inventory","");
        if(token!=generation) return;
        String outcome=observeVehicle(inventory.optJSONObject("data")), notice=vehicleNotice(outcome,inventory.optJSONObject("data"));
        boolean knowledge=inventory.optJSONObject("data")!=null && !inventory.optJSONObject("data").isNull("knowledge_manufacturer");
        // REQ-030.7: ECU identification on every connection (Knowledge signals live in the session); REQ-030.4: availability for a new vehicle only
        if(knowledge && autoSurvey && mode.equals("live")) { chain.clear(); if(outcome.equals("created")) chain.add(AVAILABILITY); worker.execute(() -> runJob("ecu.discover")); }
        readOdometer(inventory.optJSONObject("data"),outcome);
        if(inventory.optJSONObject("data")!=null) set("vehicle",inventory.optJSONObject("data"));
        set("phase","ready");
        boolean tracked=outcome.equals("created") || outcome.equals("updated") || outcome.equals("switched");
        String vin=tracked ? inventory.optJSONObject("data").optString("vin") : "";
        boolean stored=tracked && restoreSelection(vin); // REQ-014.1: stored selection wins over defaults
        // REQ-014.4: a record without a stored selection (also one created while another was selected) gets the defaults below
        if(tracked && !stored) set("selected",new JSONArray());
        JSONArray defaults=stored ? new JSONArray() : defaultSelection();
        if(defaults.length()>0) { selectSignals(defaults); if(tracked) storeSelection(); }
        message((mode.equals("demo") ? "Demo · synthetische Fahrzeugdaten" : "Carly bereit") + notice);
    }
    /** Existing default selection: the supported ones of rpm, speed, coolant and MAF, only while nothing is selected. Uses current-session status only. */
    private JSONArray defaultSelection() {
        JSONArray defaults=new JSONArray(); synchronized(stateLock) { JSONArray selected=state.optJSONArray("selected");
            if(selected.length()==0) defaults=defaultSelection(state.optJSONArray("signals"));
        } return defaults;
    }
    public static JSONArray defaultSelection(JSONArray signals) {
        JSONArray defaults=new JSONArray();
        for(String semantic:new String[]{"engine.rpm","vehicle.speed","engine.coolant_temperature","engine.maf"})
            for(int i=0;i<signals.length();i++) if(semantic.equals(signals.optJSONObject(i).optString("semantic")) && signals.optJSONObject(i).optString("status").equals("supported")) defaults.put(semantic);
        return defaults;
    }
    private String selectedVin() { return vehicles.snapshot().optString("selected"); }
    /** REQ-014.1: applies the record's stored selection (catalog-filtered, no adapter traffic); false when it has none. Live mode only (REQ-013.4). */
    private boolean restoreSelection(String vin) {
        JSONArray stored=mode.equals("live") && !vin.isEmpty() ? vehicles.selection(vin) : null;
        if(stored==null) return false;
        selectSignals(stored); return true;
    }
    /** REQ-014.1: stores the current selection for the selected record; a write failure only sets the message, the selection stays applied. */
    private void storeSelection() {
        String vin=selectedVin(); if(!mode.equals("live") || vin.isEmpty()) return;
        JSONArray selected; synchronized(stateLock) { selected=state.optJSONArray("selected"); }
        String error=vehicles.setSelection(vin,selected); if(error!=null) message(error);
    }
    /** Live sessions only: records the VIN-keyed vehicle and returns the store outcome (or ""). Never touches the adapter. */
    private String observeVehicle(JSONObject inventory) {
        return !mode.equals("live") || inventory==null ? "" : vehicles.observe(inventory,System.currentTimeMillis());
    }
    /** " · notice" (or "") for the status line. */
    private String vehicleNotice(String outcome,JSONObject inventory) {
        switch(outcome) {
            case "unreadable_vin": return " · Fahrzeug nicht gespeichert · FIN nicht lesbar";
            case "store_unreadable": return " · "+VehicleStore.UNREADABLE;
            case "write_failed": return " · "+VehicleStore.WRITE_FAILED;
            case "switched": return " · Anderes Fahrzeug erkannt: "+vehicles.name(inventory.optString("vin"));
            default: return "";
        }
    }
    /** Live sessions, stored vehicle, PID A6 advertised only (REQ-015): one Mode 01 A6 read; success or failure goes to the vehicle record. */
    private void readOdometer(JSONObject inventory,String outcome) throws Exception {
        if(!mode.equals("live") || inventory==null || !(outcome.equals("created") || outcome.equals("updated") || outcome.equals("switched")) || !"supported".equals(signalStatus("vehicle.odometer"))) return;
        String vin=inventory.optString("vin"); JSONObject result=execute("read","vehicle.odometer"); JSONObject data=result.optJSONObject("data");
        long now=System.currentTimeMillis();
        if(result.optBoolean("ok") && data!=null) vehicles.odometer(vin,data.optDouble("value"),now);
        else vehicles.odometerFailure(vin,result.optString("error","Unbekannter Fehler"),now);
    }
    private String signalStatus(String semantic) {
        synchronized(stateLock) { JSONArray signals=state.optJSONArray("signals");
            for(int i=0;i<signals.length();i++) if(semantic.equals(signals.optJSONObject(i).optString("semantic"))) return signals.optJSONObject(i).optString("status"); }
        return "";
    }
    public void selectVehicle(String vin) { vehicleAction(() -> { String error=vehicles.select(vin);
        boolean noRecording; synchronized(stateLock) { noRecording=state.optString("recording").isEmpty(); }
        if(error==null && noRecording) restoreSelection(vin); // REQ-014.1
        return error==null?"Fahrzeug gewählt: "+vehicles.name(vin):error; }); }
    public void renameVehicle(String vin,String name) { vehicleAction(() -> { String error=vehicles.rename(vin,name); return error==null?"Fahrzeug umbenannt: "+vehicles.name(vin):error; }); }
    public void deleteVehicle(String vin) { vehicleAction(() -> { String name=vehicles.name(vin), error=vehicles.delete(vin); return error==null?"Fahrzeug gelöscht: "+name:error; }); }
    public void resetVehicleStore() { vehicleAction(() -> { String error=vehicles.reset(); return error==null?"Fahrzeugspeicher zurückgesetzt":error; }); }
    private void vehicleAction(java.util.function.Supplier<String> action) { if(!worker.isShutdown()) worker.execute(() -> message(action.get())); }
    private void applySupport(JSONObject support) {
        JSONArray statuses = support.optJSONArray("signals"); if(statuses == null) return;
        synchronized(stateLock) { JSONArray catalog = state.optJSONArray("signals");
            for(int i=0;i<catalog.length();i++) { JSONObject signal=catalog.optJSONObject(i);
                for(int j=0;j<statuses.length();j++) { JSONObject status=statuses.optJSONObject(j);
                    if(signal.optString("semantic").equals(status.optString("semantic"))) put(signal,"status",status.optString("status"));
                }
            }
        }
    }
    private void releaseWakeLock() { if(wakeLock != null && wakeLock.isHeld()) wakeLock.release(); }
    private void destroyCore() { if(core!=0) { NativeCore.destroy(core); core=0; } set("uds",new JSONObject()); }
    public void disconnect() {
        AppLog.i("conn","disconnect requested (mode "+mode+")");
        policy.stop(); main.removeCallbacks(retryTick); pendingProfile=""; sessionAddress=""; // REQ-020.4
        ++generation; stopScan(); Link active=transport; transport=null;
        boolean leave;
        synchronized(stateLock) { JSONObject uds=state.optJSONObject("uds"); leave=!knowledgeJob && (udsJob || (uds!=null && uds.optBoolean("active"))); }
        if(!leave && active!=null) active.close();
        if(leave) worker.execute(() -> {
            try { if(core!=0) execute("uds.disable",""); } catch(Exception ignored) { /* a lost link stops tester present */ }
            finally { if(active!=null) active.close(); }
        });
        busy=false; worker.execute(() -> closeRecording("disconnected","Verbindung getrennt")); set("phase","disconnected"); message("Getrennt"); stopForeground(STOP_FOREGROUND_REMOVE); releaseWakeLock();
        worker.execute(this::destroyCore);
    }
    private void fail(int token,Throwable error) {
        if(token!=generation) return;
        String text=error.getMessage()==null ? error.toString():error.getMessage();
        boolean live=mode.equals("live"), established=!sessionAddress.isEmpty(); sessionAddress="";
        AppLog.e("conn",(established?"session lost: ":"attempt failed: ")+text,error);
        String resume=established && !recordingProfile.equals(AUTO_PROFILE) ? ReconnectPolicy.resumeProfile(recordingProfile) : ""; // REQ-020.8: before the recording closes
        Link active=transport; transport=null; if(active!=null) active.close(); destroyCore();
        set("phase","error"); message(text); closeRecording("error",text);
        long delay=!live ? -1 : established ? policy.lost() : policy.failed(); // REQ-020.3: a lost session, or a failed automatic attempt
        if(delay < 0) { pendingProfile=""; stopForeground(STOP_FOREGROUND_REMOVE); releaseWakeLock(); return; }
        if(established) pendingProfile=resume;
        String note=established ? "Verbindung verloren" : "Adapter nicht erreichbar", blocked=environmentBlocker();
        foreground(); // stays a foreground service while waiting, so the retry may start in the background
        if(blocked != null) { policy.block(); message(note+" · "+blocked+" · Automatische Verbindung wartet"); releaseWakeLock(); return; } // REQ-020.9
        schedule(delay,note);
    }
    /** REQ-029.2: every job (not the per-second reads) is logged with its duration. */
    private JSONObject execute(String action,String argument) throws Exception {
        if(action.equals("read")) return executeNow(action,argument);
        if(action.equals("uds.tick")) { // REQ-029.8: every ~2 s in an extended session; only a problem is worth a line
            JSONObject result=executeNow(action,argument); JSONObject uds=result.optJSONObject("uds"); String problem=result.optString("error","");
            if(problem.isEmpty() && uds!=null && !uds.isNull("error")) problem=uds.optString("error");
            if(!result.optBoolean("ok",true) || !problem.isEmpty()) AppLog.w("job","uds.tick problem: "+problem);
            return result;
        }
        long start=SystemClock.elapsedRealtime(); AppLog.i("job",action+" start");
        try { JSONObject result=executeNow(action,argument); AppLog.i("job",action+" end ok="+result.optBoolean("ok")+" "+(SystemClock.elapsedRealtime()-start)+" ms"); return result; }
        catch(Exception e) { AppLog.e("job",action+" failed after "+(SystemClock.elapsedRealtime()-start)+" ms",e); throw e; }
    }
    private JSONObject executeNow(String action,String argument) throws Exception {
        JSONObject result=new JSONObject(NativeCore.execute(core,action,argument));
        JSONObject uds=result.optJSONObject("uds");
        if(uds!=null) {
            boolean wasActive; synchronized(stateLock) { JSONObject old=state.optJSONObject("uds"); wasActive=old!=null && old.optBoolean("active"); }
            set("uds",uds);
            if(wasActive!=uds.optBoolean("active")) {
                applyCatalog(new JSONObject(NativeCore.execute(core,"catalog","")));
                JSONArray selected; synchronized(stateLock) { selected=state.optJSONArray("selected"); }
                selectSignals(selected);
            }
        }
        JSONObject event=new JSONObject(); put(event,"type","action"); put(event,"action",action); put(event,"argument",argument); put(event,"result",result); record(event);
        Link active = transport; if(mode.equals("live") && active != null && !active.isOpen()) throw new IOException("BLE-Verbindung beendet; bitte neu verbinden");
        return result;
    }
    /** #256: a Knowledge signal can share its semantic with a standard PID (or with another ECU's signal), so its id here is the core's key; the semantic stays as "meaning" for labels. */
    private static JSONObject identify(JSONObject signal) {
        if(signal!=null && signal.has("key") && !signal.has("meaning")) { put(signal,"meaning",signal.optString("semantic")); put(signal,"semantic",signal.optString("key")); }
        return signal;
    }
    private void applyCatalog(JSONObject result) {
        JSONObject data=result.optJSONObject("data"); if(data==null) return;
        JSONArray next=data.optJSONArray("signals") == null ? new JSONArray() : data.optJSONArray("signals");
        for(int i=0;i<next.length();i++) identify(next.optJSONObject(i));
        synchronized(stateLock) {
            JSONArray previous=state.optJSONArray("signals");
            for(int i=0;i<next.length();i++) {
                JSONObject signal=next.optJSONObject(i);
                if(signal.has("status")) continue;
                for(int j=0;j<previous.length();j++) {
                    JSONObject old=previous.optJSONObject(j);
                    if(signal.optString("semantic").equals(old.optString("semantic")) && old.has("status")) put(signal,"status",old.optString("status"));
                }
            }
            Set<String> available=new HashSet<>();
            for(int i=0;i<next.length();i++) available.add(next.optJSONObject(i).optString("semantic"));
            values.keySet().retainAll(available);
            put(state,"values",new JSONArray(values.values()));
        }
        set("signals",next);
        set("udsCandidates",data.optInt("uds_candidates"));
        set("profiles",data.optJSONArray("profiles") == null ? new JSONArray() : data.optJSONArray("profiles"));
        JSONArray selected; synchronized(stateLock) { selected=state.optJSONArray("selected"); }
        selectSignals(selected);
    }
    public void setSignals(JSONArray semantics) {
        synchronized(stateLock) { if(!state.optString("recording").isEmpty()) { message("Messwertauswahl während Aufnahme gesperrt"); return; } }
        selectSignals(semantics);
        storeSelection();
    }
    /** REQ-025.4: presets stay in the selected private vehicle record. */
    public void saveSignalPreset(String name, JSONArray semantics) {
        final String vin=selectedVin();
        final JSONArray draft;
        try { draft=new JSONArray(semantics.toString()); } catch(JSONException e) { message("Ungültige Messwertauswahl"); return; }
        vehicleAction(() -> {
            if(!mode.equals("live") || vin.isEmpty()) return "Zuerst ein Fahrzeug wählen";
            String error=vehicles.savePreset(vin,name,draft);
            return error==null ? "Vorlage gespeichert: "+name : error;
        });
    }
    public void applySignalPreset(String name) {
        vehicleAction(() -> {
            if(!mode.equals("live") || selectedVin().isEmpty()) return "Zuerst ein Fahrzeug wählen";
            synchronized(stateLock) { if(!state.optString("recording").isEmpty()) return "Messwertauswahl während Aufnahme gesperrt"; }
            JSONArray preset=vehicles.preset(selectedVin(),name);
            if(preset==null) return "Vorlage nicht gefunden";
            JSONArray available=new JSONArray();
            synchronized(stateLock) {
                JSONArray catalog=state.optJSONArray("signals");
                for(int i=0;i<preset.length();i++) {
                    String wanted=preset.optString(i);
                    for(int j=0;j<catalog.length();j++) {
                        JSONObject signal=catalog.optJSONObject(j);
                        if(wanted.equals(signal.optString("semantic")) && !signal.optString("status").equals("unsupported")) { available.put(wanted); break; }
                    }
                }
            }
            selectSignals(available); storeSelection();
            return "Vorlage angewendet · "+(preset.length()-available.length())+" nicht verfügbare Messwerte übersprungen";
        });
    }
    private void selectSignals(JSONArray semantics) {
        JSONArray checked=new JSONArray(); synchronized(stateLock) {
            JSONArray catalog=state.optJSONArray("signals");
            for(int i=0;i<semantics.length();i++) { String wanted=semantics.optString(i); boolean found=false;
                for(int j=0;j<catalog.length();j++) if(wanted.equals(catalog.optJSONObject(j).optString("semantic"))) found=true;
                if(found && checked.length()<32) checked.put(wanted);
            }
        } set("selected",checked);
    }
    public void runJob(String action) {
        if(busy || !phase().equals("ready")) { message("Keine freie Verbindung für diesen Auftrag"); return; }
        boolean knowledge=action.equals("ecu.discover") || action.equals("knowledge.learn") || action.equals(AVAILABILITY);
        boolean uds=action.equals("uds.enable") || action.equals("uds.disable");
        if(!action.equals("inventory") && !action.equals("dtc.scan") && !action.equals(THROUGHPUT) && !knowledge && !uds && !action.startsWith("read:")) { message("Unbekannter Diagnoseauftrag"); return; }
        if(action.equals(THROUGHPUT) || knowledge) { String blocked=throughputBlocker(); if(blocked!=null) { message(blocked); return; } }
        if(uds && (!mode.equals("live") || (action.equals("uds.enable") && !snapshot().optString("recording").isEmpty()))) { message("UDS nur mit echtem Adapter; vor dem Einschalten Aufnahme stoppen"); return; }
        final String diagnosticVin;
        synchronized(stateLock) { JSONObject vehicle=state.optJSONObject("vehicle"); diagnosticVin=vehicle==null?"":vehicle.optString("vin"); }
        busy=true; udsJob=uds; changed(); int token=generation;
        final boolean[] succeeded={false};
        worker.execute(() -> { try {
            if(token!=generation) return;
            String argument=action.startsWith("read:") ? action.substring(5) : "";
            if(knowledge) execute("uds.disable","");
            // REQ-028.4/REQ-030.5: Knowledge jobs write into the running automatic recording; the throughput test sequence keeps its own file
            boolean own=action.equals(THROUGHPUT) || (knowledge && !recordingProfile.equals(AUTO_PROFILE));
            if(own) closeRecording("stopped","",false); // REQ-028.4: the automatic recording pauses, with a marker, while the job writes its own
            if(own && !beginRecording(action.equals(THROUGHPUT)?THROUGHPUT_PROFILE:action.replace('.','-'),new JSONArray())) return;
            knowledgeJob=knowledge;
            JSONObject result=execute(argument.isEmpty()?action:"read",argument);
            if(knowledge && result.optBoolean("ok") && result.optJSONObject("data")!=null && !diagnosticVin.isEmpty()) {
                String error=vehicles.setDiagnostic(diagnosticVin,action.equals("ecu.discover")?"ecu_discovery":action.equals(AVAILABILITY)?"availability":"knowledge_learning",result.optJSONObject("data"));
                if(error!=null && token==generation) message(error);
            }
            if(token!=generation) return;
            set("jobResult",result);
            if(own || knowledge) {
                if(result.optBoolean("ok")) {
                    if(action.equals(THROUGHPUT)) set("throughput",result.opt("data"));
                    if(knowledge) {
                        String key=action.equals("ecu.discover")?"ecuDiscovery":action.equals(AVAILABILITY)?"availability":"knowledgeLearning";
                        set(key,result.opt("data"));
                        applyCatalog(execute("catalog",""));
                        if(action.equals("ecu.discover")) restoreSelection(diagnosticVin); // REQ-030.7: a stored Knowledge selection applies once its signals exist
                    }
                }
                JSONObject data=result.optJSONObject("data");
                String reason=!result.optBoolean("ok") || (data!=null && data.optBoolean("stopped")) ? "error"
                    : data!=null && data.optBoolean("truncated") ? "truncated" : "completed";
                if(own) closeRecording(reason,result.optString("error"));
            }
            if(action.equals("inventory")) { set("vehicle",result.opt("data")); if(result.optJSONObject("data") != null) applySupport(result.optJSONObject("data")); }
            if(action.equals("dtc.scan")) set("diagnostics",result.opt("data"));
            if(uds) applyCatalog(execute("catalog",""));
            if(!argument.isEmpty()) updateValue(argument,result);
            String notice=action.equals("inventory") ? vehicleNotice(observeVehicle(result.optJSONObject("data")),result.optJSONObject("data")) : "";
            message(result.optBoolean("ok") ? "Auftrag abgeschlossen: " + action + notice : result.optString("error"));
            succeeded[0]=result.optBoolean("ok");
            if(succeeded[0] && action.equals("inventory") && autoSurvey && mode.equals("live")) { chain.clear(); chain.add("ecu.discover"); chain.add(AVAILABILITY); } // REQ-030.4
        } catch(Throwable e) { fail(token,e); } finally { knowledgeJob=false; udsJob=false; if(token==generation) { busy=false; changed(); }
            String next=succeeded[0] && token==generation ? chain.poll() : null; if(next==null) chain.clear(); else if(!worker.isShutdown()) worker.execute(() -> runJob(next)); } });
    }
    /** REQ-022: action name and recording profile of the adapter throughput job (not a polling profile). */
    public static final String THROUGHPUT="adapter.throughput", THROUGHPUT_PROFILE="adapter-throughput", AVAILABILITY="knowledge.availability";
    /** REQ-022.1: the German reason why the throughput job cannot start now, or null. Live and ready only. */
    public static String throughputBlocker(JSONObject state) {
        String mode=state.optString("mode","live"), phase=state.optString("phase","idle");
        if(mode.equals("replay")) return "Nicht in der Wiedergabe einer Aufnahme";
        if(!phase.equals("ready")) return "Zuerst den Adapter verbinden";
        if(mode.equals("demo")) return "Nur mit echtem Adapter, nicht in der Demo";
        if(!state.optString("recording").isEmpty()) return "Zuerst die laufende Aufzeichnung stoppen";
        if(state.optBoolean("busy")) return "Ein anderer Auftrag läuft";
        return null;
    }
    private String throughputBlocker() { return throughputBlocker(snapshot()); }
    private void poll() {
        if(wakeLock!=null && SystemClock.elapsedRealtime()-lastWakeRefresh>5*60*1000L && (phase().equals("ready") || phase().equals("connecting") || policy.state()==ReconnectPolicy.State.WAITING)) {
            wakeLock.acquire(10*60*1000L); lastWakeRefresh=SystemClock.elapsedRealtime();
        }
        if(busy || !phase().equals("ready") || core==0) return;
        Link active=transport; if(mode.equals("live") && active!=null && !active.isOpen()) { fail(generation,new IOException("BLE-Verbindung beendet")); return; }
        JSONArray selected; synchronized(stateLock) { selected=state.optJSONArray("selected"); }
        int token=generation;
        try {
            JSONObject uds; synchronized(stateLock) { uds=state.optJSONObject("uds"); }
            if(uds!=null && uds.optBoolean("active")) execute("uds.tick","");
            if(selected.length()==0) return;
            long now=SystemClock.elapsedRealtime(); String semantic; long wait;
            synchronized(due) {
                java.util.List<String> eligible=new java.util.ArrayList<>();
                for(int i=0;i<selected.length();i++) { String candidate=selected.optString(i);
                    boolean unsupported=false; synchronized(stateLock) { JSONArray catalog=state.optJSONArray("signals");
                        for(int j=0;j<catalog.length();j++) { JSONObject signal=catalog.optJSONObject(j);
                            if(candidate.equals(signal.optString("semantic")) && (signal.optString("status").equals("unsupported")
                                || (uds!=null && uds.optBoolean("active") && !signal.has("definition_id")))) unsupported=true;
                        }
                    }
                    if(!unsupported) eligible.add(candidate);
                }
                semantic=PollSchedule.next(eligible,due,now); wait=PollSchedule.waitMs(eligible,due,now);
            }
            if(semantic!=null) {
                JSONObject result=execute("read",semantic); if(token==generation) updateValue(semantic,result);
                synchronized(due) { due.put(semantic,SystemClock.elapsedRealtime()+PollSchedule.period(intervals.get(semantic))); wait=0; }
            }
            // Next read right behind this one (other queued work runs in between); the 500 ms tick only restarts an idle chain.
            if(wait>=0 && !worker.isShutdown()) { chained=true; worker.schedule(() -> { chained=false; poll(); },Math.min(wait,500),TimeUnit.MILLISECONDS); }
        } catch(Throwable e) { fail(token,e); }
        finally { if(token==generation) changed(); }
    }
    private void updateValue(String semantic,JSONObject result) {
        JSONObject row=new JSONObject(); put(row,"semantic",semantic); put(row,"updatedAt",System.currentTimeMillis());
        Object data=result.opt("data"); if(data instanceof JSONObject) { JSONObject object=(JSONObject)data;
            Iterator<String> keys=object.keys(); while(keys.hasNext()) { String key=keys.next(); put(row,key,object.opt(key)); }
        } else put(row,"value",data);
        if(row.isNull("error")) row.remove("error");
        if(row.isNull("value")) row.remove("value");
        if(row.optString("status").equals("unavailable") && !row.has("error")) put(row,"error","Messwert nicht verfügbar");
        if(!result.optBoolean("ok")) put(row,"error",result.optString("error"));
        synchronized(stateLock) {
            JSONArray catalog=state.optJSONArray("signals"); for(int i=0;i<catalog.length();i++) { JSONObject s=catalog.optJSONObject(i);
                if(semantic.equals(s.optString("semantic"))) { put(row,"title",s.optString("title",semantic)); if(!row.has("unit")) put(row,"unit",s.optString("unit")); if(s.has("meaning")) put(row,"meaning",s.optString("meaning")); } }
            put(row,"semantic",semantic); // the result carries the plain semantic; the row is keyed by the signal id
            JSONArray history = values.containsKey(semantic) ? values.get(semantic).optJSONArray("history") : null;
            if (history == null) history = new JSONArray();
            if (row.has("value") && !row.has("error") && Double.isFinite(row.optDouble("value",Double.NaN))) {
                JSONObject sample = new JSONObject(); put(sample,"at",row.optLong("timestamp_ms",row.optLong("updatedAt"))); put(sample,"value",row.optDouble("value"));
                history.put(sample); if(history.length()>300) history.remove(0);
            }
            put(row,"history",history);
            values.put(semantic,row); put(state,"values",new JSONArray(values.values()));
        } changed();
    }
    public void startRecording(String profileId) { worker.execute(() -> startRecordingNow(profileId)); }
    private void startRecordingNow(String profileId) {
        if(!phase().equals("ready")) { message("Zum Aufzeichnen zuerst verbinden"); return; }
        if(busy) { message("Zuerst laufenden Auftrag beenden"); return; }
        JSONArray signals=null, periods=null; synchronized(stateLock) { JSONArray profiles=state.optJSONArray("profiles");
            for(int i=0;i<profiles.length();i++) if(profileId.equals(profiles.optJSONObject(i).optString("id"))) { signals=profiles.optJSONObject(i).optJSONArray("signals"); periods=profiles.optJSONObject(i).optJSONArray("intervals_ms"); } }
        if(profileId.equals("live-selection")) synchronized(stateLock) {
            try { signals=new JSONArray(state.optJSONArray("selected").toString()); }
            catch(JSONException e) { message("Ungültige Messwertauswahl"); return; }
            if(signals.length()==0) { message("Zuerst Messwerte wählen"); return; }
        }
        if(signals==null) { message("Unbekanntes Aufnahmeprofil"); return; }
        closeRecording("stopped","",false); // REQ-028.4: the automatic recording resumes only after the named one ends
        if(!beginRecording(profileId,signals)) return;
        synchronized(due) { due.clear(); intervals.clear(); for(int i=0;i<signals.length();i++) intervals.put(signals.optString(i),periods==null?PollSchedule.DEFAULT_PERIOD_MS:periods.optLong(i,PollSchedule.DEFAULT_PERIOD_MS)); }
        selectSignals(signals); message("Aufnahme läuft: " + profileId);
    }
    /** Opens a new recording file with its header and makes it the active recording; false (after a message) when that failed. */
    private boolean beginRecording(String profileId,JSONArray signals) {
        try {
            File directory=new File(getFilesDir(),"recordings"); if(!directory.exists() && !directory.mkdirs()) throw new IOException("Aufnahmeordner nicht verfügbar");
            boolean auto=profileId.equals(AUTO_PROFILE);
            String name=new SimpleDateFormat("yyyyMMdd-HHmmss-SSS",Locale.ROOT).format(new Date())+"-"+mode+(auto?"-auto":"")+".jsonl";
            synchronized(recordLock) {
                recordingOriginNanos=SystemClock.elapsedRealtimeNanos();
                writer=new BufferedWriter(new OutputStreamWriter(new FileOutputStream(new File(directory,name)),StandardCharsets.UTF_8));
                recordingBytes=0; recordingStartedAt=SystemClock.elapsedRealtime();
                set(auto?"autoRecording":"recording",name);
                JSONObject header=new JSONObject(); put(header,"type","session"); put(header,"schema","OBDENTIC-MOBILE"); put(header,"version",1); put(header,"mode",mode); put(header,"profile",profileId); put(header,"selected",signals);
                synchronized(stateLock) { put(header,"vehicle",state.opt("vehicle")); put(header,"diagnostics",state.opt("diagnostics")); put(header,"catalog",state.opt("signals")); put(header,"profiles",state.opt("profiles")); }
                // Notifications must not precede the header in the new evidence stream.
                record(header);
                if(writer==null) return false;
            }
            recordingProfile=profileId;
            AppLog.i("rec","start "+name+" profile "+profileId);
            return true;
        } catch(IOException e) { AppLog.e("rec","start failed",e); message(e.getMessage()); return false; }
    }
    public void stopRecording() {
        // A bounded native job owns the worker until it returns; closing its link cancels the pending exchange.
        if(knowledgeJob) { disconnect(); return; }
        if(!worker.isShutdown()) worker.execute(this::stopRecordingNow);
    }
    private void stopRecordingNow() { closeRecording("stopped",""); }
    private void closeRecording(String reason,String detail) { closeRecording(reason,detail,true); }
    private void closeRecording(String reason,String detail,boolean resumeAuto) {
        boolean wasAuto=recordingProfile.equals(AUTO_PROFILE);
        recordingProfile=""; synchronized(due) { due.clear(); intervals.clear(); }
        boolean completed=false; String closed; synchronized(stateLock) { closed=activeRecording(); }
        synchronized(recordLock) {
            JSONObject end=new JSONObject(); put(end,"type","session_end"); put(end,"reason",reason); if(!detail.isEmpty()) put(end,"detail",detail); record(end);
            if(writer!=null) { try { writer.close(); completed=true; } catch(IOException e) { message("Aufnahme konnte nicht abgeschlossen werden: "+e.getMessage()); } writer=null; }
        }
        set("recording",""); set("autoRecording","");
        AppLog.i("rec","end "+closed+" reason "+reason+(detail.isEmpty()?"":" ("+detail+")"));
        if(reason.equals("disconnected") || (reason.equals("error") && linked)) AppLog.seal(); // REQ-029.1: one sealed log per connection; REQ-028.12: none per unreachable attempt
        if(completed && !wasAuto) message(reason.equals("stopped")?"Aufzeichnung gespeichert":reason.equals("completed")?"Messung gespeichert":reason.equals("error")?"Aufzeichnung mit Fehler beendet: "+detail:"Aufzeichnung nach Trennen gespeichert");
        if(completed && !closed.isEmpty() && RecordingExporter.auto(this) && (exportTarget!=null || !RecordingExporter.folder(this).isEmpty())) exportLater(closed); // REQ-018.4: any ending reason
        if(resumeAuto && completed && !wasAuto && (reason.equals("stopped") || reason.equals("completed")) && mode.equals("live") && phase().equals("ready")) startAutoRecording(); // REQ-028.4
    }
    /** The file being written, named or automatic, or "". Caller holds stateLock or does not need atomicity. */
    private String activeRecording() { String named=state.optString("recording"); return named.isEmpty() ? state.optString("autoRecording") : named; }
    /** REQ-028.1: worker thread. Live mode only; a failed start is reported and never stops the session. */
    private void startAutoRecording() {
        if(!autoRecordEnabled(this) || !mode.equals("live")) return;
        synchronized(recordLock) { if(writer!=null) return; }
        JSONArray selected; synchronized(stateLock) { selected=state.optJSONArray("selected"); }
        beginRecording(AUTO_PROFILE,selected==null?new JSONArray():selected);
    }
    /** Test seam (REQ-018): when set, replaces the stored folder as export destination. */
    public volatile RecordingExporter.Target exportTarget;
    private volatile boolean linked; // REQ-028.12: the current attempt reached the adapter link
    public void exportAll() { exportLater(null); }
    private void exportLater(String only) { if(!exporter.isShutdown()) exporter.execute(() -> exportNow(only)); }
    /** REQ-018.2/.3/.6: worker thread only. Copies completed recordings (all, or just {@code only}); the recording being written is never a candidate. */
    private void exportNow(String only) {
        RecordingExporter.Target target=exportTarget; String stored=RecordingExporter.folder(this);
        if(target==null) {
            if(stored.isEmpty()) { if(only==null) message("Kein Export-Ordner gewählt"); return; }
            try { target=RecordingExporter.tree(this,stored); } catch(RecordingExporter.Unavailable e) { exportUnavailable(); return; }
        }
        List<File> files=new ArrayList<>();
        synchronized(recordLock) { String active; synchronized(stateLock) { active=activeRecording(); }
            for(String name:RecordingExporter.completed(listRecords(),active)) if(only==null || name.equals(only)) try { files.add(recordingFile(name)); } catch(IOException ignored) { /* not an app-written name */ } }
        RecordingExporter.Result result=RecordingExporter.export(files,target);
        if(!result.unavailable) { RecordingExporter.Result logs=RecordingExporter.export(AppLog.sealed(),target); AppLog.i("export","recordings: "+result.message()+" · logs: "+logs.message()); } // REQ-029.6: sealed logs ride along, reported in the log only
        if(result.unavailable) exportUnavailable(); else message(result.message());
    }
    private void exportUnavailable() { if(exportTarget==null) RecordingExporter.setFolder(this,""); message(RecordingExporter.UNAVAILABLE); }
    private void record(JSONObject event) {
        put(event,"at",System.currentTimeMillis()); synchronized(recordLock) { if(writer==null) return; put(event,"offset_us",Math.max(0,(SystemClock.elapsedRealtimeNanos()-recordingOriginNanos)/1000));
            try { String line=event.toString(); writer.write(line); writer.newLine(); writer.flush(); recordingBytes+=line.length()+1; rotateIfDue(); }
            catch(IOException e) { AppLog.e("rec","write failed",e); try { writer.close(); } catch(IOException ignored) {} writer=null; recordingProfile=""; synchronized(due) { due.clear(); intervals.clear(); } String failed; synchronized(stateLock) { failed=activeRecording(); } set("recording",""); set("autoRecording",""); message("Aufnahmefehler: "+e.getMessage());
                // REQ-018.4: a recording ended by a write error is auto-exported too (after the lock is released).
                if(!failed.isEmpty() && RecordingExporter.auto(this) && (exportTarget!=null || !RecordingExporter.folder(this).isEmpty()) ) exportLater(failed); }
        }
    }
    /** REQ-028.3: caller holds recordLock. Closes the automatic file as "rotated" and continues in a new one that starts with a fresh header. */
    private void rotateIfDue() throws IOException {
        if(!recordingProfile.equals(AUTO_PROFILE) || (recordingBytes<rotateBytes && SystemClock.elapsedRealtime()-recordingStartedAt<rotateMs)) return;
        String closed=rotate();
        if(!closed.isEmpty() && RecordingExporter.auto(this) && (exportTarget!=null || !RecordingExporter.folder(this).isEmpty()) ) exportLater(closed); // REQ-018.4: any ending reason
    }
    /** Caller holds recordLock and an automatic file is open. Ends it as "rotated" and continues in a new one; returns the closed name. */
    private String rotate() throws IOException {
        String closed; synchronized(stateLock) { closed=state.optString("autoRecording"); }
        JSONObject end=new JSONObject(); put(end,"type","session_end"); put(end,"reason","rotated"); put(end,"at",System.currentTimeMillis());
        put(end,"offset_us",Math.max(0,(SystemClock.elapsedRealtimeNanos()-recordingOriginNanos)/1000));
        writer.write(end.toString()); writer.newLine(); writer.flush(); writer.close(); writer=null; AppLog.i("rec","rotated "+closed);
        JSONArray selected; synchronized(stateLock) { selected=state.optJSONArray("selected"); }
        if(!beginRecording(AUTO_PROFILE,selected==null?new JSONArray():selected)) { recordingProfile=""; set("autoRecording",""); }
        return closed;
    }
    /** REQ-028.11: "Jetzt exportieren" - ends the running automatic file, seals the app log and exports everything completed; recording goes on. */
    public void exportCurrent() { if(!worker.isShutdown()) worker.execute(() -> {
        if(exportTarget==null && RecordingExporter.folder(this).isEmpty()) { message("Kein Export-Ordner gewählt"); return; }
        synchronized(recordLock) { if(writer!=null && recordingProfile.equals(AUTO_PROFILE)) try { rotate(); } catch(IOException e) { AppLog.e("rec","rotate failed",e); } }
        AppLog.seal(); exportLater(null);
    }); }
    /** REQ-028.7: removes completed automatic recordings; the running file, named recordings and a loaded replay stay. */
    public void deleteAutoRecordings() { if(!worker.isShutdown()) worker.execute(() -> {
        int removed=0; long freed=0;
        synchronized(recordLock) { String active; synchronized(stateLock) { active=activeRecording(); }
            File[] files=new File(getFilesDir(),"recordings").listFiles();
            if(files!=null) for(File file:files) { String name=file.getName();
                if(!file.isFile() || !name.endsWith("-auto.jsonl") || name.equals(active) || name.equals(replayFilename)) continue;
                long size=file.length(); if(file.delete()) { removed++; freed+=size; } } }
        message(removed+" automatische Aufnahmen gelöscht · "+freed/1024+" KB frei");
    }); }
    private void debug(String direction,String text) {
        JSONObject event=new JSONObject(); put(event,"type","wire"); put(event,"direction",direction); put(event,"text",text); put(event,"at",System.currentTimeMillis()); record(event);
        synchronized(stateLock) { JSONArray rows=state.optJSONArray("debug"); rows.put(event); if(rows.length()>300) rows.remove(0); } changed();
    }
    public JSONArray listRecords() {
        JSONArray records=new JSONArray(); File[] files=new File(getFilesDir(),"recordings").listFiles(); if(files==null) return records;
        Arrays.sort(files,(a,b) -> Long.compare(b.lastModified(),a.lastModified()));
        for(File file:files) if(file.isFile() && file.getName().endsWith(".jsonl")) { JSONObject row=new JSONObject(); put(row,"name",file.getName()); put(row,"bytes",file.length()); put(row,"modified",file.lastModified()); records.put(row); }
        return records;
    }
    public void deleteRecording(String filename) {
        if(!worker.isShutdown()) worker.execute(() -> deleteRecordingNow(filename));
    }
    private File recordingFile(String filename) throws IOException {
        if(filename==null || !filename.matches("[A-Za-z0-9_.-]+\\.jsonl")) throw new IOException("Ungültiger Aufnahmepfad");
        File filesDirectory=getFilesDir().getCanonicalFile();
        File directory=new File(filesDirectory,"recordings").getCanonicalFile();
        if(!filesDirectory.equals(directory.getParentFile())) throw new IOException("Ungültiger Aufnahmepfad");
        File file=new File(directory,filename).getCanonicalFile();
        if(!directory.equals(file.getParentFile()) || !filename.equals(file.getName())) throw new IOException("Ungültiger Aufnahmepfad");
        return file;
    }
    private void deleteRecordingNow(String filename) {
        File file;
        try { file=recordingFile(filename); }
        catch(IOException e) { message(e.getMessage()); return; }
        synchronized(recordLock) {
            String active;
            synchronized(stateLock) { active=activeRecording(); }
            if(filename.equals(active) && writer!=null) { message("Aufzeichnung vor dem Löschen stoppen"); return; }
            if(filename.equals(replayFilename)) { message("Im Replay geöffnete Aufzeichnung kann nicht gelöscht werden"); return; }
            if(!file.isFile()) { message("Aufnahme nicht gefunden"); return; }
            if(!file.delete()) { message("Aufnahme konnte nicht gelöscht werden"); return; }
        }
        message("Aufnahme endgültig gelöscht: " + filename);
    }
    public void openRecording(String filename) {
        if(filename==null || !filename.matches("[A-Za-z0-9_.-]+\\.jsonl")) { message("Ungültiger Aufnahmepfad"); return; }
        disconnect(); replayFilename=filename; set("replayFile",filename); int token=++generation; busy=true; mode="replay"; set("mode",mode); set("phase","replay");
        worker.execute(() -> { boolean headerAccepted=false; try {
            if(token!=generation)return;
            synchronized(stateLock) { values.clear(); } resetSessionData(); replayFilename=filename; set("replayFile",filename);
            destroyCore(); core=NativeCore.create((command,timeout) -> { throw new IOException("Offline replay cannot access an adapter"); });
            File file=recordingFile(filename);
            if(file.length()>64L*1024*1024) throw new IOException("Aufnahme größer als 64 MiB");
            try(BufferedReader input=new BufferedReader(new InputStreamReader(new FileInputStream(file),StandardCharsets.UTF_8))) {
                String line; int count=0; boolean headerFound=false, endFound=false; String outcome="", outcomeDetail=""; while((line=input.readLine())!=null && token==generation) {
                    if(endFound) throw new IOException("Daten nach dem Abschlussmarker");
                    if(line.length()>1048576 || ++count>200000) throw new IOException("Aufnahme überschreitet Leselimit");
                    JSONObject event=new JSONObject(line);
                    if(!headerFound) {
                        if(!event.optString("type").equals("session") || !event.optString("schema").equals("OBDENTIC-MOBILE") || event.optInt("version")!=1) throw new IOException("Unbekanntes Aufnahmeformat oder Version");
                        headerFound=true;
                        headerAccepted=true;
                    }
                    if(event.optString("type").equals("session_end")) { endFound=true; outcome=event.optString("reason"); outcomeDetail=event.optString("detail"); }
                    if(event.optString("type").equals("action")) { String action=event.optString("action"); JSONObject result=event.optJSONObject("result"); if(result==null) continue;
                        if(action.equals("read")) { JSONObject recorded=result.optJSONObject("data");
                            JSONObject decoded=recorded==null?result:new JSONObject(NativeCore.execute(core,"replay.read",recorded.toString()));
                            updateValue(event.optString("argument"),decoded); result=decoded;
                        }
                        else if(action.equals("catalog")) { applyCatalog(result); continue; } // REQ-028.10: automatic files start before the handshake
                        else if(action.equals("initialize")) { JSONObject support=result.optJSONObject("data"); if(support!=null) { restoreVehicle(support); applySupport(support); } continue; }
                        else if(action.equals("inventory")) { restoreVehicle(result.optJSONObject("data")); if(result.optJSONObject("data")!=null) applySupport(result.optJSONObject("data")); }
                        else if(action.equals(AVAILABILITY)) { set("availability",result.opt("data")); } // REQ-030.5: statuses are the job's own summary; raw TX/RX stay in the file
                        else if(action.equals(THROUGHPUT) || action.equals("ecu.discover") || action.equals("knowledge.learn")) {
                            JSONObject recorded=result.optJSONObject("data");
                            JSONObject decoded=recorded==null?result:new JSONObject(NativeCore.execute(core,"replay."+action,recorded.toString()));
                            if(decoded.optBoolean("ok")) set(action.equals(THROUGHPUT)?"throughput":action.equals("ecu.discover")?"ecuDiscovery":"knowledgeLearning",decoded.opt("data"));
                            if(action.equals("ecu.discover") && decoded.optBoolean("ok")) {
                                JSONArray extra=decoded.optJSONObject("data").optJSONArray("signals");
                                if(extra!=null) synchronized(stateLock) {
                                    JSONArray catalog=state.optJSONArray("signals");
                                    for(int i=0;i<extra.length();i++) { JSONObject signal=identify(extra.optJSONObject(i)); boolean found=false;
                                        for(int j=0;j<catalog.length();j++) if(signal.optString("semantic").equals(catalog.optJSONObject(j).optString("semantic"))) found=true;
                                        if(!found) catalog.put(signal);
                                    }
                                }
                            }
                            result=decoded;
                        }
                        else if(action.equals("dtc.scan")) {
                            JSONObject recorded=result.optJSONObject("data");
                            JSONObject decoded=recorded==null?result:new JSONObject(NativeCore.execute(core,"replay."+action,recorded.toString()));
                            set("diagnostics",decoded.opt("data"));
                            result=decoded;
                        }
                        set("jobResult",result);
                    } else if(event.optString("type").equals("session")) {
                        if(event.optJSONArray("catalog")!=null) set("signals",event.optJSONArray("catalog"));
                        if(event.optJSONArray("profiles")!=null) set("profiles",event.optJSONArray("profiles"));
                        selectSignals(event.optJSONArray("selected")==null?new JSONArray():event.optJSONArray("selected"));
                        if(event.optJSONObject("vehicle")!=null) restoreVehicle(event.optJSONObject("vehicle"));
                        if(event.optJSONObject("diagnostics")!=null && event.optJSONObject("diagnostics").has("raw_response")) { JSONObject decoded=new JSONObject(NativeCore.execute(core,"replay.dtc.scan",event.optJSONObject("diagnostics").toString())); set("diagnostics",decoded.opt("data")); }
                        set("replaySourceMode",event.optString("mode","unknown"));
                    } else if(event.optString("type").equals("wire")) { synchronized(stateLock) { JSONArray rows=state.optJSONArray("debug"); rows.put(event); if(rows.length()>300) rows.remove(0); } }
                }
                if(!headerFound) throw new IOException("Leere Aufnahme");
                if(token==generation) { set("replayPartial",!endFound); set("replayOutcome",outcome); message(!endFound?"Teilaufnahme ohne Abschlussmarker: "+filename:outcome.equals("error")?"Aufnahme mit Fehler beendet: "+outcomeDetail:"Offline geöffnet ("+outcome+"): "+filename); }
            }
        } catch(Exception e) { if(token==generation) { if(!headerAccepted){replayFilename="";set("replayFile","");} set("replayPartial",true); message("Teilweise oder ungültige Aufnahme: " + e.getMessage()); } }
        finally { if(token==generation) { busy=false; changed(); } } });
    }
    private void restoreVehicle(JSONObject vehicle) throws Exception {
        if(vehicle==null)return;
        String vin=vehicle.optString("vin","");
        if(!vin.isEmpty()) {
            JSONObject identity=new JSONObject(NativeCore.execute(core,"vin.describe",vin));
            if(identity.optBoolean("ok"))put(vehicle,"vin_identity",identity.optJSONObject("data"));
        }
        set("vehicle",vehicle);
    }
    public void onDestroy() { AppLog.i("service","destroy"); disconnect(); try { unregisterReceiver(bluetoothReceiver); } catch(IllegalArgumentException ignored) {} worker.execute(this::destroyCore); worker.shutdown(); exporter.shutdown(); listener=null; current=null; super.onDestroy(); }
    private final class Demo implements Link {
        private final String only; private volatile boolean open=true;
        Demo(String only) { this.only=only; }
        public boolean isOpen() { return open; }
        public void close() { open=false; }
        public String exchange(String command,long timeoutMs) throws Exception {
            if(!mode.equals(only) || !open) throw new IOException("Demo stopped");
            debug("TX",command); String response=respond(command.trim().replace(" ","").toUpperCase(Locale.ROOT));
            debug("RX",response); return response;
        }
        private String respond(String c) {
            String response;
            if(c.equals("ATI") || c.equals("ATZ")) response="ELM327 v1.4 DEMO\r>";
            else if(c.equals("AT@1")) response="CARLY-UNIVERSAL DEMO\r>";
            else if(c.equals("ATRV")) response="12.6V\r>";
            else if(c.startsWith("AT")) response="OK\r>";
            else if(c.equals("0100")) response="7E8 06 41 00 FF FF FF FF\r>";
            else if(c.equals("0120")) response="7E8 06 41 20 FF FF FF FF\r>";
            else if(c.equals("0140")) response="7E8 06 41 40 FF FF FF FF\r>";
            else if(c.equals("0160")) response="7E8 06 41 60 E0 00 00 01\r>";
            else if(c.equals("0180")) response="7E8 06 41 80 00 00 00 01\r>";
            else if(c.equals("01A0")) response="7E8 06 41 A0 04 00 00 00\r>";
            else if(c.equals("01A6")) response="7E8 06 41 A6 00 12 D6 87\r>";
            else if(c.equals("0902")) response="7E8 10 14 49 02 01 57 56 57\r7E8 21 30 30 30 30 30 30 30\r7E8 22 30 30 30 30 30 30 30\r>";
            else if(c.equals("011F")) response="7E8 04 41 1F 04 B0\r>";
            else if(c.equals("0121")) response="7E8 04 41 21 00 64\r>";
            else if(c.equals("0123")) response="7E8 04 41 23 0B B8\r>";
            else if(c.equals("012C")) response="7E8 03 41 2C 40\r>";
            else if(c.equals("012D")) response="7E8 03 41 2D 80\r>";
            else if(c.equals("0130")) response="7E8 03 41 30 05\r>";
            else if(c.equals("0131")) response="7E8 04 41 31 01 00\r>";
            else if(c.equals("0133")) response="7E8 03 41 33 64\r>";
            else if(c.equals("0145")) response="7E8 03 41 45 20\r>";
            else if(c.equals("0146")) response="7E8 03 41 46 3C\r>";
            else if(c.equals("014A")) response="7E8 03 41 4A 20\r>";
            else if(c.equals("014C")) response="7E8 03 41 4C 20\r>";
            else if(c.equals("010C")) response="7E8 04 41 0C 0C 80\r>";
            else if(c.equals("010D")) response="7E8 03 41 0D 00\r>";
            else if(c.equals("0105")) response="7E8 03 41 05 7B\r>";
            else if(c.equals("010F")) response="7E8 03 41 0F 45\r>";
            else if(c.equals("0110")) response="7E8 04 41 10 03 E8\r>";
            else if(c.equals("0104")) response="7E8 03 41 04 20\r>";
            else if(c.equals("010B")) response="7E8 03 41 0B 64\r>";
            else if(c.equals("0111")) response="7E8 03 41 11 10\r>";
            else if(c.equals("0142")) response="7E8 04 41 42 31 38\r>";
            else if(c.equals("0101")) response="7E8 06 41 01 81 07 65 00\r>";
            else if(c.equals("03")) response="7E8 07 43 01 33 00 00 00 00\r>";
            else if(c.equals("07")) response="7E8 03 47 00 00\r>";
            else if(c.equals("0A")) response="7E8 03 4A 00 00\r>";
            else {response=additionalDemoResponse(c);if(response==null)response=multiPidResponse(c);if(response==null)response="NO DATA\r>";}
            return response;
        }
        /** REQ-022: a Mode 01 request for 2 or 3 PIDs is answered from the single-PID answers (single frame, or ISO-TP with 3); more PIDs are rejected like a vehicle without multi-PID support. */
        private String multiPidResponse(String c) {
            if(!c.matches("01([0-9A-F]{2}){2,3}")) return null;
            StringBuilder bytes=new StringBuilder("41");
            for(int i=2;i<c.length();i+=2) {
                String single=respond("01"+c.substring(i,i+2)); String[] tokens=single.replace(">","").trim().split("\\s+");
                if(tokens.length<5 || !tokens[2].equals("41")) return null;
                for(int k=3;k<tokens.length;k++) bytes.append(' ').append(tokens[k]);
            }
            String[] payload=bytes.toString().split(" "); StringBuilder out=new StringBuilder();
            if(payload.length<=7) { out.append(String.format(Locale.ROOT,"7E8 %02X ",payload.length)).append(String.join(" ",payload)); }
            else {
                out.append(String.format(Locale.ROOT,"7E8 10 %02X ",payload.length)).append(String.join(" ",Arrays.copyOfRange(payload,0,6)));
                for(int at=6,seq=1;at<payload.length;at+=7,seq=(seq+1)&0xF) { String[] part=Arrays.copyOfRange(payload,at,Math.min(at+7,payload.length));
                    out.append("\r7E8 ").append(String.format(Locale.ROOT,"2%X ",seq)).append(String.join(" ",part)); for(int pad=part.length;pad<7;pad++) out.append(" 00"); }
            }
            return out.append("\r>").toString();
        }
        private String additionalDemoResponse(String command) {
            String data;
            switch(command) {
                case "0106":case "0107":case "0108":case "0109":case "012E":case "012F":
                case "0147":case "0148":case "0149":case "014B":case "0152":case "015A":data="80";break;
                case "010A":data="64";break;
                case "010E":data="90";break;
                case "0122":data="03 20";break;
                case "013C":case "013D":case "013E":case "013F":data="0F A0";break;
                case "0143":data="01 00";break;
                case "0144":data="80 00";break;
                case "0159":data="0B B8";break;
                case "015C":data="82";break;
                case "015D":data="69 00";break;
                case "015E":data="00 64";break;
                case "0161":case "0162":data="96";break;
                case "0163":data="01 90";break;
                default:return null;
            }
            return String.format(Locale.ROOT,"7E8 %02X 41 %s %s\r>",2+data.split(" ").length,command.substring(2),data);
        }
    }
}
