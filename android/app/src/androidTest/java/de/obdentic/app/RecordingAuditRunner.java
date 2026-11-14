package de.obdentic.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.os.Bundle;
import android.util.Base64;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

/** Read-only offline audit of existing app recordings; never starts a service. */
public final class RecordingAuditRunner extends Instrumentation {
    @Override public void onCreate(Bundle arguments) { super.onCreate(arguments); start(); }
    @Override public void onStart() {
        Bundle result=new Bundle(); long core=0;
        try {
            core=NativeCore.create((command,timeout)->{throw new AssertionError("Offline audit cannot send traffic");});
            JSONArray reports=new JSONArray();
            File[] files=new File(getTargetContext().getFilesDir(),"recordings").listFiles();
            if(files!=null) for(File file:files) if(file.isFile()&&file.getName().endsWith(".jsonl")) {
                JSONObject report=new JSONObject();report.put("name",file.getName());report.put("bytes",file.length());
                int tx=0,rx=0,reads=0,failures=0,decodeErrors=0,mismatches=0,jobs=0,smartSets=0,smartMalformed=0,smartOrderErrors=0,componentApplies=0,componentMalformed=0;boolean complete=false,header=false,invalid=false,smartSnapshot=false,componentSnapshot=false;String mode="unknown",reason="",profile="";long lastSmartOffset=0;
                if(file.length()>64L*1024*1024) {report.put("error","Inspection limit exceeded");reports.put(report);continue;}
                try(BufferedReader input=new BufferedReader(new InputStreamReader(new FileInputStream(file),StandardCharsets.UTF_8))) {
                    String line;int count=0;
                    while((line=input.readLine())!=null) {
                        if(++count>200000 || line.length()>1048576) throw new IOException("Inspection limit exceeded");
                        if(complete) {invalid=true;break;}
                        JSONObject event=new JSONObject(line);String type=event.optString("type");
                        if(!header) {if(!type.equals("session")||!event.optString("schema").equals("OBDENTIC-MOBILE")||event.optInt("version")!=1)throw new IOException("Unknown header");header=true;mode=event.optString("mode","unknown");profile=event.optString("profile");if(event.has("smart")&&!event.isNull("smart")) {if(event.optJSONObject("smart")!=null)smartSnapshot=true;else smartMalformed++;}if(event.has("component")&&!event.isNull("component")) {if(event.optJSONObject("component")!=null)componentSnapshot=true;else componentMalformed++;}continue;}
                        if(type.equals("session_end")) {complete=true;reason=event.optString("reason");}
                        if(type.equals("wire")) {if(event.optString("direction").equals("TX"))tx++;if(event.optString("direction").equals("RX"))rx++;}
                        if(type.equals("action")) {
                            String action=event.optString("action");JSONObject envelope=event.optJSONObject("result");if(envelope==null) {invalid=true;continue;}
                            if(!envelope.optBoolean("ok")) {failures++;continue;}
                            if(action.equals("smart.set")) {
                                // REQ-036.5: recorded evidence only; counted and shape-checked, never replayed through NativeCore, never logged.
                                JSONObject data=envelope.optJSONObject("data");String change=data==null?null:data.optString("change",null);
                                if(change==null||change.isEmpty()||!(change.equals("start")||change.equals("change")||change.equals("resume")||change.equals("availability")||change.equals("stop"))) {smartMalformed++;continue;}
                                Object offsetValue=event.opt("offset_us");long offset=(offsetValue instanceof Integer||offsetValue instanceof Long)?((Number)offsetValue).longValue():-1;
                                if(offset<0||offset<lastSmartOffset) {smartOrderErrors++;continue;}
                                lastSmartOffset=offset;smartSets++;
                                continue;
                            }
                            if(action.equals("component.apply")) {
                                // REQ-035.4: recorded snapshot evidence only; counted and shape-checked, never replayed through NativeCore, never logged.
                                if(envelope.optJSONObject("data")!=null)componentApplies++;else componentMalformed++;
                                continue;
                            }
                            if(action.equals("read")||action.equals("dtc.scan")) {
                                JSONObject data=envelope.optJSONObject("data");JSONObject decoded=new JSONObject(NativeCore.execute(core,"replay."+action,data==null?"{}":data.toString()));
                                if(!decoded.optBoolean("ok"))decodeErrors++;
                                else if(action.equals("read")) {reads++;if(data==null||!Double.isFinite(data.optDouble("value",Double.NaN))||Math.abs(data.optDouble("value")-decoded.getJSONObject("data").optDouble("value"))>1e-9)mismatches++;}
                                else jobs++;
                            }
                        }
                    }
                } catch(Exception error) {invalid=true;report.put("error",error.getClass().getSimpleName());}
                report.put("mode",mode);report.put("header",header);report.put("complete",complete&&!invalid);report.put("invalid",invalid);report.put("end_reason",reason);
                report.put("tx",tx);report.put("rx",rx);report.put("decoded_reads",reads);report.put("recorded_errors",failures);report.put("decode_errors",decodeErrors);report.put("value_mismatches",mismatches);report.put("diagnostic_jobs",jobs);
                report.put("smart_snapshot",smartSnapshot);report.put("smart_sets",smartSets);report.put("smart_malformed",smartMalformed);report.put("smart_order_errors",smartOrderErrors);
                report.put("component_snapshot",componentSnapshot);report.put("component_applies",componentApplies);report.put("component_malformed",componentMalformed);
                reports.put(report);
                if(mode.equals("live"))emit(file);
            }
            result.putString("stream",reports.toString(2));finish(Activity.RESULT_OK,result);
        } catch(Throwable error) {result.putString("stream","AUDIT FAILED: "+error.getClass().getSimpleName());finish(Activity.RESULT_CANCELED,result);}
        finally {if(core!=0)NativeCore.destroy(core);}
    }
    private void emit(File file) throws Exception {
        String data=Base64.encodeToString(Files.readAllBytes(file.toPath()),Base64.NO_WRAP);
        for(int offset=0;offset<data.length();offset+=32768) {Bundle part=new Bundle();part.putString("artifact",file.getName());part.putInt("offset",offset);part.putString("data",data.substring(offset,Math.min(offset+32768,data.length())));sendStatus(10,part);}
    }
}
