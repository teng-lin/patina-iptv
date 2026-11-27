package com.hantekllc.iptvrs.host;

import android.app.Activity;
import android.app.ActivityManager;
import android.app.Instrumentation;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.media.session.MediaController;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.net.wifi.WifiManager;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.PowerManager;
import android.os.SystemClock;

import java.io.File;
import java.lang.reflect.Field;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.util.Arrays;
import java.util.Locale;
import java.util.concurrent.Callable;

/** Tests Android OS leases with synthetic state and no discovery or receiver connection. */
public final class CastPlatformInstrumentation extends Instrumentation {
    private static final String VALIDATION_PACKAGE = "com.hantekllc.iptvrs.validation";
    private static final String KEY = "synthetic:android-platform-coverage";
    private static final String AIRPLAY_ALIAS = "iptv-rs.airplay.v1";
    private static final long CAST_ID = 301;
    // State and capability codes of the CastHost.playback JNI callback.
    private static final int BUFFERING = 0;
    private static final int PLAYING = 1;
    private static final int PAUSED = 2;
    private static final int CAN_PLAY = 1;
    private static final int CAN_PAUSE = 2;

    private boolean collectCoverage;

    @Override
    public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        collectCoverage = arguments != null && "true".equals(arguments.getString("coverage"));
        start();
    }

    @Override
    public void onStart() {
        Context context = getTargetContext();
        Bundle result = new Bundle();
        Bundle test = new Bundle();
        test.putString("class", getClass().getName());
        test.putString("test", "androidCastPlatform");
        test.putInt("numtests", 1);
        test.putInt("current", 1);
        sendStatus(1, test);
        try {
            require(VALIDATION_PACKAGE.equals(context.getPackageName())
                    && (context.getApplicationInfo().flags & ApplicationInfo.FLAG_DEBUGGABLE) != 0,
                    "debuggable validation app required");
            require(!CastHost.start(CAST_ID, "127.0.0.1", ""), "no service before registration");
            require(CastHost.topology() == null && CastHost.multicast() == null, "LAN before registration");

            Intent intent = context.getPackageManager().getLaunchIntentForPackage(context.getPackageName());
            require(intent != null, "handheld launcher is present");
            startActivitySync(intent);
            await("handheld visible", CastHost.INSTANCE::getVisible);

            credentials(context);
            result.putString("credentials", "passed real AndroidKeyStore, corruption, missing key, deletion");
            foreground(context);
            result.putString("foreground", "passed LAN, multicast, media session, replacement and screen-off");

            if (collectCoverage) {
                dumpCoverage(context);
            }
            sendStatus(0, test);
            finish(Activity.RESULT_OK, result);
        } catch (Throwable error) {
            test.putString("stack", android.util.Log.getStackTraceString(error));
            sendStatus(-2, test);
            finish(Activity.RESULT_CANCELED, result);
        }
    }

    private void credentials(Context context) throws Exception {
        File record = pairingFile(context);
        byte[] value = new byte[256];
        for (int i = 0; i < value.length; i++) {
            value[i] = (byte) (13 + 37 * i);
        }
        KeyStore store = KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        File[] existing = context.getNoBackupFilesDir().listFiles(
            (directory, filename) -> filename.startsWith("airplay-v1-"));
        require(existing != null && existing.length == 0 && !store.containsAlias(AIRPLAY_ALIAS),
                "fresh disposable pairing store required");
        CastHost.forgetCredential(KEY);
        try {
            require(status() == 0, "missing record");
            byte[] input = value.clone();
            CastHost.saveCredential(KEY, input);
            require(Arrays.equals(input, new byte[value.length]), "temporary clear bytes erased");
            byte[] sealed = Files.readAllBytes(record.toPath());
            require(sealed.length == value.length + 28, "sealed GCM record in no-backup storage");
            require(!Arrays.equals(value, Arrays.copyOfRange(sealed, 12, 12 + value.length)), "encrypted payload");
            require(Arrays.equals(value, read()), "Keystore round trip");

            value[0]++;
            CastHost.saveCredential(KEY, value.clone());
            require(Arrays.equals(value, read()), "atomic replacement");
            byte[] replacement = Files.readAllBytes(record.toPath());
            require(!Arrays.equals(Arrays.copyOf(sealed, 12), Arrays.copyOf(replacement, 12)), "fresh GCM IV");

            for (byte[] rejected : new byte[][]{new byte[0], new byte[4097]}) {
                Arrays.fill(rejected, (byte) 17);
                try {
                    CastHost.saveCredential(KEY, rejected);
                    throw new AssertionError("invalid credential size accepted");
                } catch (IllegalArgumentException expected) {
                    require(Arrays.equals(rejected, new byte[rejected.length]), "invalid clear bytes erased");
                }
            }
            require(Arrays.equals(value, read()), "invalid save leaves previous pairing intact");

            Files.write(record.toPath(), Arrays.copyOf(replacement, 16));
            require(status() == 2, "truncated record reported invalid");
            replacement[replacement.length - 1] ^= 1;
            Files.write(record.toPath(), replacement);
            require(status() == 2, "altered GCM tag reported invalid");

            store.deleteEntry(AIRPLAY_ALIAS);
            require(status() == 2, "lost key differs from missing record");
            try {
                CastHost.saveCredential(KEY, value.clone());
                throw new AssertionError("replaced ciphertext after Keystore key loss");
            } catch (IllegalStateException expected) {
                require(Arrays.equals(replacement, Files.readAllBytes(record.toPath())), "existing ciphertext kept");
            }
            CastHost.forgetCredential(KEY);
            CastHost.saveCredential(KEY, value.clone());
            require(Arrays.equals(value, read()), "explicit forget permits new key");
        } finally {
            CastHost.forgetCredential(KEY);
            store.deleteEntry(AIRPLAY_ALIAS);
        }
        require(!record.exists() && status() == 0, "record deleted");
    }

    private void foreground(Context context) throws Exception {
        String topology = CastHost.topology();
        require(topology != null, "disposable Android LAN is available");
        String address = Arrays.stream(topology.split(","))
            .map(link -> link.substring(link.indexOf(':') + 1))
            .filter(candidate -> candidate.matches("[0-9.]+"))
            .findFirst()
            .orElseThrow(() -> new AssertionError("disposable AVD requires an IPv4 link"));
        WifiManager.MulticastLock multicast = CastHost.multicast();
        require(multicast != null, "tab36 uses Wi-Fi");
        try {
            require(multicast.isHeld(), "multicast discovery lease acquired");
        } finally {
            multicast.release();
        }
        require(!multicast.isHeld(), "multicast discovery lease released");

        ActivityManager activities = context.getSystemService(ActivityManager.class);
        Field instance = field("instance");
        Field media = field("media");
        Field wake = field("wake");
        long stale = CAST_ID - 1;
        long replacement = CAST_ID + 1;
        require(CastHost.start(stale, address, topology + ",stale"), "queued old topology");
        try {
            waitForIdleSync();
            await("obsolete route has no service", () -> instance.get(null) == null && !isForeground(activities));
            require(CastHost.start(CAST_ID, address, topology), "start selected LAN");
            await("connected-device service and wake lock", () -> {
                Object service = instance.get(null);
                return service != null && isForeground(activities)
                    && wake.get(service) instanceof PowerManager.WakeLock
                    && ((PowerManager.WakeLock) wake.get(service)).isHeld();
            });
            Object service = instance.get(null);
            PowerManager.WakeLock wakeLock = (PowerManager.WakeLock) wake.get(service);
            MediaController controller = ((MediaSession) media.get(service)).getController();
            CastHost.playback(CAST_ID, PLAYING, CAN_PLAY | CAN_PAUSE);
            await("playing and controls", () -> state(controller) == PlaybackState.STATE_PLAYING);
            PlaybackState observed = controller.getPlaybackState();
            require((observed.getActions() & PlaybackState.ACTION_PLAY_PAUSE) != 0, "reader advertises Play/Pause");
            PlaybackState.CustomAction stop = observed.getCustomActions().stream()
                .filter(action -> action.getAction().equals("STOP"))
                .findFirst()
                .orElseThrow(() -> new AssertionError("session has Stop casting control"));
            controller.getTransportControls().play();
            controller.getTransportControls().pause();
            controller.getTransportControls().stop();
            controller.getTransportControls().sendCustomAction(stop, new Bundle());
            // Leave the stale request present until this observable check. An
            // incorrectly admitted lease would publish the Paused state here.
            CastHost.playback(stale, PAUSED, CAN_PLAY | CAN_PAUSE);
            waitForIdleSync();
            require(state(controller) == PlaybackState.STATE_PLAYING, "stale playback cannot change the session");

            require(CastHost.start(replacement, address, topology), "replacement route queued");
            // A playback update follows the new lease becoming available to the service.
            await("replacement playback active", () -> {
                CastHost.playback(replacement, PAUSED, CAN_PLAY);
                return state(controller) == PlaybackState.STATE_PAUSED;
            });
            CastHost.release(CAST_ID);
            waitForIdleSync();
            require(isForeground(activities) && wakeLock.isHeld(), "old lease release preserves the current service");

            key("KEYCODE_HOME");
            await("activity backgrounded", () -> !CastHost.INSTANCE.getVisible());
            require(!CastHost.start(CAST_ID, address, topology), "background cannot start another cast");
            require(isForeground(activities), "service remains foreground with activity stopped");
            PowerManager power = context.getSystemService(PowerManager.class);
            key("KEYCODE_SLEEP");
            await("screen off", () -> !power.isInteractive());
            require(isForeground(activities), "service remains foreground with screen off");
            CastHost.playback(replacement, BUFFERING, 0);
            await("screen-off state update", () -> state(controller) == PlaybackState.STATE_BUFFERING);
            require(controller.getPlaybackState().getActions() == PlaybackState.ACTION_STOP,
                    "unsupported controls omitted");
            CastHost.release(replacement);
            await("last lease and wake lock released", () -> !isForeground(activities) && !wakeLock.isHeld());
        } finally {
            CastHost.release(stale);
            CastHost.release(CAST_ID);
            CastHost.release(replacement);
            key("KEYCODE_WAKEUP");
        }
    }

    private static byte status() {
        byte[] result = CastHost.loadCredential(KEY);
        require(result.length == 1, "record status shape");
        return result[0];
    }

    private static byte[] read() {
        byte[] result = CastHost.loadCredential(KEY);
        require(result.length > 1 && result[0] == 1, "present record");
        return Arrays.copyOfRange(result, 1, result.length);
    }

    private static File pairingFile(Context context) throws Exception {
        byte[] digest = MessageDigest.getInstance("SHA-256").digest(KEY.getBytes(StandardCharsets.UTF_8));
        StringBuilder file = new StringBuilder("airplay-v1-");
        for (byte part : digest) {
            file.append(String.format(Locale.ROOT, "%02x", part));
        }
        return new File(context.getNoBackupFilesDir(), file.toString());
    }

    private static Field field(String name) throws Exception {
        Field field = CastPlaybackService.class.getDeclaredField(name);
        field.setAccessible(true);
        return field;
    }

    private static int state(MediaController controller) {
        PlaybackState current = controller.getPlaybackState();
        return current == null ? -1 : current.getState();
    }

    @SuppressWarnings("deprecation")
    private static boolean isForeground(ActivityManager activities) {
        return activities.getRunningServices(20).stream()
            .anyMatch(service -> service.foreground
                && service.service.getClassName().equals(CastPlaybackService.class.getName()));
    }

    private void key(String value) throws Exception {
        ParcelFileDescriptor result = getUiAutomation().executeShellCommand("input keyevent " + value);
        try (ParcelFileDescriptor.AutoCloseInputStream stream = new ParcelFileDescriptor.AutoCloseInputStream(result)) {
            while (stream.read() != -1) {
                // Consume the shell command output before observing the activity.
            }
        }
    }

    private static void await(String description, Callable<Boolean> predicate) throws Exception {
        long deadline = SystemClock.elapsedRealtime() + 10_000;
        while (SystemClock.elapsedRealtime() < deadline) {
            if (predicate.call()) {
                return;
            }
            SystemClock.sleep(50);
        }
        throw new AssertionError(description);
    }

    private static void require(boolean condition, String description) {
        if (!condition) {
            throw new AssertionError(description);
        }
    }

    /** Optional JaCoCo tooling adds the agent to the validation APK. */
    private static void dumpCoverage(Context context) throws Exception {
        Class<?> runtime = Class.forName("org.jacoco.agent.rt.RT");
        Object agent = runtime.getMethod("getAgent").invoke(null);
        byte[] data = (byte[]) Class.forName("org.jacoco.agent.rt.IAgent")
            .getMethod("getExecutionData", boolean.class).invoke(agent, false);
        Files.write(new File(context.getFilesDir(), "cast-platform.exec").toPath(), data);
    }
}
