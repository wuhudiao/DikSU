package me.weishu.kernelsu.ui.util

import android.graphics.Matrix
import android.graphics.SurfaceTexture
import android.media.MediaPlayer
import android.view.Surface
import android.view.TextureView
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner

/**
 * A looping, muted video painted as the page's backdrop. TextureView (not SurfaceView) so the
 * frame composes into the layout — the backdrop's blur and dimming layers apply to it the same
 * way they do to the still picture. Scaling is centre-crop: whatever the aspect, the frame fills
 * the box and the edges fall outside the clip.
 */
@Composable
fun VideoBackdrop(path: String, modifier: Modifier = Modifier) {
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val player = remember(path) { MediaPlayer() }
    val prepared = remember(path) { mutableStateOf(false) }
    val videoSize = remember(path) { mutableStateOf<Pair<Int, Int>?>(null) }
    // Set by the factory below; read from the prepare callbacks.
    val viewRef = remember(path) { mutableStateOf<TextureView?>(null) }
    val crop = me.weishu.kernelsu.ui.util.HomeWallpaperStore.videoCropState.value
    var lastLogged by remember { androidx.compose.runtime.mutableStateOf<Any?>(null) }

    fun applyCoverTransform(view: TextureView) {
        val vw = view.width.toFloat()
        val vh = view.height.toFloat()
        val size = videoSize.value ?: return
        val sw = size.first
        val sh = size.second
        if (vw <= 0f || vh <= 0f || sw <= 0 || sh <= 0) return
        val crop = me.weishu.kernelsu.ui.util.HomeWallpaperStore.videoCropState.value
        // TextureView.setTransform lives in VIEW space (identity == stretch-to-fill), so the
        // matrix scales by content/view — not by raw video pixels, which is what made the frame
        // render at the wrong zoom before.
        val cover = maxOf(vw / sw, vh / sh)
        val contentW = sw * cover * crop.zoom
        val contentH = sh * cover * crop.zoom
        val dx = (vw - contentW) * crop.biasX
        val dy = (vh - contentH) * crop.biasY
        val matrix = Matrix()
        matrix.setScale(contentW / vw, contentH / vh)
        matrix.postTranslate(dx, dy)
        view.setTransform(matrix)
        if (lastLogged != (vw to vh to sw to sh)) {
            lastLogged = vw to vh to sw to sh
            android.util.Log.i(
                "VideoBackdrop",
                "cover view=${vw}x${vh} video=${sw}x${sh} content=${contentW}x${contentH} " +
                    "dx=$dx dy=$dy crop=${crop.biasX},${crop.biasY},z=${crop.zoom}",
            )
        }
    }

    LaunchedEffect(path, crop) {
        viewRef.value?.let { v ->
            android.os.Handler(android.os.Looper.getMainLooper()).post { applyCoverTransform(v) }
        }
    }

    LaunchedEffect(path) {
        player.setOnPreparedListener { mp ->
            prepared.value = true
            videoSize.value = mp.videoWidth to mp.videoHeight
            viewRef.value?.let { applyCoverTransform(it) }
            runCatching { mp.start() }
        }
        player.setOnVideoSizeChangedListener { _, w, h ->
            if (w > 0 && h > 0) {
                videoSize.value = w to h
                viewRef.value?.let { applyCoverTransform(it) }
            }
        }
        runCatching {
            player.setDataSource(path)
            player.isLooping = true
            player.setVolume(0f, 0f)
            player.prepareAsync()
        }
    }

    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> {
                    if (prepared.value) runCatching { player.start() }
                }
                Lifecycle.Event.ON_STOP -> {
                    runCatching { player.pause() }
                }
                else -> Unit
            }
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            runCatching { player.release() }
        }
    }

    AndroidView(
        modifier = modifier,
        factory = { context ->
            TextureView(context).also { view ->
                viewRef.value = view
                view.addOnLayoutChangeListener { v, _, _, _, _, _, _, _, _ ->
                    if (v is TextureView) applyCoverTransform(v)
                }
                view.surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                    override fun onSurfaceTextureAvailable(st: SurfaceTexture, w: Int, h: Int) {
                        runCatching { player.setSurface(Surface(st)) }
                        if (prepared.value) runCatching { player.start() }
                        applyCoverTransform(view)
                    }

                    override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, w: Int, h: Int) {
                        applyCoverTransform(view)
                    }

                    override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
                        runCatching { player.setSurface(null) }
                        return true
                    }

                    override fun onSurfaceTextureUpdated(st: SurfaceTexture) = Unit
                }
            }
        },
    )
}
