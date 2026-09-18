import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.content.res.Resources;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.drawable.Drawable;
import android.os.Looper;
import android.util.Base64;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.lang.reflect.Method;

public class AppIconDumper {
    public static void main(String[] args) {
        if (args.length < 1) {
            System.err.println("Usage: AppIconDumper <packageName|apkPath> [outputPath]");
            System.exit(1);
        }

        String packageName = args[0];
        // 一个已安装的包名，或者一个 APK 文件的路径。文件管理器要对下载下来、还没安装的安装包也
        // 显示它自己的图标，所以后者走 getPackageArchiveInfo。
        boolean fromApk = packageName.startsWith("/") || packageName.toLowerCase().endsWith(".apk");
        String outputPath = args.length > 1 ? args[1]
            : "/sdcard/icon_" + (fromApk ? new File(packageName).getName() : packageName) + ".png";

        try {
            if (Looper.myLooper() == null) {
                Looper.prepare();
            }

            // Get system context via ActivityThread reflection
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

            ApplicationInfo appInfo;
            if (fromApk) {
                // 从文件里读清单。图标只有在 sourceDir 指回这个 APK 之后才能解析出来 ——
                // 后面那句 getResourcesForApplication 靠的就是它。
                PackageInfo archive = pm.getPackageArchiveInfo(packageName, 0);
                if (archive == null || archive.applicationInfo == null) {
                    System.err.println("Not an APK: " + packageName);
                    System.exit(2);
                }
                appInfo = archive.applicationInfo;
                appInfo.sourceDir = packageName;
                appInfo.publicSourceDir = packageName;
            } else {
                appInfo = pm.getApplicationInfo(packageName, 0);
            }

            Drawable icon = null;

            // Method 1: Load directly from app's Resources (bypasses OEM custom icon loaders)
            try {
                Resources res = pm.getResourcesForApplication(appInfo);
                if (appInfo.icon != 0) {
                    icon = res.getDrawable(appInfo.icon, null);
                }
                // If icon is 0, try round icon
                if (icon == null && appInfo.icon != 0) {
                    try {
                        int roundIconId = appInfo.getClass().getField("roundIcon").getInt(appInfo);
                        if (roundIconId != 0) {
                            icon = res.getDrawable(roundIconId, null);
                        }
                    } catch (Exception e) {
                        // ignore
                    }
                }
            } catch (Exception e) {
                System.err.println("Resources.getDrawable failed: " + e.getMessage());
            }

            // Method 2: loadIcon from ApplicationInfo
            if (icon == null) {
                try {
                    icon = appInfo.loadIcon(pm);
                } catch (Exception e) {
                    System.err.println("loadIcon failed: " + e.getMessage());
                }
            }

            // Method 3: getApplicationIcon from PackageManager
            if (icon == null) {
                try {
                    icon = pm.getApplicationIcon(appInfo);
                } catch (Exception e) {
                    System.err.println("getApplicationIcon failed: " + e.getMessage());
                }
            }

            if (icon == null) {
                System.err.println("All icon methods failed for package: " + packageName);
                System.exit(2);
            }

            // Determine icon size
            int iconSize = 192;
            try {
                int intrinsicWidth = icon.getIntrinsicWidth();
                int intrinsicHeight = icon.getIntrinsicHeight();
                if (intrinsicWidth > 0 && intrinsicHeight > 0) {
                    iconSize = Math.min(Math.max(intrinsicWidth, intrinsicHeight), 256);
                }
            } catch (Exception e) {
                // Ignore
            }

            // Create bitmap and draw icon
            Bitmap bitmap = Bitmap.createBitmap(iconSize, iconSize, Bitmap.Config.ARGB_8888);
            Canvas canvas = new Canvas(bitmap);
            icon.setBounds(0, 0, iconSize, iconSize);
            icon.draw(canvas);

            // Compress to PNG
            ByteArrayOutputStream baos = new ByteArrayOutputStream();
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, baos);
            byte[] pngData = baos.toByteArray();

            // Save to file
            File outputFile = new File(outputPath);
            File parent = outputFile.getParentFile();
            if (parent != null && !parent.exists()) {
                parent.mkdirs();
            }
            FileOutputStream fos = new FileOutputStream(outputFile);
            fos.write(pngData);
            fos.close();

            // Output base64 to stdout
            String base64 = Base64.encodeToString(pngData, Base64.NO_WRAP);
            System.out.println("SUCCESS");
            System.out.println("PACKAGE:" + packageName);
            System.out.println("SIZE:" + iconSize + "x" + iconSize);
            System.out.println("BYTES:" + pngData.length);
            System.out.println("FILE:" + outputPath);
            System.out.println("BASE64:" + base64);

        } catch (Exception e) {
            System.err.println("ERROR: " + e.getMessage());
            e.printStackTrace();
            System.exit(3);
        }
    }
}