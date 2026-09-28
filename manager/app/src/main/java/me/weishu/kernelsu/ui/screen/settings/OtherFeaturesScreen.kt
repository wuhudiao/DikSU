package me.weishu.kernelsu.ui.screen.settings

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Security
import androidx.compose.material.icons.filled.VisibilityOff
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.dropUnlessResumed
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import me.weishu.kernelsu.R
import me.weishu.kernelsu.ui.LocalUiMode
import me.weishu.kernelsu.ui.UiMode
import me.weishu.kernelsu.ui.component.material.SegmentedColumn
import me.weishu.kernelsu.ui.component.material.SegmentedListItem
import me.weishu.kernelsu.ui.navigation3.LocalNavigator
import me.weishu.kernelsu.ui.navigation3.Route
import me.weishu.kernelsu.ui.util.HideAppList
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.Icon as MiuixIcon
import top.yukonga.miuix.kmp.basic.IconButton as MiuixIconButton
import top.yukonga.miuix.kmp.basic.Scaffold as MiuixScaffold
import top.yukonga.miuix.kmp.basic.TopAppBar as MiuixTopAppBar
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Back
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme

/** The two extras that came over from DikSU: the one-tap Hide My Applist config, and the keyMint panel. */
@Composable
fun OtherFeaturesScreen() {
    val navigator = LocalNavigator.current
    val onBack = dropUnlessResumed { navigator.pop() }
    val onOpenKeymint = dropUnlessResumed { navigator.push(Route.Keymint) }

    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var phase by remember { mutableStateOf<HideAppListPhase?>(null) }

    val onHideAppList = { phase = HideAppListPhase.Pick }
    val onRunHideAppList: (Boolean) -> Unit = { scene ->
        phase = HideAppListPhase.Running
        scope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    val script = context.assets.open(HideAppList.SCRIPT_ASSET).use {
                        it.readBytes().toString(Charsets.UTF_8)
                    }
                    HideAppList.run(scene, script)
                }
            }
            phase = HideAppListPhase.Done(
                result.getOrElse { it.message ?: context.getString(R.string.hide_applist_failed) }
            )
        }
    }

    when (LocalUiMode.current) {
        UiMode.Material -> OtherFeaturesMaterial(onBack, onOpenKeymint, onHideAppList)
        UiMode.Miuix -> OtherFeaturesMiuix(onBack, onOpenKeymint, onHideAppList)
    }

    val open = phase
    if (open != null) {
        HideAppListDialog(
            phase = open,
            onDismiss = { phase = null },
            onRun = onRunHideAppList,
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun OtherFeaturesMaterial(
    onBack: () -> Unit,
    onOpenKeymint: () -> Unit,
    onHideAppList: () -> Unit,
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings_other)) },
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
                modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 13.dp, bottom = 13.dp),
                content = listOf(
                    {
                        val hide = stringResource(R.string.settings_hide_applist)
                        SegmentedListItem(
                            onClick = onHideAppList,
                            headlineContent = { Text(hide) },
                            supportingContent = {
                                Text(stringResource(R.string.settings_hide_applist_summary))
                            },
                            leadingContent = { Icon(Icons.Filled.VisibilityOff, hide) },
                            trailingContent = {
                                Icon(
                                    Icons.AutoMirrored.Filled.KeyboardArrowRight,
                                    contentDescription = null
                                )
                            },
                        )
                    },
                    {
                        val keymint = stringResource(R.string.settings_keymint_config)
                        SegmentedListItem(
                            onClick = onOpenKeymint,
                            headlineContent = { Text(keymint) },
                            supportingContent = {
                                Text(stringResource(R.string.settings_keymint_config_summary))
                            },
                            leadingContent = { Icon(Icons.Filled.Security, keymint) },
                            trailingContent = {
                                Icon(
                                    Icons.AutoMirrored.Filled.KeyboardArrowRight,
                                    contentDescription = null
                                )
                            },
                        )
                    },
                ),
            )
        }
    }
}

@Composable
private fun OtherFeaturesMiuix(
    onBack: () -> Unit,
    onOpenKeymint: () -> Unit,
    onHideAppList: () -> Unit,
) {
    MiuixScaffold(
        topBar = {
            MiuixTopAppBar(
                title = stringResource(R.string.settings_other),
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
            item {
                Card(modifier = Modifier.padding(top = 12.dp)) {
                    ArrowPreference(
                        title = stringResource(R.string.settings_hide_applist),
                        summary = stringResource(R.string.settings_hide_applist_summary),
                        startAction = {
                            MiuixIcon(
                                imageVector = Icons.Filled.VisibilityOff,
                                contentDescription = null,
                                tint = colorScheme.onBackground,
                                modifier = Modifier.padding(end = 6.dp),
                            )
                        },
                        onClick = onHideAppList,
                    )
                    ArrowPreference(
                        title = stringResource(R.string.settings_keymint_config),
                        summary = stringResource(R.string.settings_keymint_config_summary),
                        startAction = {
                            MiuixIcon(
                                imageVector = Icons.Filled.Security,
                                contentDescription = null,
                                tint = colorScheme.onBackground,
                                modifier = Modifier.padding(end = 6.dp),
                            )
                        },
                        onClick = onOpenKeymint,
                    )
                }
            }
        }
    }
}
