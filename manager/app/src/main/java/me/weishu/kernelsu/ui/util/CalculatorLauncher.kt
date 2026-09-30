package me.weishu.kernelsu.ui.util

import android.content.Context
import android.os.Build
import com.topjohnwu.superuser.ShellUtils
import me.weishu.kernelsu.Natives
import me.weishu.kernelsu.data.repository.SettingsRepositoryImpl
import java.io.File

/**
 * Manages the calculator that doubles as the way into the web UI.
 *
 * The launcher next door hands out a freshly generated package name on every install; this one
 * keeps the name its APK declares, because it is meant to look like an ordinary third-party
 * calculator rather than like something installed for this purpose. Typing the code in
 * `/data/adb/ksu/webui.trigger` and pressing `=` starts the server and opens the page.
 */
object CalculatorLauncher {
    /** The name the template APK is built with; every install gets one generated from it. */
    private const val TEMPLATE_PACKAGE = "com.deskc.calcu"

    private const val ASSET = "calculator.apk"
    private const val STAGED_APK = "/data/local/tmp/calculator-stage.apk"
    private const val PREFS = "calculator_launcher"
    private const val PREF_PACKAGE = "installed_package"

    /**
     * Where the installed name is left for everyone else: the web UI's HMA one-click reads it to
     * hide this app from other apps. The name is generated per install, so nothing on the device
     * can work it out on its own.
     */
    private const val PACKAGE_FILE = "/data/adb/ksu/calculator.pkg"

    /** What the calculator compares what was typed against. Digits only. */
    private const val TRIGGER_FILE = "/data/adb/ksu/webui.trigger"

    /** Written by the old in-app autostart switch; it pins the server to a fixed port. */
    private const val LEGACY_AUTOSTART = "/data/adb/service.d/ksu-webui.sh"

    fun isInstalled(context: Context): Boolean = runCatching {
        val pkg = remembered(context) ?: return@runCatching false
        ShellUtils.fastCmd(getRootShell(), "pm path $pkg").contains("package:")
    }.getOrDefault(false)

    /** Installs the bundled calculator, replacing whatever is there. */
    fun install(context: Context): Boolean = runCatching {
        val staged = File(context.cacheDir, ASSET)
        context.assets.open(ASSET).use { input ->
            staged.outputStream().use { output -> input.copyTo(output) }
        }

        // A fresh package name on every install, the way the launcher used to get one: the
        // template is rewritten and signed here, so two installs of this app are not the same app.
        val fresh = LauncherRenamer.randomPackageName()
        val patched = File(context.cacheDir, "calculator-patched.apk")
        val renamed = LauncherRenamer.rename(context, staged, patched, TEMPLATE_PACKAGE, fresh)
        val pkg = if (renamed) fresh else TEMPLATE_PACKAGE
        val apk = if (renamed) patched else staged

        // The previous identity goes first, so a re-install does not leave two calculators behind.
        remembered(context)?.let { old ->
            runCatching { ShellUtils.fastCmd(getRootShell(), "pm uninstall $old") }
        }

        val shell = getRootShell()
        // pm cannot read the app's private cache dir, so park the APK somewhere readable.
        ShellUtils.fastCmd(shell, "cp ${apk.absolutePath} $STAGED_APK && chmod 644 $STAGED_APK")
        val output = ShellUtils.fastCmd(
            shell,
            "pm install -r $STAGED_APK; rm -f $STAGED_APK ${staged.absolutePath} ${patched.absolutePath}",
        )

        remember(context, pkg)
        ShellUtils.fastCmd(getRootShell(), "printf '%s\\n' '$pkg' > $PACKAGE_FILE")
        // The calculator starts the server through su, so authorise it up front: once the Manager
        // is gone there would be nothing left to approve the request.
        grantRoot(context, pkg)
        // Fresh token for the fresh install, so an address that leaked earlier is dead.
        rotateToken()
        ShellUtils.fastCmd(getRootShell(), "rm -f $LEGACY_AUTOSTART")

        !output.contains("Failure", ignoreCase = true)
    }.getOrDefault(false)

    fun uninstall(context: Context): Boolean = runCatching {
        val pkg = remembered(context) ?: TEMPLATE_PACKAGE
        val output = ShellUtils.fastCmd(getRootShell(), "pm uninstall $pkg")
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .remove(PREF_PACKAGE).apply()
        ShellUtils.fastCmd(getRootShell(), "rm -f $PACKAGE_FILE")
        output.contains("Success", ignoreCase = true)
    }.getOrDefault(false)

    /** The name generated at the last install, which is not the one the template carries. */
    private fun remembered(context: Context): String? =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(PREF_PACKAGE, null)

    private fun remember(context: Context, pkg: String) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString(PREF_PACKAGE, pkg).apply()
    }

    /** The code in place now, or an empty string when none was ever set. */
    fun readTriggerCode(): String = runCatching {
        ShellUtils.fastCmd(getRootShell(), "cat $TRIGGER_FILE 2>/dev/null")
    }.getOrDefault("").filter { it.isDigit() }

    /**
     * Writes the code the calculator watches for.
     *
     * Empty means "no code of its own": the file is removed and the calculator falls back to the
     * one baked into it at build time.
     */
    fun setTriggerCode(code: String) {
        val clean = code.filter { it.isDigit() }
        runCatching {
            if (clean.isEmpty()) {
                ShellUtils.fastCmd(getRootShell(), "rm -f $TRIGGER_FILE")
            } else {
                ShellUtils.fastCmd(
                    getRootShell(),
                    "printf '%s\\n' '$clean' > $TRIGGER_FILE && chmod 600 $TRIGGER_FILE",
                )
            }
        }
    }

    private fun grantRoot(context: Context, pkg: String) {
        runCatching {
            val info = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                context.packageManager.getApplicationInfo(
                    pkg,
                    android.content.pm.PackageManager.ApplicationInfoFlags.of(0),
                )
            } else {
                @Suppress("DEPRECATION")
                context.packageManager.getApplicationInfo(pkg, 0)
            }
            val profile = Natives.getAppProfile(pkg, info.uid)
            Natives.setAppProfile(profile.copy(allowSu = true))
        }
    }

    /**
     * 换一把令牌，让旧地址失效。
     *
     * 但"网页端自动端口"开着的时候令牌就是用户在面板里设的那个数字密码，这里一转，面板上那条
     * 链接立刻就 403（面板显示的仍是密码，用户只会看到打不开）。所以那种情况下不换令牌，把密码
     * 原样写回去；只有自动端口没开时才真的换一把新的。
     */
    private fun rotateToken() {
        runCatching {
            ShellUtils.fastCmd(getRootShell(), "${getKsuDaemonPath()} webui --reset-token")
        }
    }
}
