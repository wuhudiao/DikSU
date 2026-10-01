package me.weishu.kernelsu.ui.screen.settings

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import me.weishu.kernelsu.R
import me.weishu.kernelsu.ui.LocalUiMode
import me.weishu.kernelsu.ui.UiMode
import me.weishu.kernelsu.ui.isMiuixFamily
import top.yukonga.miuix.kmp.basic.TextButton as MiuixTextButton
import top.yukonga.miuix.kmp.basic.TextField as MiuixTextField
import top.yukonga.miuix.kmp.basic.Text as MiuixText
import top.yukonga.miuix.kmp.overlay.OverlayDialog
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme

/**
 * The card that asks what a hidden Manager should look like: a name for the launcher, and a picture
 * for its icon. Both are baked into the APK the hide flow builds, so this only collects them —
 * [onApply] does the work. A copy that was already customized can put the plain Manager back, so it
 * offers that too.
 *
 * It follows whichever interface style the app is set to, so it does not look like it came from
 * another app: Miuix when the app is Miuix, Material otherwise.
 */
@Composable
fun HideManagerDialog(
    defaultName: String,
    canRestore: Boolean,
    onDismiss: () -> Unit,
    onApply: (String, ByteArray?) -> Unit,
    onRestore: () -> Unit,
) {
    if (LocalUiMode.current.isMiuixFamily) {
        HideManagerDialogMiuix(defaultName, canRestore, onDismiss, onApply, onRestore)
    } else {
        HideManagerDialogMaterial(defaultName, canRestore, onDismiss, onApply, onRestore)
    }
}

@Composable
private fun HideManagerDialogMaterial(
    defaultName: String,
    canRestore: Boolean,
    onDismiss: () -> Unit,
    onApply: (String, ByteArray?) -> Unit,
    onRestore: () -> Unit,
) {
    val state = rememberHideManagerState(defaultName)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.settings_hide_manager)) },
        text = {
            Column(
                verticalArrangement = Arrangement.spacedBy(12.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                IconSlotMaterial(state) { state.pick() }
                OutlinedTextField(
                    value = state.name,
                    onValueChange = { state.name = it },
                    singleLine = true,
                    label = { Text(stringResource(R.string.hide_manager_name_hint)) },
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        },
        confirmButton = {
            androidx.compose.material3.TextButton(
                onClick = { onApply(state.name.trim(), state.icon) },
                enabled = state.name.isNotBlank(),
            ) { Text(stringResource(R.string.hide_manager_apply)) }
        },
        dismissButton = {
            Row {
                if (canRestore) {
                    androidx.compose.material3.TextButton(onClick = onRestore) {
                        Text(stringResource(R.string.hide_manager_restore))
                    }
                }
                androidx.compose.material3.TextButton(onClick = onDismiss) {
                    Text(stringResource(android.R.string.cancel))
                }
            }
        },
    )
}

@Composable
private fun HideManagerDialogMiuix(
    defaultName: String,
    canRestore: Boolean,
    onDismiss: () -> Unit,
    onApply: (String, ByteArray?) -> Unit,
    onRestore: () -> Unit,
) {
    val state = rememberHideManagerState(defaultName)
    OverlayDialog(
        show = true,
        title = stringResource(R.string.settings_hide_manager),
        onDismissRequest = onDismiss,
        content = {
            Column(
                verticalArrangement = Arrangement.spacedBy(12.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Box(
                    contentAlignment = Alignment.Center,
                    modifier = Modifier
                        .size(ICON_SLOT)
                        .clip(RoundedCornerShape(ICON_CORNER))
                        .background(colorScheme.surfaceContainerHigh)
                        .clickable { state.pick() },
                ) {
                    val preview = state.preview
                    if (preview != null) {
                        Image(
                            bitmap = preview.asImageBitmap(),
                            contentDescription = null,
                            contentScale = ContentScale.Crop,
                            // Fills the already-rounded box, so size and corners match it exactly.
                            modifier = Modifier.matchParentSize(),
                        )
                    } else {
                        MiuixText(
                            text = "+",
                            fontSize = 40.sp,
                            color = androidx.compose.ui.graphics.Color.Gray,
                        )
                    }
                }
                MiuixTextField(
                    value = state.name,
                    onValueChange = { state.name = it },
                    label = stringResource(R.string.hide_manager_name_hint),
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (canRestore) {
                        MiuixTextButton(
                            text = stringResource(R.string.hide_manager_restore),
                            onClick = onRestore,
                            modifier = Modifier.weight(1f),
                        )
                    }
                    MiuixTextButton(
                        text = stringResource(android.R.string.cancel),
                        onClick = onDismiss,
                        modifier = Modifier.weight(1f),
                    )
                    MiuixTextButton(
                        text = stringResource(R.string.hide_manager_apply),
                        onClick = { onApply(state.name.trim(), state.icon) },
                        enabled = state.name.isNotBlank(),
                        modifier = Modifier.weight(1f),
                    )
                }
            }
        },
    )
}

/** The name, the picture and the picker behind either style. */
private class HideManagerState(defaultName: String) {
    var name by mutableStateOf(defaultName)
    var icon by mutableStateOf<ByteArray?>(null)
    var preview by mutableStateOf<Bitmap?>(null)
    var pick: () -> Unit = {}
}

@Composable
private fun rememberHideManagerState(defaultName: String): HideManagerState {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val state = remember { HideManagerState(defaultName) }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
        if (uri != null) {
            scope.launch {
                val picked = withContext(Dispatchers.IO) {
                    val decoded = decodeBitmap(context, uri) ?: return@withContext null
                    squarePreview(decoded) to iconPng(decoded, context.resources.displayMetrics.densityDpi)
                }
                state.preview = picked?.first
                state.icon = picked?.second
            }
        }
    }
    state.pick = {
        picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
    }
    return state
}

@Composable
private fun IconSlotMaterial(state: HideManagerState, onPick: () -> Unit) {
    Box(
        modifier = Modifier
            .size(ICON_SLOT)
            .clip(RoundedCornerShape(ICON_CORNER))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .clickable(onClick = onPick),
        contentAlignment = Alignment.Center,
    ) {
        val preview = state.preview
        if (preview != null) {
            Image(
                bitmap = preview.asImageBitmap(),
                contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier.matchParentSize(),
        )
        } else {
            Text(text = "+", fontSize = 40.sp, color = androidx.compose.ui.graphics.Color.Gray)
        }
    }
}

/**
 * The card shows the user's picture centre-cropped to a square, so it fills the icon slot exactly
 * the way the empty slot's plus sign does. The copy that goes into the APK is treated differently —
 * see [iconPng] — because an adaptive icon is drawn inside a mask.
 */
private fun squarePreview(decoded: Bitmap): Bitmap {
    val side = minOf(decoded.width, decoded.height)
    return Bitmap.createBitmap(
        decoded,
        (decoded.width - side) / 2,
        (decoded.height - side) / 2,
        side,
        side,
    )
}

/** The picked file, shrunk while decoding so a big photo does not fill memory. */
private fun decodeBitmap(context: Context, uri: Uri, hint: Int = 256): Bitmap? = runCatching {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
    val options = BitmapFactory.Options().apply {
        inSampleSize = Integer.highestOneBit((minOf(bounds.outWidth, bounds.outHeight) / hint).coerceAtLeast(1))
    }
    context.contentResolver.openInputStream(uri)?.use {
        BitmapFactory.decodeStream(it, null, options)
    }
}.getOrNull()

/**
 * The picked picture as a PNG the launcher can use.
 *
 * The icon is an adaptive icon's foreground layer: the launcher scales the 108dp canvas up and then
 * masks the edges away, so a picture that fills the canvas comes out zoomed and cropped. The mask
 * only ever shows the middle, so the picture goes there — whole, centred on white.
 */
private fun iconPng(decoded: Bitmap, densityDpi: Int): ByteArray? = runCatching {
    val target = (ICON_DP * densityDpi / 160).coerceAtMost(2048)
    // Keep the picture at the size the safe zone gives it (so the icon reads the same size as the
    // original) but fill the rest of the canvas with the picture's own edge colour instead of
    // white. A white ring appeared when a launcher showed more than the safe zone; a fill taken
    // from the picture's own border never shows a seam.
    val content = (target * SAFE_ZONE).toInt()
    val ratio = minOf(content.toFloat() / decoded.width, content.toFloat() / decoded.height)
    val inner = Bitmap.createScaledBitmap(
        decoded,
        (decoded.width * ratio).toInt().coerceAtLeast(1),
        (decoded.height * ratio).toInt().coerceAtLeast(1),
        true,
    )
    val canvas = Bitmap.createBitmap(target, target, Bitmap.Config.ARGB_8888)
    Canvas(canvas).apply {
        drawColor(edgeColor(decoded))
        drawBitmap(
            inner,
            (target - inner.width) / 2f,
            (target - inner.height) / 2f,
            Paint(Paint.FILTER_BITMAP_FLAG),
        )
    }
    java.io.ByteArrayOutputStream().use { out ->
        canvas.compress(Bitmap.CompressFormat.PNG, 100, out)
        canvas.recycle()
        inner.recycle()
        out.toByteArray()
    }
}.getOrNull()

/**
 * The average of the picture's four edges  the colour a launcher sees outside the safe zone.
 * Using it instead of white means the ring around the icon blends into the picture rather than
 * showing a hard white border.
 */
private fun edgeColor(bmp: Bitmap): Int {
    val w = bmp.width
    val h = bmp.height
    if (w < 2 || h < 2) return Color.WHITE
    var r = 0L; var g = 0L; var b = 0L; var n = 0L
    val step = maxOf(1, minOf(w, h) / 32)
    var x = 0
    while (x < w) {
        for (y in intArrayOf(0, h - 1)) {
            val p = bmp.getPixel(x, y)
            r += Color.red(p); g += Color.green(p); b += Color.blue(p); n++
        }
        x += step
    }
    var y = 0
    while (y < h) {
        for (x2 in intArrayOf(0, w - 1)) {
            val p = bmp.getPixel(x2, y)
            r += Color.red(p); g += Color.green(p); b += Color.blue(p); n++
        }
        y += step
    }
    if (n == 0L) return Color.WHITE
    return Color.rgb((r / n).toInt(), (g / n).toInt(), (b / n).toInt())
}

/** 108dp is the adaptive-icon canvas; anything the launcher shows is cut out of it. */
private const val ICON_DP = 108

/** The square in the card where the picture goes, and how round its corners are. */
private val ICON_SLOT = 80.dp
private val ICON_CORNER = 20.dp

/** The share of that canvas a launcher is guaranteed to keep — the adaptive icon's safe zone. */
private const val SAFE_ZONE = 0.66f
