package me.weishu.kernelsu.ui.screen.settings

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import android.widget.Toast
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import me.weishu.kernelsu.R
import me.weishu.kernelsu.ui.LocalUiMode
import me.weishu.kernelsu.ui.UiMode
import me.weishu.kernelsu.ui.navigation3.Navigator
import me.weishu.kernelsu.ui.navigation3.Route
import me.weishu.kernelsu.ui.util.ManagerHider
import me.weishu.kernelsu.ui.viewmodel.SettingsViewModel

@Composable
fun SettingPager(
    navigator: Navigator,
    bottomInnerPadding: Dp,
    isCurrentPage: Boolean = true,
) {
    val viewModel = viewModel<SettingsViewModel>()
    val uiState by viewModel.uiState.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var hideDialogShown by rememberSaveable { mutableStateOf(false) }
    var canRestore by rememberSaveable { mutableStateOf(ManagerHider.canRestore(context)) }
    val latestIsCurrentPage by rememberUpdatedState(isCurrentPage)
    val initialResumeHandled = rememberSaveable { mutableStateOf(false) }

    LaunchedEffect(isCurrentPage) {
        if (isCurrentPage) {
            viewModel.refresh()
        }
    }

    LifecycleResumeEffect(Unit) {
        if (initialResumeHandled.value && latestIsCurrentPage) {
            viewModel.refresh()
        }
        initialResumeHandled.value = true
        onPauseOrDispose { }
    }

    val actions = SettingsScreenActions(
        onSetCheckUpdate = viewModel::setCheckUpdate,
        onSetCheckModuleUpdate = viewModel::setCheckModuleUpdate,
        onOpenTheme = { navigator.push(Route.ColorPalette) },
        onSetUiModeIndex = { index ->
            viewModel.setUiMode(if (index == 0) UiMode.Miuix.value else UiMode.Material.value)
        },
        onOpenProfileTemplate = { navigator.push(Route.AppProfileTemplate) },
        onOpenBasicSettings = { navigator.push(Route.BasicSettings) },
        onOpenOtherFeatures = { navigator.push(Route.OtherFeatures) },
        onSetSuCompatMode = viewModel::setSuCompatMode,
        onSetKernelUmountEnabled = viewModel::setKernelUmountEnabled,
        onSetSelinuxHideEnabled = viewModel::setSelinuxHideEnabled,
        onSetSulogEnabled = viewModel::setSulogEnabled,
        onSetAdbRootEnabled = viewModel::setAdbRootEnabled,
        onSetDefaultUmountModules = viewModel::setDefaultUmountModules,
        onSetEnableWebDebugging = viewModel::setEnableWebDebugging,
        onSetAutoJailbreak = viewModel::setAutoJailbreak,
        onSetUseSoftReboot = viewModel::setUseSoftReboot,
        onOpenAbout = { navigator.push(Route.About) },
        onHideManager = {
            canRestore = ManagerHider.canRestore(context)
            hideDialogShown = true
        },
    )

    if (hideDialogShown) {
        HideManagerDialog(
            // Nothing prefilled: the card starts empty, and the user decides what to show.
            defaultName = "",
            canRestore = canRestore,
            onDismiss = { hideDialogShown = false },
            onApply = { name, icon ->
                hideDialogShown = false
                Toast.makeText(context, R.string.hide_manager_running, Toast.LENGTH_SHORT).show()
                scope.launch {
                    // Repacking, signing and installing an APK, then finishing the swap.
                    val installed = withContext(Dispatchers.IO) { ManagerHider.hide(context, name, icon) }
                    Toast.makeText(
                        context,
                        if (installed != null) {
                            context.getString(R.string.hide_manager_done, installed)
                        } else {
                            context.getString(R.string.hide_manager_failed)
                        },
                        Toast.LENGTH_LONG,
                    ).show()
                }
            },
            onRestore = {
                hideDialogShown = false
                Toast.makeText(context, R.string.hide_manager_restore_done, Toast.LENGTH_SHORT).show()
                scope.launch {
                    val restored = withContext(Dispatchers.IO) { ManagerHider.restore(context) }
                    if (!restored) {
                        Toast.makeText(context, R.string.hide_manager_failed, Toast.LENGTH_LONG).show()
                    }
                }
            },
        )
    }

    when (LocalUiMode.current) {
        UiMode.Miuix -> SettingPagerMiuix(uiState, actions, bottomInnerPadding)
        UiMode.Material -> SettingPagerMaterial(uiState, actions, bottomInnerPadding)
    }
}
