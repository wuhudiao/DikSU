package me.weishu.kernelsu.ui.util

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.MediaMetadataRetriever
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
    return remember(context, version) {
        HomeWallpaperStore.file(context).exists() ||
            HomeWallpaperStore.videoFile(context).exists()
    }
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
    private const val VIDEO_NAME = "home-wallpaper.mp4"
    /** A backdrop clip above this size is refused; phones handle far less gracefully. */
    private const val MAX_VIDEO_BYTES = 200L * 1024 * 1024
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

    fun videoFile(context: Context): File = File(context.filesDir, VIDEO_NAME)

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
        .also {
            if (it) {
                videoFile(context).delete()
                version++
            }
        }

    /**
     * A looping clip as the backdrop. Copied as-is — no crop dialog: the player centre-crops at
     * draw time. One backdrop at a time, so setting a video drops the still (and with it the
     * frosted card's sample source, which falls back to the theme gradient while a video plays).
     */
    /**
     * The MP4 a motion photo (live photo) hides after its JPEG frames: the picture is a normal
     * JPEG whose tail carries one full MP4 box — [size][ftyp]... — so a live photo saved through
     * the picker as image/jpeg can still become a moving backdrop.
     */
    private fun extractMotionMp4(jpeg: ByteArray): ByteArray? {
        if (jpeg.size < 32) return null
        // Last "ftyp" in the file: the embedded video's type box (video boxes sit in the tail).
        var pos = -1
        var i = jpeg.size - 8
        while (i >= 0) {
            if (jpeg[i + 4] == 'f'.code.toByte() && jpeg[i + 5] == 't'.code.toByte() &&
                jpeg[i + 6] == 'y'.code.toByte() && jpeg[i + 7] == 'p'.code.toByte()
            ) {
                pos = i
                break
            }
            i--
        }
        if (pos < 4) return null
        val boxStart = pos - 4
        // The video lives in the picture's tail; anything in the first half is a false hit.
        if (boxStart < jpeg.size / 2) return null
        val size = ((jpeg[boxStart].toInt() and 0xff) shl 24) or
            ((jpeg[boxStart + 1].toInt() and 0xff) shl 16) or
            ((jpeg[boxStart + 2].toInt() and 0xff) shl 8) or
            (jpeg[boxStart + 3].toInt() and 0xff)
        val end = if (size in 16..jpeg.size - boxStart) boxStart + size else jpeg.size
        return jpeg.copyOfRange(boxStart, end)
    }

    private const val VIDEO_POS = "video_pos"

    /** Live crop of the video backdrop: bias 0.5 is centred, zoom 1 is plain centre-crop. */
    data class VideoCrop(val biasX: Float, val biasY: Float, val zoom: Float)

    /** Read by the player every transform; written by the crop dialog. */
    var videoCropState = androidx.compose.runtime.mutableStateOf(VideoCrop(0.5f, 0.5f, 1f))

    fun loadVideoCrop(context: Context) {
        val sp = context.getSharedPreferences(VIDEO_POS, Context.MODE_PRIVATE)
        videoCropState.value = VideoCrop(
            sp.getFloat("x", 0.5f),
            sp.getFloat("y", 0.5f),
            sp.getFloat("zoom", 1f),
        )
    }

    fun saveVideoCrop(context: Context, biasX: Float, biasY: Float, zoom: Float) {
        context.getSharedPreferences(VIDEO_POS, Context.MODE_PRIVATE).edit()
            .putFloat("x", biasX)
            .putFloat("y", biasY)
            .putFloat("zoom", zoom)
            .apply()
        videoCropState.value = VideoCrop(biasX, biasY, zoom)
    }

    /**
     * The moving backdrop needs a still twin: the outer frame blurs it once (fixed, never
     * re-sampled) and the frosted card samples it, so rail and card stay in step with a single
     * frozen frame while the panel plays the video sharp.
     */
    private fun writeStillRepresentative(context: Context, source: android.graphics.Bitmap): Boolean =
        runCatching {
            val framed = crop(source, WALLPAPER_ASPECT, 0.5f, 0.5f, 1f)
            file(context).outputStream().use { out ->
                framed.compress(Bitmap.CompressFormat.JPEG, 95, out)
            }
            if (framed !== source) framed.recycle()
            android.util.Log.i("HomeWallpaper", "still representative written ${file(context).length()} bytes")
            true
        }.getOrDefault(false)

    /** The clip's first frame as a temp JPEG — the crop dialog previews that, like a still. */
    fun videoFirstFrame(context: Context, uri: Uri): Uri? = runCatching {
        val mmr = MediaMetadataRetriever()
        mmr.setDataSource(context, uri)
        val frame = mmr.getFrameAtTime(0)
        mmr.release()
        frame ?: return null
        val out = java.io.File(context.cacheDir, "video-frame.jpg")
        out.outputStream().use { frame.compress(android.graphics.Bitmap.CompressFormat.JPEG, 92, it) }
        if (!frame.isRecycled) frame.recycle()
        Uri.fromFile(out)
    }.getOrNull()

    fun saveVideo(context: Context, uri: Uri): Boolean {
        val tag = "HomeWallpaper"
        return try {
            val resolver = context.contentResolver
            val mime = runCatching { resolver.getType(uri) }.getOrNull()
            android.util.Log.i(tag, "saveVideo: uri=$uri mime=$mime")
            val out = videoFile(context)
            val isStill = mime?.startsWith("image/") == true
            if (isStill) {
                // Motion photo: read the picture, pull its embedded clip, save that.
                val jpeg = resolver.openInputStream(uri)?.use { input ->
                    val bos = java.io.ByteArrayOutputStream()
                    val buffer = ByteArray(64 * 1024)
                    var total = 0L
                    while (true) {
                        val read = input.read(buffer)
                        if (read < 0) break
                        total += read
                        if (total > MAX_VIDEO_BYTES) throw IllegalStateException("still too large")
                        bos.write(buffer, 0, read)
                    }
                    bos.toByteArray()
                } ?: throw IllegalStateException("cannot open still")
                val mp4 = extractMotionMp4(jpeg)
                if (mp4 == null) {
                    android.util.Log.w(tag, "saveVideo: no embedded mp4 in motion photo")
                    return false
                }
                out.outputStream().use { it.write(mp4) }
                android.util.Log.i(tag, "saveVideo: extracted motion mp4 ${mp4.size} bytes")
            } else {
                // Plain video: stream it, cap as we go (statSize is unreliable on picker uris).
                var total = 0L
                val copied = resolver.openInputStream(uri)?.use { input ->
                    out.outputStream().use { output ->
                        val buffer = ByteArray(64 * 1024)
                        while (true) {
                            val read = input.read(buffer)
                            if (read < 0) break
                            total += read
                            if (total > MAX_VIDEO_BYTES) {
                                throw IllegalStateException("video too large")
                            }
                            output.write(buffer, 0, read)
                        }
                        true
                    }
                } ?: false
                if (!copied) throw IllegalStateException("empty stream")
                android.util.Log.i(tag, "saveVideo: streamed $total bytes")
            }
            // Frozen twin for the outer frame and the frosted card (both sample stills).
            val stillOk = if (isStill) {
                val decoded = runCatching {
                    out.inputStream().use { BitmapFactory.decodeStream(it) }
                }.getOrNull()
                decoded?.let { writeStillRepresentative(context, it) } ?: false
            } else {
                val frameUri = videoFirstFrame(context, uri)
                val decoded = frameUri?.let {
                    runCatching {
                        context.contentResolver.openInputStream(it)?.use { s ->
                            BitmapFactory.decodeStream(s)
                        }
                    }.getOrNull()
                }
                decoded?.let { writeStillRepresentative(context, it) } ?: false
            }
            if (!stillOk) {
                android.util.Log.w(tag, "saveVideo: still representative failed (video kept)")
            }
            version++
            android.util.Log.i(tag, "saveVideo: ok still=$stillOk")
            true
        } catch (e: Exception) {
            android.util.Log.e(tag, "saveVideo failed", e)
            runCatching { videoFile(context).delete() }
            false
        }
    }

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
        videoFile(context).delete()
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
    var pendingVideoCrop by remember { mutableStateOf(false) }

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

    if (me.weishu.kernelsu.ui.LocalUiMode.current == me.weishu.kernelsu.ui.UiMode.Miuix) {
        // Polished build only: the moving backdrop and its crop flow.
        val videoPicker = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
            if (uri != null) {
                scope.launch {
                    val saved = withContext(Dispatchers.IO) {
                        HomeWallpaperStore.saveVideo(context, uri)
                    }
                    if (saved) {
                        val frame = withContext(Dispatchers.IO) {
                            HomeWallpaperStore.videoFirstFrame(context, uri)
                        }
                        if (frame != null) {
                            pendingVideoCrop = true
                            pending = frame
                        }
                    }
                }
            }
        }
        ArrowPreference(
            title = "选择视频背景",
            summary = "首页背景循环播放所选视频(静音),与静态壁纸互斥",
            startAction = {},
            onClick = {
                videoPicker.launch(
                    PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageAndVideo)
                )
            },
        )
    }
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
        val videoCrop = pendingVideoCrop
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
                        if (videoCrop) {
                            HomeWallpaperStore.saveVideoCrop(context, biasX, biasY, zoom)
                        } else if (statusCard) {
                            HomeWallpaperStore.saveStatusCard(
                                context, open, aspect, biasX, biasY, zoom,
                            )
                        } else {
                            HomeWallpaperStore.save(
                                context, open, aspect, biasX, biasY, zoom,
                            )
                        }
                    }
                }
                pendingVideoCrop = false
                pendingStatusCard = false
                // The crop preview frame is a temp file; drop it either way.
                runCatching {
                    if (videoCrop) {
                        val path = open.path
                        if (path != null && path.contains("video-frame.jpg")) {
                            java.io.File(path).delete()
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
