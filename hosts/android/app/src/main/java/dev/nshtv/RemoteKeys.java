package dev.nshtv;

import android.view.KeyEvent;

/**
 * What a TV remote's keys become before stock NSH's {@code NshView} sees
 * them. Each translation keeps Android's semantics for the key and only adds
 * what NSH's Android backend lacks; nothing here moves focus or decides an
 * app action.
 *
 * <ul>
 *   <li>A gamepad's A is the D-pad's center, as Android's
 *       {@code KeyEvent.isConfirmKey} treats them.</li>
 *   <li>The keys NSH has no name for keep their key code and report the scan
 *       code of a function key no remote sends ({@link #TRANSPORT}); NSH then
 *       reports them as {@code key: "Unidentified", code: "F13"...}, which
 *       {@code nsh_tv::transport} decodes. The same table is in
 *       {@code vendor/nsh-tv/src/transport.rs}, and a test compares the two.</li>
 * </ul>
 */
public final class RemoteKeys {
    private RemoteKeys() {}

    /** {Android key code, evdev scan code reported instead}: append only. */
    static final int[][] TRANSPORT = {
        {166, 183}, // CHANNEL_UP -> F13
        {167, 184}, // CHANNEL_DOWN -> F14
        {172, 185}, // GUIDE -> F15
        {165, 186}, // INFO -> F16
        {183, 187}, // PROG_RED -> F17
        {184, 188}, // PROG_GREEN -> F18
        {185, 189}, // PROG_YELLOW -> F19
        {186, 190}, // PROG_BLUE -> F20
        {84, 191}, // SEARCH -> F21
        {130, 192}, // MEDIA_RECORD -> F22
    };

    /**
     * The event NSH sees for {@code e}; {@code e} itself when nothing
     * changes.
     */
    public static KeyEvent translate(KeyEvent e) {
        int code = e.getKeyCode();
        int scan = e.getScanCode();
        if (code == KeyEvent.KEYCODE_BUTTON_A) code = KeyEvent.KEYCODE_DPAD_CENTER;
        for (int[] row : TRANSPORT) {
            if (row[0] == code) {
                scan = row[1];
                break;
            }
        }
        if (code == e.getKeyCode() && scan == e.getScanCode()) return e;
        return new KeyEvent(e.getDownTime(), e.getEventTime(), e.getAction(), code,
                e.getRepeatCount(), e.getMetaState(), e.getDeviceId(), scan, e.getFlags(),
                e.getSource());
    }

    /**
     * The keys a TV app owns once it runs: when the app's views leave one
     * unhandled, the host still consumes it, so the system does not resume
     * another app's media or open its own search over the app. Volume, Home
     * and Back stay the system's.
     */
    public static boolean appOwned(int code) {
        switch (code) {
            case KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE:
            case KeyEvent.KEYCODE_MEDIA_PLAY:
            case KeyEvent.KEYCODE_MEDIA_PAUSE:
            case KeyEvent.KEYCODE_MEDIA_STOP:
            case KeyEvent.KEYCODE_MEDIA_NEXT:
            case KeyEvent.KEYCODE_MEDIA_PREVIOUS:
            case KeyEvent.KEYCODE_MEDIA_REWIND:
            case KeyEvent.KEYCODE_MEDIA_FAST_FORWARD:
            case KeyEvent.KEYCODE_MEDIA_RECORD:
            case KeyEvent.KEYCODE_CHANNEL_UP:
            case KeyEvent.KEYCODE_CHANNEL_DOWN:
            case KeyEvent.KEYCODE_GUIDE:
            case KeyEvent.KEYCODE_INFO:
            case KeyEvent.KEYCODE_PROG_RED:
            case KeyEvent.KEYCODE_PROG_GREEN:
            case KeyEvent.KEYCODE_PROG_YELLOW:
            case KeyEvent.KEYCODE_PROG_BLUE:
            case KeyEvent.KEYCODE_SEARCH:
                return true;
            default:
                return false;
        }
    }
}
