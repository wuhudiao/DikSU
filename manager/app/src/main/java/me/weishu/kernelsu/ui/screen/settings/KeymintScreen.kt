package me.weishu.kernelsu.ui.screen.settings

import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Explore
import androidx.compose.material.icons.filled.CloudDownload
import androidx.compose.material.icons.filled.FolderOpen
import androidx.compose.material.icons.filled.GridView
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.RestartAlt
import androidx.compose.material.icons.filled.Shield
import androidx.compose.material.icons.filled.TextSnippet
import androidx.compose.material.icons.filled.VpnKey
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.DpSize
import androidx.lifecycle.compose.dropUnlessResumed
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import me.weishu.kernelsu.R
import me.weishu.kernelsu.ui.LocalUiMode
import me.weishu.kernelsu.ui.UiMode
import me.weishu.kernelsu.ui.component.AppIconImage
import me.weishu.kernelsu.ui.component.material.SegmentedColumn
import me.weishu.kernelsu.ui.component.material.SegmentedListItem
import me.weishu.kernelsu.ui.component.material.SegmentedRadioItem
import me.weishu.kernelsu.ui.component.material.SegmentedSwitchItem
import me.weishu.kernelsu.ui.navigation3.LocalNavigator
import me.weishu.kernelsu.ui.util.Keymint
import me.weishu.kernelsu.ui.util.KeymintApp
import me.weishu.kernelsu.ui.util.KeymintStatus
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.BasicComponent
import top.yukonga.miuix.kmp.basic.Icon as MiuixIcon
import top.yukonga.miuix.kmp.basic.IconButton as MiuixIconButton
import top.yukonga.miuix.kmp.basic.Scaffold as MiuixScaffold
import top.yukonga.miuix.kmp.basic.SmallTitle
import top.yukonga.miuix.kmp.basic.Text as MiuixText
import top.yukonga.miuix.kmp.basic.TextButton as MiuixTextButton
import top.yukonga.miuix.kmp.basic.TextField as MiuixTextField
import top.yukonga.miuix.kmp.basic.InfiniteProgressIndicator
import top.yukonga.miuix.kmp.basic.TopAppBar as MiuixTopAppBar
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Back
import top.yukonga.miuix.kmp.overlay.OverlayDialog
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.preference.CheckboxLocation
import top.yukonga.miuix.kmp.preference.CheckboxPreference
import top.yukonga.miuix.kmp.preference.RadioButtonLocation
import top.yukonga.miuix.kmp.preference.RadioButtonPreference
import top.yukonga.miuix.kmp.preference.SwitchPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme
import java.net.HttpURLConnection
import java.net.URL
import kotlin.math.roundToLong

/**
 * The Oh My Keymint panel that came over from DikSU, redone with this Manager's own components.
 *
 * Same content as that panel — daemons, the property fix, which apps are routed to OMK, the log
 * levels, the keybox and the two restart switches — but drawn with Miuix/Material instead of a
 * WebView, so it follows the interface style the app is set to.
 */
@Composable
fun KeymintScreen() {
    val navigator = LocalNavigator.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val onBack = dropUnlessResumed { navigator.pop() }

    var status by remember { mutableStateOf<KeymintStatus?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var loaded by remember { mutableStateOf(false) }
    var dialog by remember { mutableStateOf<KeymintDialog>(KeymintDialog.None) }

    val toast: (String) -> Unit = { message ->
        if (message.isNotEmpty()) Toast.makeText(context, message, Toast.LENGTH_SHORT).show()
    }

    fun reload() {
        scope.launch {
            val result = withContext(Dispatchers.IO) { runCatching { Keymint.status() } }
            result
                .onSuccess { status = it; error = null }
                .onFailure { error = it.message ?: it.toString() }
            loaded = true
        }
    }

    /** Runs one of the panel's actions off the main thread, reports, then re-reads the status. */
    val act: (suspend () -> String) -> Unit = { work ->
        scope.launch {
            val result = withContext(Dispatchers.IO) { runCatching { work() } }
            result
                .onSuccess(toast)
                .onFailure { toast(it.message ?: context.getString(R.string.keymint_failed)) }
            reload()
        }
    }

    LaunchedEffect(Unit) { reload() }

    val groups = keymintGroups(
        status = status,
        error = error,
        loaded = loaded,
        onRefresh = { reload() },
        onToggleFixProps = { enabled ->
            act {
                val asset = context.assets.open("omk-fixprops.sh").use { it.readBytes().toString(Charsets.UTF_8) }
                Keymint.setFixProps(enabled) { asset }
                if (enabled) "已启用：开机时校正系统属性" else "已关闭并删除该脚本"
            }
        },
        onEditApps = { dialog = KeymintDialog.Apps },
        onEditLevels = { dialog = KeymintDialog.Levels },
        onEditKeybox = { dialog = KeymintDialog.Keybox },
        onRestart = { what ->
            act {
                Keymint.restart(what)
                if (what == "all") "已请求全部重启" else "已请求重启 $what"
            }
        },
    )

    when (LocalUiMode.current) {
        UiMode.Miuix -> KeymintMiuix(groups, onBack)
        UiMode.Material -> KeymintMaterial(groups, onBack)
    }

    val current = status
    when (val open = dialog) {
        KeymintDialog.None -> Unit
        KeymintDialog.Apps -> if (current != null) {
            AppsDialog(
                status = current,
                onDismiss = { dialog = KeymintDialog.None },
                onSave = { packages ->
                    dialog = KeymintDialog.None
                    act {
                        Keymint.saveScoop(packages)
                        "已保存 ${packages.size} 个应用"
                    }
                },
            )
        }
        KeymintDialog.Levels -> if (current != null) {
            LevelDialog(
                status = current,
                onDismiss = { dialog = KeymintDialog.None },
                onPick = { which, level ->
                    dialog = KeymintDialog.None
                    act {
                        Keymint.saveLogLevel(which, level)
                        if (which == "injector") {
                            "injector 日志级别已设为 $level，立即生效"
                        } else {
                            "keymint 日志级别已设为 $level，重启 keymint 后完全生效"
                        }
                    }
                },
            )
        }
        KeymintDialog.Keybox -> {
            KeyboxDialog(
                status = current,
                onDismiss = { dialog = KeymintDialog.None },
                onLocal = { dialog = KeymintDialog.KeyboxLocal },
                onRemote = { dialog = KeymintDialog.KeyboxRemote },
            )
        }
        KeymintDialog.KeyboxLocal -> {
            val picker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
                dialog = KeymintDialog.None
                if (uri == null) return@rememberLauncherForActivityResult
                act {
                    val text = withContext(Dispatchers.IO) {
                        context.contentResolver.openInputStream(uri)?.use { it.readBytes().toString(Charsets.UTF_8) }
                            ?: error("读不到这个文件")
                    }
                    Keymint.applyKeyboxContent(text)
                    "已替换 keybox"
                }
            }
            LaunchedEffect(Unit) {
                picker.launch("*/*")
                dialog = KeymintDialog.None
            }
        }
        KeymintDialog.KeyboxRemote -> {
            RemoteKeyboxDialog(
                onDismiss = { dialog = KeymintDialog.None },
                onDownload = { url ->
                    dialog = KeymintDialog.None
                    act {
                        val text = downloadKeybox(url)
                        Keymint.applyKeyboxContent(text)
                        "远程密钥已更新"
                    }
                },
            )
        }
    }
}

/** The shape both styles render: cards of rows, a switch or an arrow each. */
private class KeymintEntry(
    val icon: ImageVector,
    val title: String,
    val summary: String?,
    val checked: Boolean? = null,
    /** Null for a row that only reports something: no arrow, nothing to press. */
    val onClick: (() -> Unit)? = null,
)

private class KeymintGroup(val title: String?, val entries: List<KeymintEntry>)

private sealed interface KeymintDialog {
    data object None : KeymintDialog
    data object Apps : KeymintDialog
    data object Levels : KeymintDialog
    data object Keybox : KeymintDialog
    data object KeyboxLocal : KeymintDialog
    data object KeyboxRemote : KeymintDialog
}

/** The GMS packages that have to stay routed; the module needs them and they are never listed. */
private val ALWAYS_ROUTED = listOf(
    "com.google.android.gsf",
    "com.google.android.gms",
    "com.android.vending",
)

@Composable
private fun keymintGroups(
    status: KeymintStatus?,
    error: String?,
    loaded: Boolean,
    onRefresh: () -> Unit,
    onToggleFixProps: (Boolean) -> Unit,
    onEditApps: () -> Unit,
    onEditLevels: () -> Unit,
    onEditKeybox: () -> Unit,
    onRestart: (String) -> Unit,
): List<KeymintGroup> {
    val installed = stringResource(R.string.keymint_installed)
    val notInstalled = stringResource(R.string.keymint_not_installed)
    val stateStatus = stringResource(R.string.keymint_state_status)
    val stateFeatures = stringResource(R.string.keymint_state_features)
    val stateRestart = stringResource(R.string.keymint_state_restart)
    val running = stringResource(R.string.keymint_running)
    val notRunning = stringResource(R.string.keymint_not_running)

    if (status == null) {
        val summary = when {
            error != null -> error
            loaded -> stringResource(R.string.keymint_not_installed_summary)
            else -> stringResource(R.string.keymint_loading)
        }
        return listOf(
            KeymintGroup(
                null,
                listOf(
                    KeymintEntry(
                        icon = Icons.Filled.Shield,
                        title = if (error != null) stringResource(R.string.keymint_read_failed) else notInstalled,
                        summary = summary,
                        onClick = onRefresh,
                    ),
                ),
            ),
        )
    }

    if (!status.installed) {
        return listOf(
            KeymintGroup(
                null,
                listOf(
                    KeymintEntry(
                        icon = Icons.Filled.Shield,
                        title = notInstalled,
                        summary = stringResource(R.string.keymint_not_installed_summary),
                        onClick = onRefresh,
                    ),
                ),
            ),
        )
    }

    val indicator = { ok: Boolean -> if (ok) running else notRunning }
    val keyboxSummary = if (status.keyboxExists) {
        stringResource(R.string.keymint_keybox_size, formatSize(status.keyboxSize))
    } else {
        stringResource(R.string.keymint_keybox_none)
    }

    return listOf(
        KeymintGroup(
            null,
            listOf(
                KeymintEntry(
                    icon = Icons.Filled.Shield,
                    title = installed,
                    summary = "${status.version} · " +
                        stringResource(if (status.enabled) R.string.keymint_enabled else R.string.keymint_disabled),
                ),
            ),
        ),
        KeymintGroup(
            stateStatus,
            listOf(
                KeymintEntry(
                    icon = Icons.Filled.Lock,
                    title = stringResource(R.string.keymint_daemon),
                    summary = indicator(status.keymintRunning),
                ),
                KeymintEntry(
                    icon = Icons.Filled.Explore,
                    title = stringResource(R.string.keymint_injector_daemon),
                    summary = indicator(status.injectorRunning),
                ),
            ),
        ),
        KeymintGroup(
            stateFeatures,
            listOf(
                KeymintEntry(
                    icon = Icons.Filled.Shield,
                    title = stringResource(R.string.keymint_fix_props),
                    summary = stringResource(
                        if (status.fixProps) R.string.keymint_fix_props_on else R.string.keymint_fix_props_off
                    ),
                    checked = status.fixProps,
                    onClick = { onToggleFixProps(!status.fixProps) },
                ),
                KeymintEntry(
                    icon = Icons.Filled.GridView,
                    title = stringResource(R.string.keymint_apps),
                    summary = stringResource(R.string.keymint_apps_summary, status.scoop.size),
                    onClick = onEditApps,
                ),
                KeymintEntry(
                    icon = Icons.Filled.TextSnippet,
                    title = stringResource(R.string.keymint_log_level),
                    summary = stringResource(
                        R.string.keymint_log_level_summary,
                        status.logLevel.ifEmpty { "?" },
                        status.injectorLogLevel.ifEmpty { "?" },
                    ),
                    onClick = onEditLevels,
                ),
                KeymintEntry(
                    icon = Icons.Filled.VpnKey,
                    title = stringResource(R.string.keymint_keybox),
                    summary = keyboxSummary,
                    onClick = onEditKeybox,
                ),
            ),
        ),
        KeymintGroup(
            stateRestart,
            listOf(
                KeymintEntry(
                    icon = Icons.Filled.RestartAlt,
                    title = stringResource(R.string.keymint_restart_keymint),
                    summary = null,
                    onClick = { onRestart("keymint") },
                ),
                KeymintEntry(
                    icon = Icons.Filled.RestartAlt,
                    title = stringResource(R.string.keymint_restart_injector),
                    summary = null,
                    onClick = { onRestart("injector") },
                ),
                KeymintEntry(
                    icon = Icons.Filled.RestartAlt,
                    title = stringResource(R.string.keymint_restart_all),
                    summary = null,
                    onClick = { onRestart("all") },
                ),
            ),
        ),
    )
}

// --- 两种风格 --------------------------------------------------------------

@Composable
private fun KeymintMiuix(groups: List<KeymintGroup>, onBack: () -> Unit) {
    MiuixScaffold(
        topBar = {
            MiuixTopAppBar(
                title = stringResource(R.string.settings_keymint_config),
                navigationIcon = {
                    MiuixIconButton(onClick = onBack) {
                        MiuixIcon(
                            imageVector = MiuixIcons.Back,
                            contentDescription = null,
                            tint = colorScheme.onSurface,
                        )
                    }
                },
            )
        },
    ) { innerPadding ->
        LazyColumn(
            modifier = Modifier.padding(horizontal = 12.dp),
            contentPadding = innerPadding,
        ) {
            groups.forEachIndexed { index, group ->
                item(key = index) {
                    if (group.title != null) {
                        SmallTitle(text = group.title, modifier = Modifier.padding(top = 12.dp))
                    }
                    Card(modifier = Modifier.padding(top = if (group.title == null) 12.dp else 2.dp)) {
                        group.entries.forEach { entry -> MiuixRow(entry) }
                    }
                }
            }
        }
    }
}

@Composable
private fun MiuixRow(entry: KeymintEntry) {
    val icon = @Composable {
        MiuixIcon(
            imageVector = entry.icon,
            contentDescription = null,
            tint = colorScheme.onBackground,
            modifier = Modifier.padding(end = 6.dp),
        )
    }
    val checked = entry.checked
    val onClick = entry.onClick
    when {
        checked != null && onClick != null -> SwitchPreference(
            title = entry.title,
            summary = entry.summary,
            startAction = icon,
            checked = checked,
            onCheckedChange = { onClick() },
        )

        onClick != null -> ArrowPreference(
            title = entry.title,
            summary = entry.summary,
            startAction = icon,
            onClick = onClick,
        )

        else -> BasicComponent(
            title = entry.title,
            summary = entry.summary,
            startAction = icon,
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun KeymintMaterial(groups: List<KeymintGroup>, onBack: () -> Unit) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings_keymint_config)) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = null)
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .padding(padding)
                .verticalScroll(rememberScrollState()),
        ) {
            groups.forEach { group ->
                SegmentedColumn(
                    modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 13.dp, bottom = 13.dp),
                    title = group.title.orEmpty(),
                    content = group.entries.map { entry ->
                        {
                            val checked = entry.checked
                            val onClick = entry.onClick
                            if (checked != null && onClick != null) {
                                SegmentedSwitchItem(
                                    icon = entry.icon,
                                    title = entry.title,
                                    summary = entry.summary,
                                    checked = checked,
                                    onCheckedChange = { onClick() },
                                )
                            } else {
                                SegmentedListItem(
                                    onClick = onClick,
                                    headlineContent = { Text(entry.title) },
                                    supportingContent = entry.summary?.let { { Text(it) } },
                                    leadingContent = { Icon(entry.icon, contentDescription = null) },
                                    trailingContent = if (onClick != null) {
                                        {
                                            Icon(
                                                Icons.AutoMirrored.Filled.KeyboardArrowRight,
                                                contentDescription = null
                                            )
                                        }
                                    } else {
                                        null
                                    },
                                )
                            }
                        }
                    },
                )
            }
        }
    }
}

// --- 弹窗：选择软件、日志级别、keybox --------------------------------------

@Composable
private fun AppsDialog(
    status: KeymintStatus,
    onDismiss: () -> Unit,
    onSave: (List<String>) -> Unit,
) {
    // The three GMS packages always ride along: they are system apps, so they never show in the
    // list below, but OMK is useless without them and the module ships them in its scoop already.
    var chosen by remember { mutableStateOf(status.scoop.toSet() + ALWAYS_ROUTED) }
    var apps by remember { mutableStateOf<List<KeymintApp>?>(null) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var query by remember { mutableStateOf("") }

    LaunchedEffect(Unit) {
        val result = withContext(Dispatchers.IO) { runCatching { Keymint.apps() } }
        result
            .onSuccess { apps = it }
            .onFailure {
                loadError = it.message ?: it.toString()
                apps = emptyList()
            }
    }

    val body = @Composable {
        val shown = (apps ?: emptyList())
            // System apps are left out entirely — routing them through OMK is not what this is for.
            .filter { !it.isSystem }
            .filter {
                query.isBlank() ||
                    it.label.contains(query, ignoreCase = true) ||
                    it.packageName.contains(query, ignoreCase = true)
            }
            // Ordered by name and never re-ordered: sorting the picked ones first made the list
            // jump away from wherever the user had scrolled to on every tap.
            .sortedBy { it.label.lowercase() }

        Column(modifier = Modifier.heightIn(max = 460.dp)) {
            SearchField(
                value = query,
                label = stringResource(R.string.keymint_apps_search),
                modifier = Modifier,
            ) { query = it }
            Spacer(Modifier.height(8.dp))
            when {
                apps == null -> Loading()
                loadError != null -> RowMessage(loadError.orEmpty())
                shown.isEmpty() -> RowMessage(stringResource(R.string.keymint_apps_empty))
                else -> LazyColumn(modifier = Modifier.weight(1f, fill = false)) {
                    items(shown, key = { it.packageName }) { app ->
                        val on = app.packageName in chosen
                        AppSwitchRow(
                            app = app,
                            checked = on,
                            onToggle = {
                                chosen = if (on) chosen - app.packageName else chosen + app.packageName
                            },
                        )
                    }
                }
            }
        }
    }

    KeymintDialogShell(
        title = stringResource(R.string.keymint_apps_title),
        summary = stringResource(R.string.keymint_apps_selected, chosen.size),
        body = body,
        onDismiss = onDismiss,
        actions = {
            ActionButton(stringResource(R.string.keymint_select_all), modifier = Modifier.weight(1f)) {
                chosen = chosen + (apps ?: emptyList())
                    .filter { !it.isSystem }
                    .map { it.packageName }
            }
            ActionButton(
                stringResource(android.R.string.cancel),
                modifier = Modifier.weight(1f),
                onClick = onDismiss,
            )
            ActionButton(
                stringResource(R.string.keymint_save),
                primary = true,
                modifier = Modifier.weight(1f),
            ) {
                onSave(chosen.toList())
            }
        },
    )
}

@Composable
private fun LevelDialog(
    status: KeymintStatus,
    onDismiss: () -> Unit,
    onPick: (String, String) -> Unit,
) {
    val body = @Composable {
        Column(modifier = Modifier.heightIn(max = 460.dp).verticalScroll(rememberScrollState())) {
            LevelGroup(
                title = stringResource(R.string.keymint_level_keymint),
                levels = Keymint.LOG_LEVELS,
                current = status.logLevel,
                onPick = { onPick("config", it) },
            )
            Spacer(Modifier.height(12.dp))
            LevelGroup(
                title = stringResource(R.string.keymint_level_injector),
                levels = Keymint.INJECTOR_LOG_LEVELS,
                current = status.injectorLogLevel,
                onPick = { onPick("injector", it) },
            )
        }
    }

    KeymintDialogShell(
        title = stringResource(R.string.keymint_log_level),
        summary = null,
        body = body,
        onDismiss = onDismiss,
        actionsCentered = true,
        actions = {
            ActionButton(
                stringResource(android.R.string.cancel),
                modifier = Modifier.weight(1f),
                onClick = onDismiss,
            )
        },
    )
}

@Composable
private fun LevelGroup(
    title: String,
    levels: List<String>,
    current: String,
    onPick: (String) -> Unit,
) {
    Column {
        DialogSectionTitle(title)
        Card {
            levels.forEach { level ->
                val currentMark = if (level == current) {
                    stringResource(R.string.keymint_level_current)
                } else {
                    ""
                }
                if (LocalUiMode.current == UiMode.Miuix) {
                    RadioButtonPreference(
                        title = level + currentMark,
                        summary = Keymint.LEVEL_NOTES[level],
                        radioButtonLocation = RadioButtonLocation.End,
                        selected = level == current,
                        onClick = { onPick(level) },
                    )
                } else {
                    SegmentedRadioItem(
                        title = level + currentMark,
                        summary = Keymint.LEVEL_NOTES[level],
                        selected = level == current,
                        onClick = { onPick(level) },
                    )
                }
            }
        }
    }
}

/** A section heading inside a dialog, in whichever style the app is set to. */
@Composable
private fun DialogSectionTitle(title: String) {
    if (LocalUiMode.current == UiMode.Miuix) {
        SmallTitle(text = title)
    } else {
        Text(
            text = title,
            style = MaterialTheme.typography.titleSmall,
            modifier = Modifier.padding(start = 4.dp, top = 8.dp, bottom = 4.dp),
        )
    }
}

/** A row that leads somewhere: an arrow for Miuix, a list item for Material. */
@Composable
private fun DialogNavRow(
    title: String,
    summary: String?,
    icon: ImageVector,
    onClick: () -> Unit,
) {
    if (LocalUiMode.current == UiMode.Miuix) {
        ArrowPreference(
            title = title,
            summary = summary,
            insideMargin = PaddingValues(horizontal = DIALOG_INSET, vertical = 16.dp),
            startAction = {
                MiuixIcon(
                    imageVector = icon,
                    contentDescription = null,
                    tint = colorScheme.onBackground,
                    modifier = Modifier.padding(end = 6.dp),
                )
            },
            onClick = onClick,
        )
    } else {
        SegmentedListItem(
            onClick = onClick,
            headlineContent = { Text(title) },
            supportingContent = summary?.let { { Text(it) } },
            leadingContent = { Icon(icon, contentDescription = null) },
            trailingContent = {
                Icon(
                    Icons.AutoMirrored.Filled.KeyboardArrowRight,
                    contentDescription = null,
                )
            },
        )
    }
}

@Composable
private fun KeyboxDialog(
    status: KeymintStatus?,
    onDismiss: () -> Unit,
    onLocal: () -> Unit,
    onRemote: () -> Unit,
) {
    val body = @Composable {
        Column {
            DialogNavRow(
                title = stringResource(R.string.keymint_keybox_local),
                summary = stringResource(R.string.keymint_keybox_local_summary),
                icon = Icons.Filled.FolderOpen,
                onClick = onLocal,
            )
            DialogNavRow(
                title = stringResource(R.string.keymint_keybox_remote),
                summary = stringResource(R.string.keymint_keybox_remote_hint),
                icon = Icons.Filled.CloudDownload,
                onClick = onRemote,
            )
        }
    }

    KeymintDialogShell(
        title = stringResource(R.string.keymint_keybox),
        summary = if (status?.keyboxExists == true) {
            stringResource(R.string.keymint_keybox_size, formatSize(status.keyboxSize))
        } else {
            stringResource(R.string.keymint_keybox_none)
        },
        body = body,
        onDismiss = onDismiss,
        actionsCentered = true,
        actions = {
            ActionButton(
                stringResource(android.R.string.cancel),
                modifier = Modifier.weight(1f),
                onClick = onDismiss,
            )
        },
    )
}

@Composable
private fun RemoteKeyboxDialog(
    onDismiss: () -> Unit,
    onDownload: (String) -> Unit,
) {
    var url by remember { mutableStateOf(DEFAULT_KEYBOX_URL) }
    val body = @Composable {
        Column {
            SearchField(
                value = url,
                label = stringResource(R.string.keymint_keybox_url),
                modifier = Modifier,
            ) { url = it }
        }
    }

    KeymintDialogShell(
        title = stringResource(R.string.keymint_keybox_remote),
        summary = stringResource(R.string.keymint_keybox_remote_hint),
        body = body,
        onDismiss = onDismiss,
        actions = {
            ActionButton(
                stringResource(R.string.keymint_keybox_start),
                primary = true,
                modifier = Modifier.weight(1f),
            ) {
                val target = url.trim()
                if (!target.startsWith("https://", ignoreCase = true)) {
                    // Plain http would put the key material on the wire in the clear.
                    return@ActionButton
                }
                onDownload(target)
            }
            ActionButton(
                stringResource(android.R.string.cancel),
                modifier = Modifier.weight(1f),
                onClick = onDismiss,
            )
        },
    )
}

/** The dialog frame either style uses, with the buttons along the bottom. */
@Composable
private fun KeymintDialogShell(
    title: String,
    summary: String?,
    body: @Composable () -> Unit,
    onDismiss: () -> Unit,
    actionsCentered: Boolean = false,
    actions: @Composable RowScope.() -> Unit,
) {
    val uiMode = LocalUiMode.current
    val content = @Composable {
        Column {
            body()
            Spacer(Modifier.height(12.dp))
            Row(
                // Same inset the app's own dialogs use, so nothing sits on the dialog's edge.
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp),
                // Buttons keep their own width unless a caller asks for a share of the row, so a
                // lone "cancel" does not turn into a wide coloured bar along the bottom.
                horizontalArrangement = if (actionsCentered) Arrangement.Center else Arrangement.spacedBy(12.dp, Alignment.End),
                verticalAlignment = Alignment.CenterVertically,
            ) { actions() }
        }
    }

    if (uiMode == UiMode.Miuix) {
        OverlayDialog(
            show = true,
            title = title,
            summary = summary,
            onDismissRequest = onDismiss,
            // The same inside margin the app's own list dialog uses; without it the list inside
            // measures against a zero-width slot and every row collapses.
            insideMargin = DpSize(24.dp, 24.dp),
            content = content,
        )
    } else {
        AlertDialog(
            onDismissRequest = onDismiss,
            title = { Text(title) },
            text = {
                Column {
                    if (summary != null) {
                        Text(
                            text = summary,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Spacer(Modifier.height(10.dp))
                    }
                    content()
                }
            },
            confirmButton = {},
        )
    }
}

@Composable
private fun ActionButton(
    text: String,
    primary: Boolean = false,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    if (LocalUiMode.current == UiMode.Miuix) {
        MiuixTextButton(
            text = text,
            onClick = onClick,
            modifier = modifier,

        )
    } else {
        TextButton(onClick = onClick, modifier = modifier) { Text(text) }
    }
}

@Composable
private fun SearchField(
    value: String,
    label: String,
    modifier: Modifier = Modifier,
    onValueChange: (String) -> Unit,
) {
    if (LocalUiMode.current == UiMode.Miuix) {
        MiuixTextField(
            value = value,
            onValueChange = onValueChange,
            label = label,
            modifier = modifier.fillMaxWidth(),
        )
    } else {
        OutlinedTextField(
            value = value,
            onValueChange = onValueChange,
            singleLine = true,
            label = { Text(label) },
            modifier = modifier.fillMaxWidth(),
        )
    }
}

@Composable
private fun AppSwitchRow(app: KeymintApp, checked: Boolean, onToggle: () -> Unit) {
    val icon = @Composable {
        // AppIconImage fills whatever it is given, so it needs a size of its own in a row.
        AppIconImage(
            modifier = Modifier
                .padding(end = 6.dp)
                .size(32.dp),
            packageInfo = app.packageInfo,
            label = app.label,
        )
    }
    if (LocalUiMode.current == UiMode.Miuix) {
        // A switch row (SwitchPreference) gets squeezed to nothing inside this dialog; the
        // checkbox row is the shape the app's own list dialog uses and measures correctly.
        CheckboxPreference(
            title = app.label,
            summary = app.packageName,
            startAction = icon,
            insideMargin = PaddingValues(horizontal = 30.dp, vertical = 16.dp),
            checkboxLocation = CheckboxLocation.End,
            checked = checked,
            onCheckedChange = { onToggle() },
        )
    } else {
        SegmentedSwitchItem(
            title = app.label,
            summary = app.packageName,
            checked = checked,
            onCheckedChange = { onToggle() },
        )
    }
}

@Composable
private fun Loading() {
    Box(modifier = Modifier.fillMaxWidth().height(72.dp), contentAlignment = Alignment.Center) {
        when (LocalUiMode.current) {
            UiMode.Material -> CircularProgressIndicator()
            UiMode.Miuix -> InfiniteProgressIndicator()
        }
    }
}

/** A line of text where the app list would be, when there is nothing to list. */
@Composable
private fun RowMessage(message: String) {
    val modifier = Modifier
        .fillMaxWidth()
        .padding(horizontal = 4.dp, vertical = 12.dp)
    when (LocalUiMode.current) {
        // The Material one draws black inside a Miuix dialog: no Material theme is in scope there.
        UiMode.Miuix -> MiuixText(text = message, color = colorScheme.onSurface, modifier = modifier)
        UiMode.Material -> Text(text = message, modifier = modifier)
    }
}

private fun downloadKeybox(url: String): String {
    val connection = (URL(url).openConnection() as HttpURLConnection).apply {
        connectTimeout = 10_000
        readTimeout = 15_000
        instanceFollowRedirects = true
    }
    try {
        if (connection.responseCode !in 200..299) error("下载失败：HTTP ${connection.responseCode}")
        return connection.inputStream.use { stream -> stream.readBytes().toString(Charsets.UTF_8) }
    } finally {
        connection.disconnect()
    }
}

private fun formatSize(bytes: Long): String = when {
    bytes < 1024 -> "${bytes}B"
    else -> {
        val units = listOf("K", "M", "G", "T")
        var value = bytes.toDouble()
        var index = -1
        do {
            value /= 1024
            index++
        } while (value >= 1024 && index < units.size - 1)
        val rounded = if (value < 10) "%.1f".format(value) else value.roundToLong().toString()
        "$rounded${units[index]}"
    }
}

private const val DEFAULT_KEYBOX_URL =
    "https://gist.githubusercontent.com/wuhudiao/c3f7ee2cc3a10663fa53c719b567b2d8/raw/keybox.xml"

/** How far the dialog's own controls stay away from its edges. */
private val DIALOG_INSET = 16.dp
