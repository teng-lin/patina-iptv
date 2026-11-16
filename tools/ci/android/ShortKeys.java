import java.lang.reflect.Constructor;
import java.lang.reflect.Method;

/** Sends short pairs by default, or one deliberately held key with --hold-ms. */
public final class ShortKeys {
    public static void main(String[] args) throws Exception {
        int first = 0;
        long holdMillis = 0;
        if (args.length > 0 && args[0].equals("--hold-ms")) {
            if (args.length != 3) throw new IllegalArgumentException("Expected --hold-ms N KEYCODE_NAME");
            holdMillis = Long.parseLong(args[1]);
            if (holdMillis < 1 || holdMillis > 5000) {
                throw new IllegalArgumentException("Hold must be between 1 and 5000 ms");
            }
            first = 2;
        }
        Class<?> managerClass;
        try {
            managerClass = Class.forName("android.hardware.input.InputManagerGlobal");
        } catch (ClassNotFoundException olderAndroid) {
            managerClass = Class.forName("android.hardware.input.InputManager");
        }
        Object manager = managerClass.getMethod("getInstance").invoke(null);
        Class<?> inputEvent = Class.forName("android.view.InputEvent");
        Class<?> keyEvent = Class.forName("android.view.KeyEvent");
        Method inject = managerClass.getMethod("injectInputEvent", inputEvent, int.class);
        Method keyCodeFromString = keyEvent.getMethod("keyCodeFromString", String.class);
        Method setSource = keyEvent.getMethod("setSource", int.class);
        Method uptimeMillis = Class.forName("android.os.SystemClock").getMethod("uptimeMillis");
        Constructor<?> event = keyEvent.getConstructor(
                long.class, long.class, int.class, int.class, int.class);

        for (int at = first; at < args.length; at++) {
            String name = args[at];
            int code = (Integer) keyCodeFromString.invoke(null, name);
            if (code == 0) throw new IllegalArgumentException("Unknown key: " + name);
            long now = (Long) uptimeMillis.invoke(null);
            Object down = event.newInstance(now, now, 0, code, 0);
            setSource.invoke(down, 0x101); // SOURCE_KEYBOARD, like adb input keyevent.
            // Queue Down ASYNC, then queue Up before waiting for its delivery.
            // adb input's WAIT_FOR_FINISH after Down can spend the app's entire
            // long-press timeout before it even submits Up. Waiting only on Up
            // also keeps the next pair behind this pair's app acknowledgement.
            // A deliberate hold waits for Down delivery before its hold period.
            // Android input --longpress holds for exactly the default threshold;
            // dispatch latency can make the app observe a slightly shorter press.
            boolean downAccepted = (Boolean) inject.invoke(manager, down, holdMillis > 0 ? 2 : 0);
            if (holdMillis > 0) Thread.sleep(holdMillis);
            long released = holdMillis > 0 ? (Long) uptimeMillis.invoke(null) : now + 1;
            Object up = event.newInstance(now, released, 1, code, 0);
            setSource.invoke(up, 0x101);
            boolean upAccepted = (Boolean) inject.invoke(manager, up, 2);
            if (!downAccepted || !upAccepted) {
                throw new IllegalStateException("Input injection rejected: " + name);
            }
        }
    }
}
