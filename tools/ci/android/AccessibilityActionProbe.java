import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.UiAutomation;
import android.os.HandlerThread;
import android.os.Looper;
import android.os.SystemClock;
import android.view.accessibility.AccessibilityNodeInfo;
import java.lang.reflect.Constructor;

/**
 * Native accessibility focus/click probe for a disposable synthetic emulator.
 * Keeps existing services (including TalkBack) bound. It checks actions, not speech.
 */
@SuppressWarnings("deprecation")
public class AccessibilityActionProbe {
    private static AccessibilityNodeInfo find(AccessibilityNodeInfo node, String text) {
        if (node == null || !node.isVisibleToUser()) return null;
        if (text.contentEquals(String.valueOf(node.getContentDescription()))
                || text.contentEquals(String.valueOf(node.getText()))) {
            // A page title can share its name with an actionable field.
            // Keep searching when the match has no clickable ancestor.
            AccessibilityNodeInfo target = node;
            while (!target.isClickable() && target.getParent() != null) target = target.getParent();
            if (target.isClickable()) return target;
        }
        for (int index = 0; index < node.getChildCount(); index++) {
            AccessibilityNodeInfo result = find(node.getChild(index), text);
            if (result != null) return result;
        }
        return null;
    }

    public static void main(String[] args) {
        try {
            run(args);
        } catch (Exception error) {
            System.err.println("Accessibility action failed: " + error.getMessage());
            System.exit(1);
        }
    }

    private static void run(String[] args) throws Exception {
        if (args.length != 1) {
            throw new IllegalArgumentException("Pass exactly one synthetic control label");
        }
        Looper.prepareMainLooper();
        HandlerThread thread = new HandlerThread("patina-accessibility");
        thread.start();
        Class<?> iface = Class.forName("android.app.IUiAutomationConnection");
        Object connection = Class.forName("android.app.UiAutomationConnection")
                .getConstructor().newInstance();
        Constructor<UiAutomation> constructor = UiAutomation.class.getConstructor(Looper.class, iface);
        UiAutomation automation = constructor.newInstance(thread.getLooper(), connection);
        UiAutomation.class.getMethod("connect", int.class).invoke(
                automation, UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES);
        try {
            AccessibilityServiceInfo info = automation.getServiceInfo();
            info.flags &= ~AccessibilityServiceInfo.FLAG_INCLUDE_NOT_IMPORTANT_VIEWS;
            automation.setServiceInfo(info);
            AccessibilityNodeInfo node;
            long deadline = SystemClock.uptimeMillis() + 2000;
            do {
                node = find(automation.getRootInActiveWindow(), args[0]);
                if (node == null) SystemClock.sleep(50);
            } while (node == null && SystemClock.uptimeMillis() < deadline);
            if (node == null) throw new IllegalStateException("No accessible node: " + args[0]);
            while (!node.isClickable() && node.getParent() != null) node = node.getParent();
            boolean focus = node.performAction(AccessibilityNodeInfo.ACTION_ACCESSIBILITY_FOCUS);
            boolean click = node.performAction(AccessibilityNodeInfo.ACTION_CLICK);
            System.out.println("class=" + node.getClassName() + " name=" + node.getContentDescription()
                    + " focus=" + focus + " click=" + click);
            if ((!focus && !node.isAccessibilityFocused()) || !click) {
                throw new IllegalStateException("Native accessibility action refused");
            }
        } finally {
            UiAutomation.class.getMethod("disconnect").invoke(automation);
            thread.quit();
        }
    }
}
