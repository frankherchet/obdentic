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
    private volatile int generation;
    private volatile BleTransport transport;
    private volatile boolean busy;
    private long core;
    private BluetoothLeScanner scanner;
    private ScanCallback scanCallback;
    private volatile String mode = "live";
    private volatile String replayFilename = "";
    private BufferedWriter writer;
    private long lastPoll, lastWakeRefresh, recordingOriginNanos;
    private int scanGeneration;
    private android.os.PowerManager.WakeLock wakeLock;
    private volatile String recordingProfile = "";
    private volatile int sequenceIndex = -1;
    private final Map<String, Long> due = new HashMap<>(), intervals = new HashMap<>();
    private final Map<String, JSONObject> values = new LinkedHashMap<>();
    public volatile VehicleStore vehicles;
    public void onCreate() {
        super.onCreate(); current = this;
        vehicles = new VehicleStore(new File(getFilesDir(), "vehicles.json"));
        synchronized (stateLock) {
            put(state,"phase","idle"); put(state,"mode","live"); put(state,"busy",false);
            put(state,"message","Carly verbinden oder Demo starten");
            for (String key : new String[]{"devices","signals","profiles","values","debug","selected","records"}) put(state,key,new JSONArray());
            put(state,"recording",""); put(state,"vehicle",new JSONObject()); put(state,"diagnostics",new JSONObject());
        }
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel("connection", "OBD-Verbindung", NotificationManager.IMPORTANCE_LOW));
        worker.scheduleWithFixedDelay(this::poll, 500, 500, TimeUnit.MILLISECONDS);
        worker.execute(() -> { try { long temporary = NativeCore.create((c,t) -> { throw new IOException("Not connected"); });
            try { applyCatalog(new JSONObject(NativeCore.execute(temporary,"catalog",""))); } finally { NativeCore.destroy(temporary); }
            restoreSelection(selectedVin()); // REQ-013.6/REQ-014.1: after the catalog, so unknown semantics are filtered
        } catch (Throwable e) { message("Rust-Kern: " + e.getMessage()); } });
    }
    public int onStartCommand(Intent intent, int flags, int startId) { return START_NOT_STICKY; }
    public IBinder onBind(Intent intent) { return null; }
    public void setListener(Listener value) { listener = value; }
    public void clearListener(Listener value) { if (listener == value) listener = null; }
    private void changed() { Listener value = listener; if(value != null) main.post(() -> { if(listener == value) value.changed(); }); }
    private static void put(JSONObject object, String key, Object value) { try { object.put(key,value); } catch (JSONException e) { throw new IllegalArgumentException(e); } }
    private void set(String key,Object value) { synchronized(stateLock) { put(state,key,value); } changed(); }
    private String phase() { synchronized(stateLock) { return state.optString("phase"); } }
    private void message(String value) { set("message",value); }
    public JSONObject snapshot() {
        synchronized(stateLock) { put(state,"busy",busy || sequenceIndex>=0); put(state,"records",listRecords()); put(state,"vehicles",vehicles.snapshot());
            try { return new JSONObject(state.toString()); } catch(JSONException e) { return new JSONObject(); } }
    }
    private boolean permitted() {
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
            Notification notification = new Notification.Builder(this,"connection").setContentTitle("OBDentic verbunden")
                .setContentText("Diagnose und lokale Aufzeichnung laufen").setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
                .setContentIntent(pending).setOngoing(true).build();
            if(Build.VERSION.SDK_INT >= 29) startForeground(1,notification,android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE);
            else startForeground(1,notification);
            if(wakeLock == null) wakeLock = getSystemService(android.os.PowerManager.class).newWakeLock(android.os.PowerManager.PARTIAL_WAKE_LOCK, "obdentic:connection");
            wakeLock.setReferenceCounted(false); wakeLock.acquire(10*60*1000L); lastWakeRefresh=SystemClock.elapsedRealtime();
            return true;
        } catch(RuntimeException e) { message("Hintergrundverbindung: " + e.getMessage()); return false; }
    }
    public void connect(String address) {
        if (!permitted()) { message("Bluetooth-Berechtigungen fehlen"); return; }
        if (busy || phase().equals("ready")) { message("Zuerst aktuelle Verbindung trennen"); return; }
        BluetoothAdapter adapter=adapter();
        if(adapter==null || !adapter.isEnabled()) { message("Bitte Bluetooth einschalten"); return; }
        stopScan(); resetSessionData(); int token=++generation; busy=true; mode="live"; set("mode",mode); set("phase","connecting");
        if(!foreground()) { busy=false; set("phase","error"); return; }
        worker.execute(() -> {
            try {
                destroyCore(); BleTransport next=new BleTransport(this::debug); transport=next;
                if(token != generation) { next.close(); return; }
                next.connect(this,adapter.getRemoteDevice(address));
                if(token != generation) return;
                core=NativeCore.create(next); initialize(token);
            } catch(Throwable e) { fail(token,e); } finally { if(token==generation) { busy=false; changed(); } }
        });
    }
    public void setDemo() {
        disconnect(); resetSessionData(); int token=++generation; busy=true; mode="demo"; set("mode",mode); set("phase","connecting");
        if(permitted()) foreground();
        worker.execute(() -> { try { destroyCore(); core=NativeCore.create(new Demo()); initialize(token); }
            catch(Throwable e) { fail(token,e); } finally { if(token==generation) { busy=false; changed(); } } });
    }
    private void resetSessionData() {
        replayFilename="";
        synchronized(stateLock) { values.clear(); put(state,"values",new JSONArray()); put(state,"debug",new JSONArray());
            put(state,"replayFile","");
            put(state,"vehicle",new JSONObject()); put(state,"diagnostics",new JSONObject()); put(state,"jobResult",new JSONObject()); put(state,"replayPartial",false);
            put(state,"testSequence",new JSONObject()); put(state,"ashEvidence",new JSONObject());
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
    private void destroyCore() { if(core!=0) { NativeCore.destroy(core); core=0; } }
    public void disconnect() {
        ++generation; stopScan(); BleTransport active=transport; transport=null; if(active!=null) active.close();
        busy=false; worker.execute(() -> closeRecording("disconnected","Verbindung getrennt")); set("phase","disconnected"); message("Getrennt"); stopForeground(STOP_FOREGROUND_REMOVE); releaseWakeLock();
        worker.execute(this::destroyCore);
    }
    private void fail(int token,Throwable error) {
        if(token!=generation) return;
        BleTransport active=transport; transport=null; if(active!=null) active.close(); destroyCore();
        set("phase","error"); message(error.getMessage()==null ? error.toString():error.getMessage()); closeRecording("error",error.getMessage()==null?error.toString():error.getMessage()); stopForeground(STOP_FOREGROUND_REMOVE); releaseWakeLock();
    }
    private JSONObject execute(String action,String argument) throws Exception {
        JSONObject result=new JSONObject(NativeCore.execute(core,action,argument));
        JSONObject event=new JSONObject(); put(event,"type","action"); put(event,"action",action); put(event,"argument",argument); put(event,"result",result); record(event);
        BleTransport active = transport; if(mode.equals("live") && active != null && !active.isOpen()) throw new IOException("BLE-Verbindung beendet; bitte neu verbinden");
        return result;
    }
    private void applyCatalog(JSONObject result) {
        JSONObject data=result.optJSONObject("data"); if(data==null) return;
        set("signals",data.optJSONArray("signals") == null ? new JSONArray() : data.optJSONArray("signals"));
        set("profiles",data.optJSONArray("profiles") == null ? new JSONArray() : data.optJSONArray("profiles"));
    }
    public void setSignals(JSONArray semantics) {
        synchronized(stateLock) { if(!state.optString("recording").isEmpty()) { message("Messwertauswahl während Aufnahme gesperrt"); return; } }
        selectSignals(semantics);
        storeSelection();
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
        if(busy || sequenceIndex>=0 || !phase().equals("ready")) { message("Keine freie Verbindung für diesen Auftrag"); return; }
        if(!action.equals("inventory") && !action.equals("dtc.scan") && !isEa189Job(action) && !action.equals(CLUSTER_ODOMETER) && !action.startsWith("read:")) { message("Unbekannter Diagnoseauftrag"); return; }
        busy=true; changed(); int token=generation;
        worker.execute(() -> { try {
            if(token!=generation) return;
            if(sequenceIndex>=0) { message("Während des Testlaufs sind andere Aufträge gesperrt"); return; }
            String argument=action.startsWith("read:") ? action.substring(5) : "";
            if(action.equals(CLUSTER_ODOMETER)) { String blocked=clusterOdometerBlocker(snapshot()); if(blocked!=null) { message(blocked); return; } } // REQ-016.2: nothing is sent
            if(isEa189Job(action)) { JSONObject inventory=execute("inventory",""); if(token!=generation) return; set("vehicle",inventory.opt("data")); }
            JSONObject result=execute(argument.isEmpty()?action:"read",argument);
            if(token!=generation) return;
            set("jobResult",result);
            if(action.equals("inventory")) { set("vehicle",result.opt("data")); if(result.optJSONObject("data") != null) applySupport(result.optJSONObject("data")); }
            if(action.equals("dtc.scan")) set("diagnostics",result.opt("data"));
            if(!argument.isEmpty()) updateValue(argument,result);
            if(isEa189Job(action)) applyDpf(result);
            if(action.equals(CLUSTER_ODOMETER)) { message(storeClusterOdometer(result)); return; }
            String notice=action.equals("inventory") ? vehicleNotice(observeVehicle(result.optJSONObject("data")),result.optJSONObject("data")) : "";
            message(result.optBoolean("ok") ? "Auftrag abgeschlossen: " + action + notice : result.optString("error"));
        } catch(Throwable e) { fail(token,e); } finally { if(token==generation) { busy=false; changed(); } } });
    }
    public static final String CLUSTER_ODOMETER = "vw.cluster.odometer";
    /** REQ-016.2: null when the experimental cluster read may start, otherwise the German reason shown next to the disabled button. Busy state is not part of it. */
    public static String clusterOdometerBlocker(JSONObject snapshot) {
        String mode=snapshot.optString("mode","live");
        if(mode.equals("replay")) return "Im Replay nicht verfügbar";
        if(mode.equals("demo")) return "Kein gespeichertes Fahrzeug (Demo)";
        if(!snapshot.optString("phase").equals("ready")) return "Keine aktive Sitzung";
        JSONObject store=snapshot.optJSONObject("vehicles"), session=snapshot.optJSONObject("vehicle"), record=null;
        JSONArray rows=store==null ? null : store.optJSONArray("vehicles");
        for(int i=0;rows!=null && i<rows.length();i++) if(rows.optJSONObject(i).optString("vin").equals(store.optString("selected"))) record=rows.optJSONObject(i);
        if(record==null) return "Kein gespeichertes Fahrzeug gewählt";
        String vin=session==null ? "" : session.optString("vin");
        if(vin.isEmpty() || !vin.equals(record.optString("vin"))) return "Das gewählte Fahrzeug ist nicht das verbundene";
        if(!VehicleStore.isVwGroup(record.optString("manufacturer_hint"))) return "Kein Herstellerhinweis der VW-Gruppe";
        return null;
    }
    /** REQ-016.1/.4/.5/.8: live mode only; the result goes to the record of the current session VIN, never into the PID A6 fields. Returns the status message. */
    private String storeClusterOdometer(JSONObject result) {
        JSONObject data=result.optJSONObject("data");
        if(!result.optBoolean("ok") || data==null) return result.optString("error","Kombiinstrument-Auftrag nicht ausgeführt"); // refused: nothing was sent
        String outcome=data.optString("outcome"), error, text;
        long now=System.currentTimeMillis();
        JSONObject session; synchronized(stateLock) { session=state.optJSONObject("vehicle"); }
        String vin=session==null ? "" : session.optString("vin");
        if(outcome.equals("value")) { long km=data.optLong("value_km");
            text="Kombiinstrument: "+java.text.NumberFormat.getIntegerInstance(Locale.GERMANY).format(km)+" km (experimentell, ungeprüft)";
            error=mode.equals("live") && !vin.isEmpty() && vin.equals(selectedVin()) ? vehicles.clusterOdometer(vin,km,now,data.optString("source"),data.optString("classification"),data.optString("hardware_validation")) : null;
        } else { JSONArray bytes=data.optJSONArray("response_data"); StringBuilder raw=new StringBuilder();
            for(int i=0;bytes!=null && i<bytes.length();i++) raw.append(i>0?" ":"").append(String.format(Locale.ROOT,"%02X",bytes.optInt(i)));
            boolean unreadable=outcome.equals("unparseable_length");
            String reason=unreadable ? "nicht auswertbar" : data.optString("error","Keine Antwort");
            text="Kombiinstrument-km-Stand nicht gelesen: "+reason;
            error=mode.equals("live") && !vin.isEmpty() && vin.equals(selectedVin()) ? vehicles.clusterOdometerFailure(vin,reason,unreadable ? raw.toString() : null,now) : null;
        }
        return error==null ? text : error;
    }
    private static boolean isEa189Job(String action) {
        return action.equals("ea189.dpf.probe") || action.equals("ea189.extended.probe");
    }
    private void applyDpf(JSONObject result) {
        JSONObject data=result.optJSONObject("data"); if(data==null) return;
        JSONArray readings=data.optJSONArray("readings"); if(readings==null) return;
        for(int i=0;i<readings.length();i++) { JSONObject reading=readings.optJSONObject(i); if(reading==null) continue;
            JSONObject envelope=new JSONObject(); put(envelope,"ok",!reading.has("error")); put(envelope,"data",reading); put(envelope,"error",reading.opt("error"));
            updateValue(reading.optString("semantic"),envelope);
        }
    }
    private void poll() {
        if(wakeLock!=null && SystemClock.elapsedRealtime()-lastWakeRefresh>5*60*1000L && (phase().equals("ready") || phase().equals("connecting"))) {
            wakeLock.acquire(10*60*1000L); lastWakeRefresh=SystemClock.elapsedRealtime();
        }
        if(busy || !phase().equals("ready") || core==0) return;
        BleTransport active=transport; if(mode.equals("live") && active!=null && !active.isOpen()) { fail(generation,new IOException("BLE-Verbindung beendet")); return; }
        if(sequenceIndex>=0) { pollSequence(); return; }
        JSONArray selected; synchronized(stateLock) { selected=state.optJSONArray("selected"); }
        String probeAction=recordingProfile.equals("ea189-dpf")?"ea189.dpf.probe":recordingProfile.equals("ea189-extended")?"ea189.extended.probe":"";
        boolean dpf=!probeAction.isEmpty();
        if((selected.length()==0 && !dpf) || SystemClock.elapsedRealtime()-lastPoll<(dpf?10000:500)) return;
        int token=generation;
        try {
            if(dpf) {
                boolean validated; synchronized(stateLock) { JSONObject vehicle=state.optJSONObject("vehicle"); validated=vehicle!=null && vehicle.optBoolean("engine_target_validated"); }
                if(!validated) { JSONObject inventory=execute("inventory",""); if(token!=generation) return; set("vehicle",inventory.opt("data")); }
                JSONObject result=execute(probeAction,""); if(token!=generation) return; applyDpf(result); set("jobResult",result);
            } else {
                long now=SystemClock.elapsedRealtime(), earliest=Long.MAX_VALUE; String semantic=null;
                synchronized(due) { for(int i=0;i<selected.length();i++) { String candidate=selected.optString(i); long next=due.getOrDefault(candidate,0L);
                    boolean unsupported=false; synchronized(stateLock) { JSONArray catalog=state.optJSONArray("signals");
                        for(int j=0;j<catalog.length();j++) { JSONObject signal=catalog.optJSONObject(j); if(candidate.equals(signal.optString("semantic")) && signal.optString("status").equals("unsupported")) unsupported=true; }
                    }
                    if(!unsupported && next<=now && next<earliest) { semantic=candidate; earliest=next; }
                } }
                if(semantic!=null) {
                    JSONObject result=execute("read",semantic); if(token==generation) updateValue(semantic,result);
                    synchronized(due) { due.put(semantic,SystemClock.elapsedRealtime()+Math.max(1000,intervals.getOrDefault(semantic,2000L))); }
                }
            }
        } catch(Throwable e) { fail(token,e); }
        finally { lastPoll=SystemClock.elapsedRealtime(); if(token==generation) changed(); }
    }
    private void updateValue(String semantic,JSONObject result) {
        JSONObject row=new JSONObject(); put(row,"semantic",semantic); put(row,"updatedAt",System.currentTimeMillis());
        Object data=result.opt("data"); if(data instanceof JSONObject) { JSONObject object=(JSONObject)data;
            Iterator<String> keys=object.keys(); while(keys.hasNext()) { String key=keys.next(); put(row,key,object.opt(key)); }
        } else put(row,"value",data);
        if(!result.optBoolean("ok")) put(row,"error",result.optString("error"));
        synchronized(stateLock) {
            JSONArray catalog=state.optJSONArray("signals"); for(int i=0;i<catalog.length();i++) { JSONObject s=catalog.optJSONObject(i);
                if(semantic.equals(s.optString("semantic"))) { put(row,"title",s.optString("title",semantic)); if(!row.has("unit")) put(row,"unit",s.optString("unit")); } }
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
    public void startRecording(String profileId) { startRecording(profileId,""); }
    public void startRecording(String profileId,String context) { worker.execute(() -> startRecordingNow(profileId,context)); }
    private void startRecordingNow(String profileId,String context) {
        if(!phase().equals("ready")) { message("Zum Aufzeichnen zuerst verbinden"); return; }
        if(busy || sequenceIndex>=0) { message("Zuerst laufenden Auftrag oder Testlauf beenden"); return; }
        boolean test=profileId.equals(TestSequence.PROFILE);
        if(test && !TestSequence.validContext(context)) { message("Für den Testlauf zuerst den Betriebszustand auswählen"); return; }
        JSONArray signals=null, periods=null; synchronized(stateLock) { JSONArray profiles=state.optJSONArray("profiles");
            for(int i=0;i<profiles.length();i++) if(profileId.equals(profiles.optJSONObject(i).optString("id"))) { signals=profiles.optJSONObject(i).optJSONArray("signals"); periods=profiles.optJSONObject(i).optJSONArray("intervals_ms"); } }
        if(signals==null) { message("Unbekanntes Aufnahmeprofil"); return; }
        stopRecordingNow();
        try {
            File directory=new File(getFilesDir(),"recordings"); if(!directory.exists() && !directory.mkdirs()) throw new IOException("Aufnahmeordner nicht verfügbar");
            String name=new SimpleDateFormat("yyyyMMdd-HHmmss-SSS",Locale.ROOT).format(new Date())+"-"+mode+".jsonl";
            synchronized(recordLock) {
                recordingOriginNanos=SystemClock.elapsedRealtimeNanos();
                writer=new BufferedWriter(new OutputStreamWriter(new FileOutputStream(new File(directory,name)),StandardCharsets.UTF_8));
                set("recording",name);
                JSONObject header=new JSONObject(); put(header,"type","session"); put(header,"schema","OBDENTIC-MOBILE"); put(header,"version",1); put(header,"mode",mode); put(header,"profile",profileId); put(header,"selected",profileId.equals("ea189-dpf")||profileId.equals("ea189-extended")?new JSONArray():signals);
                if(test) {
                    set("ashEvidence",new JSONObject());
                    JSONObject sequence=new JSONObject(); put(sequence,"profile",profileId); put(sequence,"operator_context",context); put(sequence,"context_label",TestSequence.contextLabel(context));
                    put(sequence,"context_source","operator_report"); put(sequence,"status","running"); put(sequence,"step",0); put(sequence,"total",TestSequence.Step.values().length); put(sequence,"file",name);
                    put(header,"test_sequence",sequence); set("testSequence",sequence);
                } else set("testSequence",new JSONObject());
                synchronized(stateLock) { put(header,"vehicle",state.opt("vehicle")); put(header,"diagnostics",state.opt("diagnostics")); put(header,"catalog",state.opt("signals")); put(header,"profiles",state.opt("profiles")); }
                // Notifications must not precede the header in the new evidence stream.
                record(header);
                if(writer==null) { if(test)sequenceProgress("aborted","Aufnahme konnte nicht geschrieben werden"); return; }
            }
            recordingProfile=profileId;
            sequenceIndex=test?0:-1;
            synchronized(due) { due.clear(); intervals.clear(); for(int i=0;i<signals.length();i++) intervals.put(signals.optString(i),periods==null?1200L:periods.optLong(i,1200L)); }
            if(test||profileId.equals("ea189-dpf")||profileId.equals("ea189-extended"))set("selected",new JSONArray());else selectSignals(signals); message("Aufnahme läuft: " + profileId);
        } catch(IOException e) { message(e.getMessage()); }
    }
    public void stopRecording() { if(!worker.isShutdown()) worker.execute(this::stopRecordingNow); }
    private void stopRecordingNow() { closeRecording("stopped",""); }
    private void closeRecording(String reason,String detail) {
        if(sequenceIndex>=0) sequenceProgress(reason.equals("completed")?"completed":"aborted",detail);
        sequenceIndex=-1;
        recordingProfile=""; synchronized(due) { due.clear(); intervals.clear(); }
        boolean completed=false;
        synchronized(recordLock) {
            JSONObject end=new JSONObject(); put(end,"type","session_end"); put(end,"reason",reason); if(!detail.isEmpty()) put(end,"detail",detail); record(end);
            if(writer!=null) { try { writer.close(); completed=true; } catch(IOException e) { message("Aufnahme konnte nicht abgeschlossen werden: "+e.getMessage()); } writer=null; }
        }
        set("recording","");
        if(completed) message(reason.equals("completed")?"Testlauf aufgezeichnet · Antworten auswerten":reason.equals("stopped")?"Aufzeichnung gespeichert":reason.equals("error")?"Aufzeichnung mit Fehler beendet: "+detail:"Aufzeichnung nach Trennen gespeichert");
    }
    private void sequenceProgress(String status,String label) {
        JSONObject progress;
        synchronized(stateLock) {
            try { progress=new JSONObject(state.getJSONObject("testSequence").toString()); }
            catch(JSONException e) { throw new IllegalStateException(e); }
        }
        put(progress,"status",status); put(progress,"step",Math.min(sequenceIndex+1,TestSequence.Step.values().length)); put(progress,"label",label);
        set("testSequence",progress);
        JSONObject event=new JSONObject(); put(event,"type","test_sequence"); put(event,"progress",progress); record(event);
    }
    private void pollSequence() {
        int token=generation;
        TestSequence.Step step=TestSequence.Step.values()[sequenceIndex];
        sequenceProgress("running",step.label);
        try {
            synchronized(recordLock) { if(writer==null || sequenceIndex<0)return; }
            JSONObject result=execute(step.action,step.argument);
            if(token!=generation) return;
            synchronized(recordLock) { if(writer==null) { sequenceIndex=-1; return; } }
            set("jobResult",result);
            if(!result.optBoolean("ok")) { closeRecording("error",result.optString("error","Testschritt fehlgeschlagen")); return; }
            JSONObject data=result.optJSONObject("data");
            if(step==TestSequence.Step.INVENTORY) {
                set("vehicle",data); if(data!=null)applySupport(data);
                if(data==null || !data.optBoolean("engine_target_validated")) { closeRecording("error","Motorsteuergerät nicht gezielt bestätigt"); return; }
            } else if(step==TestSequence.Step.ASH) set("ashEvidence",data);
            else if(step==TestSequence.Step.DPF) applyDpf(result);
            else updateValue(step.argument,result);
            if(sequenceIndex+1==TestSequence.Step.values().length)closeRecording("completed","");
            else sequenceIndex++;
        } catch(Throwable e) { fail(token,e); }
    }
    private void record(JSONObject event) {
        put(event,"at",System.currentTimeMillis()); synchronized(recordLock) { if(writer==null) return; put(event,"offset_us",Math.max(0,(SystemClock.elapsedRealtimeNanos()-recordingOriginNanos)/1000));
            try { writer.write(event.toString()); writer.newLine(); writer.flush(); }
            catch(IOException e) { try { writer.close(); } catch(IOException ignored) {} writer=null; recordingProfile=""; if(sequenceIndex>=0){sequenceIndex=-1;synchronized(stateLock){put(state.optJSONObject("testSequence"),"status","aborted");}} synchronized(due) { due.clear(); intervals.clear(); } set("recording",""); message("Aufnahmefehler: "+e.getMessage()); }
        }
    }
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
            synchronized(stateLock) { active=state.optString("recording"); }
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
                        else if(action.equals("inventory")) restoreVehicle(result.optJSONObject("data"));
                        else if(action.equals("dtc.scan") || action.equals("ea189.ash.probe") || action.equals(CLUSTER_ODOMETER) || isEa189Job(action)) {
                            JSONObject recorded=result.optJSONObject("data");
                            JSONObject decoded=recorded==null?result:new JSONObject(NativeCore.execute(core,"replay."+action,recorded.toString()));
                            if(action.equals("dtc.scan")) set("diagnostics",decoded.opt("data")); else if(action.equals("ea189.ash.probe"))set("ashEvidence",decoded.opt("data")); else if(!action.equals(CLUSTER_ODOMETER)) applyDpf(decoded); // REQ-016.7: re-decoded, never stored
                            result=decoded;
                        }
                        set("jobResult",result);
                    } else if(event.optString("type").equals("session")) {
                        String savedProfile=event.optString("profile");if(savedProfile.equals("ea189-dpf")||savedProfile.equals("ea189-extended"))set("selected",new JSONArray());else selectSignals(event.optJSONArray("selected")==null?new JSONArray():event.optJSONArray("selected"));
                        if(event.optJSONObject("vehicle")!=null) restoreVehicle(event.optJSONObject("vehicle"));
                        if(event.optJSONObject("test_sequence")!=null)set("testSequence",event.optJSONObject("test_sequence"));
                        if(event.optJSONObject("diagnostics")!=null && event.optJSONObject("diagnostics").has("raw_response")) { JSONObject decoded=new JSONObject(NativeCore.execute(core,"replay.dtc.scan",event.optJSONObject("diagnostics").toString())); set("diagnostics",decoded.opt("data")); }
                        set("replaySourceMode",event.optString("mode","unknown"));
                    } else if(event.optString("type").equals("test_sequence")) { if(event.optJSONObject("progress")!=null)set("testSequence",event.optJSONObject("progress"));
                    } else if(event.optString("type").equals("wire")) { synchronized(stateLock) { JSONArray rows=state.optJSONArray("debug"); rows.put(event); if(rows.length()>300) rows.remove(0); } }
                }
                if(!headerFound) throw new IOException("Leere Aufnahme");
                if(token==generation) { restoreSequenceOutcome(endFound,outcome); set("replayPartial",!endFound); set("replayOutcome",outcome); message(!endFound?"Teilaufnahme ohne Abschlussmarker: "+filename:outcome.equals("error")?"Aufnahme mit Fehler beendet: "+outcomeDetail:"Offline geöffnet ("+outcome+"): "+filename); }
            }
        } catch(Exception e) { if(token==generation) { if(!headerAccepted){replayFilename="";set("replayFile","");} restoreSequenceOutcome(false,""); set("replayPartial",true); message("Teilweise oder ungültige Aufnahme: " + e.getMessage()); } }
        finally { if(token==generation) { busy=false; changed(); } } });
    }
    private void restoreSequenceOutcome(boolean complete,String outcome) {
        synchronized(stateLock) {
            JSONObject sequence=state.optJSONObject("testSequence");
            if(sequence==null || sequence.length()==0)return;
            if(!complete)put(sequence,"status","incomplete");
            else if(sequence.optString("status").equals("running"))put(sequence,"status",outcome.equals("completed")?"completed":"aborted");
        }
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
    public void onDestroy() { disconnect(); worker.execute(this::destroyCore); worker.shutdown(); listener=null; current=null; super.onDestroy(); }
    private final class Demo implements NativeCore.Exchange {
        public String exchange(String command,long timeoutMs) throws Exception {
            if(!mode.equals("demo")) throw new IOException("Demo stopped");
            debug("TX",command); String c=command.trim().replace(" ","").toUpperCase(Locale.ROOT); String response;
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
            else if(c.equals("22114E")) response="7E8 05 62 11 4E 03 E8\r>";
            else if(c.equals("22114F")) response="7E8 05 62 11 4F 04 F8\r>";
            else if(c.equals("221156")) response="7E8 07 62 11 56 00 00 8C A0\r>";
            else if(c.equals("22115E")) response="7E8 07 62 11 5E 00 00 0D 2F\r>";
            else if(c.equals("2211B2")) response="7E8 05 62 11 B2 16 A8\r>";
            else if(c.equals("2210F9")) response="7E8 05 62 10 F9 14 B4\r>";
            else if(c.equals("2214F5")) response="7E8 05 62 14 F5 00 0A\r>";
            else if(c.equals("22178C")) response="7E8 03 7F 22 31\r>";
            else if(c.equals("222203")) response="77E 05 62 22 03 30 39\r>";
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
            else {response=additionalDemoResponse(c);if(response==null)response="NO DATA\r>";}
            debug("RX",response); return response;
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
                case "221153":data="00 0C";break;
                case "22115A":data="01 F4";break;
                case "22115B":case "22115C":case "2211C1":data="00";break;
                case "221057":case "221149":data="03 E8";break;
                case "2211BE":data="0E 2F";break;
                case "22162D":data="0C 3B";break;
                case "2210FF":case "221100":data="00 0A";break;
                case "221104":case "221105":data="FF F6";break;
                case "221169":case "22116A":data="0B B8";break;
                default:return null;
            }
            boolean uds=command.startsWith("22");
            int length=(uds?3:2)+data.split(" ").length;
            return String.format(Locale.ROOT,"7E8 %02X %s %s %s\r>",length,uds?"62":"41",
                uds?command.substring(2,4)+" "+command.substring(4):command.substring(2),data);
        }
    }
}
