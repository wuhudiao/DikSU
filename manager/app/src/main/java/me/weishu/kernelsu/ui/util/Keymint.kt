package me.weishu.kernelsu.ui.util

import android.util.Base64
import kotlinx.coroutines.runBlocking
import me.weishu.kernelsu.data.repository.SuperUserRepositoryImpl
import me.weishu.kernelsu.ui.viewmodel.SuperUserViewModel

private const val MODULE_DIR = "/data/adb/modules/oh_my_keymint"
private const val CONFIG_DIR = "/data/misc/keystore/omk"
private const val STATE_DIR = "/data/adb/omk"
private const val KEYSTORE_UID = 1017
private const val FIX_PROPS_PATH = "/data/adb/service.d/omk-fixprops.sh"
private const val FIX_PROPS_ASSET = "omk-fixprops.sh"

private const val CONFIG_PATH = "$CONFIG_DIR/config.toml"
private const val INJECTOR_PATH = "$CONFIG_DIR/injector.toml"
private const val KEYBOX_PATH = "$CONFIG_DIR/keybox.xml"

private const val CONFIG_MARK = "===CONFIG==="
private const val INJECTOR_MARK = "===INJECTOR==="

/** What [Keymint.status] reads out of the module, in one go. */
data class KeymintStatus(
    val installed: Boolean,
    val version: String,
    val enabled: Boolean,
    val keymintRunning: Boolean,
    val injectorRunning: Boolean,
    val fixProps: Boolean,
    val scoop: List<String>,
    val defaultGoogleScoop: List<String>,
    val logLevel: String,
    val injectorLogLevel: String,
    val keyboxExists: Boolean,
    val keyboxSize: Long,
)

/** One row in the routing list. */
data class KeymintApp(
    val packageName: String,
    val label: String,
    val isSystem: Boolean,
    val packageInfo: android.content.pm.PackageInfo,
)

/**
 * The Oh My Keymint controls, ported from DikSU's `webui_keymint.rs`.
 *
 * DikSU runs these behind its own web server; here they are plain calls into the module's files
 * through a root shell. Everything is blocking — call it off the main thread.
 */
object Keymint {

    val LOG_LEVELS = listOf("off", "error", "warn", "info", "debug", "trace")
    val INJECTOR_LOG_LEVELS = listOf("off", "error", "warn", "warning", "info", "debug", "trace")

    /** Names of the levels as they are worth reading, for the level picker. */
    val LEVEL_NOTES = mapOf(
        "off" to "不写日志：最安静，但出问题时也不留线索",
        "error" to "只记录错误",
        "warn" to "记录错误和警告",
        "warning" to "记录错误和警告（等同于 warn）",
        "info" to "记录常规信息（关键流程）",
        "debug" to "调试信息：模块默认值，反馈问题时最有用",
        "trace" to "最详细，日志量最大",
    )

    fun status(): KeymintStatus {
        val raw = sh(STATUS_SCRIPT)
        val fields = raw.substringBefore(CONFIG_MARK).lineSequence()
            .mapNotNull { line ->
                val at = line.indexOf('=')
                if (at > 0) line.substring(0, at) to line.substring(at + 1) else null
            }
            .toMap()
        val config = raw.substringAfter(CONFIG_MARK, "").substringBefore(INJECTOR_MARK).removePrefix("\n")
        val injector = raw.substringAfter(INJECTOR_MARK, "").removePrefix("\n")
        val scoop = parseScoop(injector)
        val keyboxSize = fields["keyboxsize"]?.toLongOrNull() ?: -1L

        return KeymintStatus(
            installed = fields["installed"] == "1",
            version = fields["version"].orEmpty(),
            enabled = fields["enabled"] == "1",
            keymintRunning = fields["keymint"] == "1",
            injectorRunning = fields["injector"] == "1",
            fixProps = fields["fixprops"] == "1",
            scoop = scoop,
            defaultGoogleScoop = scoop.filter { it.isGoogleDefault() },
            logLevel = logLevelOf(config),
            injectorLogLevel = logLevelOf(injector),
            keyboxExists = (fields["keyboxsize"]?.toLongOrNull() ?: -1L) >= 0,
            keyboxSize = if (keyboxSize > 0) keyboxSize else 0,
        )
    }

    /** Installs or removes the boot-time property fix; returns whether it is in place afterwards. */
    fun setFixProps(enable: Boolean, asset: (String) -> String): Boolean {
        if (enable) {
            writeFile(FIX_PROPS_PATH, asset(FIX_PROPS_ASSET), mode = "755", uid = 0)
        } else {
            sh("rm -f $FIX_PROPS_PATH")
        }
        return isFile(FIX_PROPS_PATH)
    }

    fun saveScoop(packages: List<String>) {
        val updated = setScoop(readFile(INJECTOR_PATH), packages)
        require(findScoopArray(updated) != null) { "写出来的 injector.toml 里没有合法的 scoop 数组" }
        writeFile(INJECTOR_PATH, updated)
    }

    fun saveLogLevel(which: String, level: String) {
        val path = when (which) {
            "config" -> CONFIG_PATH
            "injector" -> INJECTOR_PATH
            else -> error("未知的配置文件：$which")
        }
        val allowed = if (which == "injector") INJECTOR_LOG_LEVELS else LOG_LEVELS
        require(level in allowed) { "日志级别只能是 ${allowed.joinToString(" / ")} 之一" }

        val updated = replaceLogLevel(readFile(path), level)
            ?: error("${path.substringAfterLast('/')} 里没有 log_level 这一行")
        writeFile(path, updated)
    }

    fun applyKeyboxContent(content: String) {
        require(content.contains("<AndroidAttestation") && content.contains("<PrivateKey")) {
            "这个文件不是有效的 keybox.xml：缺少 <AndroidAttestation> 或 <PrivateKey>"
        }
        writeFile(KEYBOX_PATH, content)
    }

    /** Asks the module to restart one daemon (or both) on its next pass. */
    fun restart(what: String) {
        val flag = when (what) {
            "keymint" -> "restart.keymint"
            "injector" -> "restart.injector"
            "all" -> "restart.all"
            else -> error("未知的重启目标：$what")
        }
        require(isDirectory(STATE_DIR)) { "$STATE_DIR 不存在，模块可能没有在运行" }
        sh(": > $STATE_DIR/$flag 2>/dev/null || touch $STATE_DIR/$flag")
    }

    /** Every app on the device, for the routing list; the manager's own list, not a fresh scan. */
    fun apps(): List<KeymintApp> {
        var list = SuperUserViewModel.apps
        if (list.isEmpty()) {
            // The manager's list is normally already loaded; when it is not, load it here — and let
            // the reason a load failed reach the panel instead of showing an empty list.
            runBlocking { SuperUserRepositoryImpl().getAppList() }
                .onSuccess { list = it.first }
                .onFailure { throw it }
        }
        // The manager's list spans every user, so the same package can show up more than once.
        return list
            .filterNot { it.special }
            .distinctBy { it.packageName }
            .map { info ->
                val flags = info.packageInfo.applicationInfo?.flags ?: 0
                KeymintApp(
                    packageName = info.packageName,
                    label = info.label,
                    isSystem = flags and android.content.pm.ApplicationInfo.FLAG_SYSTEM != 0,
                    packageInfo = info.packageInfo,
                )
            }
    }

    private fun String.isGoogleDefault() =
        startsWith("com.google.") || startsWith("com.android.vending")

    private fun logLevelOf(text: String): String =
        findValueLine(text, "log_level")?.substringAfter('=')?.trim()?.trim('"').orEmpty()

    /** The first `key = value` line that is not a comment, the way the Rust side reads a key. */
    private fun findValueLine(text: String, key: String): String? =
        text.lineSequence().firstOrNull { line ->
            val trimmed = line.trimStart()
            !trimmed.startsWith('#') && trimmed.startsWith(key) &&
                trimmed.substring(key.length).trimStart().startsWith('=')
        }

    // --- scoop：DikSU 的 parse_scoop / set_scoop -----------------------------

    private fun parseScoop(text: String): List<String> {
        val range = findScoopArray(text) ?: return emptyList()
        val inner = text.substring(range.first + 1, range.last)
        return QUOTED.findAll(inner).map { it.groupValues[1] }.toList()
    }

    /** Indices of the `[` a `scoop` line opens and the `]` that closes it. */
    private fun findScoopArray(text: String): IntRange? {
        var at = 0
        for (line in text.split('\n')) {
            val trimmed = line.trimStart()
            if (!trimmed.startsWith('#')) {
                val rest = trimmed.removePrefix("scoop")
                if (rest != trimmed && rest.trimStart().startsWith('=')) {
                    val open = line.indexOf('[')
                    if (open >= 0) {
                        val close = matchingBracket(text, at + open)
                        if (close != null) return (at + open)..close
                    }
                }
            }
            at += line.length + 1
        }
        return null
    }

    private fun matchingBracket(text: String, open: Int): Int? {
        var depth = 0
        var quote: Char? = null
        var i = open
        while (i < text.length) {
            val c = text[i]
            if (quote != null) {
                if (c == quote) quote = null
                else if (c == '\\' && quote == '"') i++
            } else when (c) {
                '"', '\'' -> quote = c
                '#' -> while (i < text.length && text[i] != '\n') i++
                '[' -> depth++
                ']' -> {
                    depth--
                    if (depth == 0) return i
                }
            }
            i++
        }
        return null
    }

    private fun setScoop(text: String, packages: List<String>): String {
        val quoted = packages.map { "\"" + it.replace("\"", "") + "\"" }
        val range = findScoopArray(text)
        if (range == null) {
            val at = firstTableHeader(text) ?: text.length
            val head = text.substring(0, at).trimEnd()
            val tail = text.substring(at)
            return buildString {
                if (head.isNotEmpty()) append(head).append("\n\n")
                append("scoop = [").append(quoted.joinToString(", ")).append("]\n")
                if (tail.isNotEmpty()) append('\n').append(tail)
            }
        }

        val open = range.first
        val close = range.last
        val inner = if (text.substring(open, close).contains('\n')) {
            val line = text.substring(0, open).split('\n').last()
            val indent = line.length - line.trimStart().length
            val pad = " ".repeat(indent + 2)
            "\n" + quoted.joinToString(",\n$pad") { pad + it } + "\n" + " ".repeat(indent)
        } else {
            quoted.joinToString(", ")
        }
        return text.substring(0, open) + "[" + inner + "]" + text.substring(close + 1)
    }

    private fun firstTableHeader(text: String): Int? {
        var at = 0
        for (line in text.split('\n')) {
            if (line.trimStart().startsWith('[')) return at
            at += line.length + 1
        }
        return null
    }

    // --- log level ----------------------------------------------------------

    /** The one `log_level` line that is not a comment, rewritten; null when the file has none. */
    private fun replaceLogLevel(text: String, level: String): String? {
        var replaced = false
        val out = StringBuilder(text.length)
        for (line in text.split('\n')) {
            val trimmed = line.trimStart()
            if (!replaced && !trimmed.startsWith('#') && trimmed.startsWith("log_level") &&
                trimmed.contains('=')
            ) {
                val indent = line.substring(0, line.length - trimmed.length)
                out.append(indent).append("log_level = \"").append(level).append('"')
                replaced = true
            } else {
                out.append(line)
            }
            out.append('\n')
        }
        if (!replaced) return null
        // split('\n') plus the newline this loop adds per line leaves one extra at the end.
        return out.toString().removeSuffix("\n")
    }

    // --- shell --------------------------------------------------------------

    private fun sh(command: String): String {
        val out = ArrayList<String>()
        val err = ArrayList<String>()
        val result = withNewRootShell { newJob().add(command).to(out, err).exec() }
        if (result.code != 0) {
            throw IllegalStateException(err.joinToString("\n").ifBlank { "命令执行失败（$command）" })
        }
        return out.joinToString("\n")
    }

    private fun readFile(path: String): String {
        val out = ArrayList<String>()
        withNewRootShell { newJob().add("cat $path 2>/dev/null").to(out, ArrayList()).exec() }
        return out.joinToString("\n")
    }

    /**
     * Writes through `<path>.new` and renames over the original, so keystore — the user that reads
     * these files — keeps owning them.
     */
    private fun writeFile(
        path: String,
        content: String,
        mode: String = "600",
        uid: Int = KEYSTORE_UID,
    ) {
        val encoded = Base64.encodeToString(content.toByteArray(Charsets.UTF_8), Base64.NO_WRAP)
        val out = sh(
            """
            if [ -f $path ]; then cp -a $path $path.new; else : > $path.new; fi
            printf '%s' '$encoded' | base64 -d > $path.new || exit 1
            chown $uid:$uid $path.new 2>/dev/null
            chmod $mode $path.new
            mv -f $path.new $path
            echo ok
            """.trimIndent()
        )
        require(out.contains("ok")) { "写入 $path 失败" }
    }

    private fun isFile(path: String) = sh("[ -f $path ] && echo 1 || echo 0").trim() == "1"

    private fun isDirectory(path: String) = sh("[ -d $path ] && echo 1 || echo 0").trim() == "1"

    private val QUOTED = Regex("\"([^\"]*)\"")

    private val STATUS_SCRIPT = """
        [ -d $MODULE_DIR ] && echo installed=1 || echo installed=0
        [ -d $MODULE_DIR ] && [ ! -e $MODULE_DIR/disable ] && echo enabled=1 || echo enabled=0
        sed -n 's/^version=/version=/p' $MODULE_DIR/module.prop 2>/dev/null | head -n 1
        [ -f $FIX_PROPS_PATH ] && echo fixprops=1 || echo fixprops=0
        [ -s $STATE_DIR/keymint-daemon.pid ] && kill -0 `cat $STATE_DIR/keymint-daemon.pid` 2>/dev/null && echo keymint=1 || echo keymint=0
        [ -s $STATE_DIR/injector-daemon.pid ] && kill -0 `cat $STATE_DIR/injector-daemon.pid` 2>/dev/null && echo injector=1 || echo injector=0
        [ -f $CONFIG_PATH ] && echo configsize=`stat -c %s $CONFIG_PATH` || echo configsize=-1
        [ -f $INJECTOR_PATH ] && echo injectorsize=`stat -c %s $INJECTOR_PATH` || echo injectorsize=-1
        [ -f $KEYBOX_PATH ] && echo keyboxsize=`stat -c %s $KEYBOX_PATH` || echo keyboxsize=-1
        echo $CONFIG_MARK
        cat $CONFIG_PATH 2>/dev/null
        echo $INJECTOR_MARK
        cat $INJECTOR_PATH 2>/dev/null
    """.trimIndent()
}
