package me.weishu.kernelsu.ui.util

import android.util.Base64
import me.weishu.kernelsu.ksuApp

/**
 * The one-tap Hide My Applist config, ported from DikSU's `webui.rs` (`run_hma_config`).
 *
 * DikSU runs this behind its own web server; here it is a plain root shell call. The script itself
 * is DikSU's own copy (`assets/hma-oss-config.sh`), so the file to keep an eye on when that one
 * changes is theirs — the two changes made for the Manager are that its package name goes into
 * every app's extra-hide list, and that the reload step only has to start HMA before pressing back,
 * because it re-reads the config on launch.
 *
 * Everything is blocking — call it off the main thread.
 */
object HideAppList {
    /** The script as it ships; [SCRIPT_ASSET] is the copy inside the APK. */
    const val SCRIPT_ASSET = "hma-oss-config.sh"

    /** Where the script is put before it runs. ksud's directory is root-only and already exists. */
    private const val SCRIPT_PATH = "/data/adb/ksu/hma-config.sh"

    /** Scene's package name, the one the Scene variant keeps out of the hidden scope entirely. */
    private const val SCENE_PACKAGE = "com.omarea.vtools"

    /**
     * Writes [script] to the device and runs it over every third-party app, returning what it
     * printed.
     *
     * [scene] picks the variant: Scene itself stays out of the scope and the accessibility preset
     * stays unchecked, because Scene reads the app list through an accessibility service.
     */
    fun run(scene: Boolean, script: String): String {
        writeScript(script)
        val env = buildString {
            // The Manager can be running under a name of its own, so ask the app what it is called:
            // that is the package every hidden app has to be told to hide.
            append("HMA_MANAGER_PKG='").append(ksuApp.packageName).append("' ")
            if (scene) {
                append("HMA_EXTRA_EXCLUDE='").append(SCENE_PACKAGE).append("' ")
                append("HMA_NO_ACCESSIBILITY=1 ")
            }
        }
        return sh("${env}sh $SCRIPT_PATH")
    }

    /** The script goes over as base64 so nothing in it has to survive a second round of quoting. */
    private fun writeScript(script: String) {
        val encoded = Base64.encodeToString(script.toByteArray(Charsets.UTF_8), Base64.NO_WRAP)
        val out = sh(
            """
            printf '%s' '$encoded' | base64 -d > $SCRIPT_PATH || exit 1
            chmod 700 $SCRIPT_PATH
            echo ok
            """.trimIndent()
        )
        require(out.contains("ok")) { "写入 $SCRIPT_PATH 失败" }
    }

    /**
     * One command in a fresh root shell. The script reports its own reasons on stdout ("未安装
     * HMA", a failed self-check) and exits non-zero, so both streams are read either way and the
     * text becomes the message.
     */
    private fun sh(command: String): String {
        val out = ArrayList<String>()
        val err = ArrayList<String>()
        val result = withNewRootShell { newJob().add(command).to(out, err).exec() }
        val text = (out + err).filter { it.isNotBlank() }.joinToString("\n").trim()
        if (result.code != 0) {
            throw IllegalStateException(text.ifBlank { "命令执行失败（$command）" })
        }
        return text
    }
}
