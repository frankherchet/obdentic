package de.obdentic.app;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.app.Dialog;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.Gravity;
import android.view.View;
import android.view.Window;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;
import android.text.Editable;
import android.text.TextWatcher;
import org.json.JSONArray;
import org.json.JSONObject;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.LinkedHashMap;
import java.util.Map;

/** Local-only diagnostic workbench. Transport and jobs belong to ObdService. */
public class MainActivity extends Activity {
    private static final int BG=0xff0c121b, CARD=0xff17212e, TEXT=0xffedf3fa, MUTED=0xffa5b5c7, ACCENT=0xff63dfc1;
    /** The visible instance, for the phone acceptance (cleared on pause). */
    public static volatile MainActivity resumed;
    /** The last text sent to accessibility services (REQ-021.5). */
    public volatile String lastAnnouncement="";
    /**
     * REQ-021 action -> how its end is observed in the service snapshot (any service message after the tap, counted by messageSeq, also ends it:
     * that is the error or refusal text). Limits: connect 30 s, search 15 s, jobs and export 60 s, rest 10 s.
     *   rec.start  recording non-empty            rec.stop   recording empty
     *   scan       phase left "scanning" after its message
     *   connect    phase ready/error, or refusal  disconnect message "Getrennt"
     *   job        busy false after its message   export.all message with the result
     *   auto.export autoExport flipped            auto.connect autoConnect flipped      auto.stop autoPending false
     *   vehicle.*  message with the result
     */
    private static boolean msg(JSONObject now,JSONObject base){return now.optLong("messageSeq")!=base.optLong("messageSeq");}
    private static boolean recording(JSONObject s){return !s.optString("recording").isEmpty();}
    private static boolean udsActive(JSONObject s){JSONObject uds=s.optJSONObject("uds");return uds!=null&&uds.optBoolean("active");}
    private static final PendingActions.Spec
        REC_START=new PendingActions.Spec("rec.start","Aufzeichnung starten","Aufzeichnung wird gestartet …",10000,(n,b)->recording(n)||msg(n,b)),
        REC_STOP=new PendingActions.Spec("rec.stop","Aufzeichnung stoppen","Aufzeichnung wird gestoppt …",10000,(n,b)->!recording(n)||msg(n,b)),
        SCAN=new PendingActions.Spec("scan","Adapter suchen","Suche läuft …",15000,(n,b)->!"scanning".equals(n.optString("phase"))&&msg(n,b)),
        CONNECT=new PendingActions.Spec("connect","Verbinden","Verbinde …",30000,(n,b)->{String p=n.optString("phase");return p.equals("ready")||p.equals("error")||(msg(n,b)&&!n.optBoolean("busy")&&!p.equals("connecting"));}),
        DISCONNECT=new PendingActions.Spec("disconnect","Verbindung trennen","Trenne …",10000,MainActivity::msg),
        AUTO_CONNECT=new PendingActions.Spec("auto.connect","Automatisch verbinden","Ändere automatische Verbindung …",10000,(n,b)->n.optBoolean("autoConnect")!=b.optBoolean("autoConnect")||msg(n,b)),
        AUTO_STOP=new PendingActions.Spec("auto.stop","Automatische Verbindung stoppen","Stoppe automatische Verbindung …",10000,(n,b)->!n.optBoolean("autoPending")||msg(n,b)),
        AUTO_RECORD=new PendingActions.Spec("auto.record","Dauer-Aufzeichnung","Ändere Dauer-Aufzeichnung …",10000,(n,b)->n.optBoolean("autoRecord")!=b.optBoolean("autoRecord")),
        AUTO_RECORD_DELETE=new PendingActions.Spec("auto.record.delete","Automatische Aufnahmen löschen","Lösche automatische Aufnahmen …",60000,MainActivity::msg),
        AUTO_EXPORT=new PendingActions.Spec("auto.export","Automatischer Export","Ändere automatischen Export …",10000,(n,b)->n.optBoolean("autoExport")!=b.optBoolean("autoExport")),
        EXPORT_ALL=new PendingActions.Spec("export.all","Alle exportieren","Exportiere …",60000,MainActivity::msg),
        UDS_ENABLE=new PendingActions.Spec("uds.enable","UDS-Sitzung aktivieren","UDS-Sitzung wird aktiviert …",10000,(n,b)->udsActive(n)!=udsActive(b)||msg(n,b)),
        UDS_DISABLE=new PendingActions.Spec("uds.disable","UDS-Sitzung beenden","Standard-Sitzung wird wiederhergestellt …",10000,(n,b)->udsActive(n)!=udsActive(b)||msg(n,b)),
        VEHICLE_SELECT=new PendingActions.Spec("vehicle.select","Fahrzeug auswählen","Wähle Fahrzeug …",10000,MainActivity::msg),
        VEHICLE_RENAME=new PendingActions.Spec("vehicle.rename","Fahrzeug umbenennen","Benenne um …",10000,MainActivity::msg),
        VEHICLE_DELETE=new PendingActions.Spec("vehicle.delete","Fahrzeug löschen","Lösche Fahrzeug …",10000,MainActivity::msg);
    private static PendingActions.Spec jobSpec(String name){return new PendingActions.Spec("job",name,"Auftrag läuft …",60000,(n,b)->!n.optBoolean("busy")&&msg(n,b));}
    private final Handler handler=new Handler(Looper.getMainLooper());
    private final Map<String,TextView> valueViews=new HashMap<>();
    private final Map<String,TextView> freshnessViews=new HashMap<>();
    private final Map<String,SparklineView> graphViews=new HashMap<>();
    private final Map<String,TextView> rangeViews=new HashMap<>();
    private final Map<String,ArrayList<Float>> histories=new HashMap<>();
    private final Map<String,String> lastSamples=new HashMap<>();
    private JSONObject state=new JSONObject();
    private LinearLayout body, nav;
    private ScrollView scroll;
    private TextView badge, status, udsIndicator, udsRemaining;
    public volatile Dialog activeSignalSelector;
    private Button chip;
    private TextView debugLog;
    private int tab=0;
    private String structure="", profile="";
    private String historyMode="";
    private boolean launchDemo=false, scanAfterPermission=false;
    private String pendingExport="";
    private final Runnable poll=new Runnable(){public void run(){refresh();handler.postDelayed(this,500);}};

    @Override public void onCreate(Bundle saved){AppLog.init(this);
        setTheme(android.R.style.Theme_Material_NoActionBar);
        super.onCreate(saved);
        if(saved!=null){tab=saved.getInt("tab",0);profile=saved.getString("profile","");pendingExport=saved.getString("pendingExport","");}
        launchDemo="demo".equals(getIntent().getStringExtra("mode"));
        getWindow().setStatusBarColor(BG);getWindow().setNavigationBarColor(BG);
        LinearLayout root=new LinearLayout(this);root.setOrientation(LinearLayout.VERTICAL);root.setBackgroundColor(BG);
        root.setOnApplyWindowInsetsListener((v,insets)->{v.setPadding(insets.getSystemWindowInsetLeft(),insets.getSystemWindowInsetTop(),insets.getSystemWindowInsetRight(),insets.getSystemWindowInsetBottom());return insets;});
        setContentView(root);
        LinearLayout heading=row();heading.setPadding(dp(20),dp(12),dp(20),dp(6));
        TextView name=text("OBDentic",27,TEXT);name.setTypeface(null,Typeface.BOLD);heading.addView(name,new LinearLayout.LayoutParams(0,-2,1));
        udsIndicator=text("UDS erweiterte Sitzung aktiv",11,ACCENT);udsIndicator.setGravity(Gravity.CENTER);udsIndicator.setMaxWidth(dp(128));udsIndicator.setMaxLines(3);udsIndicator.setPadding(dp(7),dp(6),dp(7),dp(6));udsIndicator.setBackground(surface(0xff203f3a,10));udsIndicator.setVisibility(View.GONE);heading.addView(udsIndicator);
        badge=text("STARTET",12,ACCENT);badge.setTypeface(null,Typeface.BOLD);badge.setGravity(Gravity.CENTER);badge.setMaxLines(2);badge.setMaxWidth(dp(220));badge.setPadding(dp(12),dp(8),dp(12),dp(8));badge.setBackground(surface(0xff203f3a,10));heading.addView(badge);root.addView(heading);
        chip=button("Kein Fahrzeug gewählt",()->{tab=5;structure="";scroll.scrollTo(0,0);buildNav();refresh();});chip.setSingleLine(true);chip.setEllipsize(android.text.TextUtils.TruncateAt.END);
        LinearLayout.LayoutParams chipParams=new LinearLayout.LayoutParams(-1,-2);chipParams.setMargins(dp(20),0,dp(20),dp(2));root.addView(chip,chipParams);
        status=text("Diagnose wird vorbereitet …",14,MUTED);status.setMaxLines(3);status.setEllipsize(android.text.TextUtils.TruncateAt.END);status.setPadding(dp(20),dp(2),dp(20),dp(12));root.addView(status);
        scroll=new ScrollView(this);scroll.setFillViewport(true);body=new LinearLayout(this);body.setOrientation(LinearLayout.VERTICAL);body.setPadding(dp(16),dp(4),dp(16),dp(20));scroll.addView(body);root.addView(scroll,new LinearLayout.LayoutParams(-1,0,1));
        nav=row();nav.setPadding(dp(4),dp(6),dp(4),dp(6));root.addView(nav);buildNav();
        // Starting from the visible Activity is allowed. The service promotes itself
        // once a real Bluetooth connection has the required runtime permissions.
        startService(new Intent(this,ObdService.class));
    }
    @Override protected void onStart(){super.onStart();getWindow().addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);}
    @Override protected void onStop(){getWindow().clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);super.onStop();}
    @Override protected void onResume(){super.onResume();resumed=this;handler.post(poll);if(service()!=null)service().resumeIfBlocked();}
    @Override protected void onPause(){handler.removeCallbacks(poll);if(resumed==this)resumed=null;super.onPause();}
    @Override protected void onSaveInstanceState(Bundle out){out.putInt("tab",tab);out.putString("profile",profile);out.putString("pendingExport",pendingExport);super.onSaveInstanceState(out);}
    private ObdService service(){return ObdService.current;}
    private void act(Runnable action){try{if(service()!=null)action.run();else toast("Dienst startet. Bitte kurz warten.");}catch(Exception e){toast(e.getMessage()==null?"Aktion fehlgeschlagen":e.getMessage());}refresh();}
    /** REQ-021.1/.4: starts the pending state, then the service call; a second tap while pending starts nothing. */
    private void perform(PendingActions.Spec spec,String suffix,Runnable call){
        ObdService s=service();if(s==null){toast("Dienst startet. Bitte kurz warten.");return;}
        if(!s.pending.start(spec,suffix,s.snapshot(),SystemClock.elapsedRealtime()))return;
        announce(spec.pending);
        try{call.run();}catch(Exception e){s.pending.resolve(spec.key+suffix);toast(e.getMessage()==null?"Aktion fehlgeschlagen":e.getMessage());}
        refresh();
    }
    /** REQ-021.1/.5: while pending the button is disabled, shows the pending text (also as content description) and a progress indicator. */
    private Button actionButton(LinearLayout parent,PendingActions.Spec spec,String suffix,String idle,boolean enabled,Runnable onTap){
        boolean pend=service()!=null&&service().pending.isPending(spec.key+suffix);
        Button b=button(pend?spec.pending:idle,onTap);b.setEnabled(enabled&&!pend);
        LinearLayout.LayoutParams p=new LinearLayout.LayoutParams(-1,-2);p.setMargins(0,dp(5),0,dp(3));
        if(!pend){parent.addView(b,p);return b;}
        b.setContentDescription(spec.pending);
        LinearLayout r=row();ProgressBar bar=new ProgressBar(this,null,android.R.attr.progressBarStyleSmall);bar.setIndeterminate(true);bar.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
        r.addView(bar,new LinearLayout.LayoutParams(dp(28),dp(28)));r.addView(b,new LinearLayout.LayoutParams(0,-2,1));parent.addView(r,p);return b;
    }
    private void announce(String text){lastAnnouncement=text;if(status!=null)status.announceForAccessibility(text);}
    private void refresh(){
        if(service()==null)return;
        if(launchDemo){launchDemo=false;service().setDemo();}
        state=service().snapshot();
        PendingActions pending=service().pending;long seq=state.optLong("messageSeq");
        for(PendingActions.Entry e:pending.observe(state))announce(msg(state,e.baseline)?state.optString("message"):e.name+" abgeschlossen"); // REQ-021.2/.5
        for(PendingActions.Entry e:pending.expire(SystemClock.elapsedRealtime())){String note="Keine Rückmeldung · "+e.name;pending.setNotice(note,seq);announce(note);} // REQ-021.3
        JSONObject chosen=selectedVehicle();String chipName=chosen==null?"Kein Fahrzeug gewählt":chosen.optString("name");
        chip.setText(chipName);chip.setContentDescription("Gewähltes Fahrzeug: "+(chosen==null?"keines":chipName));
        String mode=state.optString("mode","live"), phase=state.optString("phase","idle");
        String replayBadge="REPLAY · "+(state.optString("replaySourceMode").equals("demo")?"DEMO":"DATEI");
        if(mode.equals("replay")&&state.optBoolean("replayPartial"))replayBadge+="\nTEILAUFNAHME";
        badge.setText(mode.equals("demo")?"DEMO · SIMULIERT":mode.equals("replay")?replayBadge:phase.equals("ready")?"LIVE · ADAPTER":"LIVE · GETRENNT");
        int tint=mode.equals("live")?ACCENT:0xffffca75;badge.setTextColor(tint);
        boolean extended=udsActive(state);udsIndicator.setText(extended?"UDS erweiterte Sitzung aktiv\n(OBD2 pausiert)":"UDS erweiterte Sitzung aktiv");udsIndicator.setVisibility(extended?View.VISIBLE:View.GONE);udsIndicator.setContentDescription(extended?"UDS erweiterte Sitzung aktiv, OBD2 pausiert":"UDS Standardsitzung");
        String message=pending.notice(seq);if(message.isEmpty())message=state.optString("message","");
        status.setText(phaseName(phase)+(state.optBoolean("busy")?" · Auftrag läuft":"")+(message.isEmpty()?"":"\n"+message));
        status.setTextColor(phase.equals("error")?0xffffa79c:MUTED);
        if(!historyMode.equals(mode)){historyMode=mode;histories.clear();lastSamples.clear();}
        collectHistory();
        String key=pending.signature()+"|"+tab+"|"+mode+"|"+phase+"|"+state.optString("recording")+"|"+state.optString("autoRecording")+state.optBoolean("autoRecord")+"|"+array("selected").toString()+"|"+state.opt("vehicles")+"|"+udsStructureKey();
        if(tab==1||tab==2||tab==4)key+="|"+state.optBoolean("busy");
        if(tab==1)key+="|"+state.opt("diagnostics")+"|"+state.opt("jobResult")+"|"+state.opt("vehicle")+"|"+state.opt("throughput");
        if(tab==2)key+="|"+array("records")+"|"+RecordingExporter.folder(this)+"|"+RecordingExporter.auto(this)+"|"+array("profiles")+"|"+profile+"|"+state.optString("replayFile");
        if(tab==3)key+="|"+(array("debug").length()==0);
        if(tab==4)key+="|"+array("devices").toString()+"|"+state.optBoolean("autoConnect")+"|"+state.optBoolean("autoPending")+"|"+state.optString("knownAdapter");
        if(!key.equals(structure)){structure=key;int y=scroll.getScrollY();render();scroll.post(()->scroll.scrollTo(0,y));}
        updateValues();
        updateUdsStatus();
        if(debugLog!=null){String content=debugText();if(!content.contentEquals(debugLog.getText()))debugLog.setText(content);}
    }
    private String phaseName(String p){switch(p){case "ready":return "Verbunden · bereit";case "scanning":return "Adapter werden gesucht …";case "connecting":return "Verbindung wird aufgebaut …";case "disconnected":return "Getrennt";case "error":return "Verbindungsfehler";case "replay":return "Gespeicherte Sitzung";default:return "Bereit zum Verbinden";}}
    private String udsStructureKey(){JSONObject uds=state.optJSONObject("uds");return uds==null?"":uds.optString("state")+"|"+uds.optBoolean("active")+"|"+uds.optString("error")+"|"+uds.optString("target",uds.optString("target_id"));}
    private void updateUdsStatus(){if(udsIndicator==null)return;boolean active=udsActive(state);udsIndicator.setText(active?"UDS erweiterte Sitzung aktiv\n(OBD2 pausiert)":"UDS erweiterte Sitzung aktiv");udsIndicator.setVisibility(active?View.VISIBLE:View.GONE);udsIndicator.setContentDescription(active?"UDS erweiterte Sitzung aktiv, OBD2 pausiert":"UDS Standardsitzung");if(udsRemaining!=null)udsRemaining.setText(udsStatusText());}
    private String udsStatusText(){JSONObject uds=state.optJSONObject("uds");if(udsActive(state)){long seconds=uds==null?0:uds.optLong("remaining_s",uds.optLong("remaining_ms")/1000);String target=uds==null?"":uds.optString("target",uds.optString("target_id"));return "UDS erweiterte Sitzung aktiv · OBD2 pausiert"+(target.isEmpty()?"":" · "+target)+(seconds>0?" · noch etwa "+seconds+" s":"");}String error=uds==null?"":uds.optString("error");if(error.isEmpty()&&uds!=null&&"indeterminate".equals(uds.optString("state")))error="Sitzungsstatus unklar";return error.isEmpty()?udsEnableBlocker():"Erweiterte Sitzung nicht aktiv · "+error;}
    private String udsEnableBlocker(){if(!state.optString("mode","live").equals("live"))return "Nur im Livebetrieb verfügbar";if(!state.optString("phase").equals("ready"))return "Erst eine bereite Live-Verbindung herstellen";if(!state.optString("recording").isEmpty())return "Während einer Aufzeichnung nicht aktivierbar";JSONObject uds=state.optJSONObject("uds");if(uds!=null&&"indeterminate".equals(uds.optString("state")))return "UDS-Sitzungsstatus unklar · Adapter neu verbinden";if(state.optBoolean("busy"))return "Ein Diagnoseauftrag läuft";if(!identifiedEngineResponded())return "Erst ein identifiziertes Motorsteuergerät erkennen";if(!udsCandidatesExpected())return "Keine passenden session-markierten Knowledge-Messwerte verfügbar";return "";}
    private boolean identifiedEngineResponded(){JSONObject result=state.optJSONObject("ecuDiscovery"),record=selectedVehicle();if((result==null||result.length()==0)&&record!=null)result=record.optJSONObject("ecu_discovery");JSONArray targets=result==null?null:result.optJSONArray("targets");for(int i=0;targets!=null&&i<targets.length();i++){JSONObject target=targets.optJSONObject(i);if(target==null||!"present".equals(target.optString("status")))continue;String role=target.optString("role",target.optString("logical_role"));boolean engine=role.toLowerCase(java.util.Locale.ROOT).contains("engine")||role.toLowerCase(java.util.Locale.ROOT).contains("motor"),identified=false;JSONArray identity=target.optJSONArray("identity");if(identity==null)identity=target.optJSONArray("identification");for(int j=0;identity!=null&&j<identity.length();j++){JSONObject item=identity.optJSONObject(j);if(item==null)continue;String semantic=item.optString("semantic"),value=item.optString("value");if(semantic.contains("logical_role")&&(value.toLowerCase(java.util.Locale.ROOT).contains("engine")||value.toLowerCase(java.util.Locale.ROOT).contains("motor")))engine=true;if(!value.isEmpty()||positiveUdsResponse(item.opt("response")))identified=true;}if(engine&&identified)return true;}return false;}
    private static boolean positiveUdsResponse(Object response){if(response instanceof JSONArray){JSONArray bytes=(JSONArray)response;return bytes.length()>0&&bytes.optInt(0,-1)==0x62;}if(response instanceof String)return ((String)response).trim().toLowerCase(java.util.Locale.ROOT).startsWith("62");return false;}
    private boolean udsCandidatesExpected(){if(state.optInt("udsCandidates",0)>0)return true;JSONArray candidates=state.optJSONArray("udsCandidates");if(candidates!=null&&candidates.length()>0)return true;for(int i=0;i<array("signals").length();i++){JSONObject signal=array("signals").optJSONObject(i);if(signal==null)continue;String protocol=signalField(signal,"protocol").toLowerCase(java.util.Locale.ROOT),session=signalField(signal,"session_requirement").toLowerCase(java.util.Locale.ROOT);if(protocol.equals("uds")||signal.optBoolean("session_required")||session.contains("extended")||session.contains("required"))return true;}return false;}
    private void buildNav(){nav.removeAllViews();nav.setWeightSum(5);String[] titles={"Live","Diagnose","Aufnahmen","Debug","Adapter"};for(int i=0;i<titles.length;i++){final int next=i;Button b=button(titles[i],()->{tab=next;structure="";scroll.scrollTo(0,0);buildNav();refresh();});b.setTextSize(12);b.setMinWidth(0);b.setMinimumWidth(0);b.setPadding(dp(2),dp(4),dp(2),dp(4));b.setTextColor(tab==i?BG:MUTED);b.setBackground(surface(tab==i?ACCENT:CARD,12));b.setBackgroundTintList(null);LinearLayout.LayoutParams p=new LinearLayout.LayoutParams(0,dp(48),1);p.setMargins(dp(2),0,dp(2),0);nav.addView(b,p);}}
    private void render(){body.removeAllViews();valueViews.clear();freshnessViews.clear();graphViews.clear();rangeViews.clear();debugLog=null;udsRemaining=null;switch(tab){case 0:live();break;case 1:diagnose();break;case 2:recordings();break;case 3:debug();break;case 5:vehicles();break;default:connection();}}
    private void live(){
        title("Live-Messwerte","Werte und Verlauf auf einen Blick");
        addButton(body,"Messwerte auswählen",this::chooseSignals).setEnabled(state.optString("recording").isEmpty());
        JSONArray selected=array("selected");
        boolean canRecordSelection=!state.optString("mode","live").equals("replay")&&state.optString("phase").equals("ready")&&!state.optBoolean("busy")&&state.optString("recording").isEmpty()&&selected.length()>0;
        actionButton(body,REC_START,":selection","Gewählte Messwerte aufzeichnen",canRecordSelection,()->perform(REC_START,":selection",()->service().startRecording("live-selection")));
        if(!state.optString("recording").isEmpty())note(body,"● Aufzeichnung läuft · "+state.optString("recording"),ACCENT);
        JSONArray values=array("values");
        LinkedHashMap<String,JSONObject> shown=new LinkedHashMap<>();
        for(int i=0;i<selected.length();i++){String s=selected.optString(i);shown.put(s,signal(s));}
        for(int i=0;i<values.length();i++){JSONObject v=values.optJSONObject(i);if(v!=null&&selected.length()==0)shown.put(v.optString("semantic"),v);}
        if(shown.isEmpty())empty("Noch keine Messwerte","Wähle Messwerte aus und verbinde deinen Adapter. Mit „Demo starten“ unter Adapter kannst du die Oberfläche ohne Auto testen.");
        int columns=getResources().getConfiguration().orientation==android.content.res.Configuration.ORIENTATION_LANDSCAPE?3:2,col=0;LinearLayout row=null;
        for(Map.Entry<String,JSONObject> entry:shown.entrySet()){
            String id=entry.getKey();JSONObject s=entry.getValue();
            if(col==0){row=new LinearLayout(this);row.setOrientation(LinearLayout.HORIZONTAL);LinearLayout.LayoutParams rp=new LinearLayout.LayoutParams(-1,-2);rp.setMargins(0,dp(6),0,0);body.addView(row,rp);}
            LinearLayout tile=new LinearLayout(this);tile.setOrientation(LinearLayout.VERTICAL);tile.setPadding(dp(10),dp(10),dp(10),dp(10));tile.setBackground(surface(CARD,20));tile.setTag("live-tile");
            LinearLayout.LayoutParams tp=new LinearLayout.LayoutParams(0,-2,1);tp.setMargins(col==0?0:dp(3),0,col==columns-1?0:dp(3),0);row.addView(tile,tp);
            TextView name=text(signalLabel(id,s),13,MUTED);name.setSingleLine(true);name.setEllipsize(android.text.TextUtils.TruncateAt.END);tile.addView(name);
            TextView number=text("—",24,TEXT);number.setTypeface(Typeface.MONOSPACE);number.setFontFeatureSettings("tnum");number.setSingleLine(true);number.setEllipsize(android.text.TextUtils.TruncateAt.END);tile.addView(number);valueViews.put(id,number);
            SparklineView graph=new SparklineView(this,ACCENT);tile.addView(graph,new LinearLayout.LayoutParams(-1,dp(28)));graphViews.put(id,graph);
            TextView range=text("",11,MUTED);range.setSingleLine(true);range.setEllipsize(android.text.TextUtils.TruncateAt.END);range.setTag("live-range");tile.addView(range);rangeViews.put(id,range);
            TextView freshness=text("",11,0xffffca75);freshness.setVisibility(View.GONE);tile.addView(freshness);freshnessViews.put(id,freshness);
            String protocol=signalProtocolLabel(s,udsActive(state));if(!protocol.isEmpty()){TextView meta=text(protocol,10,ACCENT);meta.setTag("live-protocol");tile.addView(meta);}
            String validation=knowledgeValidationBadge(s);if(!validation.isEmpty()){TextView meta=text(validation,10,MUTED);meta.setTag("live-validation");tile.addView(meta);}
            col=(col+1)%columns;
        }
        while(col!=0&&col<columns){row.addView(new View(this),new LinearLayout.LayoutParams(0,1,1));col++;}
        JSONObject vehicle=state.optJSONObject("vehicle");if(vehicle!=null&&vehicle.length()>0){LinearLayout c=card();note(c,"FAHRZEUG",ACCENT);note(c,vehicle.optString("vin","Fahrzeug erfasst"),TEXT);JSONArray responders=vehicle.optJSONArray("responders");if(responders!=null)note(c,responders.length()+" Responder · Details unter Diagnose",MUTED);}
    }
    private void diagnose(){
        title("Diagnose","Lesende Aufträge · Ergebnisse bleiben lokal");
        LinearLayout c=card();note(c,"FAHRZEUG & FEHLERSPEICHER",ACCENT);
        job(c,"Fahrzeug und Steuergeräte erfassen","inventory");job(c,"Fehlercodes lesen","dtc.scan");
        throughputCard();
        ecuDiscoveryCard();
        knowledgeLearningCard();
        udsSessionCard();

        JSONObject vehicle=state.optJSONObject("vehicle");
        if(vehicle!=null&&vehicle.length()>0)showInventory(vehicle);
        JSONObject dtcs=state.optJSONObject("diagnostics");
        if(dtcs!=null&&dtcs.length()>0)showDtcs(dtcs);
        JSONObject result=state.optJSONObject("jobResult");
        if(result!=null&&result.length()>0&&!result.optBoolean("ok",true)&&!result.optString("error").isEmpty()){
            LinearLayout error=card();note(error,"AUFTRAG NICHT ABGESCHLOSSEN",0xffffa79c);note(error,result.optString("error"),TEXT);
        }
        if((vehicle==null||vehicle.length()==0)&&(dtcs==null||dtcs.length()==0)&&(result==null||result.length()==0))empty("Bereit für den ersten Auftrag","Verbinde den Adapter oder starte die Demo. Rohdaten und Antworten findest du unter Debug.");
    }
    /** REQ-022.1/.6: the button (live and ready only, otherwise the reason) and the summary of the last or replayed measurement. */
    private void throughputCard(){
        LinearLayout c=card();note(c,"ADAPTER-LEISTUNG",ACCENT);
        PendingActions.Spec spec=jobSpec("Adapter-Leistung messen");String suffix=":"+ObdService.THROUGHPUT;
        boolean pend=service()!=null&&service().pending.isPending(spec.key+suffix);String blocked=pend?null:ObdService.throughputBlocker(state);
        actionButton(c,spec,suffix,"Adapter-Leistung messen",blocked==null,()->perform(spec,suffix,()->service().runJob(ObdService.THROUGHPUT)));
        if(blocked!=null)note(c,blocked,MUTED);
        smallNote(c,"Liest bis zu 6 Messwerte je 10-mal einzeln und dann 2, 3 und 6 Werte in einer Anfrage (nur Lesen, höchstens 60 s). Alle Antworten werden in einer eigenen Aufnahme gespeichert.",MUTED);
        JSONObject t=state.optJSONObject("throughput");if(t==null||t.length()==0)return;
        showThroughput(c,t);
    }
    private void showThroughput(LinearLayout c,JSONObject t){
        JSONObject overall=t.optJSONObject("phase1")==null?new JSONObject():t.optJSONObject("phase1").optJSONObject("overall");if(overall==null)overall=new JSONObject();
        note(c,"Gesamt: "+(overall.isNull("reads_per_s")?"—":MeasurementFormat.formatMeasurement(overall.optDouble("reads_per_s"),"Lesungen/s"))+" · Median "+ms(overall,"median_ms")+" · P90 "+ms(overall,"p90_ms")+" ("+overall.optInt("n")+" Lesungen)",TEXT);
        JSONArray pids=t.optJSONObject("phase1")==null?null:t.optJSONObject("phase1").optJSONArray("pids");
        for(int i=0;pids!=null&&i<pids.length();i++){JSONObject p=pids.optJSONObject(i);if(p==null)continue;
            smallNote(c,SignalNames.shortName(p.optString("semantic"),p.optString("semantic"))+" · Median "+ms(p,"median_ms")+" · P90 "+ms(p,"p90_ms")+(p.optInt("errors")>0?" · "+p.optInt("errors")+" Fehler":""),MUTED);}
        JSONArray multi=t.optJSONArray("phase2");
        for(int i=0;multi!=null&&i<multi.length();i++){JSONObject r=multi.optJSONObject(i);if(r==null)continue;
            String outcome=r.optString("outcome");StringBuilder text=new StringBuilder(r.optJSONArray("pids").length()+" Werte: "+(outcome.equals("full")?"vollständig":outcome.equals("partial")?"teilweise":"abgelehnt")+", "+ms(r,"elapsed_ms"));
            JSONArray values=r.optJSONArray("values");StringBuilder missing=new StringBuilder();
            for(int j=0;outcome.equals("partial")&&values!=null&&j<values.length();j++){JSONObject v=values.optJSONObject(j);if(v!=null&&!v.optString("status").equals("ok")){if(missing.length()>0)missing.append(", ");missing.append(SignalNames.shortName(v.optString("semantic"),v.optString("semantic")));}}
            if(missing.length()>0)text.append(" · ohne Antwort: ").append(missing);
            note(c,text.toString(),outcome.equals("full")?TEXT:0xffffca75);}
        if(!t.isNull("phase2_note")&&!t.optString("phase2_note").isEmpty())note(c,"Mehrfachanfrage nicht möglich: weniger als 2 unterstützte Messwerte",MUTED);
        if(t.optBoolean("truncated"))note(c,"Nach 60 s beendet, Messung unvollständig",0xffffca75);
        if(t.optBoolean("replay"))smallNote(c,"Aus gespeicherter Aufnahme neu berechnet",MUTED);
    }
    private static String ms(JSONObject o,String key){return o.isNull(key)||!o.has(key)?"—":MeasurementFormat.formatMeasurement(o.optDouble(key),"ms");}
    /** Canonical identification is opaque: show bytes and label printable text as a preview. */
    public static String identityValue(JSONObject item){
        if(item.isNull("value"))return "";
        JSONArray bytes=item.optJSONArray("value");if(bytes==null)return item.optString("value","");
        StringBuilder hex=new StringBuilder(),ascii=new StringBuilder();boolean printable=bytes.length()>0;
        for(int i=0;i<bytes.length();i++){int value=bytes.optInt(i,-1);if(value<0||value>255)return "Ungültige Identifikationsbytes";if(i>0)hex.append(' ');hex.append(String.format(java.util.Locale.ROOT,"%02X",value));if(value<32||value>126)printable=false;ascii.append((char)value);}
        return printable?"ASCII: "+ascii+" · Bytes: "+hex:"Bytes: "+hex;
    }
    private void showInventory(JSONObject vehicle){
        LinearLayout c=card();note(c,"FAHRZEUG · TEILABDECKUNG",ACCENT);note(c,"Funktionale OBD-Erkennung · kein vollständiges Herstellerverzeichnis",MUTED);
        String vin=vehicle.optString("vin","");String identityError=vehicle.optString("identity_error","");
        note(c,!vin.isEmpty()?"FIN: "+vin:!identityError.isEmpty()?"FIN nicht lesbar: "+identityError:"FIN noch nicht erfasst",vin.isEmpty()?MUTED:TEXT);
        JSONObject identity=vehicle.optJSONObject("vin_identity");
        if(identity!=null){
            String make=identity.optString("make_hint","");if(identity.isNull("make_hint"))make="";
            String manufacturer=identity.optString("manufacturer","");if(identity.isNull("manufacturer"))manufacturer="";
            if(identity.optString("status").equals("identified")){
                note(c,"Herstellerhinweis: "+(!make.isEmpty()?make:manufacturer)+" · WMI "+identity.optString("wmi"),TEXT);
                if(!manufacturer.isEmpty()&&!manufacturer.equals(make))note(c,manufacturer,MUTED);
            }else note(c,identity.optString("status").equals("invalid_vin")?"FIN-Format nicht auswertbar":"Hersteller in der lokalen WMI-Liste nicht eindeutig bestimmt",MUTED);
            note(c,"Modell und Motor nicht aus dem Herstellerpräfix bestimmt. Ein Diagnoseprofil wird dadurch nicht automatisch gewählt.",MUTED);
        }
        JSONArray responders=vehicle.optJSONArray("responders");
        if(responders==null||responders.length()==0)note(c,"Keine funktionalen Responder erfasst",MUTED);
        else{StringBuilder ids=new StringBuilder();for(int i=0;i<responders.length();i++){if(i>0)ids.append(", ");ids.append(responders.optString(i));}note(c,"Responder (Steuergeräte): "+ids,TEXT);}
        JSONArray signals=vehicle.optJSONArray("signals");if(signals!=null){int supported=0;for(int i=0;i<signals.length();i++)if("supported".equals(signals.optJSONObject(i).optString("status")))supported++;note(c,"Messwerte unterstützt: "+supported+" von "+signals.length(),MUTED);}
        if(vehicle.optBoolean("engine_target_validated"))note(c,"Motorsteuergerät gezielt geprüft",MUTED);
    }
    /** REQ-023.5: shows persisted present ECUs and not-observed targets separately. */
    private void ecuDiscoveryCard(){
        LinearLayout c=card();note(c,"STEUERGERÄTE ERKENNEN",ACCENT);job(c,"Steuergeräte aus Knowledge-Zielen erkennen","ecu.discover");
        JSONObject result=state.optJSONObject("ecuDiscovery"),record=selectedVehicle();if((result==null||result.length()==0)&&record!=null)result=record.optJSONObject("ecu_discovery");
        JSONArray targets=result==null?null:result.optJSONArray("targets");if(targets==null||targets.length()==0){smallNote(c,"Erkennt nur die für den Hersteller hinterlegten Ziele · nicht beobachtet bedeutet nicht abwesend.",MUTED);return;}
        boolean hasPresent=false,hasUnobserved=false,hasOther=false;for(int i=0;i<targets.length();i++){JSONObject target=targets.optJSONObject(i);if(target==null)continue;String status=target.optString("status");if("present".equals(status))hasPresent=true;else if("not_observed".equals(status))hasUnobserved=true;else hasOther=true;}
        if(hasPresent){note(c,"VORHANDEN",TEXT);for(int i=0;i<targets.length();i++){JSONObject target=targets.optJSONObject(i);if(target==null||!"present".equals(target.optString("status")))continue;
            String role=target.optString("role","Steuergerät"),request=signalField(target,"request_id"),response=signalField(target,"response_id"),route=!request.isEmpty()?" · Anfrage "+request+(response.isEmpty()?"":" · erwartete Antwort "+response):signalField(target,"address");if(route.isEmpty())route=signalField(target,"target");note(c,role+(route.isEmpty()?"":" · "+route),TEXT);
            JSONArray identity=target.optJSONArray("identity");if(identity==null)identity=target.optJSONArray("identification");
            if(identity!=null){for(int j=0;j<identity.length();j++){JSONObject item=identity.optJSONObject(j);if(item==null)continue;String semantic=signalField(item,"semantic");if(semantic.isEmpty())semantic=signalField(item,"name");if(semantic.isEmpty())semantic="Identifikation";String value=identityValue(item);String label=SignalNames.shortName(semantic,semantic);if(label.equals(semantic)&&semantic.contains("."))label=semantic.substring(semantic.lastIndexOf('.')+1).replace('_',' ');if(!value.isEmpty())smallNote(c,label+": "+value,MUTED);else if(!item.isNull("nrc"))smallNote(c,label+" · NRC "+item.optString("nrc"),MUTED);}}
            else{JSONObject fields=target.optJSONObject("identity");if(fields==null)fields=target.optJSONObject("identification");if(fields!=null){java.util.Iterator<String> keys=fields.keys();while(keys.hasNext()){String key=keys.next(),value=fields.optString(key);if(!value.isEmpty())smallNote(c,key.replace('_',' ')+": "+value,MUTED);}}}
        }}
        if(hasUnobserved){note(c,"NICHT BEOBACHTET",TEXT);for(int i=0;i<targets.length();i++){JSONObject target=targets.optJSONObject(i);if(target==null||!"not_observed".equals(target.optString("status")))continue;String role=target.optString("role","Steuergerät"),address=target.optString("address",target.optString("target"));smallNote(c,role+(address.isEmpty()?"":" · "+address)+" · nicht beobachtet",MUTED);}}
        if(hasOther){note(c,"WEITERE ERGEBNISSE",TEXT);for(int i=0;i<targets.length();i++){JSONObject target=targets.optJSONObject(i);if(target==null)continue;String status=target.optString("status");if(!status.isEmpty()&&!status.equals("present")&&!status.equals("not_observed"))smallNote(c,target.optString("role","Steuergerät")+" · "+status,MUTED);}}
    }
    /** REQ-026.2: keeps default-session learning outcomes and unconfirmed values visible. */
    private void knowledgeLearningCard(){
        LinearLayout c=card();note(c,"STANDARD-SITZUNG LERNEN",ACCENT);job(c,"Knowledge-Messwerte prüfen","knowledge.learn");
        JSONObject result=state.optJSONObject("knowledgeLearning"),record=selectedVehicle();if((result==null||result.length()==0)&&record!=null)result=record.optJSONObject("knowledge_learning");
        JSONArray rows=result==null?null:result.optJSONArray("results");if(rows==null||rows.length()==0){smallNote(c,"Prüft markierte Knowledge-Messwerte in der Standard-Sitzung · Werte bleiben unbestätigt.",MUTED);return;}
        for(int i=0;i<rows.length();i++){JSONObject item=rows.optJSONObject(i);if(item==null)continue;String semantic=item.optString("semantic",item.optString("name",item.optString("did","Messwert"))),status=item.optString("status");
            String label=SignalNames.shortName(semantic,semantic);String outcome=status.equals("answered")?"beantwortet · Wert unbestätigt":status.equals("rejected")?"abgelehnt":status.equals("not_answered")?"nicht beantwortet":status;
            String nrc=item.optString("nrc","");if(!nrc.isEmpty()&&status.equals("rejected"))outcome+=" · NRC "+nrc;
            String value=item.optString("value",item.optString("text",""));note(c,label+" · "+outcome,TEXT);if(!value.isEmpty())smallNote(c,"Wert: "+value,MUTED);
        }
    }
    /** REQ-027.1/.3: explicit, gated extended-session controls; disabling remains available during recording. */
    private void udsSessionCard(){
        LinearLayout c=card();note(c,"UDS · ERWEITERTE SITZUNG",ACCENT);boolean active=udsActive(state);
        udsRemaining=text("",14,active?ACCENT:MUTED);c.addView(udsRemaining);
        PendingActions.Spec spec=active?UDS_DISABLE:UDS_ENABLE;String action=active?"uds.disable":"uds.enable";
        boolean enabled=active?!state.optBoolean("busy"):udsEnableBlocker().isEmpty();
        actionButton(c,spec,"",active?"Standard-Sitzung wiederherstellen":"Erweiterte Sitzung aktivieren",enabled,()->perform(spec,"",()->service().runJob(action)));
        if(!active){String reason=udsEnableBlocker();if(!reason.isEmpty())smallNote(c,reason,MUTED);}
        smallNote(c,"Nur Knowledge-Messwerte des identifizierten Steuergeräts · Rückkehr zur Standard-Sitzung nach dem Ausschalten und vor funktionalen OBD-Anfragen.",MUTED);
    }
    private void showDtcs(JSONObject dtcs){
        LinearLayout c=card();note(c,"GESPEICHERTE FEHLERCODES",ACCENT);JSONArray observations=dtcs.optJSONArray("observations");
        if(observations==null||observations.length()==0)note(c,"Keine Steuergeräte-Antworten",MUTED);
        else for(int i=0;i<observations.length();i++){JSONObject item=observations.optJSONObject(i);if(item==null)continue;String responder=displayResponder(item.optString("responder",""));String error=item.optString("error","");JSONArray codes=item.optJSONArray("codes");
            if(!error.isEmpty())note(c,responder+" · Fehler: "+error,0xffffa79c);
            else if(item.optBoolean("no_dtcs")||(codes!=null&&codes.length()==0))note(c,responder+" · Keine gespeicherten Fehlercodes",TEXT);
            else if(codes!=null){StringBuilder values=new StringBuilder();for(int j=0;j<codes.length();j++){if(j>0)values.append(", ");values.append(codes.optString(j));}note(c,responder+" · "+values,TEXT);}
            else note(c,responder+" · Keine auswertbaren Codes",MUTED);
        }
        JSONArray errors=dtcs.optJSONArray("errors");if(errors!=null)for(int i=0;i<errors.length();i++){JSONObject item=errors.optJSONObject(i);if(item!=null)note(c,displayResponder(item.optString("responder",""))+" · "+item.optString("error","Antwortfehler"),0xffffa79c);}
    }
    private String displayResponder(String responder){return responder.isEmpty()?"Unbekannter Responder":responder;}
    private void job(LinearLayout parent,String label,String action){PendingActions.Spec spec=jobSpec(label);actionButton(parent,spec,":"+action,label,canRun(),()->perform(spec,":"+action,()->service().runJob(action)));}
    private boolean canRun(){String mode=state.optString("mode");return !state.optBoolean("busy")&&!mode.equals("replay")&&(mode.equals("demo")||state.optString("phase").equals("ready"));}
    private void recordings(){
        title("Aufnahmen","Lokal sichern · später ohne Auto untersuchen");
        JSONArray profiles=array("profiles");if(profile.isEmpty()&&profiles.length()>0)profile=profiles.optJSONObject(0).optString("id");
        JSONObject chosen=new JSONObject();for(int i=0;i<profiles.length();i++)if(profile.equals(profiles.optJSONObject(i).optString("id")))chosen=profiles.optJSONObject(i);
        LinearLayout c=card();note(c,"AUFNAHMEPROFIL",ACCENT);addButton(c,profile.isEmpty()?"Profil auswählen":chosen.optString("title",profile),this::chooseProfile);
        boolean running=!state.optString("recording").isEmpty();
        String selectedProfile=profile;
        actionButton(c,running?REC_STOP:REC_START,"",running?"Aufzeichnung stoppen":"Aufzeichnung starten",running||canRun(),()->{if(running)perform(REC_STOP,"",()->service().stopRecording());else perform(REC_START,"",()->service().startRecording(selectedProfile));});
        note(c,running?"● "+state.optString("recording"):"Aufzeichnungen enthalten möglicherweise Fahrzeugkennungen. Export erfolgt nur auf deine ausdrückliche Auswahl.",running?ACCENT:MUTED);
        autoRecordCard();
        exportCard();
        JSONArray records=array("records");if(records.length()==0)empty("Noch keine Aufnahmen","Starte eine Aufzeichnung im Live- oder Demomodus. Die Dateien bleiben auf diesem Handy.");
        for(int i=0;i<records.length();i++){JSONObject record=records.optJSONObject(i);if(record==null)continue;String name=record.optString("name");LinearLayout item=card();note(item,name,TEXT);note(item,record.optLong("bytes")+" Bytes",MUTED);addButton(item,"Als Replay öffnen",()->act(()->{service().openRecording(name);tab=0;buildNav();structure="";}));addButton(item,"Datei exportieren …",()->export(name));Button delete=addButton(item,"Aufnahme löschen …",()->confirmDeleteRecording(name));delete.setEnabled(!name.equals(state.optString("recording"))&&!name.equals(state.optString("replayFile")));}
    }
    /** REQ-028: always-on session recording switch and delete-all. */
    private void autoRecordCard(){
        boolean on=state.optBoolean("autoRecord",true);
        LinearLayout c=card();note(c,"DAUER-AUFZEICHNUNG",ACCENT);
        note(c,"Jede Live-Verbindung wird ab dem ersten Verbindungsversuch automatisch aufgezeichnet (…-auto.jsonl, alle 30 min oder 10 MB neue Datei). Die App löscht nichts selbst.",MUTED);
        if(!state.optString("autoRecording").isEmpty())note(c,"● "+state.optString("autoRecording"),ACCENT);
        actionButton(c,AUTO_RECORD,"","Dauer-Aufzeichnung: "+(on?"an":"aus"),true,()->perform(AUTO_RECORD,"",()->service().setAutoRecord(!on)));
        actionButton(c,AUTO_RECORD_DELETE,"","Automatische Aufnahmen löschen …",true,()->new AlertDialog.Builder(this).setTitle("Automatische Aufnahmen löschen?").setMessage("Alle abgeschlossenen automatischen Aufnahmen werden dauerhaft gelöscht. Benannte Aufnahmen und exportierte Kopien bleiben.").setPositiveButton("Löschen",(d,w)->perform(AUTO_RECORD_DELETE,"",()->service().deleteAutoRecordings())).setNegativeButton("Abbrechen",null).show());
    }
    /** REQ-018: export folder, one-tap and automatic export, share sheet. */
    private void exportCard(){
        String folder=RecordingExporter.folder(this);boolean auto=RecordingExporter.auto(this);
        LinearLayout c=card();note(c,"EXPORT",ACCENT);
        note(c,folder.isEmpty()?"Kein Export-Ordner gewählt":"Export-Ordner: "+folderName(folder),folder.isEmpty()?MUTED:TEXT);
        addButton(c,"Export-Ordner wählen …",this::chooseFolder);
        actionButton(c,EXPORT_ALL,"","Alle exportieren",!folder.isEmpty(),()->perform(EXPORT_ALL,"",()->service().exportAll()));
        actionButton(c,AUTO_EXPORT,"","Automatischer Export: "+(auto?"an":"aus"),true,()->{if(!auto&&RecordingExporter.folder(this).isEmpty()){toast("Zuerst Export-Ordner wählen");return;}perform(AUTO_EXPORT,"",()->RecordingExporter.setAuto(this,!auto));});
        addButton(c,"Aufnahmen teilen …",this::shareAll);
        smallNote(c,"Empfohlen: Download/OBDentic. Exportierte Aufnahmen können FIN, Steuergerätekennungen und km-Stand enthalten; nur abgeschlossene Aufnahmen werden kopiert, Originale bleiben auf dem Handy.",MUTED);
    }
    private static String folderName(String tree){try{return Uri.decode(android.provider.DocumentsContract.getTreeDocumentId(Uri.parse(tree)));}catch(RuntimeException e){return tree;}}
    private void chooseFolder(){
        Runnable pick=()->{
            Intent open=new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION|Intent.FLAG_GRANT_WRITE_URI_PERMISSION|Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
                .putExtra(android.provider.DocumentsContract.EXTRA_INITIAL_URI,android.provider.DocumentsContract.buildDocumentUri("com.android.externalstorage.documents","primary:Download/OBDentic"));
            try{startActivityForResult(open,42);}catch(Exception e){toast("Ordnerauswahl nicht möglich: "+e.getMessage());}
        };
        if(!RecordingExporter.folder(this).isEmpty()){pick.run();return;}
        new AlertDialog.Builder(this).setTitle("Export-Ordner wählen").setMessage("Aufnahmen können Fahrzeugdaten enthalten: FIN, Steuergerätekennungen und km-Stand. Sie werden nur in den Ordner kopiert, den du jetzt wählst, und erst wenn du exportierst oder den automatischen Export einschaltest.\n\nEmpfohlen: Download/OBDentic (Standardquelle von tools/pull-recordings.sh).")
            .setPositiveButton("Ordner wählen",(d,w)->pick.run()).setNegativeButton("Abbrechen",null).show();
    }
    private void shareAll(){
        java.util.List<String> names=RecordingExporter.completed(array("records"),activeRecordingName());
        if(names.isEmpty()){toast("Keine abgeschlossenen Aufnahmen");return;}
        try{startActivity(Intent.createChooser(RecordingExporter.share(this,names),"Aufnahmen teilen"));}catch(Exception e){toast("Teilen nicht möglich: "+e.getMessage());}
    }
    private JSONObject vehicleStore(){JSONObject v=state.optJSONObject("vehicles");return v==null?new JSONObject():v;}
    private JSONObject selectedVehicle(){JSONObject store=vehicleStore();JSONArray rows=store.optJSONArray("vehicles");String selected=store.optString("selected");if(rows!=null)for(int i=0;i<rows.length();i++)if(selected.equals(rows.optJSONObject(i).optString("vin")))return rows.optJSONObject(i);return null;}
    private static String formatDate(long millis){return new java.text.SimpleDateFormat("dd.MM.yyyy",java.util.Locale.GERMANY).format(new java.util.Date(millis));}
    private String date(long millis){return formatDate(millis);}
    private static String km(double value){java.text.NumberFormat f=java.text.NumberFormat.getNumberInstance(java.util.Locale.GERMANY);f.setMaximumFractionDigits(1);return f.format(value)+" km";}
    private void vehicles(){
        title("Fahrzeuge","Lokal gespeichert · nur auf diesem Handy");
        JSONObject store=vehicleStore();
        if(store.optBoolean("unreadable")){LinearLayout e=card();note(e,"FAHRZEUGSPEICHER NICHT LESBAR",0xffffa79c);note(e,"Die gespeicherten Fahrzeuge konnten nicht gelesen werden. Verbindungen funktionieren weiter. Die Datei wird nicht überschrieben, solange du den Speicher nicht selbst zurücksetzt.",MUTED);addButton(e,"Fahrzeugspeicher zurücksetzen …",this::confirmResetVehicles);return;}
        JSONArray rows=store.optJSONArray("vehicles");
        if(rows==null||rows.length()==0){empty("Noch keine Fahrzeuge","Verbinde den Adapter mit einem Fahrzeug. Die FIN wird beim ersten Verbinden gelesen und das Fahrzeug hier gespeichert.");return;}
        for(int i=0;i<rows.length();i++){
            JSONObject row=rows.optJSONObject(i);if(row==null)continue;String vin=row.optString("vin"),name=row.optString("name");boolean selected=vin.equals(store.optString("selected"));
            LinearLayout c=card();note(c,name+(selected?" · gewählt":""),TEXT);
            note(c,(vin.length()>=7?vin.substring(0,3)+"…"+vin.substring(vin.length()-4):vin)+" · "+odometerListText(row),MUTED);
            note(c,"Zuletzt verbunden: "+date(row.optLong("last_connected")),MUTED);
            actionButton(c,VEHICLE_SELECT,":"+vin,"Auswählen",!selected,()->perform(VEHICLE_SELECT,":"+vin,()->service().selectVehicle(vin)));
            addButton(c,"Details",()->showVehicle(row));actionButton(c,VEHICLE_RENAME,":"+vin,"Umbenennen …",true,()->renameVehicle(vin,name));actionButton(c,VEHICLE_DELETE,":"+vin,"Löschen …",true,()->confirmDeleteVehicle(vin,name));
        }
    }
    static String odometerListText(JSONObject row){
        JSONObject a6=row.optJSONObject("odometer");
        if(a6!=null)return km(a6.optDouble("km"))+" · "+formatDate(a6.optLong("read_at"));
        return "km-Stand nicht verfügbar";
    }
    private void showVehicle(JSONObject row){
        new AlertDialog.Builder(this).setTitle(row.optString("name")).setMessage(vehicleDetails(row)).setPositiveButton("Schließen",null).show();
    }
    public static String vehicleDetails(JSONObject row){
        StringBuilder m=new StringBuilder("FIN: "+row.optString("vin")+"\nHerstellerhinweis: "+row.optString("manufacturer_hint","keiner"));
        JSONArray responders=row.optJSONArray("responders");StringBuilder ids=new StringBuilder();for(int i=0;responders!=null&&i<responders.length();i++){if(i>0)ids.append(", ");ids.append(responders.optString(i));}
        m.append("\nResponder: ").append(ids.length()==0?"keine":ids);
        JSONArray signals=row.optJSONArray("signals");int supported=0;for(int i=0;signals!=null&&i<signals.length();i++)if("supported".equals(signals.optJSONObject(i).optString("status")))supported++;
        m.append("\nMesswerte unterstützt: ").append(supported).append(" von ").append(signals==null?0:signals.length());
        m.append("\nMotorsteuergerät gezielt geprüft: ").append(row.optBoolean("engine_target_validated")?"ja":"nein");
        JSONObject odometer=row.optJSONObject("odometer"),failure=row.optJSONObject("odometer_error"),decrease=row.optJSONObject("odometer_decrease");
        m.append("\nkm-Stand: ").append(odometer==null?"nicht verfügbar":km(odometer.optDouble("km"))+" (Quelle: "+odometer.optString("source")+", gelesen "+formatDate(odometer.optLong("read_at"))+")");
        if(failure!=null)m.append("\nLetzter Leseversuch fehlgeschlagen: ").append(failure.optString("error")).append(" (").append(formatDate(failure.optLong("at"))).append(")");
        if(decrease!=null)m.append("\nNiedriger als zuvor gelesen: ").append(km(decrease.optDouble("previous_km"))).append(" am ").append(formatDate(decrease.optLong("previous_at")));
        m.append("\nErste Verbindung: ").append(formatDate(row.optLong("first_connected"))).append("\nLetzte Verbindung: ").append(formatDate(row.optLong("last_connected")));
        return m.toString();
    }
    private void renameVehicle(String vin,String name){
        android.widget.EditText input=new android.widget.EditText(this);input.setSingleLine(true);input.setText(name);input.selectAll();
        new AlertDialog.Builder(this).setTitle("Fahrzeug umbenennen").setMessage("1 bis 40 Zeichen").setView(input).setPositiveButton("Speichern",(d,w)->{String entered=input.getText().toString();perform(VEHICLE_RENAME,":"+vin,()->service().renameVehicle(vin,entered));}).setNegativeButton("Abbrechen",null).show();
    }
    private void confirmDeleteVehicle(String vin,String name){new AlertDialog.Builder(this).setTitle("Fahrzeug endgültig löschen?").setMessage(name+"\n\nDer Eintrag wird dauerhaft aus dem internen Speicher gelöscht. Aufnahmen bleiben erhalten.").setPositiveButton("Endgültig löschen",(d,w)->perform(VEHICLE_DELETE,":"+vin,()->service().deleteVehicle(vin))).setNegativeButton("Abbrechen",null).show();}
    private void confirmResetVehicles(){new AlertDialog.Builder(this).setTitle("Fahrzeugspeicher zurücksetzen?").setMessage("Alle gespeicherten Fahrzeuge werden dauerhaft gelöscht und der Speicher wird neu angelegt. Aufnahmen bleiben erhalten.").setPositiveButton("Zurücksetzen",(d,w)->act(()->service().resetVehicleStore())).setNegativeButton("Abbrechen",null).show();}
    private void confirmDeleteRecording(String name){new AlertDialog.Builder(this).setTitle("Aufnahme endgültig löschen?").setMessage("Diese Datei wird dauerhaft aus dem internen Speicher gelöscht. Exportierte Kopien bleiben bestehen.\n\n"+name).setPositiveButton("Endgültig löschen",(dialog,which)->act(()->service().deleteRecording(name))).setNegativeButton("Abbrechen",null).show();}
    private void debug(){
        title("Debug-Daten","Zeitgestempelte TX/RX-Daten dieser Sitzung");
        addButton(body,"Debug-Daten lokal kopieren",()->{ClipboardManager cm=(ClipboardManager)getSystemService(CLIPBOARD_SERVICE);cm.setPrimaryClip(ClipData.newPlainText("OBDentic Debug",debugText()));toast("In die lokale Zwischenablage kopiert");});
        note(body,"Kann VIN, Steuergerätekennungen und weitere Fahrzeugdaten enthalten. Nur gezielt teilen.",MUTED);
        if(array("debug").length()==0)empty("Noch kein Datenverkehr","Verbinden oder einen Diagnoseauftrag starten. Hier erscheinen gesendete Befehle, Antworten und Dienstmeldungen.");
        else{LinearLayout c=card();debugLog=text(debugText(),12,TEXT);debugLog.setTypeface(Typeface.MONOSPACE);debugLog.setTextIsSelectable(true);c.addView(debugLog);}
    }
    private String debugText(){StringBuilder s=new StringBuilder();JSONArray logs=array("debug");for(int i=0;i<logs.length();i++){JSONObject j=logs.optJSONObject(i);if(j==null){s.append(logs.optString(i)).append('\n');continue;}s.append(j.optString("at")).append("  ").append(j.optString("direction")).append("  ").append(j.optString("text")).append('\n');}return s.toString();}
    private void connection(){
        title("Adapter","Carly-Adapter suchen und gezielt verbinden");
        LinearLayout c=card();actionButton(c,SCAN,"","Adapter suchen",true,this::requestScan);actionButton(c,DISCONNECT,"","Verbindung trennen",!state.optString("phase").equals("idle"),()->perform(DISCONNECT,"",()->service().disconnect()));
        boolean autoOn=state.optBoolean("autoConnect",true);String known=state.optString("knownAdapter");
        LinearLayout auto=card();note(auto,"AUTOMATISCH VERBINDEN",ACCENT);
        note(auto,known.isEmpty()?"Noch kein Adapter bekannt · wird nach der ersten erfolgreichen Verbindung gemerkt":"Bekannter Adapter: "+known+" · Verbindung beim Start und nach Abbruch, Pausen 2, 5, 10 s, dann alle 30 s",known.isEmpty()?MUTED:TEXT);
        actionButton(auto,AUTO_CONNECT,"","Automatisch verbinden: "+(autoOn?"an":"aus"),true,()->perform(AUTO_CONNECT,"",()->service().setAutoConnect(!autoOn)));
        if(state.optBoolean("autoPending")||service().pending.isPending(AUTO_STOP.key))actionButton(auto,AUTO_STOP,"","Automatische Verbindung stoppen",true,()->perform(AUTO_STOP,"",()->service().stopAutoConnect()));
        JSONArray devices=array("devices");if(devices.length()==0)note(c,"Noch keine Adapter gefunden. Adapter mit Strom versorgen, Bluetooth einschalten und Suche starten.",MUTED);
        for(int i=0;i<devices.length();i++){JSONObject d=devices.optJSONObject(i);if(d==null)continue;String address=d.optString("address"),name=d.optString("name","Bluetooth-Gerät");LinearLayout device=card();note(device,name,TEXT);note(device,address+" · "+d.optInt("rssi")+" dBm",MUTED);actionButton(device,CONNECT,":"+address,"Verbinden",!state.optBoolean("busy"),()->perform(CONNECT,":"+address,()->service().connect(address)));}
        LinearLayout demo=card();note(demo,"OHNE FAHRZEUG TESTEN",ACCENT);note(demo,"Die Demo liefert simulierte Messwerte und Diagnoseantworten. Aufnahmen und Export lassen sich damit am Schreibtisch prüfen.",MUTED);addButton(demo,"Demo starten",()->act(()->{service().setDemo();tab=0;structure="";buildNav();}));
    }
    private void requestScan(){ArrayList<String> missing=new ArrayList<>();if(Build.VERSION.SDK_INT>=31){missing.add(Manifest.permission.BLUETOOTH_SCAN);missing.add(Manifest.permission.BLUETOOTH_CONNECT);}else missing.add(Manifest.permission.ACCESS_FINE_LOCATION);if(Build.VERSION.SDK_INT>=33)missing.add(Manifest.permission.POST_NOTIFICATIONS);missing.removeIf(p->checkSelfPermission(p)==PackageManager.PERMISSION_GRANTED);if(!missing.isEmpty()){scanAfterPermission=true;requestPermissions(missing.toArray(new String[0]),7);}else perform(SCAN,"",()->service().scan());}
    @Override public void onRequestPermissionsResult(int request,String[] permissions,int[] results){super.onRequestPermissionsResult(request,permissions,results);if(request==7&&scanAfterPermission){scanAfterPermission=false;boolean allowed=true;for(int i=0;i<permissions.length;i++)if(!permissions[i].equals(Manifest.permission.POST_NOTIFICATIONS)&&results[i]!=PackageManager.PERMISSION_GRANTED)allowed=false;if(allowed)perform(SCAN,"",()->service().scan());else new AlertDialog.Builder(this).setTitle("Bluetooth-Berechtigung fehlt").setMessage("Erlaube Geräte in der Nähe in den Android-App-Einstellungen, um den Adapter zu suchen und zu verbinden.").setPositiveButton("Einstellungen",(d,w)->startActivity(new Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,Uri.parse("package:"+getPackageName())))).setNegativeButton("Schließen",null).show();}}
    /** REQ-014.2: current-session support wins; otherwise the selected record's last observation, marked with its date. */
    public static String supportLabel(String current,String stored,long seenAt){
        if(!current.isEmpty())return current.equals("unsupported")?" · nicht unterstützt":current.equals("supported")?"":" · ungeprüft";
        if(stored.equals("supported")||stored.equals("unsupported"))return (stored.equals("supported")?" · unterstützt":" · nicht unterstützt")+" (zuletzt gesehen "+formatDate(seenAt)+")";
        return " · ungeprüft";
    }
    private void chooseSignals(){
        if(!state.optString("recording").isEmpty()){toast("Messwertauswahl während Aufnahme gesperrt");return;}
        JSONArray signals=array("signals");JSONObject vehicle=selectedVehicle();JSONObject stored=new JSONObject();long seenAt=0;String name="";
        boolean session=!state.optString("mode","live").equals("live")||state.optString("phase").equals("ready");
        if(vehicle!=null){seenAt=vehicle.optLong("last_connected");name=vehicle.optString("name");JSONArray rows=vehicle.optJSONArray("signals");for(int i=0;rows!=null&&i<rows.length();i++){JSONObject row=rows.optJSONObject(i);if(row!=null)try{stored.put(row.optString("semantic"),row.optString("status"));}catch(org.json.JSONException ignored){}}}
        final long supportSeenAt=seenAt;
        LinkedHashMap<String,LinkedHashMap<String,ArrayList<Integer>>> groups=new LinkedHashMap<>();
        for(int i=0;i<signals.length();i++){
            JSONObject signal=signals.optJSONObject(i);if(signal==null)continue;
            String ecu=signalField(signal,"ecu");if(ecu.isEmpty())ecu="OBD-Standard";
            String group=signalField(signal,"group");if(group.isEmpty())group=signalField(signal,"subsystem");if(group.isEmpty())group="Allgemein";
            groups.computeIfAbsent(ecu,k->new LinkedHashMap<>()).computeIfAbsent(group,k->new ArrayList<>()).add(i);
        }
        LinkedHashSet<String> chosen=new LinkedHashSet<>();JSONArray current=array("selected");for(int i=0;i<current.length()&&chosen.size()<32;i++)chosen.add(current.optString(i));
        Dialog dialog=new Dialog(this,android.R.style.Theme_Material_NoActionBar);dialog.setCanceledOnTouchOutside(true);
        LinearLayout root=new LinearLayout(this);root.setOrientation(LinearLayout.VERTICAL);root.setBackgroundColor(BG);root.setPadding(dp(18),dp(12),dp(18),dp(12));
        LinearLayout heading=row();TextView title=text("Live-Messwerte",22,TEXT);title.setTypeface(null,Typeface.BOLD);heading.addView(title,new LinearLayout.LayoutParams(0,-2,1));Button close=button("Schließen",dialog::dismiss);heading.addView(close);root.addView(heading);
        if(!name.isEmpty())smallNote(root,name,MUTED);
        EditText search=new EditText(this);search.setSingleLine(true);search.setTextColor(TEXT);search.setHintTextColor(MUTED);search.setHint("Messwert, Gruppe oder Einheit suchen");search.setPadding(dp(12),0,dp(12),0);search.setBackground(surface(CARD,12));LinearLayout.LayoutParams searchParams=new LinearLayout.LayoutParams(-1,dp(50));searchParams.setMargins(0,dp(8),0,dp(6));root.addView(search,searchParams);
        TextView count=text("",15,ACCENT);count.setTypeface(null,Typeface.BOLD);root.addView(count);
        TextView feedback=text("",13,0xffffca75);root.addView(feedback);
        ScrollView listScroll=new ScrollView(this);listScroll.setFillViewport(false);LinearLayout list=new LinearLayout(this);list.setOrientation(LinearLayout.VERTICAL);listScroll.addView(list);root.addView(listScroll,new LinearLayout.LayoutParams(-1,0,1));
        boolean locked=!state.optString("recording").isEmpty();
        LinearLayout presetRow=row();Button save=button("Preset speichern",()->saveSignalPreset(chosen));save.setEnabled(!locked&&vehicle!=null&&state.optString("mode","live").equals("live"));presetRow.addView(save,new LinearLayout.LayoutParams(0,-2,1));Button apply=button("Preset anwenden",()->chooseSignalPreset(dialog));apply.setEnabled(!locked&&vehicle!=null&&state.optString("mode","live").equals("live"));presetRow.addView(apply,new LinearLayout.LayoutParams(0,-2,1));root.addView(presetRow);
        LinearLayout actions=row();Button cancel=button("Abbrechen",dialog::dismiss);actions.addView(cancel,new LinearLayout.LayoutParams(0,-2,1));Button accept=button("Übernehmen",()->{
            if(service()==null)return;
            if(!service().snapshot().optString("recording").isEmpty()){toast("Messwertauswahl während Aufnahme gesperrt");return;}
            JSONArray selection=new JSONArray();for(String semantic:chosen)selection.put(semantic);
            dialog.dismiss();act(()->service().setSignals(selection));
        });actions.addView(accept,new LinearLayout.LayoutParams(0,-2,1));root.addView(actions);
        Runnable[] rebuild=new Runnable[1];
        Runnable updateCount=()->count.setText(chosen.size()+" / 32 gewählt");
        rebuild[0]=()->{
            String query=search.getText().toString().trim().toLowerCase(java.util.Locale.ROOT);list.removeAllViews();
            for(Map.Entry<String,LinkedHashMap<String,ArrayList<Integer>>> section:groups.entrySet()){
                boolean sectionShown=false;LinearLayout sectionView=new LinearLayout(this);sectionView.setOrientation(LinearLayout.VERTICAL);
                for(Map.Entry<String,ArrayList<Integer>> group:section.getValue().entrySet()){
                    ArrayList<Integer> matches=new ArrayList<>();
                    for(int index:group.getValue())if(matchesSignal(signals.optJSONObject(index),group.getKey(),query))matches.add(index);
                    if(matches.isEmpty())continue;
                    sectionShown=true;
                    LinearLayout groupHeader=row();TextView groupTitle=text(group.getKey(),16,ACCENT);groupTitle.setTypeface(null,Typeface.BOLD);groupHeader.addView(groupTitle,new LinearLayout.LayoutParams(0,-2,1));
                    Button none=button("Keine",()->{if(recordingNow()){toast("Messwertauswahl während Aufnahme gesperrt");return;}for(int index:group.getValue())chosen.remove(signals.optJSONObject(index).optString("semantic"));feedback.setText("");rebuild[0].run();});none.setTextSize(12);none.setEnabled(!locked);groupHeader.addView(none);
                    Button all=button("Alle",()->{if(recordingNow()){toast("Messwertauswahl während Aufnahme gesperrt");return;}int room=32-chosen.size(),skipped=0;for(int index:group.getValue()){String semantic=signals.optJSONObject(index).optString("semantic");if(chosen.contains(semantic))continue;if(room>0){chosen.add(semantic);room--;}else skipped++;}feedback.setText(skipped>0?"Gruppe gekürzt · maximal 32 Messwerte":"");rebuild[0].run();});all.setTextSize(12);all.setEnabled(!locked);groupHeader.addView(all);sectionView.addView(groupHeader);
                    for(int index:matches){JSONObject signal=signals.optJSONObject(index);String semantic=signal.optString("semantic");String unit=MeasurementFormat.displayUnit(signal.optString("unit"));String protocol=signalProtocolLabel(signal,udsActive(state));CheckBox check=new CheckBox(this);check.setText(signalLabel(semantic,signal)+(unit.isEmpty()?"":" · "+unit));check.setTextColor(TEXT);check.setMinHeight(dp(44));check.setChecked(chosen.contains(semantic));check.setEnabled(!locked);check.setContentDescription(check.getText()+", "+signalSupport(signal,stored.optString(semantic,""),session,supportSeenAt)+", "+validationState(signal)+(protocol.isEmpty()?"":", "+protocol));
                        android.widget.CompoundButton.OnCheckedChangeListener[] listener=new android.widget.CompoundButton.OnCheckedChangeListener[1];listener[0]=(button,on)->{if(recordingNow()){button.setOnCheckedChangeListener(null);button.setChecked(chosen.contains(semantic));button.setOnCheckedChangeListener(listener[0]);feedback.setText("Messwertauswahl während Aufnahme gesperrt");return;}if(on&&chosen.size()>=32){button.setChecked(false);feedback.setText("Maximal 32 Messwerte · Auswahl nicht erweitert");return;}if(on)chosen.add(semantic);else chosen.remove(semantic);feedback.setText("");updateCount.run();};check.setOnCheckedChangeListener(listener[0]);
                        sectionView.addView(check);
                        TextView details=text(signalSupport(signal,stored.optString(semantic,""),session,supportSeenAt)+" · "+validationState(signal)+(protocol.isEmpty()?"":" · "+protocol),12,MUTED);details.setPadding(dp(36),0,0,dp(5));sectionView.addView(details);
                    }
                }
                if(sectionShown){TextView sectionTitle=text(section.getKey().equals("OBD-Standard")?"OBD-Standard":"Steuergerät · "+section.getKey(),18,TEXT);sectionTitle.setTypeface(null,Typeface.BOLD);sectionTitle.setPadding(0,dp(12),0,dp(4));list.addView(sectionTitle);list.addView(sectionView);}
            }
            if(list.getChildCount()==0)note(list,"Keine passenden Messwerte",MUTED);
            updateCount.run();
        };
        search.addTextChangedListener(new TextWatcher(){public void beforeTextChanged(CharSequence s,int start,int count,int after){}public void onTextChanged(CharSequence s,int start,int before,int count){rebuild[0].run();}public void afterTextChanged(Editable e){}});
        rebuild[0].run();dialog.setContentView(root);dialog.setOnDismissListener(ignored->{if(activeSignalSelector==dialog)activeSignalSelector=null;});dialog.show();activeSignalSelector=dialog;Window window=dialog.getWindow();if(window!=null){window.setLayout(-1,-1);window.setBackgroundDrawable(new android.graphics.drawable.ColorDrawable(BG));window.setStatusBarColor(BG);window.setNavigationBarColor(BG);}
    }
    private static String signalField(JSONObject signal,String key){Object value=signal.opt(key);if(value instanceof JSONObject){JSONObject object=(JSONObject)value;return object.optString("status",object.optString("state",object.optString("name","")));}return value==null||JSONObject.NULL.equals(value)?"":String.valueOf(value);}
    public static String signalProtocolLabel(JSONObject signal,boolean udsActive){String protocol=signalField(signal,"protocol"),lower=protocol.toLowerCase(java.util.Locale.ROOT);boolean extended=signal.optBoolean("extended_session_required")||signal.optBoolean("session_required");if(udsActive&&extended)return "UDS · erweiterte Sitzung";if(udsActive&&isObd2Signal(signal))return "OBD2 · pausiert";if(protocol.isEmpty())return "";return lower.equals("uds")&&udsActive?"UDS · erweiterte Sitzung":protocol.toUpperCase(java.util.Locale.ROOT);}
    private static boolean isObd2Signal(JSONObject signal){String protocol=signalField(signal,"protocol").toLowerCase(java.util.Locale.ROOT);if(!protocol.isEmpty())return protocol.contains("obd");String ecu=signalField(signal,"ecu");return ecu.isEmpty()||ecu.equalsIgnoreCase("OBD-Standard");}
    private String activeRecordingName(){String named=state.optString("recording");return named.isEmpty()?state.optString("autoRecording"):named;}
    private boolean recordingNow(){return service()!=null&&!service().snapshot().optString("recording").isEmpty();}
    private static boolean matchesSignal(JSONObject signal,String group,String query){if(query.isEmpty())return true;String name=SignalNames.shortName(signal.optString("meaning",signal.optString("semantic")),signal.optString("title","")),unit=MeasurementFormat.displayUnit(signal.optString("unit"));return (name+" "+group+" "+unit).toLowerCase(java.util.Locale.ROOT).contains(query);}
    private static String validationState(JSONObject signal){String value=signalField(signal,"validation");if(value.isEmpty())return "Validierung: nicht angegeben";if(value.equals("not_validated"))value="nicht validiert";else if(value.equals("validated"))value="validiert";else if(value.equals("unconfirmed"))value="unbestätigt";return "Validierung: "+value;}
    private static String validationLabel(JSONObject signal){String value=signalField(signal,"validation_label");if(!value.isEmpty())return value;value=signalField(signal,"validation");if(value.equals("not_validated"))return "nicht abgeglichen";if(value.equals("validated"))return "validiert";if(value.equals("not_applicable"))return "nicht anwendbar";if(value.equals("unconfirmed"))return "unbestätigt";return value.isEmpty()?"Validierung nicht angegeben":value;}
    public static String knowledgeValidationBadge(JSONObject signal){return signal.has("definition_id")&&!signal.isNull("definition_id")?validationLabel(signal):"";}
    private static String signalSupport(JSONObject signal,String stored,boolean session,long seenAt){String status=session?signal.optString("status",""):"";if(status.isEmpty()&&session&&signal.has("supported")&&!signal.isNull("supported"))status=signal.optBoolean("supported")?"supported":"unsupported";boolean lastSeen=false;if(status.isEmpty()){status=stored;lastSeen=!status.isEmpty();}String label=status.equals("supported")?"unterstützt":status.equals("unsupported")?"nicht unterstützt":"ungeprüft";if(lastSeen)label+=" (zuletzt gesehen "+formatDate(seenAt)+")";return "Unterstützung: "+label;}
    private void saveSignalPreset(LinkedHashSet<String> selection){
        if(service()==null)return;EditText input=new EditText(this);input.setSingleLine(true);input.setHint("Name · 1 bis 40 Zeichen");
        new AlertDialog.Builder(this).setTitle("Auswahl als Preset speichern").setView(input).setPositiveButton("Speichern",(d,w)->{String name=input.getText().toString().trim();if(name.isEmpty()||name.length()>40){toast("Der Presetname muss 1 bis 40 Zeichen lang sein");return;}JSONArray semantics=new JSONArray();for(String semantic:selection)semantics.put(semantic);act(()->service().saveSignalPreset(name,semantics));}).setNegativeButton("Abbrechen",null).show();
    }
    private void chooseSignalPreset(Dialog selector){
        JSONObject vehicle=selectedVehicle();if(vehicle==null){toast("Kein Fahrzeug für Presets gewählt");return;}
        JSONArray presets=vehicle.optJSONArray("presets");if(presets==null||presets.length()==0){toast("Für dieses Fahrzeug sind keine Presets gespeichert");return;}
        ArrayList<String> names=new ArrayList<>();for(int i=0;i<presets.length();i++){JSONObject preset=presets.optJSONObject(i);if(preset!=null)names.add(preset.optString("name"));}
        new AlertDialog.Builder(this).setTitle("Preset anwenden").setItems(names.toArray(new String[0]),(d,which)->{String name=names.get(which);selector.dismiss();act(()->service().applySignalPreset(name));}).setNegativeButton("Abbrechen",null).show();
    }
    private void chooseProfile(){JSONArray p=array("profiles");String[] names=new String[p.length()];int chosen=0;for(int i=0;i<p.length();i++){JSONObject item=p.optJSONObject(i);names[i]=item.optString("title",item.optString("id"));if(item.optString("id").equals(profile))chosen=i;}new AlertDialog.Builder(this).setTitle("Aufnahmeprofil / Testsequenz").setSingleChoiceItems(names,chosen,(dialog,which)->{profile=p.optJSONObject(which).optString("id");structure="";dialog.dismiss();refresh();}).setNegativeButton("Schließen",null).show();}
    private void export(String name) {
        if(name.equals(activeRecordingName())) {toast("Aufzeichnung vor dem Export stoppen");return;}
        pendingExport=name;
        Intent save=new Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
            .setType("application/x-ndjson").putExtra(Intent.EXTRA_TITLE,name);
        try {startActivityForResult(save,41);} catch(Exception e) {pendingExport="";toast("Export nicht möglich: "+e.getMessage());}
    }
    @Override protected void onActivityResult(int request,int result,Intent data) {
        super.onActivityResult(request,result,data);
        if(request==42){
            Uri tree=result==RESULT_OK&&data!=null?data.getData():null;if(tree==null)return;
            int flags=Intent.FLAG_GRANT_READ_URI_PERMISSION|Intent.FLAG_GRANT_WRITE_URI_PERMISSION;
            try{getContentResolver().takePersistableUriPermission(tree,flags);}catch(SecurityException e){toast("Ordner nicht dauerhaft nutzbar: "+e.getMessage());return;}
            String old=RecordingExporter.folder(this);
            if(!old.isEmpty()&&!old.equals(tree.toString()))try{getContentResolver().releasePersistableUriPermission(Uri.parse(old),flags);}catch(RuntimeException ignored){}
            RecordingExporter.setFolder(this,tree.toString());toast("Export-Ordner gespeichert");structure="";refresh();return;
        }
        if(request!=41)return;
        String name=pendingExport;pendingExport="";
        if(result!=RESULT_OK||data==null||data.getData()==null||name.isEmpty())return;
        Uri destination=data.getData();
        Uri source=new Uri.Builder().scheme("content").authority(RecordingExporter.authority(this)).appendPath("recordings").appendPath(name).build();
        new Thread(()->{
            try {
                try(java.io.InputStream input=getContentResolver().openInputStream(source);
                    java.io.OutputStream output=getContentResolver().openOutputStream(destination,"wt")) {
                    if(input==null||output==null)throw new java.io.IOException("Dateizugriff nicht verfügbar");
                    byte[] buffer=new byte[8192];int count;
                    while((count=input.read(buffer))!=-1)output.write(buffer,0,count);
                    output.flush();
                }
                runOnUiThread(()->toast("Aufzeichnung gespeichert"));
            } catch(Exception e) {runOnUiThread(()->toast("Export fehlgeschlagen: "+e.getMessage()));}
        },"obdentic-export").start();
    }
    private void collectHistory(){JSONArray values=array("values");String mode=state.optString("mode");for(int i=0;i<values.length();i++){JSONObject v=values.optJSONObject(i);if(v==null)continue;String id=v.optString("semantic"),stamp=mode+v.optString("updatedAt")+v.optString("value");if(stamp.equals(lastSamples.get(id)))continue;lastSamples.put(id,stamp);JSONArray recordedHistory=v.optJSONArray("history");if(recordedHistory!=null){ArrayList<Float> restored=new ArrayList<>();for(int j=0;j<recordedHistory.length();j++){JSONObject sample=recordedHistory.optJSONObject(j);if(sample!=null&&Double.isFinite(sample.optDouble("value",Double.NaN)))restored.add((float)sample.optDouble("value"));}histories.put(id,restored);continue;}double number=v.optDouble("value",Double.NaN);if(!Double.isFinite(number))continue;ArrayList<Float> h=histories.get(id);if(h==null){h=new ArrayList<>();histories.put(id,h);}h.add((float)number);while(h.size()>90)h.remove(0);}}
    /** #239: freshness text shown on a tile only when a live value is stale (> 10 s); empty otherwise. */
    public static String staleText(long ageSeconds,boolean replay){return ageSeconds>10&&!replay?"vor "+ageSeconds+" s":"";}
    /** #239: "min … · max …" over a signal's session history. */
    public static String rangeText(ArrayList<Float> h,String unit){if(h==null||h.isEmpty())return "";float lo=h.get(0),hi=lo;for(float f:h){lo=Math.min(lo,f);hi=Math.max(hi,f);}return "min "+MeasurementFormat.formatMeasurement(lo,unit)+" · max "+MeasurementFormat.formatMeasurement(hi,unit);}
    private void updateValues(){JSONArray values=array("values");for(int i=0;i<values.length();i++){JSONObject v=values.optJSONObject(i);if(v==null)continue;String id=v.optString("semantic");TextView target=valueViews.get(id);if(target==null)continue;String error=v.optString("error","");String value=v.has("value")?MeasurementFormat.formatReading(v):MeasurementFormat.formatMeasurement(v.optString("text","—"),v.optString("unit"));long age=Math.max(0,(System.currentTimeMillis()-v.optLong("updatedAt",System.currentTimeMillis()))/1000);boolean replay=state.optString("mode").equals("replay"),stale=age>10&&!replay;String stateText=staleText(age,replay);target.setText(error.isEmpty()?value:"Nicht verfügbar");target.setTextColor(error.isEmpty()&&!stale?TEXT:0xffffca75);target.setContentDescription(signalLabel(id,signal(id))+": "+target.getText()+(error.isEmpty()?(stale?". Veraltet, "+stateText:""):". Nicht verfügbar. "+error));TextView freshness=freshnessViews.get(id);if(freshness!=null){freshness.setText(stateText);freshness.setVisibility(error.isEmpty()&&stale?View.VISIBLE:View.GONE);}TextView range=rangeViews.get(id);if(range!=null)range.setText(rangeText(histories.get(id),v.optString("unit")));SparklineView graph=graphViews.get(id);if(graph!=null)graph.setValues(histories.get(id));}}
    private JSONObject signal(String id){JSONArray signals=array("signals");for(int i=0;i<signals.length();i++){JSONObject s=signals.optJSONObject(i);if(s!=null&&s.optString("semantic").equals(id))return s;}JSONObject fallback=new JSONObject();try{fallback.put("semantic",id);fallback.put("title",id);}catch(Exception ignored){}return fallback;}
    private String signalLabel(String id,JSONObject signal){return SignalNames.shortName(signal.optString("meaning",id),signal.optString("title",""));}
    private JSONArray array(String key){JSONArray a=state.optJSONArray(key);return a==null?new JSONArray():a;}
    private boolean contains(JSONArray a,String s){for(int i=0;i<a.length();i++)if(s.equals(a.optString(i)))return true;return false;}
    private void title(String title,String subtitle){TextView t=text(title,25,TEXT);t.setTypeface(null,Typeface.BOLD);body.addView(t);note(body,subtitle,MUTED);}
    private void empty(String title,String detail){LinearLayout c=card();note(c,title,TEXT);note(c,detail,MUTED);}
    private LinearLayout card(){LinearLayout c=new LinearLayout(this);c.setOrientation(LinearLayout.VERTICAL);c.setPadding(dp(16),dp(14),dp(16),dp(14));c.setBackground(surface(CARD,20));LinearLayout.LayoutParams p=new LinearLayout.LayoutParams(-1,-2);p.setMargins(0,dp(12),0,0);body.addView(c,p);return c;}
    private LinearLayout row(){LinearLayout r=new LinearLayout(this);r.setOrientation(LinearLayout.HORIZONTAL);r.setGravity(Gravity.CENTER_VERTICAL);return r;}
    private TextView text(String value,int size,int color){TextView t=new TextView(this);t.setText(value);t.setTextSize(size);t.setTextColor(color);t.setLineSpacing(dp(3),1);return t;}
    private void note(LinearLayout p,String value,int color){TextView t=text(value,14,color);t.setPadding(0,dp(4),0,dp(7));p.addView(t);}
    private void smallNote(LinearLayout p,String value,int color){TextView t=text(value,12,color);t.setPadding(0,dp(2),0,dp(5));p.addView(t);}
    private Button button(String title,Runnable action){Button b=new Button(this);b.setText(title);b.setTextSize(14);b.setAllCaps(false);b.setTextColor(new android.content.res.ColorStateList(new int[][]{new int[]{-android.R.attr.state_enabled},new int[]{}},new int[]{0xff78899c,TEXT}));b.setMinHeight(dp(48));b.setMinimumHeight(dp(48));b.setPadding(dp(16),dp(8),dp(16),dp(8));b.setBackgroundTintList(new android.content.res.ColorStateList(new int[][]{new int[]{-android.R.attr.state_enabled},new int[]{}},new int[]{0xff1d2938,0xff28394b}));b.setOnClickListener(v->action.run());return b;}
    private Button addButton(LinearLayout parent,String title,Runnable action){Button b=button(title,action);LinearLayout.LayoutParams p=new LinearLayout.LayoutParams(-1,-2);p.setMargins(0,dp(5),0,dp(3));parent.addView(b,p);return b;}
    private GradientDrawable surface(int color,int radius){GradientDrawable d=new GradientDrawable();d.setColor(color);d.setCornerRadius(dp(radius));return d;}
    private void toast(String s){Toast.makeText(this,s,Toast.LENGTH_LONG).show();}
    private int dp(int n){return Math.round(n*getResources().getDisplayMetrics().density);}
}
