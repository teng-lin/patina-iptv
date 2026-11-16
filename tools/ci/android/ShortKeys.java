import java.lang.reflect.Constructor;
import java.lang.reflect.Method;

/** Sends ordered short key pairs without waiting for the app between Down and Up. */
public final class ShortKeys {
    public static void main(String[] args) throws Exception {
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

        for (String name : args) {
            int code = (Integer) keyCodeFromString.invoke(null, name);
            if (code == 0) throw new IllegalArgumentException("Unknown key: " + name);
            long now = (Long) uptimeMillis.invoke(null);
            Object down = event.newInstance(now, now, 0, code, 0);
            Object up = event.newInstance(now, now + 1, 1, code, 0);
            setSource.invoke(down, 0x101); // SOURCE_KEYBOARD, like adb input keyevent.
            setSource.invoke(up, 0x101);
            // Queue Down ASYNC, then queue Up before waiting for its delivery.
            // adb input's WAIT_FOR_FINISH after Down can spend the app's entire
            // long-press timeout before it even submits Up. Waiting only on Up
            // also keeps the next pair behind this pair's app acknowledgement.
            boolean downAccepted = (Boolean) inject.invoke(manager, down, 0);
            boolean upAccepted = (Boolean) inject.invoke(manager, up, 2);
            if (!downAccepted || !upAccepted) {
                throw new IllegalStateException("Input injection rejected: " + name);
            }
        }
    }
}
