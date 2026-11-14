public final class AlwaysFinish {
    public static void main(String[] args) throws Exception {
        Class<?> activityManager = Class.forName("android.app.ActivityManager");
        Object service = activityManager.getDeclaredMethod("getService").invoke(null);
        Class<?> contract = Class.forName("android.app.IActivityManager");
        contract.getMethod("setAlwaysFinish", boolean.class).invoke(service, Boolean.parseBoolean(args[0]));
    }
}
