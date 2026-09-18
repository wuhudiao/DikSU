import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.os.Looper;
import java.lang.reflect.Method;
import java.util.List;

/**
 * Dump all installed apps' display labels via Android PackageManager.
 * Runs under app_process (root), output format: packageName\tuid\tlabel\n
 * This is the only reliable way to get localized app labels from native code.
 */
public class AppLabelDumper {
    public static void main(String[] args) {
        try {
            // Looper.prepare() is required before ActivityThread.systemMain()
            // because ActivityThread creates a Handler internally.
            if (Looper.myLooper() == null) {
                Looper.prepare();
            }

            // Get system context via ActivityThread reflection (app_process has no Context)
            Class<?> atClass = Class.forName("android.app.ActivityThread");
            Object at = null;
            try {
                Method currentAt = atClass.getMethod("currentActivityThread");
                at = currentAt.invoke(null);
            } catch (Exception e) {
                // ignore
            }
            if (at == null) {
                Method systemMain = atClass.getMethod("systemMain");
                at = systemMain.invoke(null);
            }
            Method getSystemContext = atClass.getMethod("getSystemContext");
            Object ctx = getSystemContext.invoke(at);
            Method getPm = ctx.getClass().getMethod("getPackageManager");
            PackageManager pm = (PackageManager) getPm.invoke(ctx);

            @SuppressWarnings("deprecation")
            List<ApplicationInfo> apps = pm.getInstalledApplications(0);

            StringBuilder sb = new StringBuilder();
            for (ApplicationInfo app : apps) {
                String label;
                try {
                    label = pm.getApplicationLabel(app).toString();
                } catch (Exception e) {
                    label = app.packageName;
                }
                if (label == null || label.isEmpty()) {
                    label = app.packageName;
                }
                sb.append(app.packageName)
                  .append('\t')
                  .append(app.uid)
                  .append('\t')
                  .append(label.replace('\t', ' ').replace('\n', ' '))
                  .append('\n');
            }
            System.out.print(sb.toString());
        } catch (Throwable t) {
            System.err.println("AppLabelDumper error: " + t);
            t.printStackTrace();
            System.exit(1);
        }
    }
}