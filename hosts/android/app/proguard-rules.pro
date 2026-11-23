# JNI_OnLoad in NSH looks up these classes and methods by name. The
# default Android rules keep Java declarations of native methods.
-keep,allowoptimization class com.longbridge.nsh.TextLayouts {
    static *** prepare(...);
    static *** measure(...);
    static *** layout(...);
    static *** lines(...);
    static *** attachments(...);
    static *** hitTest(...);
    static *** caretRect(...);
    static *** rangeRects(...);
    static *** registerFont(...);
}
-keep,allowoptimization class com.longbridge.nsh.Net {
    static *** fetch(...);
    static *** send(...);
    static *** finish(...);
    static *** pause(...);
    static *** cancel(...);
}
-keep,allowoptimization class com.longbridge.nsh.NshView {
    void submit(byte[], java.lang.Object[], long);
    void reportError(java.lang.String);
    byte[] readAsset(java.lang.String);
    void requestJsFrame();
    void runMainThreadTasks();
    void shaderFrame(int, java.nio.ByteBuffer, int, int, boolean, float);
    void appStopped();
    void hostMessage(java.lang.String, java.lang.String, long);
    void openUrlMessage(java.lang.String, long);
    void devStatus(java.lang.String);
    void devError(java.lang.String, java.lang.String, boolean);
}
-keepclassmembers,allowoptimization class com.longbridge.nsh.SelectableView {
    java.lang.String copiedText(int, int, int);
    int offsetAt(int, float, float);
}

# Only used when the native NSH dev feature is enabled in a profile build.
-keep,allowoptimization class com.longbridge.nsh.TestAgent {
    !private static <methods>;
}

# The Rust host keeps these classes through a JNI global reference and
# invokes these callbacks by name, without a Java caller.
-keepclassmembers,allowoptimization class com.hantekllc.iptvrs.host.RecordingHost {
    static void setLanguage(java.lang.String);
    static int schedule(long, boolean);
    static void requestAlarmAccess();
}
-keepclassmembers,allowoptimization class com.hantekllc.iptvrs.host.ExternalPlayerHost {
    static boolean open(java.lang.String);
}
