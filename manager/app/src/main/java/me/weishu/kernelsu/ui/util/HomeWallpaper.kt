package me.weishu.kernelsu.ui.util

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.foundation.Image
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.BiasAlignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.overlay.OverlayDialog
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.Modifier
import androidx.compose.foundation.layout.padding
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import me.weishu.kernelsu.R
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme
import java.io.File

/** Whether the reader has put a picture behind the pages: the switch the app's mode follows. */
@Composable
fun rememberWallpaperSet(): Boolean {
    val context = LocalContext.current
    val version = HomeWallpaperStore.version
    return remember(context, version) { HomeWallpaperStore.file(context).exists() }
}

/**
 * The picture the home's backdrop is made of.
 *
 * One file in the app's own directory. The picked image is re-encoded smaller on the way in, so a
 * phone photo neither blows memory up when it is decoded nor asks for a storage permission later.
 *
 * [version] is what the home watches: bumping it re-reads the file, so a new picture lands without
 * restarting the app.
 */
object HomeWallpaperStore {
    /** What the two slots are cropped to: the backdrop is the screen, the card is a wide strip. */
    const val WALLPAPER_ASPECT = 0.55f
    const val STATUS_ASPECT = 2.5f

    private const val NAME = "home-wallpaper.jpg"
    private const val STATUS_NAME = "home-status-card.jpg"
    // Big enough for a full screen: at 1440 the picture was upscaled to fill one and read as soft.
    private const val MAX_EDGE = 2880
    private const val QUALITY = 95
    /** Most of a side a picked picture may lose to black bars. */
    private const val MAX_BAR = 0.25f

    /**
     * How solid the page panel is. Pinned at nothing: the page is meant to sit on the picture — or
     * on the theme's own gradient when there is no picture — so the panel adds no fill of its own.
     */
    const val PANEL_FILL = 0f

    var version by mutableIntStateOf(0)
        private set

    fun file(context: Context): File = File(context.filesDir, NAME)

    fun statusFile(context: Context): File = File(context.filesDir, STATUS_NAME)

    /** The picture in [file], decoded small enough to keep a phone photo out of trouble. */
    fun load(file: File): ImageBitmap? = runCatching {
        if (!file.exists()) return@runCatching null
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(file.absolutePath, bounds)
        if (bounds.outWidth <= 0) return@runCatching null
        var sample = 1
        while (maxOf(bounds.outWidth, bounds.outHeight) / sample > MAX_EDGE) sample *= 2
        BitmapFactory.decodeFile(
            file.absolutePath,
            BitmapFactory.Options().apply { inSampleSize = sample },
        )?.asImageBitmap()
    }.getOrNull()

    /** [uri] decoded with its longest edge at most [maxEdge] long. */
    fun decode(context: Context, uri: Uri, maxEdge: Int): Bitmap? {
        val resolver = context.contentResolver
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        var sample = 1
        while (maxOf(bounds.outWidth, bounds.outHeight) / sample > maxEdge) sample *= 2
        return resolver.openInputStream(uri)?.use {
            BitmapFactory.decodeStream(it, null, BitmapFactory.Options().apply { inSampleSize = sample })
        }?.let { trimBars(it) }
    }

    /**
     * The picture without the black bars a file sometimes arrives with — a shot inside a taller
     * canvas has them down both sides, and they are part of the picture, so cropping cannot remove
     * them later; the card just ends up with two empty ends. Capped, so a dark photo only ever
     * loses its very edge.
     */
    private fun trimBars(source: Bitmap): Bitmap {
        val maxX = (source.width * MAX_BAR).toInt()
        val maxY = (source.height * MAX_BAR).toInt()
        val stepX = maxOf(1, source.width / 64)
        val stepY = maxOf(1, source.height / 64)
        fun black(x: Int, y: Int): Boolean {
            // Every channel under 16, alpha ignored.
            return (source.getPixel(x, y) and 0x00FFFFFF) < 0x00101010
        }
        var left = 0
        while (left < maxX && (0 until source.height step stepY).all { black(left, it) }) left++
        var right = source.width - 1
        while (source.width - 1 - right < maxX && (0 until source.height step stepY).all { black(right, it) }) right--
        var top = 0
        while (top < maxY && (0 until source.width step stepX).all { black(it, top) }) top++
        var bottom = source.height - 1
        while (source.height - 1 - bottom < maxY && (0 until source.width step stepX).all { black(it, bottom) }) bottom--
        if (left == 0 && top == 0 && right == source.width - 1 && bottom == source.height - 1) {
            return source
        }
        return Bitmap.createBitmap(source, left, top, right - left + 1, bottom - top + 1)
            .also { source.recycle() }
    }

    /**
     * The part of [source] the reader kept: the largest rectangle of [aspect] that fits, narrowed by
     * [zoom], placed by [biasX]/[biasY] — 0..1 across the room left over.
     *
     * Cropping on the way in rather than remembering where to look later means everything that reads
     * the file afterwards just draws a picture.
     */
    private fun crop(source: Bitmap, aspect: Float, biasX: Float, biasY: Float, zoom: Float): Bitmap {
        val width = source.width.toFloat()
        val height = source.height.toFloat()
        val base = if (width / height > aspect) height * aspect to height else width to width / aspect
        val croppedWidth = (base.first / zoom).coerceAtLeast(1f)
        val croppedHeight = (base.second / zoom).coerceAtLeast(1f)
        val left = ((width - croppedWidth) * biasX).coerceIn(0f, width - croppedWidth)
        val top = ((height - croppedHeight) * biasY).coerceIn(0f, height - croppedHeight)
        return Bitmap.createBitmap(
            source,
            left.toInt(),
            top.toInt(),
            croppedWidth.toInt(),
            croppedHeight.toInt(),
        )
    }

    /** Copies [uri] in, shrunk and cropped, over whatever [target] holds. */
    private fun copyIn(
        context: Context,
        uri: Uri,
        target: File,
        aspect: Float,
        biasX: Float,
        biasY: Float,
        zoom: Float,
    ): Boolean = runCatching {
        val source = decode(context, uri, MAX_EDGE) ?: return false
        val cropped = crop(source, aspect, biasX, biasY, zoom)
        target.outputStream().use { cropped.compress(Bitmap.CompressFormat.JPEG, QUALITY, it) }
        if (cropped !== source) cropped.recycle()
        source.recycle()
        true
    }.getOrDefault(false)

    fun save(
        context: Context,
        uri: Uri,
        aspect: Float,
        biasX: Float,
        biasY: Float,
        zoom: Float,
    ): Boolean = copyIn(context, uri, file(context), aspect, biasX, biasY, zoom)
        .also { if (it) version++ }

    fun saveStatusCard(
        context: Context,
        uri: Uri,
        aspect: Float,
        biasX: Float,
        biasY: Float,
        zoom: Float,
    ): Boolean =
        copyIn(context, uri, statusFile(context), aspect, biasX, biasY, zoom)
            .also { if (it) version++ }

    fun clearStatusCard(context: Context) {
        statusFile(context).delete()
        version++
    }

    fun clear(context: Context) {
        file(context).delete()
        version++
    }
}

/** The appearance setting that picks or clears the picture [HomeWallpaperStore] serves. */
@Composable
fun HomeWallpaperPreference() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // The picked picture goes through the crop dialog first: what the file keeps is what the dialog
    // showed, so the choice is made once and never revisited while drawing.
    var pending by remember { mutableStateOf<Uri?>(null) }
    var pendingStatusCard by remember { mutableStateOf(false) }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
        if (uri != null) {
            pendingStatusCard = false
            pending = uri
        }
    }

    ArrowPreference(
        title = stringResource(id = R.string.home_wallpaper),
        summary = stringResource(id = R.string.home_wallpaper_summary),
        startAction = {},
        onClick = {
            picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
        },
    )
    // The status card gets a picture of its own: it is the one card the panel's opacity leaves
    // solid, so a picture there has to be chosen on purpose.
    val statusPicker = rememberLauncherForActivityResult(
        ActivityResultContracts.PickVisualMedia()
    ) { uri ->
        if (uri != null) {
            pendingStatusCard = true
            pending = uri
        }
    }
    ArrowPreference(
        title = stringResource(id = R.string.home_status_wallpaper),
        startAction = {},
        onClick = {
            statusPicker.launch(
                PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)
            )
        },
    )
    // One row for both pictures: they are one setting as far as the reader is concerned.
    ArrowPreference(
        title = stringResource(id = R.string.home_wallpaper_clear),
        startAction = {},
        onClick = {
            HomeWallpaperStore.clear(context)
            HomeWallpaperStore.clearStatusCard(context)
        },
    )
    val open = pending
    if (open != null) {
        val statusCard = pendingStatusCard
        val aspect = if (statusCard) {
            HomeWallpaperStore.STATUS_ASPECT
        } else {
            HomeWallpaperStore.WALLPAPER_ASPECT
        }
        CropDialog(
            uri = open,
            aspect = aspect,
            onDismiss = { pending = null },
            onConfirm = { biasX, biasY, zoom ->
                pending = null
                scope.launch {
                    withContext(Dispatchers.IO) {
                        if (statusCard) {
                            HomeWallpaperStore.saveStatusCard(
                                context, open, aspect, biasX, biasY, zoom,
                            )
                        } else {
                            HomeWallpaperStore.save(context, open, aspect, biasX, biasY, zoom)
                        }
                    }
                }
            },
        )
    }
}

/**
 * Where in the picture each slot's rectangle comes from.
 *
 * Drag to move it, pinch to tighten it. The preview shows the crop the file will get, so what is on
 * screen when the reader confirms is what the app shows afterwards.
 */
@Composable
private fun CropDialog(
    uri: Uri,
    aspect: Float,
    onDismiss: () -> Unit,
    onConfirm: (Float, Float, Float) -> Unit,
) {
    val context = LocalContext.current
    val preview = remember(uri) {
        HomeWallpaperStore.decode(context, uri, 1080)?.asImageBitmap()
    }
    var biasX by remember { mutableFloatStateOf(0.5f) }
    var biasY by remember { mutableFloatStateOf(0.5f) }
    var zoom by remember { mutableFloatStateOf(1f) }

    OverlayDialog(
        show = true,
        title = stringResource(id = R.string.home_crop_title),
        onDismissRequest = onDismiss,
        content = {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text(
                    text = stringResource(id = R.string.home_crop_hint),
                    fontSize = MiuixTheme.textStyles.body2.fontSize,
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    modifier = Modifier.padding(bottom = 10.dp),
                )
                // Centred on the dialog rather than left to the column's own alignment: a narrow slot
                // (the status card is 2.5 to 1) came out against the start edge.
                Box(
                    modifier = Modifier.fillMaxWidth(),
                    contentAlignment = Alignment.Center,
                ) {
                    Box(
                        modifier = Modifier
                            // Sized to stay inside a dialog whichever way the slot is shaped: a 2.5-wide
                            // card and a 0.55-tall backdrop cannot share one fixed pair of numbers.
                            .size(
                                width = minOf(260.dp, 300.dp * aspect),
                                height = minOf(300.dp, 260.dp / aspect),
                            )
                            .clip(RoundedCornerShape(16.dp))
                            .pointerInput(Unit) {
                                detectTransformGestures { _, pan, pinch, _ ->
                                    zoom = (zoom * pinch).coerceIn(1f, 3f)
                                    biasX = (biasX - pan.x / size.width / zoom).coerceIn(0f, 1f)
                                    biasY = (biasY - pan.y / size.height / zoom).coerceIn(0f, 1f)
                                }
                            },
                    ) {
                        val bitmap = preview
                        if (bitmap != null) {
                            Image(
                                bitmap = bitmap,
                                contentDescription = null,
                                contentScale = ContentScale.Crop,
                                alignment = BiasAlignment(biasX * 2f - 1f, biasY * 2f - 1f),
                                modifier = Modifier
                                    .matchParentSize()
                                    .graphicsLayer {
                                        scaleX = zoom
                                        scaleY = zoom
                                    },
                            )
                        }
                    }
                }
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 16.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    TextButton(
                        text = stringResource(android.R.string.cancel),
                        onClick = onDismiss,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(
                        text = stringResource(android.R.string.ok),
                        onClick = { onConfirm(biasX, biasY, zoom) },
                        modifier = Modifier.weight(1f),
                    )
                }
            }
        },
    )
}
