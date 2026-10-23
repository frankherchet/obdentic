package de.obdentic.app;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
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
import android.view.Gravity;
import android.view.View;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;
import org.json.JSONArray;
import org.json.JSONObject;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.Map;

/** Local-only diagnostic workbench. Transport and jobs belong to ObdService. */
public class MainActivity extends Activity {
    private static final int BG=0xff0c121b, CARD=0xff17212e, TEXT=0xffedf3fa, MUTED=0xffa5b5c7, ACCENT=0xff63dfc1;
    private final Handler handler=new Handler(Looper.getMainLooper());
    private final Map<String,TextView> valueViews=new HashMap<>();
    private final Map<String,TextView> freshnessViews=new HashMap<>();
    private final Map<String,SparklineView> graphViews=new HashMap<>();
    private final Map<String,ArrayList<Float>> histories=new HashMap<>();
    private final Map<String,String> lastSamples=new HashMap<>();
    private JSONObject state=new JSONObject();
    private LinearLayout body, nav;
    private ScrollView scroll;
    private TextView badge, status;
    private Button chip;
    private TextView debugLog;
    private int tab=0;
    private String structure="", profile="";
    private String historyMode="";
    private boolean launchDemo=false, scanAfterPermission=false;
    private String pendingExport="";
    private final Runnable poll=new Runnable(){public void run(){refresh();handler.postDelayed(this,500);}};

    @Override public void onCreate(Bundle saved){
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
    @Override protected void onResume(){super.onResume();handler.post(poll);}
    @Override protected void onPause(){handler.removeCallbacks(poll);super.onPause();}
    @Override protected void onSaveInstanceState(Bundle out){out.putInt("tab",tab);out.putString("profile",profile);out.putString("pendingExport",pendingExport);super.onSaveInstanceState(out);}
    private ObdService service(){return ObdService.current;}
    private void act(Runnable action){try{if(service()!=null)action.run();else toast("Dienst startet. Bitte kurz warten.");}catch(Exception e){toast(e.getMessage()==null?"Aktion fehlgeschlagen":e.getMessage());}refresh();}
    private void refresh(){
        if(service()==null)return;
        if(launchDemo){launchDemo=false;service().setDemo();}
        state=service().snapshot();
        JSONObject chosen=selectedVehicle();String chipName=chosen==null?"Kein Fahrzeug gewählt":chosen.optString("name");
        chip.setText(chipName);chip.setContentDescription("Gewähltes Fahrzeug: "+(chosen==null?"keines":chipName));
        String mode=state.optString("mode","live"), phase=state.optString("phase","idle");
        String replayBadge="REPLAY · "+(state.optString("replaySourceMode").equals("demo")?"DEMO":"DATEI");
        if(mode.equals("replay")&&state.optBoolean("replayPartial"))replayBadge+="\nTEILAUFNAHME";
        badge.setText(mode.equals("demo")?"DEMO · SIMULIERT":mode.equals("replay")?replayBadge:phase.equals("ready")?"LIVE · ADAPTER":"LIVE · GETRENNT");
        int tint=mode.equals("live")?ACCENT:0xffffca75;badge.setTextColor(tint);
        String message=state.optString("message","");
        status.setText(phaseName(phase)+(state.optBoolean("busy")?" · Auftrag läuft":"")+(message.isEmpty()?"":"\n"+message));
        status.setTextColor(phase.equals("error")?0xffffa79c:MUTED);
        if(!historyMode.equals(mode)){historyMode=mode;histories.clear();lastSamples.clear();}
        collectHistory();
        String key=tab+"|"+mode+"|"+phase+"|"+state.optString("recording")+"|"+array("selected").toString()+"|"+state.opt("vehicles");
        if(tab==1||tab==2||tab==4)key+="|"+state.optBoolean("busy");
        if(tab==1)key+="|"+state.opt("diagnostics")+"|"+state.opt("jobResult")+"|"+state.opt("vehicle");
        if(tab==2)key+="|"+array("records")+"|"+array("profiles")+"|"+profile+"|"+state.optString("replayFile")+"|"+state.opt("testSequence")+"|"+state.opt("ashEvidence");
        if(tab==3)key+="|"+(array("debug").length()==0);
        if(tab==4)key+="|"+array("devices").toString();
        if(!key.equals(structure)){structure=key;int y=scroll.getScrollY();render();scroll.post(()->scroll.scrollTo(0,y));}
        updateValues();
        if(debugLog!=null){String content=debugText();if(!content.contentEquals(debugLog.getText()))debugLog.setText(content);}
    }
    private String phaseName(String p){switch(p){case "ready":return "Verbunden · bereit";case "scanning":return "Adapter werden gesucht …";case "connecting":return "Verbindung wird aufgebaut …";case "disconnected":return "Getrennt";case "error":return "Verbindungsfehler";case "replay":return "Gespeicherte Sitzung";default:return "Bereit zum Verbinden";}}
    private void buildNav(){nav.removeAllViews();nav.setWeightSum(5);String[] titles={"Live","Diagnose","Aufnahmen","Debug","Adapter"};for(int i=0;i<titles.length;i++){final int next=i;Button b=button(titles[i],()->{tab=next;structure="";scroll.scrollTo(0,0);buildNav();refresh();});b.setTextSize(12);b.setMinWidth(0);b.setMinimumWidth(0);b.setPadding(dp(2),dp(4),dp(2),dp(4));b.setTextColor(tab==i?BG:MUTED);b.setBackground(surface(tab==i?ACCENT:CARD,12));b.setBackgroundTintList(null);LinearLayout.LayoutParams p=new LinearLayout.LayoutParams(0,dp(48),1);p.setMargins(dp(2),0,dp(2),0);nav.addView(b,p);}}
    private void render(){body.removeAllViews();valueViews.clear();freshnessViews.clear();graphViews.clear();debugLog=null;switch(tab){case 0:live();break;case 1:diagnose();break;case 2:recordings();break;case 3:debug();break;case 5:vehicles();break;default:connection();}}
    private void live(){
        title("Live-Messwerte","Werte und Verlauf auf einen Blick");
        addButton(body,"Messwerte auswählen",this::chooseSignals).setEnabled(state.optString("recording").isEmpty());
        if(!state.optString("recording").isEmpty())note(body,"● Aufzeichnung läuft · "+state.optString("recording"),ACCENT);
        JSONArray selected=array("selected");JSONArray values=array("values");
        LinkedHashMap<String,JSONObject> shown=new LinkedHashMap<>();
        for(int i=0;i<selected.length();i++){String s=selected.optString(i);shown.put(s,signal(s));}
        for(int i=0;i<values.length();i++){JSONObject v=values.optJSONObject(i);if(v!=null&&selected.length()==0)shown.put(v.optString("semantic"),v);}
        if(shown.isEmpty())empty("Noch keine Messwerte","Wähle Messwerte aus und verbinde deinen Adapter. Mit „Demo starten“ unter Adapter kannst du die Oberfläche ohne Auto testen.");
        for(Map.Entry<String,JSONObject> entry:shown.entrySet()){
            String id=entry.getKey();JSONObject s=entry.getValue();LinearLayout card=card();
            note(card,signalLabel(id,s),TEXT);TextView number=text("—",32,TEXT);number.setTypeface(Typeface.MONOSPACE);number.setFontFeatureSettings("tnum");card.addView(number);valueViews.put(id,number);
            SparklineView graph=new SparklineView(this,ACCENT);card.addView(graph,new LinearLayout.LayoutParams(-1,dp(56)));graphViews.put(id,graph);
            TextView freshness=text("Warte auf erste Antwort …",12,MUTED);card.addView(freshness);freshnessViews.put(id,freshness);
            smallNote(card,id,0xff8297ad);
        }
        JSONObject vehicle=state.optJSONObject("vehicle");if(vehicle!=null&&vehicle.length()>0){LinearLayout c=card();note(c,"FAHRZEUG",ACCENT);note(c,vehicle.optString("vin","Fahrzeug erfasst"),TEXT);JSONArray responders=vehicle.optJSONArray("responders");if(responders!=null)note(c,responders.length()+" Responder · Details unter Diagnose",MUTED);}
    }
    private void diagnose(){
        title("Diagnose","Lesende Aufträge · Ergebnisse bleiben lokal");
        LinearLayout c=card();note(c,"FAHRZEUG & FEHLERSPEICHER",ACCENT);
        job(c,"Fahrzeug und Steuergeräte erfassen","inventory");job(c,"Fehlercodes lesen","dtc.scan");
        LinearLayout d=card();note(d,"EXPERIMENTELL · EA189 / DPF",0xffffca75);note(d,"Vorhandene DPF-Messgrößen abfragen. Verfügbarkeit und Interpretation hängen vom Fahrzeug ab.",MUTED);job(d,"DPF-Diagnose ausführen","ea189.dpf.probe");
        LinearLayout extra=card();note(extra,"EXPERIMENTELL · EA189 ZUSATZWERTE",0xffffca75);
        note(extra,"Nur für ein passendes EA189-Motorsteuergerät wählen. Zusätzliche Öl-, Ladedruck-, Kraftstoff- und Regenerationswerte sind noch nicht am Fahrzeug bestätigt. Statuszahlen bleiben ohne gesicherte Bedeutungszuordnung.",MUTED);
        job(extra,"EA189-Zusatzwerte abfragen","ea189.extended.probe");

        JSONObject vehicle=state.optJSONObject("vehicle");
        if(vehicle!=null&&vehicle.length()>0)showInventory(vehicle);
        JSONObject dtcs=state.optJSONObject("diagnostics");
        if(dtcs!=null&&dtcs.length()>0)showDtcs(dtcs);
        JSONObject result=state.optJSONObject("jobResult");JSONObject data=result==null?null:result.optJSONObject("data");
        if(data!=null&&data.optJSONArray("readings")!=null)showDpf(data.optJSONArray("readings"));
        else if(result!=null&&result.length()>0&&!result.optBoolean("ok",true)&&!result.optString("error").isEmpty()){
            LinearLayout error=card();note(error,"AUFTRAG NICHT ABGESCHLOSSEN",0xffffa79c);note(error,result.optString("error"),TEXT);
        }
        if((vehicle==null||vehicle.length()==0)&&(dtcs==null||dtcs.length()==0)&&(result==null||result.length()==0))empty("Bereit für den ersten Auftrag","Verbinde den Adapter oder starte die Demo. Rohdaten und Antworten findest du unter Debug.");
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
    private void showDpf(JSONArray readings){
        LinearLayout c=card();note(c,"EA189-MESSWERTE · EXPERIMENTELL",0xffffca75);
        for(int i=0;i<readings.length();i++){JSONObject reading=readings.optJSONObject(i);if(reading==null)continue;String semantic=reading.optString("semantic");note(c,dpfLabel(semantic),MUTED);String error=reading.optString("error","");
            note(c,error.isEmpty()?reading.optString("value","—")+" "+reading.optString("unit",""):"Nicht verfügbar · "+error,error.isEmpty()?TEXT:0xffffa79c);
            if(reading.has("provenance"))smallNote(c,reading.optString("provenance"),0xff8297ad);
        }
    }
    private String displayResponder(String responder){return responder.isEmpty()?"Unbekannter Responder":responder;}
    private String dpfLabel(String semantic){switch(semantic){case "dpf.soot_mass_calculated":return "Berechnete Rußmasse";case "dpf.soot_mass_measured":return "Gemessene Rußmasse";case "dpf.distance_since_regeneration":return "Strecke seit Regeneration";case "dpf.time_since_regeneration":return "Zeit seit Regeneration";case "exhaust.temperature.pre_dpf":return "Abgastemperatur vor DPF (Zuordnung experimentell)";case "exhaust.temperature.post_dpf":return "Abgastemperatur nach DPF (Zuordnung experimentell)";case "dpf.differential_pressure":return "Differenzdruck am DPF";case "dpf.oil_ash_volume":return "Öl-Aschevolumen";case "dpf.fuel_consumption_since_regeneration":return "Kraftstoff seit letzter Regeneration";case "dpf.field_regeneration_request_status":return "Regenerationsanforderung · Rohstatus";case "dpf.service_regeneration_status":return "Service-Regeneration · Rohstatus";case "dpf.field_regeneration_lock_status":return "Regenerationssperre · Rohstatus";case "engine.boost_pressure_actual":return "Ladedruck Ist";case "engine.boost_pressure_commanded":return "Ladedruck Soll";case "engine.oil_temperature":return "Motoröltemperatur";case "engine.fuel_temperature":return "Kraftstofftemperatur";case "engine.rail_pressure_actual":return "Raildruck Ist";case "engine.rail_pressure_commanded":return "Raildruck Soll";case "engine.injector_deviation.cylinder_1":return "Einspritzmengenabweichung Zylinder 1";case "engine.injector_deviation.cylinder_2":return "Einspritzmengenabweichung Zylinder 2";case "engine.injector_deviation.cylinder_3":return "Einspritzmengenabweichung Zylinder 3";case "engine.injector_deviation.cylinder_4":return "Einspritzmengenabweichung Zylinder 4";default:return semantic;}}
    private void job(LinearLayout parent,String label,String action){Button b=addButton(parent,label,()->act(()->service().runJob(action)));b.setEnabled(canRun());}
    private boolean canRun(){String mode=state.optString("mode");return !state.optBoolean("busy")&&!mode.equals("replay")&&(mode.equals("demo")||state.optString("phase").equals("ready"));}
    private void recordings(){
        title("Aufnahmen","Lokal sichern · später ohne Auto untersuchen");
        JSONArray profiles=array("profiles");if(profile.isEmpty()&&profiles.length()>0)profile=profiles.optJSONObject(0).optString("id");
        JSONObject chosen=new JSONObject();for(int i=0;i<profiles.length();i++)if(profile.equals(profiles.optJSONObject(i).optString("id")))chosen=profiles.optJSONObject(i);
        LinearLayout c=card();note(c,"AUFNAHMEPROFIL / TESTSEQUENZ",ACCENT);addButton(c,profile.isEmpty()?"Profil auswählen":chosen.optString("title",profile),this::chooseProfile);
        if(profile.equals(TestSequence.PROFILE))note(c,"Einmaliger Test: Motorsteuergerät prüfen → beide Asche-Kandidaten lesen → DPF-Vergleich → Drehzahl prüfen. Stoppt automatisch. Antworten bleiben experimentelle Rohdaten; es erfolgt kein Sitzungswechsel.",MUTED);
        boolean running=!state.optString("recording").isEmpty();
        Button start=addButton(c,running?"Aufzeichnung stoppen":profile.equals(TestSequence.PROFILE)?"Testlauf vorbereiten …":"Aufzeichnung starten",()->act(()->{if(running)service().stopRecording();else if(profile.equals(TestSequence.PROFILE))prepareTestSequence();else service().startRecording(profile);}));start.setEnabled(running||canRun());
        note(c,running?"● "+state.optString("recording"):"Aufzeichnungen enthalten möglicherweise Fahrzeugkennungen. Export erfolgt nur auf deine ausdrückliche Auswahl.",running?ACCENT:MUTED);
        JSONObject sequence=state.optJSONObject("testSequence");
        if(sequence!=null&&sequence.length()>0){note(c,sequence.optString("context_label")+" · selbst angegeben",MUTED);String outcome=sequence.optString("status");note(c,outcome.equals("running")?"Schritt "+sequence.optInt("step")+" / "+sequence.optInt("total")+" · "+sequence.optString("label"):outcome.equals("completed")?"Testlauf aufgezeichnet · Auswertung offen":outcome.equals("incomplete")?"Unvollständige Aufnahme · Testlauf nicht abgeschlossen":"Testlauf abgebrochen · Teilaufnahme gesichert",outcome.equals("aborted")||outcome.equals("incomplete")?0xffffa79c:ACCENT);}
        JSONObject ash=state.optJSONObject("ashEvidence");if(ash!=null&&ash.optJSONArray("readings")!=null)showAshEvidence(ash.optJSONArray("readings"));
        JSONArray records=array("records");if(records.length()==0)empty("Noch keine Aufnahmen","Starte eine Aufzeichnung im Live- oder Demomodus. Die Dateien bleiben auf diesem Handy.");
        for(int i=0;i<records.length();i++){JSONObject record=records.optJSONObject(i);if(record==null)continue;String name=record.optString("name");LinearLayout item=card();note(item,name,TEXT);note(item,record.optLong("bytes")+" Bytes",MUTED);addButton(item,"Als Replay öffnen",()->act(()->{service().openRecording(name);tab=0;buildNav();structure="";}));addButton(item,"Datei exportieren …",()->export(name));Button delete=addButton(item,"Aufnahme löschen …",()->confirmDeleteRecording(name));delete.setEnabled(!name.equals(state.optString("recording"))&&!name.equals(state.optString("replayFile")));}
    }
    private void showAshEvidence(JSONArray readings){
        LinearLayout evidence=card();note(evidence,"ASCHE-KANDIDATEN · AUSWERTUNG OFFEN",ACCENT);
        for(int i=0;i<readings.length();i++){
            JSONObject reading=readings.optJSONObject(i);if(reading==null)continue;
            note(evidence,reading.optString("candidate").equals("ash_volume_1153")?"Aschevolumen-Kandidat · 1153":"Aschemassen-Kandidat · 178C",TEXT);
            String outcome=reading.optString("outcome");
            note(evidence,outcome.equals("positive_candidate")?"Antwort empfangen · Einheit und Skalierung noch zu prüfen":outcome.equals("negative_response")?"Vom Steuergerät abgelehnt · "+reading.optString("nrc"):"Nicht verfügbar · "+reading.optString("error","Keine auswertbare Antwort"),outcome.equals("positive_candidate")?MUTED:0xffffca75);
        }
        smallNote(evidence,"Originalantworten bleiben in der Aufnahme und unter Debug erhalten. Die Kandidaten werden nicht als bestätigte Aschewerte angezeigt.",MUTED);
    }
    private void prepareTestSequence(){
        String[] labels={"Zündung an · Motor aus","Motor im Leerlauf"};String[] contexts={"engine_off","idle"};
        new AlertDialog.Builder(this).setTitle("Betriebszustand für diesen Testlauf").setItems(labels,(dialog,which)->{
            String selectedProfile=profile;
            new AlertDialog.Builder(this).setTitle("Asche-Testlauf starten?").setMessage(labels[which]+"\n\nBitte diesen Zustand während des gesamten Testlaufs beibehalten. Deine Angabe wird zusammen mit den Messantworten gespeichert. Der Test endet automatisch; nicht verfügbare Werte bleiben als Ergebnis erhalten.").setPositiveButton("Testlauf starten",(d,w)->act(()->service().startRecording(selectedProfile,contexts[which]))).setNegativeButton("Abbrechen",null).show();
        }).setNegativeButton("Abbrechen",null).show();
    }
    private JSONObject vehicleStore(){JSONObject v=state.optJSONObject("vehicles");return v==null?new JSONObject():v;}
    private JSONObject selectedVehicle(){JSONObject store=vehicleStore();JSONArray rows=store.optJSONArray("vehicles");String selected=store.optString("selected");if(rows!=null)for(int i=0;i<rows.length();i++)if(selected.equals(rows.optJSONObject(i).optString("vin")))return rows.optJSONObject(i);return null;}
    private String date(long millis){return new java.text.SimpleDateFormat("dd.MM.yyyy",java.util.Locale.GERMANY).format(new java.util.Date(millis));}
    private String km(double value){java.text.NumberFormat f=java.text.NumberFormat.getNumberInstance(java.util.Locale.GERMANY);f.setMaximumFractionDigits(1);return f.format(value)+" km";}
    private void vehicles(){
        title("Fahrzeuge","Lokal gespeichert · nur auf diesem Handy");
        JSONObject store=vehicleStore();
        if(store.optBoolean("unreadable")){LinearLayout e=card();note(e,"FAHRZEUGSPEICHER NICHT LESBAR",0xffffa79c);note(e,"Die gespeicherten Fahrzeuge konnten nicht gelesen werden. Verbindungen funktionieren weiter. Die Datei wird nicht überschrieben, solange du den Speicher nicht selbst zurücksetzt.",MUTED);addButton(e,"Fahrzeugspeicher zurücksetzen …",this::confirmResetVehicles);return;}
        JSONArray rows=store.optJSONArray("vehicles");
        if(rows==null||rows.length()==0){empty("Noch keine Fahrzeuge","Verbinde den Adapter mit einem Fahrzeug. Die FIN wird beim ersten Verbinden gelesen und das Fahrzeug hier gespeichert.");return;}
        for(int i=0;i<rows.length();i++){
            JSONObject row=rows.optJSONObject(i);if(row==null)continue;String vin=row.optString("vin"),name=row.optString("name");boolean selected=vin.equals(store.optString("selected"));
            LinearLayout c=card();note(c,name+(selected?" · gewählt":""),TEXT);
            note(c,(vin.length()>=7?vin.substring(0,3)+"…"+vin.substring(vin.length()-4):vin)+" · "+(row.optJSONObject("odometer")==null?"km-Stand nicht verfügbar":km(row.optJSONObject("odometer").optDouble("km"))+" · "+date(row.optJSONObject("odometer").optLong("read_at"))),MUTED);
            note(c,"Zuletzt verbunden: "+date(row.optLong("last_connected")),MUTED);
            addButton(c,"Auswählen",()->act(()->service().selectVehicle(vin))).setEnabled(!selected);
            addButton(c,"Details",()->showVehicle(row));addButton(c,"Umbenennen …",()->renameVehicle(vin,name));addButton(c,"Löschen …",()->confirmDeleteVehicle(vin,name));
        }
    }
    private void showVehicle(JSONObject row){
        StringBuilder m=new StringBuilder("FIN: "+row.optString("vin")+"\nHerstellerhinweis: "+row.optString("manufacturer_hint","keiner"));
        JSONArray responders=row.optJSONArray("responders");StringBuilder ids=new StringBuilder();for(int i=0;responders!=null&&i<responders.length();i++){if(i>0)ids.append(", ");ids.append(responders.optString(i));}
        m.append("\nResponder: ").append(ids.length()==0?"keine":ids);
        JSONArray signals=row.optJSONArray("signals");int supported=0;for(int i=0;signals!=null&&i<signals.length();i++)if("supported".equals(signals.optJSONObject(i).optString("status")))supported++;
        m.append("\nMesswerte unterstützt: ").append(supported).append(" von ").append(signals==null?0:signals.length());
        m.append("\nMotorsteuergerät gezielt geprüft: ").append(row.optBoolean("engine_target_validated")?"ja":"nein");
        JSONObject odometer=row.optJSONObject("odometer"),failure=row.optJSONObject("odometer_error"),decrease=row.optJSONObject("odometer_decrease");
        m.append("\nkm-Stand: ").append(odometer==null?"nicht verfügbar":km(odometer.optDouble("km"))+" (Quelle: "+odometer.optString("source")+", gelesen "+date(odometer.optLong("read_at"))+")");
        if(failure!=null)m.append("\nLetzter Leseversuch fehlgeschlagen: ").append(failure.optString("error")).append(" (").append(date(failure.optLong("at"))).append(")");
        if(decrease!=null)m.append("\nNiedriger als zuvor gelesen: ").append(km(decrease.optDouble("previous_km"))).append(" am ").append(date(decrease.optLong("previous_at")));
        m.append("\nErste Verbindung: ").append(date(row.optLong("first_connected"))).append("\nLetzte Verbindung: ").append(date(row.optLong("last_connected")));
        new AlertDialog.Builder(this).setTitle(row.optString("name")).setMessage(m).setPositiveButton("Schließen",null).show();
    }
    private void renameVehicle(String vin,String name){
        android.widget.EditText input=new android.widget.EditText(this);input.setSingleLine(true);input.setText(name);input.selectAll();
        new AlertDialog.Builder(this).setTitle("Fahrzeug umbenennen").setMessage("1 bis 40 Zeichen").setView(input).setPositiveButton("Speichern",(d,w)->act(()->service().renameVehicle(vin,input.getText().toString()))).setNegativeButton("Abbrechen",null).show();
    }
    private void confirmDeleteVehicle(String vin,String name){new AlertDialog.Builder(this).setTitle("Fahrzeug endgültig löschen?").setMessage(name+"\n\nDer Eintrag wird dauerhaft aus dem internen Speicher gelöscht. Aufnahmen bleiben erhalten.").setPositiveButton("Endgültig löschen",(d,w)->act(()->service().deleteVehicle(vin))).setNegativeButton("Abbrechen",null).show();}
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
        LinearLayout c=card();addButton(c,"Adapter suchen",this::requestScan);Button disconnect=addButton(c,"Verbindung trennen",()->act(()->service().disconnect()));disconnect.setEnabled(!state.optString("phase").equals("idle"));
        JSONArray devices=array("devices");if(devices.length()==0)note(c,"Noch keine Adapter gefunden. Adapter mit Strom versorgen, Bluetooth einschalten und Suche starten.",MUTED);
        for(int i=0;i<devices.length();i++){JSONObject d=devices.optJSONObject(i);if(d==null)continue;String address=d.optString("address"),name=d.optString("name","Bluetooth-Gerät");LinearLayout device=card();note(device,name,TEXT);note(device,address+" · "+d.optInt("rssi")+" dBm",MUTED);Button b=addButton(device,"Verbinden",()->act(()->service().connect(address)));b.setEnabled(!state.optBoolean("busy"));}
        LinearLayout demo=card();note(demo,"OHNE FAHRZEUG TESTEN",ACCENT);note(demo,"Die Demo liefert simulierte Messwerte und Diagnoseantworten. Aufnahmen und Export lassen sich damit am Schreibtisch prüfen.",MUTED);addButton(demo,"Demo starten",()->act(()->{service().setDemo();tab=0;structure="";buildNav();}));
    }
    private void requestScan(){ArrayList<String> missing=new ArrayList<>();if(Build.VERSION.SDK_INT>=31){missing.add(Manifest.permission.BLUETOOTH_SCAN);missing.add(Manifest.permission.BLUETOOTH_CONNECT);}else missing.add(Manifest.permission.ACCESS_FINE_LOCATION);if(Build.VERSION.SDK_INT>=33)missing.add(Manifest.permission.POST_NOTIFICATIONS);missing.removeIf(p->checkSelfPermission(p)==PackageManager.PERMISSION_GRANTED);if(!missing.isEmpty()){scanAfterPermission=true;requestPermissions(missing.toArray(new String[0]),7);}else act(()->service().scan());}
    @Override public void onRequestPermissionsResult(int request,String[] permissions,int[] results){super.onRequestPermissionsResult(request,permissions,results);if(request==7&&scanAfterPermission){scanAfterPermission=false;boolean allowed=true;for(int i=0;i<permissions.length;i++)if(!permissions[i].equals(Manifest.permission.POST_NOTIFICATIONS)&&results[i]!=PackageManager.PERMISSION_GRANTED)allowed=false;if(allowed)act(()->service().scan());else new AlertDialog.Builder(this).setTitle("Bluetooth-Berechtigung fehlt").setMessage("Erlaube Geräte in der Nähe in den Android-App-Einstellungen, um den Adapter zu suchen und zu verbinden.").setPositiveButton("Einstellungen",(d,w)->startActivity(new Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,Uri.parse("package:"+getPackageName())))).setNegativeButton("Schließen",null).show();}}
    private void chooseSignals(){JSONArray signals=array("signals");String[] labels=new String[signals.length()];boolean[] checked=new boolean[signals.length()];for(int i=0;i<signals.length();i++){JSONObject s=signals.optJSONObject(i);String semantic=s.optString("semantic");labels[i]=signalLabel(semantic,s)+"  ·  "+s.optString("unit")+(s.optString("status").equals("unsupported")?" · nicht unterstützt":s.optString("status").equals("supported")?"":" · ungeprüft");checked[i]=contains(array("selected"),semantic);}new AlertDialog.Builder(this).setTitle("Live-Messwerte · maximal 32").setMultiChoiceItems(labels,checked,(d,which,on)->checked[which]=on).setPositiveButton("Übernehmen",(d,w)->{JSONArray selected=new JSONArray();for(int i=0;i<checked.length;i++)if(checked[i])selected.put(signals.optJSONObject(i).optString("semantic"));act(()->service().setSignals(selected));}).setNegativeButton("Abbrechen",null).show();}
    private void chooseProfile(){JSONArray p=array("profiles");String[] names=new String[p.length()];int chosen=0;for(int i=0;i<p.length();i++){JSONObject item=p.optJSONObject(i);names[i]=item.optString("title",item.optString("id"));if(item.optString("id").equals(profile))chosen=i;}new AlertDialog.Builder(this).setTitle("Aufnahmeprofil / Testsequenz").setSingleChoiceItems(names,chosen,(dialog,which)->{profile=p.optJSONObject(which).optString("id");structure="";dialog.dismiss();refresh();}).setNegativeButton("Schließen",null).show();}
    private void export(String name) {
        if(name.equals(state.optString("recording"))) {toast("Aufzeichnung vor dem Export stoppen");return;}
        pendingExport=name;
        Intent save=new Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
            .setType("application/x-ndjson").putExtra(Intent.EXTRA_TITLE,name);
        try {startActivityForResult(save,41);} catch(Exception e) {pendingExport="";toast("Export nicht möglich: "+e.getMessage());}
    }
    @Override protected void onActivityResult(int request,int result,Intent data) {
        super.onActivityResult(request,result,data);
        if(request!=41)return;
        String name=pendingExport;pendingExport="";
        if(result!=RESULT_OK||data==null||data.getData()==null||name.isEmpty())return;
        Uri destination=data.getData();
        Uri source=new Uri.Builder().scheme("content").authority("de.obdentic.app.files").appendPath("recordings").appendPath(name).build();
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
    private void updateValues(){JSONArray values=array("values");for(int i=0;i<values.length();i++){JSONObject v=values.optJSONObject(i);if(v==null)continue;String id=v.optString("semantic");TextView target=valueViews.get(id);if(target==null)continue;String error=v.optString("error","");String value=v.has("value")?v.optString("value"):v.optString("text","—");long age=Math.max(0,(System.currentTimeMillis()-v.optLong("updatedAt",System.currentTimeMillis()))/1000);boolean replay=state.optString("mode").equals("replay"),stale=age>10&&!replay;target.setText(error.isEmpty()?value+" "+v.optString("unit"):"Nicht verfügbar");target.setTextColor(error.isEmpty()&&!stale?TEXT:0xffffca75);target.setContentDescription(signalLabel(id,signal(id))+": "+target.getText()+(error.isEmpty()?"":". "+error));TextView freshness=freshnessViews.get(id);if(freshness!=null)freshness.setText(!error.isEmpty()?error:replay?"Aus gespeicherter Aufnahme":age<2?"Gerade aktualisiert":(stale?"Letzter Wert · ":"Aktualisiert · ")+"vor "+age+" s");SparklineView graph=graphViews.get(id);if(graph!=null)graph.setValues(histories.get(id));}}
    private JSONObject signal(String id){JSONArray signals=array("signals");for(int i=0;i<signals.length();i++){JSONObject s=signals.optJSONObject(i);if(s!=null&&s.optString("semantic").equals(id))return s;}JSONObject fallback=new JSONObject();try{fallback.put("semantic",id);fallback.put("title",id);}catch(Exception ignored){}return fallback;}
    private String signalLabel(String id,JSONObject signal){switch(id){case "engine.rpm":return "Motordrehzahl";case "engine.coolant_temperature":return "Kühlmitteltemperatur";case "vehicle.speed":return "Fahrzeuggeschwindigkeit";case "engine.maf":return "Luftmassenstrom";case "engine.load":return "Motorlast";case "engine.intake_manifold_pressure":return "Saugrohrdruck";case "engine.intake_air_temperature":return "Ansauglufttemperatur";case "engine.egr.commanded":return "AGR-Ansteuerung";case "engine.egr.error":return "AGR-Abweichung";case "engine.runtime":return "Motorlaufzeit seit Start";case "vehicle.accelerator_pedal_d":return "Gaspedalstellung D";case "vehicle.accelerator_pedal_e":return "Gaspedalstellung E";case "engine.relative_throttle":return "Relative Drosselklappenstellung";case "engine.barometric_pressure":return "Umgebungsdruck";case "engine.control_module_voltage":return "Steuergerätspannung";case "engine.throttle_position":return "Drosselklappenstellung";case "vehicle.distance_with_mil_on":return "Strecke mit Motorkontrollleuchte";case "engine.fuel_rail_gauge_pressure":return "Kraftstoffrail-Druck";case "vehicle.warmups_since_dtc_clear":return "Warmläufe seit Fehlerlöschung";case "vehicle.distance_since_dtc_clear":return "Strecke seit Fehlerlöschung";case "vehicle.ambient_air_temperature":return "Außentemperatur";case "engine.throttle_actuator.commanded":return "Sollwert Drosselklappensteller";case "engine.short_term_fuel_trim_bank1":return "Kurzzeit-Kraftstoffkorrektur Bank 1";case "engine.long_term_fuel_trim_bank1":return "Langzeit-Kraftstoffkorrektur Bank 1";case "engine.short_term_fuel_trim_bank2":return "Kurzzeit-Kraftstoffkorrektur Bank 2";case "engine.long_term_fuel_trim_bank2":return "Langzeit-Kraftstoffkorrektur Bank 2";case "engine.fuel_rail_gauge_pressure_low":return "Niederdruck-Kraftstoffdruck";case "engine.timing_advance":return "Zündwinkel";case "engine.fuel_rail_pressure_relative":return "Raildruck relativ zum Saugrohr";case "engine.evap_purge_commanded":return "Tankentlüftung Soll";case "vehicle.fuel_tank_level":return "Tankfüllstand";case "engine.catalyst_temperature_b1s1":return "Katalysatortemperatur Bank 1 Sensor 1";case "engine.catalyst_temperature_b2s1":return "Katalysatortemperatur Bank 2 Sensor 1";case "engine.catalyst_temperature_b1s2":return "Katalysatortemperatur Bank 1 Sensor 2";case "engine.catalyst_temperature_b2s2":return "Katalysatortemperatur Bank 2 Sensor 2";case "engine.absolute_load":return "Absolute Motorlast";case "engine.commanded_equivalence_ratio":return "Gemisch-Äquivalenzverhältnis Soll";case "engine.throttle_position_b":return "Drosselklappenstellung B";case "engine.throttle_position_c":return "Drosselklappenstellung C";case "vehicle.accelerator_pedal_f":return "Gaspedalstellung F";case "engine.ethanol_fuel_percentage":return "Ethanolanteil";case "engine.fuel_rail_pressure_absolute":return "Absoluter Kraftstoffrail-Druck";case "vehicle.relative_accelerator_pedal_position":return "Relative Gaspedalstellung";case "engine.oil_temperature":return "Motoröltemperatur";case "engine.fuel_injection_timing":return "Einspritzzeitpunkt";case "engine.fuel_rate":return "Kraftstoffverbrauch pro Stunde";case "engine.driver_demand_torque_percent":return "Fahrerdrehmomentwunsch";case "engine.actual_torque_percent":return "Tatsächliches Motordrehmoment";case "engine.reference_torque":return "Referenzdrehmoment";default:String experimental=dpfLabel(id);if(!experimental.equals(id))return experimental;String label=signal.optString("title","");return label.isEmpty()?"Messwert":label;}}
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
