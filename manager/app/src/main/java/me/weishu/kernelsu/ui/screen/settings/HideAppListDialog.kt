package me.weishu.kernelsu.ui.screen.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import me.weishu.kernelsu.R
import me.weishu.kernelsu.ui.LocalUiMode
import me.weishu.kernelsu.ui.UiMode
import me.weishu.kernelsu.ui.isMiuixFamily
import top.yukonga.miuix.kmp.basic.TextButton as MiuixTextButton
import top.yukonga.miuix.kmp.basic.Text as MiuixText
import top.yukonga.miuix.kmp.overlay.OverlayDialog
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme

/** Where the one-tap Hide My Applist config has got to, as far as the dialog is concerned. */
sealed interface HideAppListPhase {
    /** Asking which of the two variants to run. */
    data object Pick : HideAppListPhase

    /** The script is on the device, writing HMA's config. */
    data object Running : HideAppListPhase

    /** Finished; [text] is what the script printed. */
    data class Done(val text: String) : HideAppListPhase
}

/**
 * The two variants DikSU offers, and what came back from the one that ran.
 *
 * They are the same script with a different environment, so they are one row in the settings list
 * and this dialog rather than two rows that would read alike.
 *
 * It follows whichever interface style the app is set to, so it does not look like it came from
 * another app: Miuix when the app is Miuix, Material otherwise.
 */
@Composable
fun HideAppListDialog(
    phase: HideAppListPhase,
    onDismiss: () -> Unit,
    onRun: (Boolean) -> Unit,
) {
    if (LocalUiMode.current.isMiuixFamily) {
        HideAppListDialogMiuix(phase, onDismiss, onRun)
    } else {
        HideAppListDialogMaterial(phase, onDismiss, onRun)
    }
}

/** A running script cannot be called back, so while it runs the dialog has no way out. */
private fun HideAppListPhase.dismissable() = this != HideAppListPhase.Running

@Composable
private fun HideAppListDialogMaterial(
    phase: HideAppListPhase,
    onDismiss: () -> Unit,
    onRun: (Boolean) -> Unit,
) {
    AlertDialog(
        onDismissRequest = { if (phase.dismissable()) onDismiss() },
        title = { Text(stringResource(R.string.settings_hide_applist)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                when (phase) {
                    HideAppListPhase.Pick -> {
                        Text(stringResource(R.string.hide_applist_about))
                        Text(stringResource(R.string.hide_applist_scene_summary))
                    }

                    HideAppListPhase.Running -> Text(stringResource(R.string.hide_applist_running))
                    is HideAppListPhase.Done -> Text(phase.text)
                }
            }
        },
        confirmButton = {
            when (phase) {
                HideAppListPhase.Pick -> androidx.compose.material3.TextButton(
                    onClick = { onRun(false) },
                ) { Text(stringResource(R.string.hide_applist_plain)) }

                HideAppListPhase.Running -> Unit
                is HideAppListPhase.Done -> androidx.compose.material3.TextButton(
                    onClick = onDismiss,
                ) { Text(stringResource(android.R.string.ok)) }
            }
        },
        dismissButton = {
            if (phase == HideAppListPhase.Pick) {
                Row {
                    androidx.compose.material3.TextButton(onClick = { onRun(true) }) {
                        Text(stringResource(R.string.hide_applist_scene))
                    }
                    androidx.compose.material3.TextButton(onClick = onDismiss) {
                        Text(stringResource(android.R.string.cancel))
                    }
                }
            }
        },
    )
}

@Composable
private fun HideAppListDialogMiuix(
    phase: HideAppListPhase,
    onDismiss: () -> Unit,
    onRun: (Boolean) -> Unit,
) {
    OverlayDialog(
        show = true,
        title = stringResource(R.string.settings_hide_applist),
        onDismissRequest = { if (phase.dismissable()) onDismiss() },
        content = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                when (phase) {
                    HideAppListPhase.Pick -> {
                        MiuixText(
                            text = stringResource(R.string.hide_applist_about),
                            color = colorScheme.onSurface,
                        )
                        MiuixText(
                            text = stringResource(R.string.hide_applist_scene_summary),
                            color = colorScheme.onSurface,
                        )
                    }

                    HideAppListPhase.Running -> MiuixText(
                        text = stringResource(R.string.hide_applist_running),
                        color = colorScheme.onSurface,
                    )

                    is HideAppListPhase.Done -> MiuixText(
                        text = phase.text,
                        color = colorScheme.onSurface,
                    )
                }
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    when (phase) {
                        HideAppListPhase.Pick -> {
                            MiuixTextButton(
                                text = stringResource(R.string.hide_applist_plain),
                                onClick = { onRun(false) },
                                modifier = Modifier.weight(1f),
                            )
                            MiuixTextButton(
                                text = stringResource(R.string.hide_applist_scene),
                                onClick = { onRun(true) },
                                modifier = Modifier.weight(1f),
                            )
                            MiuixTextButton(
                                text = stringResource(android.R.string.cancel),
                                onClick = onDismiss,
                                modifier = Modifier.weight(1f),
                            )
                        }

                        HideAppListPhase.Running -> Unit
                        is HideAppListPhase.Done -> MiuixTextButton(
                            text = stringResource(android.R.string.ok),
                            onClick = onDismiss,
                            modifier = Modifier.weight(1f),
                        )
                    }
                }
            }
        },
    )
}
