package de.obdentic.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.net.Uri;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;

/** Phone acceptance against simulated BLE responses passed through the real Rust core. */
public final class AcceptanceRunner extends Instrumentation {
    private ObdService service;
    private Activity activity;
    private final StringBuilder report = new StringBuilder();
    private boolean screenOff;
    /** Default runs never lock the phone; pass `-e screenOff true` to also exercise a real screen-off period. */
    @Override public void onCreate(Bundle arguments) { super.onCreate(arguments); screenOff = arguments != null && "true".equals(arguments.getString("screenOff")); start(); }
    @Override public void onStart() {
        ObdService.autoSurvey=false; // REQ-030.4: only AvailabilityAcceptance switches the automatic survey on
        Bundle result = new Bundle();
        // Demo recordings made by this run must not be auto-exported into the user's real export folder.
        boolean userAutoExport = RecordingExporter.auto(getTargetContext());
        RecordingExporter.setAuto(getTargetContext(), false);
        // The user's known adapter must neither be connected automatically nor changed by this run (REQ-020): switch off before the app starts, restore at the end.
        android.content.SharedPreferences adapterPrefs = getTargetContext().getSharedPreferences("adapter", android.content.Context.MODE_PRIVATE);
        java.util.Map<String, ?> userAdapter = new java.util.HashMap<>(adapterPrefs.getAll());
        adapterPrefs.edit().putBoolean("auto", false).commit();
        try {
            Intent launch = new Intent(getTargetContext(), MainActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            activity = startActivitySync(launch);
            waitFor(() -> (activity.getWindow().getAttributes().flags & android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) != 0,"Visible Activity keeps the screen awake");
            passed("The visible app keeps the display awake");
            waitFor(() -> ObdService.current != null, "service creation");
            service = ObdService.current;
            runOnMainSync(service::setDemo);
            waitFor(() -> "ready".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"), "demo initialization");
            JSONObject state = service.snapshot();
            require("demo".equals(state.optString("mode")), "Demo must be visibly separate from live");
            require(state.getJSONArray("signals").length() == 50, "Expanded semantic catalog");
            require(state.getJSONArray("profiles").length() == 4, "Standard capture profiles only");
            passed("Rust JNI library, real initialization/parser, signal and profile catalogs");
            SignalSelectorAcceptance.run(getTargetContext());
            SignalSelectorAcceptance.verifyUi(this::onMain, activity, service);
            UdsAcceptance.verifyDemoGate(this::onMain, activity, service);
            UdsAcceptance.verifyLive(this::onMain, activity, service, getTargetContext());
            passed("Private signal presets, persisted ECU/learning results, UDS demo gate and simulated UDS activation, recording, default-session return before disconnect and offline replay");
            AvailabilityUiAcceptance.run(this::onMain, activity, service, getTargetContext());
            passed("Availability in the selector and on tiles (REQ-031): measured states, hidden-count and 'Nur verfügbare' filter, every signal selectable, tiles state their reason instead of a dash");

            service.setSignals(new JSONArray());waitIdle();
            JSONArray catalog=state.getJSONArray("signals");
            for(int i=0;i<catalog.length();i++)job("read:"+catalog.getJSONObject(i).getString("semantic"));
            require(service.snapshot().getJSONArray("values").length()==50,"Every catalog scalar decoded in Android demo");
            JSONObject odometer=findValue(service.snapshot().getJSONArray("values"),"vehicle.odometer");
            require(odometer!=null&&odometer.optDouble("value")==123456.7&&"km".equals(odometer.optString("unit")),"REQ-015.1 demo odometer (PID A6, bytes 00 12 D6 87) decodes to 123456.7 km through the real JNI: "+odometer);
            service.setDemo();waitFor(() -> "ready".equals(service.snapshot().optString("phase"))&&!service.snapshot().optBoolean("busy"),"reset demo values");
            passed("All 50 standard scalars, including the A6 odometer via support pages up to 01A0, decode through the actual Android/JNI transport seam");

            service.setSignals(new JSONArray().put("engine.rpm").put("engine.coolant_temperature"));
            waitFor(() -> service.snapshot().optJSONArray("values").length() >= 2, "live scalar reads");
            boolean numeric = false;
            JSONArray values = service.snapshot().getJSONArray("values");
            for (int i=0; i<values.length(); i++) if ("engine.rpm".equals(values.getJSONObject(i).optString("semantic"))) {
                numeric = values.getJSONObject(i).optDouble("value", -1) > 0;
            }
            require(numeric, "RPM decoded from raw adapter response");
            capture(activity,"Live","acceptance-live.png");
            FormatAcceptance.names(service.snapshot().getJSONArray("signals"));
            final Activity liveActivity=activity;
            require(onMain(() -> hasText(liveActivity.getWindow().getDecorView(),"Drehzahl",true)),"#232 live card shows the short name Drehzahl");
            runOnMainSync(() -> FormatAcceptance.noSemanticIds(liveActivity.getWindow().getDecorView()));
            LiveGridAcceptance.run(this::onMain, service, liveActivity);
            service.setSignals(new JSONArray());
            waitIdle();
            passed("Live RPM/coolant readings and changing observation selection");
            activity = PendingFeedbackAcceptance.run(this, activity, service); // recreates the Activity (REQ-021.6)
            passed("Pending feedback (REQ-021): disabled pending button with text, indicator and content description, tap while pending ignored, state survives tab switch and Activity recreation, ends on the observed target state, time-limit message and usable button, announcements, real recording start and stop");

            job("inventory");
            require(service.snapshot().optJSONObject("vehicle").optBoolean("engine_target_validated"), "Evidenced engine target");
            JSONObject vinIdentity=service.snapshot().getJSONObject("vehicle").getJSONObject("vin_identity");
            require("Volkswagen".equals(vinIdentity.optString("make_hint")),"Synthetic demo VIN manufacturer hint");
            require(vinIdentity.isNull("model")&&vinIdentity.isNull("engine"),"WMI must not guess model or engine");
            job("dtc.scan");
            require(service.snapshot().getJSONObject("diagnostics").toString().contains("P0133"), "Responder-scoped DTC decoding");
            capture(activity,"Diagnose","acceptance-diagnose.png");
            capture(activity,"Debug","acceptance-debug.png");
            capture(activity,"Adapter","acceptance-connection.png");
            passed("Inventory and stored-DTC jobs");

            service.startRecording("engine-drive");
            waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "recording start");
            String filename = service.snapshot().getString("recording");
            File file = new File(new File(getTargetContext().getFilesDir(), "recordings"), filename);
            waitFor(() -> file.length() > 500, "recording evidence");
            deleteRecording(filename,"Aufzeichnung vor dem Löschen stoppen");
            require(file.isFile()&&filename.equals(service.snapshot().optString("recording")),"Active recording cannot be deleted");
            deleteRecording("../outside.jsonl","Ungültiger Aufnahmepfad");
            deleteRecording("bad/name.jsonl","Ungültiger Aufnahmepfad");
            deleteRecording("missing-"+System.nanoTime()+".jsonl","Aufnahme nicht gefunden");
            require(file.isFile(),"Rejected active, malformed and missing deletions preserve recording evidence");
            passed("Recording deletion rejects active files, malformed paths and missing files with clear errors");
            capture(activity,"Diagnose","acceptance-diagnose.png");
            FormatAcceptance.run();passed("#216 decoded values render rounded, German and with display units");
            PollScheduleAcceptance.run();passed("#231 live polling reads back-to-back at the 500 ms period");
            getUiAutomation().executeShellCommand("input keyevent 3").close();
            waitFor(() -> (activity.getWindow().getAttributes().flags & android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) == 0,"Activity clears keep-screen-on when paused");
            long backgrounded=file.length();
            if(screenOff) { getUiAutomation().executeShellCommand("input keyevent 223").close(); SystemClock.sleep(4000); }
            waitFor(() -> file.length() > backgrounded, "Background recording appends actual evidence");
            require(filename.equals(service.snapshot().optString("recording")), "Background recording remains active");
            if(screenOff) getUiAutomation().executeShellCommand("input keyevent 224").close();
            service.stopRecording();
            waitFor(() -> service.snapshot().optString("recording").isEmpty(),"recording stop at transaction boundary");
            service.setSignals(new JSONArray());
            waitIdle();
            boolean tx=false, rx=false, read=false;
            try(BufferedReader input=new BufferedReader(new InputStreamReader(new FileInputStream(file), StandardCharsets.UTF_8))) {
                String line, lastType=""; boolean first=true;
                while((line=input.readLine())!=null) {
                    JSONObject event = new JSONObject(line);
                    if(first) { require("session".equals(event.optString("type")),"Header is first, before asynchronous RX"); first=false; }
                    lastType=event.optString("type");
                    tx |= "TX".equals(event.optString("direction"));
                    rx |= "RX".equals(event.optString("direction"));
                    read |= "read".equals(event.optString("action"));
                }
                require("session_end".equals(lastType),"Terminal marker is last, after active transaction");
            }
            require(tx && rx && read, "Recording preserves requests, responses and decoded operations");
            java.nio.file.Files.copy(file.toPath(),new File(getTargetContext().getExternalFilesDir("acceptance"),"demo-recording.jsonl").toPath(),java.nio.file.StandardCopyOption.REPLACE_EXISTING);
            passed("Profile recording, background operation"+(screenOff?" with screen off":" (screen stays on; opt in with -e screenOff true)")+", explicit stop and flushed TX/RX evidence");

            Uri uri = Uri.parse("content://" + getTargetContext().getPackageName() + ".files/recordings/" + filename);
            try(ParcelFileDescriptor fd=getTargetContext().getContentResolver().openFileDescriptor(uri,"r")) {
                require(fd != null && fd.getStatSize() > 0, "Content URI export readable");
            }
            boolean denied=false;
            try { getTargetContext().getContentResolver().openFileDescriptor(uri,"w").close(); }
            catch(Exception expected) { denied=true; }
            require(denied,"Export provider refuses writes");
            passed("Explicit file export grants read-only recording access");
            capture(activity,"Aufnahmen","acceptance-recordings.png");

            service.disconnect();
            require("disconnected".equals(service.snapshot().optString("phase")),"Disconnect state");
            service.openRecording(filename);
            waitFor(() -> "replay".equals(service.snapshot().optString("mode")) && !service.snapshot().optBoolean("busy"), "offline replay");
            require(service.snapshot().getJSONArray("values").length()>0,"Replay restores readings");
            require("Volkswagen".equals(service.snapshot().getJSONObject("vehicle").getJSONObject("vin_identity").optString("make_hint")),"Offline replay derives manufacturer locally");
            deleteRecording(filename,"Im Replay geöffnete Aufzeichnung kann nicht gelöscht werden");
            require(file.isFile()&&filename.equals(service.snapshot().optString("replayFile")),"Loaded replay cannot be deleted");
            passed("Disconnect and offline replay without a live connection");

            long core=NativeCore.create((command,timeout)->{throw new AssertionError("Rejected/offline actions must never send traffic");});
            try {
                JSONObject identity=new JSONObject(NativeCore.execute(core,"vin.describe","wvw00000000000000"));
                require(identity.getJSONObject("data").getString("status").equals("identified"),"VIN helper never uses transport");
                for(String action:new String[]{"dtc.clear","write","ATZ","uds.send","ea189.dpf.probe","replay.ea189.ash.probe","vw.cluster.odometer"})
                    require(!new JSONObject(NativeCore.execute(core,action,"")).optBoolean("ok"),"Reject " + action);
                JSONObject stored = new JSONObject().put("semantic","engine.rpm").put("value",999999)
                    .put("response",new JSONArray().put(0x41).put(0x0c).put(0x0c).put(0x80));
                JSONObject replay = new JSONObject(NativeCore.execute(core,"replay.read",stored.toString()));
                require(replay.optBoolean("ok") && replay.getJSONObject("data").getDouble("value")==800.0,"Replay re-decodes raw bytes");
            } finally { NativeCore.destroy(core); }
            passed("Mutation/raw-command rejection and deterministic offline re-decoding");

            service.setDemo();
            waitFor(() -> "ready".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"),"reconnect demo");
            legacyRecordingWithRemovedActions();
            passed("#230 an old recording with removed actions opens in replay: unknown actions are skipped, standard readings restored, nothing is sent");
            File exportedCopy=new File(getTargetContext().getExternalFilesDir("acceptance"),"demo-recording.jsonl");
            service.deleteRecording(filename);
            waitFor(() -> ("Aufnahme endgültig gelöscht: "+filename).equals(service.snapshot().optString("message")),"completed recording deletion");
            require(!file.exists(),"Completed internal recording is deleted");
            require(exportedCopy.isFile()&&exportedCopy.length()>0,"Deleting an internal recording leaves its exported copy intact");
            require(!containsRecord(filename),"Deleted recording disappears from the listed recordings");
            passed("Completed recording is permanently deleted while its exported copy remains");
            RecordingExportAcceptance.run(service,getTargetContext());
            passed("Recording export (REQ-018): copy/skip/conflict/partial-failure/unavailable-folder logic against a directory target, active recording excluded, byte-identical copies, originals untouched, opt-in automatic export after stop and disconnect, share intent with read-only provider URIs, unchanged permissions (system folder picker itself not driven)");
            VehicleRecordsAcceptance.run(service,(name,listText) -> showVehicles(activity,name,listText));
            passed("Known vehicles: VIN-keyed records, stored odometer, demo/replay isolation, selection, rename, delete, corrupt-store and write-failure safety, header chip on every tab");
            ReconnectAcceptance.run(service,getTargetContext());
            passed("Automatic connection (REQ-020): policy schedule 2/5/10/30 s and reset, known adapter stored/replaced app-private, retries only to the known address, manual disconnect/stop/demo/replay end attempts, switch default on and persisted, partial recording kept and same-profile continuation, Bluetooth/permission off shows the reason and resumes, no job repeated; fake adapter link, no real BLE (direct GATT connect, engine start/stop and app restart remain for #178)");
            AutoRecordingAcceptance.run(service,getTargetContext());
            passed("Continuous recording (REQ-028): starts before the first adapter byte with the handshake, ends with a marker, failed attempts leave an error file, named recordings replace and resume it, rotation by size and age without a gap, persistent switch, unwritable folder keeps the session alive, explicit delete only, replay re-derives the vehicle");
            AppLogAcceptance.run(service,getTargetContext());
            passed("App debug log (REQ-029): line format, size/connection seal with unique names and newest-50 retention, stack traces with causes and uncaught exceptions, VIN/address/raw-traffic scrubbing, unwritable log never disturbs a session, job/connection/recording/reconnect events, sealed logs exported byte-identical and skipped on rerun");
            SignalKeyAcceptance.run(service,getTargetContext());
            passed("#256 a Knowledge signal sharing its semantic with a standard PID has its own id, status and value; the unsupported PID is not polled");
            AvailabilityAcceptance.run(service,getTargetContext());
            passed("Availability check (REQ-030): starts automatically for a new vehicle and after every inventory job, one closed 0x22 read per default-session signal, answered/rejected(NRC) classification without values, stored with the vehicle, own recording, replay restores the statuses without traffic, never in demo");
            ThroughputAcceptance.run(service,getTargetContext(),new ThroughputAcceptance.Ui(){
                public String text(String tab) throws Exception { return tabText(activity,tab); }
                public Boolean enabled(String label,boolean expected) throws Exception { 
                    Boolean now=null; for(int i=0;i<50;i++) { now=onMain(() -> { findButton(activity.getWindow().getDecorView(),"Diagnose").performClick(); /* re-renders from the current state: the paused Activity does not poll */ Button b=findButton(activity.getWindow().getDecorView(),label); return b==null?null:b.isEnabled(); }); if(now!=null&&now==expected) break; SystemClock.sleep(100); }
                    return now; }
            });
            passed("Adapter throughput job (REQ-022): unavailable with a reason in demo, not connected and replay without any traffic; live and ready runs the closed job against the simulated adapter (6 PIDs x 10 reads with median/p90/reads per second, multi-PID with 2, 3 and 6 PIDs decoded per PID: full, full, rejected), own complete recording with every TX/RX and the result, summary card in Diagnose, replay re-derives the same summary (real multi-PID answers of a vehicle remain for #178)");
            capture(activity,"Diagnose","acceptance-diagnose.png");
            runOnMainSync(activity::recreate);
            SystemClock.sleep(1500);
            require("ready".equals(service.snapshot().optString("phase")),"Activity recreation preserves session");
            passed("Session reconnect and Activity recreation");
            service.disconnect();
            runOnMainSync(service::scan);
            waitFor(() -> !"scanning".equals(service.snapshot().optString("phase")),"real Android BLE scan completion");
            require("idle".equals(service.snapshot().optString("phase")),"BLE scan terminates without an Android error");
            passed("Actual Android BLE scan lifecycle (adapter connection remains a separate vehicle test)");
            VehicleRecordsAcceptance.selection(service);
            passed("Known vehicles: per-record signal selection restored without adapter traffic, stored support labelled with its date and never used for discovery, default selection");
            runOnMainSync(service::setDemo);
            waitFor(() -> "ready".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"),"restore demo for UI review");
            for(String name:new String[]{"acceptance-live.png","acceptance-diagnose.png","acceptance-debug.png","acceptance-connection.png","acceptance-recordings.png","acceptance-vehicles.png","demo-recording.jsonl"}) emitArtifact(name);
            result.putString("stream", report + "\nDESK ACCEPTANCE PASSED\nReal Carly/vehicle acceptance remains separate.\n");
            RecordingExporter.setAuto(getTargetContext(), userAutoExport); restoreAdapterPrefs(adapterPrefs, userAdapter);
            finish(Activity.RESULT_OK,result);
        } catch(Throwable error) {
            result.putString("stream",report + "\nFAILED: " + error + "\n" + android.util.Log.getStackTraceString(error));
            RecordingExporter.setAuto(getTargetContext(), userAutoExport); restoreAdapterPrefs(adapterPrefs, userAdapter);
            finish(Activity.RESULT_CANCELED,result);
        }
    }
    private static void restoreAdapterPrefs(android.content.SharedPreferences prefs, java.util.Map<String, ?> saved) {
        android.content.SharedPreferences.Editor editor = prefs.edit().clear();
        if (saved.get("known_address") instanceof String) editor.putString("known_address", (String) saved.get("known_address"));
        if (saved.get("auto") instanceof Boolean) editor.putBoolean("auto", (Boolean) saved.get("auto"));
        editor.commit();
    }
    /** #230: an old recording holding actions of removed features must open, skip them, restore standard readings and send nothing. */
    private void legacyRecordingWithRemovedActions() throws Exception {
        String name="legacy-removed-actions-"+System.nanoTime()+".jsonl";
        File directory=new File(getTargetContext().getFilesDir(),"recordings"); directory.mkdirs();
        File legacy=new File(directory,name);
        String[] lines={
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"demo\",\"profile\":\"ea189-ash-test\",\"selected\":[],\"test_sequence\":{\"operator_context\":\"idle\",\"status\":\"running\"}}",
            "{\"type\":\"test_sequence\",\"progress\":{\"status\":\"running\",\"step\":1}}",
            "{\"type\":\"action\",\"action\":\"ea189.ash.probe\",\"argument\":\"\",\"result\":{\"ok\":true,\"data\":{\"readings\":[{\"candidate\":\"ash_volume_1153\"}]}}}",
            "{\"type\":\"action\",\"action\":\"ea189.dpf.probe\",\"argument\":\"\",\"result\":{\"ok\":true,\"data\":{\"readings\":[{\"semantic\":\"dpf.soot_mass_calculated\",\"value\":1}]}}}",
            "{\"type\":\"action\",\"action\":\"vw.cluster.odometer\",\"argument\":\"\",\"result\":{\"ok\":true,\"data\":{\"outcome\":\"value\",\"value_km\":12345}}}",
            "{\"type\":\"action\",\"action\":\"read\",\"argument\":\"engine.rpm\",\"result\":{\"ok\":true,\"data\":{\"semantic\":\"engine.rpm\",\"response\":[65,12,12,128],\"value\":99999}}}",
            "{\"type\":\"session_end\",\"reason\":\"completed\"}"};
        java.nio.file.Files.write(legacy.toPath(),(String.join("\n",lines)+"\n").getBytes(StandardCharsets.UTF_8));
        try {
            service.openRecording(name);
            waitFor(() -> "replay".equals(service.snapshot().optString("mode")) && !service.snapshot().optBoolean("busy"),"legacy replay");
            JSONObject state=service.snapshot();
            require(!state.optBoolean("replayPartial"),"legacy recording is not partial: "+state.optString("message"));
            JSONObject rpm=findValue(state.optJSONArray("values"),"engine.rpm");
            require(rpm!=null&&rpm.optDouble("value")==800.0,"standard reading of a legacy recording is re-decoded: "+rpm);
            require(findValue(state.optJSONArray("values"),"dpf.soot_mass_calculated")==null,"removed readings are not restored");
            require(state.optJSONArray("selected").length()==0,"legacy profile restores no polling selection");
        } finally { legacy.delete(); }
        service.setDemo();
        waitFor(() -> "ready".equals(service.snapshot().optString("phase"))&&!service.snapshot().optBoolean("busy"),"return from legacy replay to demo");
    }
    private void job(String action) throws Exception {
        waitIdle(); service.runJob(action); waitIdle();
        require(service.snapshot().optJSONObject("jobResult") != null && service.snapshot().getJSONObject("jobResult").optBoolean("ok"), action + ": " + service.snapshot().opt("jobResult"));
    }
    private JSONObject findValue(JSONArray values,String semantic) {
        if(values==null)return null;
        for(int i=0;i<values.length();i++){JSONObject value=values.optJSONObject(i);if(value!=null&&semantic.equals(value.optString("semantic")))return value;}
        return null;
    }
    private void deleteRecording(String filename,String expectedMessage) throws Exception {
        service.deleteRecording(filename);
        waitFor(() -> expectedMessage.equals(service.snapshot().optString("message")),"recording deletion result: "+expectedMessage);
    }
    private boolean containsRecord(String filename) {
        JSONArray records=service.snapshot().optJSONArray("records");
        if(records==null)return false;
        for(int i=0;i<records.length();i++)if(filename.equals(records.optJSONObject(i).optString("name")))return true;
        return false;
    }
    private Button findButton(View view,String text) {
        if(view instanceof Button && text.contentEquals(((Button)view).getText())) return (Button)view;
        if(view instanceof ViewGroup) {
            ViewGroup group=(ViewGroup)view;
            for(int i=0;i<group.getChildCount();i++) { Button found=findButton(group.getChildAt(i),text); if(found!=null) return found; }
        }
        return null;
    }
    /** REQ-013.5: the selected vehicle's name is visible on every tab; the chip opens the vehicle list with a masked VIN. */
    private void showVehicles(Activity activity,String name,String listText) throws Exception {
        for(String tab:new String[]{"Live","Diagnose","Aufnahmen","Debug","Adapter"}) {
            runOnMainSync(() -> { Button button=findButton(activity.getWindow().getDecorView(),tab); require(button!=null,"Navigation tab "+tab); button.performClick(); });
            require(onMain(() -> hasText(activity.getWindow().getDecorView(),name,true)),"REQ-013.5 header shows the vehicle name on tab "+tab);
        }
        runOnMainSync(() -> { View chip=findDescribed(activity.getWindow().getDecorView(),"Gewähltes Fahrzeug: "+name); require(chip!=null,"REQ-013.5 header chip content description"); chip.performClick(); });
        require(onMain(() -> hasText(activity.getWindow().getDecorView(),"Fahrzeuge",true)),"REQ-013.5 chip opens the vehicle list");
        require(onMain(() -> hasText(activity.getWindow().getDecorView(),listText,false)),"REQ-013.5/REQ-015.2 list shows masked VIN and odometer state: "+listText);
        require(!onMain(() -> hasText(activity.getWindow().getDecorView(),"WVW00000000009999",false)),"REQ-013.5 list never shows the full VIN");
        shoot(activity,"acceptance-vehicles.png");
        runOnMainSync(() -> findButton(activity.getWindow().getDecorView(),"Live").performClick());
    }
    private <T> T onMain(java.util.concurrent.Callable<T> check) throws Exception {
        java.util.concurrent.FutureTask<T> task=new java.util.concurrent.FutureTask<>(check);
        runOnMainSync(task); return task.get();
    }
    /** Own visibility up to the content root: isShown() is false for a stopped (backgrounded) Activity's window. */
    private boolean visible(View view) {
        for(View v=view;v!=null && v.getId()!=android.R.id.content;) { if(v.getVisibility()!=View.VISIBLE) return false; v=v.getParent() instanceof View?(View)v.getParent():null; }
        return true;
    }
    private boolean hasText(View view,String text,boolean exact) {
        if(view instanceof android.widget.TextView && visible(view)) { CharSequence value=((android.widget.TextView)view).getText(); if(exact?text.contentEquals(value):value.toString().contains(text)) return true; }
        if(view instanceof ViewGroup) { ViewGroup group=(ViewGroup)view; for(int i=0;i<group.getChildCount();i++) if(hasText(group.getChildAt(i),text,exact)) return true; }
        return false;
    }
    private View findDescribed(View view,String description) {
        if(view.getContentDescription()!=null && description.contentEquals(view.getContentDescription())) return view;
        if(view instanceof ViewGroup) { ViewGroup group=(ViewGroup)view; for(int i=0;i<group.getChildCount();i++) { View found=findDescribed(group.getChildAt(i),description); if(found!=null) return found; } }
        return null;
    }
    /** REQ-022: opens {@code tab}, waits for the 500 ms refresh and returns all visible texts. */
    private String tabText(Activity activity,String tab) throws Exception {
        runOnMainSync(() -> { Button button=findButton(activity.getWindow().getDecorView(),tab); require(button!=null,"Navigation tab "+tab); button.performClick(); });
        SystemClock.sleep(1500);
        return onMain(() -> { StringBuilder all=new StringBuilder(); collectText(activity.getWindow().getDecorView(),all); return all.toString(); });
    }
    private void collectText(View view,StringBuilder out) {
        if(view instanceof android.widget.TextView && visible(view)) out.append(((android.widget.TextView)view).getText()).append('\n');
        if(view instanceof ViewGroup) { ViewGroup group=(ViewGroup)view; for(int i=0;i<group.getChildCount();i++) collectText(group.getChildAt(i),out); }
    }
    private void capture(Activity activity,String tab,String name) {
        runOnMainSync(() -> {
            View root=activity.getWindow().getDecorView(); Button button=findButton(root,tab);
            require(button!=null,"Navigation tab "+tab); button.performClick();
        });
        waitForIdleSync();
        shoot(activity,name);
    }
    private void shoot(Activity activity,String name) {
        runOnMainSync(() -> {
            View root=activity.getWindow().getDecorView();
            require(root.getWidth()>0 && root.getHeight()>0,"Laid out Activity");
            // Screen-off acceptance pauses normal window traversals; render the current tree.
            int width=root.getWidth(),height=root.getHeight();
            root.measure(View.MeasureSpec.makeMeasureSpec(width,View.MeasureSpec.EXACTLY),View.MeasureSpec.makeMeasureSpec(height,View.MeasureSpec.EXACTLY));
            root.layout(0,0,width,height);
            Bitmap bitmap=Bitmap.createBitmap(root.getWidth(),root.getHeight(),Bitmap.Config.ARGB_8888);
            root.draw(new Canvas(bitmap));
            try(java.io.FileOutputStream output=new java.io.FileOutputStream(new File(getTargetContext().getExternalFilesDir("acceptance"),name))) {
                bitmap.compress(Bitmap.CompressFormat.PNG,100,output);
            } catch(Exception e) { throw new RuntimeException(e); }
            finally { bitmap.recycle(); }
        });
    }
    private void waitIdle() throws Exception { waitFor(() -> !service.snapshot().optBoolean("busy"), "idle worker"); }
    private interface Condition { boolean ready() throws Exception; }
    private void waitFor(Condition condition,String name) throws Exception {
        long until=SystemClock.elapsedRealtime()+30000;
        while(SystemClock.elapsedRealtime()<until) { if(condition.ready()) return; SystemClock.sleep(100); }
        throw new AssertionError("Timeout: "+name+(service==null?"":"; "+service.snapshot()));
    }
    private void passed(String text) { report.append("PASS: ").append(text).append('\n'); }
    private void emitArtifact(String name) throws Exception {
        byte[] bytes=java.nio.file.Files.readAllBytes(new File(getTargetContext().getExternalFilesDir("acceptance"),name).toPath());
        String encoded=android.util.Base64.encodeToString(bytes,android.util.Base64.NO_WRAP);
        for(int offset=0;offset<encoded.length();offset+=32768) {
            Bundle chunk=new Bundle();chunk.putString("artifact",name);chunk.putInt("offset",offset);
            chunk.putString("data",encoded.substring(offset,Math.min(offset+32768,encoded.length())));
            sendStatus(10,chunk);
        }
    }
    private void require(boolean condition,String message) { if(!condition) throw new AssertionError(message); }
}
