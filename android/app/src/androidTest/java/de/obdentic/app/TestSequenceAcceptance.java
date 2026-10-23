package de.obdentic.app;

import android.os.SystemClock;
import org.json.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.*;

/** Finite-sequence evidence and replay checks through the actual service/JNI seam. */
final class TestSequenceAcceptance {
    static void run(ObdService service, File filesDir) throws Exception {
        service.setSignals(new JSONArray());
        service.startRecording(TestSequence.PROFILE);
        SystemClock.sleep(250);
        require(service.snapshot().optString("recording").isEmpty(), "Test requires an explicit operating context");
        service.startRecording(TestSequence.PROFILE,"idle");
        waitFor(() -> service.snapshot().optJSONObject("testSequence")!=null &&
            service.snapshot().optJSONObject("testSequence").optString("status").equals("completed"), "sequence completes itself");
        require(service.snapshot().optString("recording").isEmpty(), "completed sequence closes recording");
        require(service.snapshot().optJSONArray("selected").length()==0, "no generic polling after sequence");
        JSONObject sequence=service.snapshot().getJSONObject("testSequence");
        File file=new File(new File(filesDir,"recordings"),sequence.getString("file"));
        List<String> lines=Files.readAllLines(file.toPath(), StandardCharsets.UTF_8);
        JSONObject header=new JSONObject(lines.get(0));
        require(header.getJSONObject("test_sequence").getString("operator_context").equals("idle"), "reported context recorded");
        List<String> actions=new ArrayList<>(); int first=0,second=0;
        for(String line:lines) {
            JSONObject event=new JSONObject(line);
            if(event.optString("type").equals("action"))actions.add(event.getString("action"));
            if(event.optString("type").equals("wire") && event.optString("direction").equals("TX")) {
                String command=event.getString("text").trim().replace(" ","").toUpperCase(Locale.ROOT);
                if(command.equals("221153"))first++;
                if(command.equals("22178C"))second++;
                require(!command.startsWith("10")&&!command.startsWith("27")&&!command.startsWith("2E")&&!command.startsWith("31")&&!command.equals("04"), "no session/security/write/clear traffic");
            }
        }
        require(actions.equals(Arrays.asList("inventory","ea189.ash.probe","ea189.dpf.probe","read")), "exact finite action order: "+actions);
        require(first==1 && second==1, "each ash candidate sent only once");
        require(new JSONObject(lines.get(lines.size()-1)).optString("reason").equals("completed"), "completion terminal marker");
        service.startRecording(TestSequence.PROFILE,"engine_off");
        waitFor(() -> !service.snapshot().optString("recording").isEmpty(),"second sequence starts");
        require(service.snapshot().getJSONObject("ashEvidence").length()==0,"new context clears old candidate evidence");
        service.runJob("dtc.scan");
        service.stopRecording();
        waitFor(() -> service.snapshot().optString("recording").isEmpty(),"sequence manually stopped");
        require(service.snapshot().getJSONObject("testSequence").getString("status").equals("aborted"),"stop is not completion");
        service.openRecording(file.getName());
        waitFor(() -> service.snapshot().optString("mode").equals("replay")&&!service.snapshot().optBoolean("busy"),"sequence replay");
        require(service.snapshot().getJSONObject("testSequence").getString("status").equals("completed"),"replay restores sequence summary");
        require(service.snapshot().getJSONObject("ashEvidence").getJSONArray("readings").length()==2,"replay restores both raw ash candidates");
        service.disconnect();
        service.deleteRecording(file.getName());
        waitFor(() -> service.snapshot().optString("message").equals("Im Replay geöffnete Aufzeichnung kann nicht gelöscht werden"),"disconnect retains replay deletion protection");
        require(file.isFile(),"disconnect does not unprotect visible replay");
        File partial=new File(file.getParentFile(),"sequence-partial-"+System.nanoTime()+".jsonl");
        Files.write(partial.toPath(),lines.subList(0,lines.size()-1),StandardCharsets.UTF_8);
        service.openRecording(partial.getName());
        waitFor(() -> service.snapshot().optString("mode").equals("replay")&&!service.snapshot().optBoolean("busy"),"partial sequence replay");
        require(service.snapshot().optBoolean("replayPartial"),"missing terminal marker remains partial");
        require(service.snapshot().getJSONObject("testSequence").optString("status").equals("incomplete"),"partial replay never appears running or completed");
        File invalid=new File(file.getParentFile(),"sequence-invalid-"+System.nanoTime()+".jsonl");
        Files.write(invalid.toPath(),Collections.singletonList("invalid header"),StandardCharsets.UTF_8);
        service.openRecording(invalid.getName());
        waitFor(() -> !service.snapshot().optBoolean("busy")&&service.snapshot().optBoolean("replayPartial"),"invalid replay rejection");
        service.deleteRecording(invalid.getName());
        waitFor(() -> !invalid.exists(),"invalid replay file is deletable");
        service.setDemo();
        waitFor(() -> service.snapshot().optString("phase").equals("ready")&&!service.snapshot().optBoolean("busy"), "return to demo");
        service.deleteRecording(partial.getName());
    }
    private interface Check { boolean ok() throws Exception; }
    private static void waitFor(Check check,String name) throws Exception {
        long until=SystemClock.elapsedRealtime()+45000;
        while(SystemClock.elapsedRealtime()<until) { if(check.ok())return; SystemClock.sleep(50); }
        throw new AssertionError(name+" timed out");
    }
    private static void require(boolean value,String message) { if(!value)throw new AssertionError(message); }
}
