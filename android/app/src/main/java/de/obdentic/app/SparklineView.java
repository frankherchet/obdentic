package de.obdentic.app;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Path;
import android.view.View;
import java.util.ArrayList;

/** Small fixed-height trend. No animation: values may change several times a second. */
public final class SparklineView extends View {
    private final Paint paint=new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Path path=new Path();
    private ArrayList<Float> values;
    private final int color;
    public SparklineView(Context context,int color){super(context);this.color=color;setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);}
    public void setValues(ArrayList<Float> values){this.values=values;invalidate();}
    @Override protected void onDraw(Canvas canvas){super.onDraw(canvas);float density=getResources().getDisplayMetrics().density;paint.setStyle(Paint.Style.STROKE);paint.setStrokeWidth(density);paint.setColor(0xff334456);canvas.drawLine(0,getHeight()-density,getWidth(),getHeight()-density,paint);if(values==null||values.size()<2)return;float min=Float.MAX_VALUE,max=-Float.MAX_VALUE;for(float v:values){min=Math.min(min,v);max=Math.max(max,v);}float range=Math.max(max-min,Math.max(Math.abs(max)*0.02f,1));path.reset();for(int i=0;i<values.size();i++){float x=i*(getWidth()/(float)(values.size()-1));float y=getHeight()-6*density-(values.get(i)-min)/range*(getHeight()-12*density);if(i==0)path.moveTo(x,y);else path.lineTo(x,y);}paint.setColor(color);paint.setStrokeWidth(2*density);paint.setStrokeJoin(Paint.Join.ROUND);canvas.drawPath(path,paint);}
}
