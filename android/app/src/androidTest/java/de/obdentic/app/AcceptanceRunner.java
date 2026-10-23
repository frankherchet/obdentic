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
    private final StringBuilder report = new StringBuilder();
    @Override public void onCreate(Bundle arguments) { super.onCreate(arguments); start(); }
    @Override public void onStart() {
        Bundle result = new Bundle();
        try {
            Intent launch = new Intent(getTargetContext(), MainActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            Activity activity = startActivitySync(launch);
            waitFor(() -> ObdService.current != null, "service creation");
            service = ObdService.current;
            runOnMainSync(service::setDemo);
            waitFor(() -> "ready".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"), "demo initialization");
            JSONObject state = service.snapshot();
            require("demo".equals(state.optString("mode")), "Demo must be visibly separate from live");
            require(state.getJSONArray("signals").length() >= 20, "Full semantic catalog");
            require(state.getJSONArray("profiles").length() >= 4, "Capture profiles and DPF profile");
            passed("Rust JNI library, real initialization/parser, signal and profile catalogs");

            service.setSignals(new JSONArray().put("engine.rpm").put("engine.coolant_temperature"));
            waitFor(() -> service.snapshot().optJSONArray("values").length() >= 2, "live scalar reads");
            boolean numeric = false;
            JSONArray values = service.snapshot().getJSONArray("values");
            for (int i=0; i<values.length(); i++) if ("engine.rpm".equals(values.getJSONObject(i).optString("semantic"))) {
                numeric = values.getJSONObject(i).optDouble("value", -1) > 0;
            }
            require(numeric, "RPM decoded from raw adapter response");
            capture(activity,"Live","acceptance-live.png");
            service.setSignals(new JSONArray());
            waitIdle();
            passed("Live RPM/coolant readings and changing observation selection");

            job("inventory");
            require(service.snapshot().optJSONObject("vehicle").optBoolean("engine_target_validated"), "Evidenced engine target");
            job("dtc.scan");
            require(service.snapshot().getJSONObject("diagnostics").toString().contains("P0133"), "Responder-scoped DTC decoding");
            job("ea189.dpf.probe");
            require(service.snapshot().getJSONObject("jobResult").optBoolean("ok"), "Seven closed DPF reads");
            JSONArray dpf = service.snapshot().getJSONObject("jobResult").getJSONObject("data").getJSONArray("readings");
            require(dpf.length()==7,"All seven DPF readings retained");
            for(int i=0;i<dpf.length();i++) require(dpf.getJSONObject(i).has("value") && dpf.getJSONObject(i).optBoolean("experimental"),"Demo DPF decoder succeeds and stays experimental");
            capture(activity,"Diagnose","acceptance-diagnose.png");
            capture(activity,"Debug","acceptance-debug.png");
            capture(activity,"Adapter","acceptance-connection.png");
            passed("Inventory, stored-DTC and bounded experimental DPF jobs");

            service.startRecording("engine-drive");
            waitFor(() -> !service.snapshot().optString("recording").isEmpty(), "recording start");
            String filename = service.snapshot().getString("recording");
            File file = new File(new File(getTargetContext().getFilesDir(), "recordings"), filename);
            waitFor(() -> file.length() > 500, "recording evidence");
            long initial = file.length();
            getUiAutomation().executeShellCommand("input keyevent 3").close();
            getUiAutomation().executeShellCommand("input keyevent 223").close();
            SystemClock.sleep(4000);
            require(filename.equals(service.snapshot().optString("recording")), "Screen-off recording remains active");
            require(file.length() > initial, "Background recording appends actual evidence");
            getUiAutomation().executeShellCommand("input keyevent 224").close();
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
            passed("Profile recording, actual screen-off/background operation, explicit stop and flushed TX/RX evidence");

            Uri uri = Uri.parse("content://de.obdentic.app.files/recordings/" + filename);
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
            passed("Disconnect and offline replay without a live connection");

            long core=NativeCore.create((command,timeout)->{throw new AssertionError("Rejected/offline actions must never send traffic");});
            try {
                for(String action:new String[]{"dtc.clear","write","ATZ","uds.send"})
                    require(!new JSONObject(NativeCore.execute(core,action,"")).optBoolean("ok"),"Reject " + action);
                JSONObject stored = new JSONObject().put("semantic","engine.rpm").put("value",999999)
                    .put("response",new JSONArray().put(0x41).put(0x0c).put(0x0c).put(0x80));
                JSONObject replay = new JSONObject(NativeCore.execute(core,"replay.read",stored.toString()));
                require(replay.optBoolean("ok") && replay.getJSONObject("data").getDouble("value")==800.0,"Replay re-decodes raw bytes");
            } finally { NativeCore.destroy(core); }
            passed("Mutation/raw-command rejection and deterministic offline re-decoding");

            service.setDemo();
            waitFor(() -> "ready".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"),"reconnect demo");
            runOnMainSync(activity::recreate);
            SystemClock.sleep(1500);
            require("ready".equals(service.snapshot().optString("phase")),"Activity recreation preserves session");
            passed("Session reconnect and Activity recreation");
            service.disconnect();
            runOnMainSync(service::scan);
            waitFor(() -> !"scanning".equals(service.snapshot().optString("phase")),"real Android BLE scan completion");
            require("idle".equals(service.snapshot().optString("phase")),"BLE scan terminates without an Android error");
            passed("Actual Android BLE scan lifecycle (adapter connection remains a separate vehicle test)");
            runOnMainSync(service::setDemo);
            waitFor(() -> "ready".equals(service.snapshot().optString("phase")) && !service.snapshot().optBoolean("busy"),"restore demo for UI review");
            for(String name:new String[]{"acceptance-live.png","acceptance-diagnose.png","acceptance-debug.png","acceptance-connection.png","acceptance-recordings.png","demo-recording.jsonl"}) emitArtifact(name);
            result.putString("stream", report + "\nDESK ACCEPTANCE PASSED\nReal Carly/vehicle acceptance remains separate.\n");
            finish(Activity.RESULT_OK,result);
        } catch(Throwable error) {
            result.putString("stream",report + "\nFAILED: " + error + "\n" + android.util.Log.getStackTraceString(error));
            finish(Activity.RESULT_CANCELED,result);
        }
    }
    private void job(String action) throws Exception {
        waitIdle(); service.runJob(action); waitIdle();
        require(service.snapshot().optJSONObject("jobResult") != null && service.snapshot().getJSONObject("jobResult").optBoolean("ok"), action + ": " + service.snapshot().opt("jobResult"));
    }
    private Button findButton(View view,String text) {
        if(view instanceof Button && text.contentEquals(((Button)view).getText())) return (Button)view;
        if(view instanceof ViewGroup) {
            ViewGroup group=(ViewGroup)view;
            for(int i=0;i<group.getChildCount();i++) { Button found=findButton(group.getChildAt(i),text); if(found!=null) return found; }
        }
        return null;
    }
    private void capture(Activity activity,String tab,String name) {
        runOnMainSync(() -> {
            View root=activity.getWindow().getDecorView(); Button button=findButton(root,tab);
            require(button!=null,"Navigation tab "+tab); button.performClick();
        });
        waitForIdleSync();
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
