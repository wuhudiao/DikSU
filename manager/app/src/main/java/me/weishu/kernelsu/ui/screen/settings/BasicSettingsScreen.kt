package me.weishu.kernelsu.ui.screen.settings

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Article
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.automirrored.filled.Rule
import androidx.compose.material.icons.filled.Adb
import androidx.compose.material.icons.filled.AdminPanelSettings
import androidx.compose.material.icons.filled.DeveloperMode
import androidx.compose.material.icons.filled.FlashOn
import androidx.compose.material.icons.filled.LayersClear
import androidx.compose.material.icons.filled.RestartAlt
import androidx.compose.material.icons.filled.Security
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.compose.dropUnlessResumed
import androidx.lifecycle.viewmodel.compose.viewModel
import me.weishu.kernelsu.R
import me.weishu.kernelsu.ui.LocalUiMode
import me.weishu.kernelsu.ui.UiMode
import me.weishu.kernelsu.ui.component.material.SegmentedColumn
import me.weishu.kernelsu.ui.component.material.SegmentedDropdownItem
import me.weishu.kernelsu.ui.component.material.SegmentedListItem
import me.weishu.kernelsu.ui.component.material.SegmentedSwitchItem
import me.weishu.kernelsu.ui.navigation3.LocalNavigator
import me.weishu.kernelsu.ui.navigation3.Route
import me.weishu.kernelsu.ui.viewmodel.SettingsViewModel
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.Scaffold as MiuixScaffold
import top.yukonga.miuix.kmp.basic.TopAppBar as MiuixTopAppBar
import top.yukonga.miuix.kmp.basic.Icon as MiuixIcon
import top.yukonga.miuix.kmp.basic.IconButton as MiuixIconButton
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Back
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.preference.OverlayDropdownPreference
import top.yukonga.miuix.kmp.preference.SwitchPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme

/**
 * The switches that used to sit in the middle of Settings, moved to a page of their own so the main
 * page is about the Manager as a whole.
 *
 * `su` compatibility stays a three-way choice (on by default / off until reboot / off), everything
 * else is on or off, and every row keeps the state check it had before: a feature the running
 * kernel does not support stays disabled.
 */
@Composable
fun BasicSettingsScreen() {
    val navigator = LocalNavigator.current
    val viewModel = viewModel<SettingsViewModel>()
    val uiState by viewModel.uiState.collectAsStateWithLifecycle()
    val onBack = dropUnlessResumed { navigator.pop() }
    val onOpenProfileTemplate = dropUnlessResumed { navigator.push(Route.AppProfileTemplate) }

    when (LocalUiMode.current) {
        UiMode.Miuix, UiMode.MiuixStock -> BasicSettingsMiuix(
            uiState, viewModel, onBack, onOpenProfileTemplate,
        )
        UiMode.Material -> BasicSettingsMaterial(
            uiState, viewModel, onBack, onOpenProfileTemplate,
        )
    }
}

/** One row's worth of what both styles need. */
private class BasicRow(
    val icon: ImageVector,
    val title: Int,
    val summary: String,
    val enabled: Boolean,
    val checked: Boolean,
    val onCheckedChange: (Boolean) -> Unit,
)

@Composable
private fun basicRows(uiState: SettingsUiState, viewModel: SettingsViewModel): List<BasicRow> = listOf(
    BasicRow(
        icon = Icons.Filled.LayersClear,
        title = R.string.settings_kernel_umount,
        summary = statusSummary(
            uiState.kernelUmountStatus,
            R.string.settings_kernel_umount_summary,
        ),
        enabled = uiState.kernelUmountStatus == "supported",
        checked = uiState.isKernelUmountEnabled,
        onCheckedChange = viewModel::setKernelUmountEnabled,
    ),
    BasicRow(
        icon = Icons.Filled.Security,
        title = R.string.settings_selinux_hide,
        summary = statusSummary(
            uiState.selinuxHideStatus,
            R.string.settings_selinux_hide_summary,
        ),
        enabled = uiState.selinuxHideStatus == "supported",
        checked = uiState.isSelinuxHideEnabled,
        onCheckedChange = viewModel::setSelinuxHideEnabled,
    ),
    BasicRow(
        icon = Icons.AutoMirrored.Filled.Article,
        title = R.string.settings_sulog,
        summary = statusSummary(uiState.sulogStatus, R.string.settings_sulog_summary),
        enabled = uiState.sulogStatus == "supported",
        checked = uiState.isSulogEnabled,
        onCheckedChange = viewModel::setSulogEnabled,
    ),
    BasicRow(
        icon = Icons.Filled.Adb,
        title = R.string.settings_adb_root,
        summary = statusSummary(uiState.adbRootStatus, R.string.settings_adb_root_summary),
        enabled = uiState.adbRootStatus == "supported",
        checked = uiState.isAdbRootEnabled,
        onCheckedChange = viewModel::setAdbRootEnabled,
    ),
    BasicRow(
        icon = Icons.Filled.RestartAlt,
        title = R.string.settings_soft_reboot,
        summary = stringResource(R.string.settings_soft_reboot_summary),
        enabled = !uiState.isLateLoadMode,
        checked = uiState.isLateLoadMode || uiState.useSoftReboot,
        onCheckedChange = viewModel::setUseSoftReboot,
    ),
    BasicRow(
        icon = Icons.AutoMirrored.Filled.Rule,
        title = R.string.settings_umount_modules_default,
        summary = stringResource(R.string.settings_umount_modules_default_summary),
        enabled = true,
        checked = uiState.isDefaultUmountModules,
        onCheckedChange = viewModel::setDefaultUmountModules,
    ),
    BasicRow(
        icon = Icons.Filled.DeveloperMode,
        title = R.string.enable_web_debugging,
        summary = stringResource(R.string.enable_web_debugging_summary),
        enabled = true,
        checked = uiState.enableWebDebugging,
        onCheckedChange = viewModel::setEnableWebDebugging,
    ),
    BasicRow(
        icon = Icons.Filled.FlashOn,
        title = R.string.settings_auto_jailbreak,
        summary = stringResource(R.string.settings_auto_jailbreak_summary),
        enabled = uiState.isLateLoadMode,
        checked = uiState.autoJailbreak,
        onCheckedChange = viewModel::setAutoJailbreak,
    ),
)

/** A feature the kernel reports as unsupported or managed says so instead of its own summary. */
@Composable
private fun statusSummary(status: String, fallback: Int): String = when (status) {
    "unsupported" -> stringResource(R.string.feature_status_unsupported_summary)
    "managed" -> stringResource(R.string.feature_status_managed_summary)
    else -> stringResource(fallback)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun BasicSettingsMaterial(
    uiState: SettingsUiState,
    viewModel: SettingsViewModel,
    onBack: () -> Unit,
    onOpenProfileTemplate: () -> Unit,
) {
    val rows = basicRows(uiState, viewModel)
    val suCompatItems = listOf(
        stringResource(R.string.settings_mode_enable_by_default),
        stringResource(R.string.settings_mode_disable_until_reboot),
        stringResource(R.string.settings_mode_disable_always),
    )
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings_basic)) },
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
            SegmentedColumn(
                modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 13.dp),
                content = buildList {
                    add {
                        val profileTemplate = stringResource(R.string.settings_profile_template)
                        SegmentedListItem(
                            onClick = onOpenProfileTemplate,
                            headlineContent = { Text(profileTemplate) },
                            supportingContent = {
                                Text(stringResource(R.string.settings_profile_template_summary))
                            },
                            leadingContent = {
                                Icon(Icons.AutoMirrored.Filled.Article, profileTemplate)
                            },
                            trailingContent = {
                                Icon(
                                    Icons.AutoMirrored.Filled.KeyboardArrowRight,
                                    contentDescription = null
                                )
                            },
                        )
                    }
                    add {
                        SegmentedDropdownItem(
                            icon = Icons.Filled.AdminPanelSettings,
                            title = stringResource(R.string.settings_sucompat),
                            summary = statusSummary(
                                uiState.suCompatStatus,
                                R.string.settings_sucompat_summary,
                            ),
                            items = suCompatItems,
                            enabled = uiState.suCompatStatus == "supported",
                            selectedIndex = uiState.suCompatMode,
                            onItemSelected = viewModel::setSuCompatMode,
                        )
                    }
                    rows.forEach { row ->
                        add {
                            SegmentedSwitchItem(
                                icon = row.icon,
                                title = stringResource(row.title),
                                summary = row.summary,
                                enabled = row.enabled,
                                checked = row.checked,
                                onCheckedChange = row.onCheckedChange,
                            )
                        }
                    }
                },
            )
        }
    }
}

@Composable
private fun BasicSettingsMiuix(
    uiState: SettingsUiState,
    viewModel: SettingsViewModel,
    onBack: () -> Unit,
    onOpenProfileTemplate: () -> Unit,
) {
    val rows = basicRows(uiState, viewModel)
    val suCompatItems = listOf(
        stringResource(R.string.settings_mode_enable_by_default),
        stringResource(R.string.settings_mode_disable_until_reboot),
        stringResource(R.string.settings_mode_disable_always),
    )
    MiuixScaffold(
        topBar = {
            MiuixTopAppBar(
                title = stringResource(R.string.settings_basic),
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
        // A LazyColumn, the same container the settings page itself uses, and it has to be told
        // about the top bar's height — otherwise the first rows sit behind the bar.
        LazyColumn(
            modifier = Modifier.padding(horizontal = 12.dp),
            contentPadding = innerPadding,
        ) {
            item {
                Card(modifier = Modifier.padding(top = 12.dp)) {
                    val profileTemplate = stringResource(R.string.settings_profile_template)
                    ArrowPreference(
                        title = profileTemplate,
                        summary = stringResource(R.string.settings_profile_template_summary),
                        startAction = {
                            MiuixIcon(
                                imageVector = Icons.AutoMirrored.Filled.Article,
                                contentDescription = null,
                                tint = colorScheme.onBackground,
                                modifier = Modifier.padding(end = 6.dp),
                            )
                        },
                        onClick = onOpenProfileTemplate,
                    )
                }
                Card(modifier = Modifier.padding(top = 12.dp)) {
                    OverlayDropdownPreference(
                        title = stringResource(R.string.settings_sucompat),
                        summary = statusSummary(uiState.suCompatStatus, R.string.settings_sucompat_summary),
                        items = suCompatItems,
                        startAction = {
                            MiuixIcon(
                                imageVector = Icons.Filled.AdminPanelSettings,
                                contentDescription = null,
                                tint = colorScheme.onBackground,
                                modifier = Modifier.padding(end = 6.dp),
                            )
                        },
                        enabled = uiState.suCompatStatus == "supported",
                        selectedIndex = uiState.suCompatMode,
                        onSelectedIndexChange = viewModel::setSuCompatMode,
                    )
                    rows.take(ROWS_WITH_SU_COMPAT).forEach { MiuixSwitchRow(it) }
                }
                Card(modifier = Modifier.padding(top = 12.dp)) {
                    rows.drop(ROWS_WITH_SU_COMPAT).forEach { MiuixSwitchRow(it) }
                }
            }
        }
    }
}

/** How many of the switches belong in the card with the su-compat choice; the rest get their own. */
private const val ROWS_WITH_SU_COMPAT = 5

@Composable
private fun MiuixSwitchRow(row: BasicRow) {
    SwitchPreference(
        title = stringResource(row.title),
        summary = row.summary,
        startAction = {
            MiuixIcon(
                imageVector = row.icon,
                contentDescription = null,
                tint = colorScheme.onBackground,
                modifier = Modifier.padding(end = 6.dp),
            )
        },
        enabled = row.enabled,
        checked = row.checked,
        onCheckedChange = row.onCheckedChange,
    )
}
