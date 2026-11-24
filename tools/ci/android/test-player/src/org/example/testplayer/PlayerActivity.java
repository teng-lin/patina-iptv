package org.example.testplayer;

import android.app.Activity;
import android.media.MediaPlayer;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.util.Log;
import android.view.SurfaceHolder;
import android.view.SurfaceView;

/**
 * A stand-in for another installed video app: it takes a video URL from an
 * ACTION_VIEW intent, plays it with the platform MediaPlayer and logs its
 * position every ten seconds under the tag "TestPlayer", so a long handoff
 * shows in logcat whether playback kept advancing.
 */
public final class PlayerActivity extends Activity implements SurfaceHolder.Callback {
    private static final String TAG = "TestPlayer";
    private final Handler main = new Handler(Looper.getMainLooper());
    private MediaPlayer player;
    private long started;

    private final Runnable report = new Runnable() {
        @Override public void run() {
            if (player != null) {
                Log.i(TAG, "elapsed " + (SystemClock.elapsedRealtime() - started) / 1000
                    + " s position " + player.getCurrentPosition() / 1000
                    + " s playing " + player.isPlaying());
            }
            main.postDelayed(this, 10_000);
        }
    };

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        SurfaceView view = new SurfaceView(this);
        setContentView(view);
        view.getHolder().addCallback(this);
    }

    @Override public void surfaceCreated(SurfaceHolder holder) {
        if (player != null) {
            player.setDisplay(holder);
            return;
        }
        player = new MediaPlayer();
        player.setOnPreparedListener(prepared -> {
            Log.i(TAG, "prepared");
            prepared.start();
        });
        player.setOnErrorListener((failed, what, extra) -> {
            Log.w(TAG, "error " + what + " " + extra);
            return false;
        });
        player.setOnInfoListener((source, what, extra) -> {
            Log.i(TAG, "info " + what + " " + extra);
            return false;
        });
        player.setOnCompletionListener(done -> Log.i(TAG, "completed"));
        try {
            player.setDisplay(holder);
            player.setDataSource(this, getIntent().getData());
            started = SystemClock.elapsedRealtime();
            player.prepareAsync();
            main.postDelayed(report, 10_000);
            Log.i(TAG, "opened");
        } catch (Exception error) {
            Log.w(TAG, "open failed", error);
        }
    }

    @Override public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {}

    @Override public void surfaceDestroyed(SurfaceHolder holder) {
        if (player != null) player.setDisplay(null);
    }

    @Override protected void onDestroy() {
        main.removeCallbacks(report);
        if (player != null) {
            player.release();
            player = null;
        }
        Log.i(TAG, "closed");
        super.onDestroy();
    }
}
