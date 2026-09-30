package me.weishu.kernelsu.ui

import android.annotation.SuppressLint
import android.content.Intent
import android.os.Bundle
import android.os.SystemClock
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.PagerDefaults.flingBehavior
import androidx.compose.foundation.pager.PagerDefaults.pageNestedScrollConnection
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntSize
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigationevent.NavigationEventInfo
import androidx.navigationevent.compose.NavigationBackHandler
import androidx.navigationevent.compose.rememberNavigationEventState
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.withContext
import me.weishu.kernelsu.Natives
import me.weishu.kernelsu.ui.component.bottombar.BottomBar
import me.weishu.kernelsu.ui.component.bottombar.MainPagerState
import me.weishu.kernelsu.ui.component.bottombar.NavigationBadgeState
import me.weishu.kernelsu.ui.component.bottombar.SideRail
import me.weishu.kernelsu.ui.component.bottombar.rememberMainPagerState
import me.weishu.kernelsu.ui.component.bottombar.useNavigationRail
import me.weishu.kernelsu.ui.navigation3.IntentDispatcher
import me.weishu.kernelsu.ui.navigation3.LocalNavigator
import me.weishu.kernelsu.ui.navigation3.Navigator
import me.weishu.kernelsu.ui.navigation3.Route
import me.weishu.kernelsu.ui.navigation3.rememberNavigator
import me.weishu.kernelsu.ui.screen.about.AboutScreen
import me.weishu.kernelsu.ui.screen.appprofile.AppProfileScreen
import me.weishu.kernelsu.ui.screen.colorpalette.ColorPaletteScreen
import me.weishu.kernelsu.ui.screen.executemoduleaction.ExecuteModuleActionScreen
import me.weishu.kernelsu.ui.screen.flash.FlashScreen
import me.weishu.kernelsu.ui.screen.home.HomePager
import me.weishu.kernelsu.ui.screen.install.InstallScreen
import me.weishu.kernelsu.ui.screen.module.ModulePager
import me.weishu.kernelsu.ui.screen.modulerepo.ModuleRepoDetailScreen
import me.weishu.kernelsu.ui.screen.modulerepo.ModuleRepoScreen
import me.weishu.kernelsu.ui.screen.settings.SettingPager
import me.weishu.kernelsu.ui.screen.settings.BasicSettingsScreen
import me.weishu.kernelsu.ui.screen.settings.KeymintScreen
import me.weishu.kernelsu.ui.screen.settings.OtherFeaturesScreen
import me.weishu.kernelsu.ui.screen.sulog.SulogScreen
import me.weishu.kernelsu.ui.screen.superuser.SuperUserPager
import me.weishu.kernelsu.ui.screen.template.AppProfileTemplateScreen
import me.weishu.kernelsu.ui.screen.templateeditor.TemplateEditorScreen
import me.weishu.kernelsu.ui.theme.KernelSUTheme
import me.weishu.kernelsu.ui.theme.LocalColorMode
import me.weishu.kernelsu.ui.theme.LocalEnableBlur
import me.weishu.kernelsu.ui.theme.LocalEnableFloatingBottomBar
import me.weishu.kernelsu.ui.theme.LocalEnableFloatingBottomBarBlur
import me.weishu.kernelsu.ui.theme.LocalEnableNavigationBadge
import me.weishu.kernelsu.ui.theme.LocalModuleDescriptionMaxLines
import me.weishu.kernelsu.ui.theme.withWallpaperMode
import me.weishu.kernelsu.ui.util.getSuperuserCount
import me.weishu.kernelsu.ui.util.install
import me.weishu.kernelsu.ui.util.ManagerHider
import me.weishu.kernelsu.ui.util.rememberBlurBackdrop
import me.weishu.kernelsu.ui.screen.home.GlassNudge
import me.weishu.kernelsu.ui.util.rememberContentReady
import me.weishu.kernelsu.ui.viewmodel.MainActivityViewModel
import me.weishu.kernelsu.ui.viewmodel.MainPagerConfig
import me.weishu.kernelsu.ui.viewmodel.ModuleViewModel
import me.weishu.kernelsu.ui.viewmodel.SuperUserViewModel
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.blur.rememberLayerBackdrop
import top.yukonga.miuix.kmp.nav.core.NavDisplay
import top.yukonga.miuix.kmp.nav.core.NavDisplayEffects
import top.yukonga.miuix.kmp.nav.core.rememberNavSystemCornerRadius
import top.yukonga.miuix.kmp.nav.transition.NavSwipeDirection
import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.width
import androidx.compose.ui.draw.blur
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.text.TextStyle
import top.yukonga.miuix.kmp.theme.TextStyles
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.asAndroidPath
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalContext
import me.weishu.kernelsu.ui.util.HomeWallpaperStore
import me.weishu.kernelsu.ui.util.rememberWallpaperSet
import me.weishu.kernelsu.ui.util.VideoBackdrop
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.unit.dp
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.utils.PagerGestureNestedScrollConnection
import top.yukonga.miuix.kmp.utils.PagerInterceptionMode
import top.yukonga.miuix.kmp.utils.PagerNavigationSpringSpec
import top.yukonga.miuix.kmp.utils.pagerGestureOverride

open class MainActivity : ComponentActivity() {

    private val intentChannel = Channel<Intent>(capacity = Channel.BUFFERED)
    private var contentReady = false
    private var splashStartedAt = 0L
    private val splashAnimationDurationMs = 500L


    @SuppressLint("UnusedMaterial3ScaffoldPaddingParameter")
    override fun onCreate(savedInstanceState: Bundle?) {
        val splashScreen = installSplashScreen()
        splashStartedAt = SystemClock.uptimeMillis()
        super.onCreate(savedInstanceState)
        GlassNudge.load(this)
        HomeWallpaperStore.loadVideoCrop(this)
        BackgroundDim.load(this)
        splashScreen.setKeepOnScreenCondition {
            !contentReady || SystemClock.uptimeMillis() - splashStartedAt < splashAnimationDurationMs
        }

        val isManager = Natives.isManager
        if (isManager && Natives.kernelUAPIVersion == Natives.managerUAPIVersion) install()

        // The hide flow grants its own copy root so that copy can finish the job. Once this app IS
        // the manager that grant is redundant, so drop it rather than leave an entry behind in the
        // Superuser list.
        if (isManager) {
            val self = packageName
            lifecycleScope.launch(Dispatchers.IO) { ManagerHider.revokeTemporaryGrant(self) }
        }

        // A renamed copy is launched with the rest of the hide to do: drop the package that still
        // holds the manager seat, then reinstall itself, which is the package event the kernel
        // searches on. It needs the root the old install granted it before launching this.
        if (savedInstanceState == null) {
            intent?.getStringExtra(ManagerHider.EXTRA_HIDDEN_FROM)?.let { previous ->
                val staged = intent?.getStringExtra(ManagerHider.EXTRA_HIDDEN_APK).orEmpty()
                if (previous.isNotEmpty() && staged.isNotEmpty()) {
                    lifecycleScope.launch(Dispatchers.IO) {
                        val original = intent?.getStringExtra(ManagerHider.EXTRA_HIDDEN_ORIGINAL)
                        ManagerHider.finishHide(this@MainActivity, previous, staged, original)
                    }
                }
            }
        }

        if (savedInstanceState == null) intent?.let { intentChannel.trySend(it) }

        setContent {
            val viewModel = viewModel<MainActivityViewModel>()
            val uiState by viewModel.uiState.collectAsStateWithLifecycle()
            val selectedMainPage by viewModel.selectedMainPage.collectAsStateWithLifecycle()
            val appSettings = uiState.appSettings
            val uiMode = uiState.uiMode
            // The picture is the mode: no picture is the light app, a picture is the dark one.
            val hasWallpaper = rememberWallpaperSet()
            val themeSettings = if (uiMode == UiMode.Miuix) appSettings.withWallpaperMode(hasWallpaper) else appSettings
            val darkMode = themeSettings.colorMode.isDark || (themeSettings.colorMode.isSystem && isSystemInDarkTheme())

            DisposableEffect(darkMode) {
                enableEdgeToEdge(
                    statusBarStyle = SystemBarStyle.auto(
                        android.graphics.Color.TRANSPARENT,
                        android.graphics.Color.TRANSPARENT
                    ) { darkMode },
                    navigationBarStyle = SystemBarStyle.auto(
                        android.graphics.Color.TRANSPARENT,
                        android.graphics.Color.TRANSPARENT
                    ) { darkMode },
                )
                window.isNavigationBarContrastEnforced = false
                onDispose { }
            }

            val navigator = rememberNavigator(Route.Main)
            val systemDensity = LocalDensity.current
            val density = remember(systemDensity, uiState.pageScale) {
                Density(systemDensity.density * uiState.pageScale, systemDensity.fontScale)
            }

            CompositionLocalProvider(
                LocalNavigator provides navigator,
                LocalDensity provides density,
                LocalColorMode provides themeSettings.colorMode.value,
                LocalEnableBlur provides uiState.enableBlur,
                LocalEnableFloatingBottomBar provides uiState.enableFloatingBottomBar,
                LocalEnableFloatingBottomBarBlur provides uiState.enableFloatingBottomBarBlur,
                LocalEnableNavigationBadge provides uiState.enableNavigationBadge,
                LocalModuleDescriptionMaxLines provides uiState.moduleDescriptionMaxLines,
                LocalUiMode provides uiMode,
            ) {
                KernelSUTheme(appSettings = themeSettings, uiMode = uiMode) {
                    IntentDispatcher(intentChannel = intentChannel)
                    val swipeDismiss = if (uiState.enableSwipeDismiss) {
                        if (LocalLayoutDirection.current == androidx.compose.ui.unit.LayoutDirection.Rtl) {
                            NavSwipeDirection.RightToLeft
                        } else {
                            NavSwipeDirection.LeftToRight
                        }
                    } else {
                        NavSwipeDirection.None
                    }
                    val mainScreenEntry = @Composable {
                        MainScreen(
                            initialPage = selectedMainPage,
                            pagerInterceptionMode = uiState.pagerInterceptionMode,
                            onPageChanged = viewModel::setSelectedMainPage,
                        )
                    }

                    val navDisplay = @Composable {
                        NavDisplay(
                            backStack = navigator.backStack,
                            effects = NavDisplayEffects(cornerClipRadius = rememberNavSystemCornerRadius()),
                            onBack = {
                                when (val top = navigator.current()) {
                                    is Route.TemplateEditor -> {
                                        if (!top.readOnly) {
                                            navigator.setResult("template_edit", true)
                                        } else {
                                            navigator.pop()
                                        }
                                    }

                                    else -> navigator.pop()
                                }
                            }) {
                            entry<Route.Main>(swipeDismiss = swipeDismiss) { mainScreenEntry() }
                            entry<Route.About>(swipeDismiss = swipeDismiss) { AboutScreen() }
                            entry<Route.Sulog>(swipeDismiss = swipeDismiss) { SulogScreen() }
                            entry<Route.ColorPalette>(swipeDismiss = swipeDismiss) { ColorPaletteScreen() }
                            entry<Route.AppProfileTemplate>(swipeDismiss = swipeDismiss) { AppProfileTemplateScreen() }
                            entry<Route.TemplateEditor>(swipeDismiss = swipeDismiss) { key -> TemplateEditorScreen(key.template, key.readOnly) }
                            entry<Route.AppProfile>(swipeDismiss = swipeDismiss) { key -> AppProfileScreen(key.uid) }
                            entry<Route.ModuleRepo>(swipeDismiss = swipeDismiss) { ModuleRepoScreen() }
                            entry<Route.ModuleRepoDetail>(swipeDismiss = swipeDismiss) { key -> ModuleRepoDetailScreen(key.module) }
                            entry<Route.Install>(swipeDismiss = swipeDismiss) { InstallScreen() }
                            entry<Route.Flash>(swipeDismiss = swipeDismiss) { key -> FlashScreen(key.flashIt) }
                            entry<Route.ExecuteModuleAction>(swipeDismiss = swipeDismiss) { key ->
                                ExecuteModuleActionScreen(
                                    key.moduleId,
                                    key.fromShortcut
                                )
                            }
                            entry<Route.Home>(swipeDismiss = swipeDismiss) { mainScreenEntry() }
                            entry<Route.SuperUser>(swipeDismiss = swipeDismiss) { mainScreenEntry() }
                            entry<Route.Module>(swipeDismiss = swipeDismiss) { mainScreenEntry() }
                            entry<Route.Settings>(swipeDismiss = swipeDismiss) { mainScreenEntry() }
                            entry<Route.BasicSettings>(swipeDismiss = swipeDismiss) { BasicSettingsScreen() }
                            entry<Route.OtherFeatures>(swipeDismiss = swipeDismiss) { OtherFeaturesScreen() }
                            entry<Route.Keymint>(swipeDismiss = swipeDismiss) { KeymintScreen() }
                        }
                    }

                    when (uiMode) {
                        UiMode.Material -> androidx.compose.material3.Scaffold(
                            containerColor = MaterialTheme.colorScheme.surfaceContainer
                        ) { navDisplay() }

                        UiMode.Miuix, UiMode.MiuixStock -> Scaffold { navDisplay() }
                    }
                    SideEffect { contentReady = true }
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        intentChannel.trySend(intent)
    }
}

val LocalMainPagerState = staticCompositionLocalOf<MainPagerState> { error("LocalMainPagerState not provided") }

/**
 * Backdrop dimming for the picture/video behind the pages, tuned live from the theme settings
 * slider. One value drives every background sheet — outer frame and panel — so they never drift.
 */
object BackgroundDim {
    private const val PREF = "background_dim"
    val value = mutableFloatStateOf(0.35f)

    fun load(context: android.content.Context) {
        val sp = context.getSharedPreferences(PREF, android.content.Context.MODE_PRIVATE)
        value.floatValue = sp.getFloat("dim", 0.35f)
    }

    fun save(context: android.content.Context, dim: Float) {
        value.floatValue = dim
        context.getSharedPreferences(PREF, android.content.Context.MODE_PRIVATE).edit()
            .putFloat("dim", dim)
            .apply()
    }
}

/**
 * The page panel's real pixel size and window position. The frosted card crops the wallpaper with
 * exactly these numbers so its picture is the same crop the panel shows; residual error is dialled
 * out by the GlassNudge constants in HomeMiuix.
 */
object PanelMetrics {
    val size = mutableStateOf(IntSize.Zero)
    val pos = mutableStateOf(Offset.Zero)
}

/** The page panel's edge shading: how wide the band is, and how dark it starts out. */
private val EDGE_SHADE = 12.dp
private val EDGE_SHADOW = Color.Black.copy(alpha = 0.12f)
private val PANEL_CORNER = 24.dp

@SuppressLint("UnusedMaterial3ScaffoldPaddingParameter")
@Composable
fun MainScreen(
    initialPage: Int = 0,
    pagerInterceptionMode: Int = PagerInterceptionMode.CrossAxisInterceptor.ordinal,
    onPageChanged: (Int) -> Unit = {},
) {
    val navController = LocalNavigator.current
    val enableBlur = LocalEnableBlur.current
    val enableFloatingBottomBar = LocalEnableFloatingBottomBar.current
    val enableFloatingBottomBarBlur = LocalEnableFloatingBottomBarBlur.current
    val useNavigationRail = useNavigationRail(enableFloatingBottomBar)
    val pagerState = rememberPagerState(initialPage = initialPage, pageCount = { MainPagerConfig.PAGE_COUNT })
    val mainPagerState = rememberMainPagerState(
        pagerState = pagerState,
        animatePageChanges = !useNavigationRail,
    )
    val isFullFeatured = Natives.isFullFeatured()
    val pagerMode = PagerInterceptionMode.entries.getOrElse(pagerInterceptionMode) {
        PagerInterceptionMode.Native
    }
    val interceptPagerGestures = pagerMode == PagerInterceptionMode.CrossAxisInterceptor
    var userScrollEnabled by remember(isFullFeatured) { mutableStateOf(isFullFeatured) }

    val enableNavigationBadge = LocalEnableNavigationBadge.current
    val badgeEnabled = enableNavigationBadge && isFullFeatured
    val moduleViewModel = viewModel<ModuleViewModel>()
    val moduleUiState by moduleViewModel.uiState.collectAsStateWithLifecycle()

    val superUserViewModel = viewModel<SuperUserViewModel>()
    val grantedUidCount by remember(superUserViewModel) {
        superUserViewModel.uiState
            .map { state -> state.groupedApps.count { it.anyAllowSu } }
            .distinctUntilChanged()
    }.collectAsStateWithLifecycle(0)

    var startupPreloadStarted by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(isFullFeatured) {
        if (!isFullFeatured || startupPreloadStarted) {
            return@LaunchedEffect
        }

        moduleViewModel.initializePreferences()
        val moduleState = moduleViewModel.uiState.value
        if (!moduleState.hasLoaded) {
            if (!moduleState.isRefreshing) moduleViewModel.fetchModuleList()
            moduleViewModel.uiState.first { it.hasLoaded }
        }
        moduleViewModel.syncModuleUpdateInfo(moduleViewModel.uiState.value.modules)

        val superUserState = superUserViewModel.uiState.value
        if (!superUserState.hasLoaded) {
            superUserViewModel.initializePreferences()
            if (superUserState.isRefreshing) {
                superUserViewModel.uiState.first { it.hasLoaded }
            } else {
                superUserViewModel.loadAppList().join()
            }
        }

        startupPreloadStarted = true
    }

    // Loading the app list just for a badge is too expensive; read the kernel allowlist instead.
    var superuserCount by remember { mutableIntStateOf(0) }
    LaunchedEffect(badgeEnabled, grantedUidCount) {
        superuserCount = if (badgeEnabled) withContext(Dispatchers.IO) { getSuperuserCount() } else 0
    }

    val navigationBadge = if (badgeEnabled) {
        NavigationBadgeState(
            superuserCount = superuserCount,
            moduleEnabledCount = moduleUiState.modules.count { it.enabled },
            moduleUpdatableCount = moduleUiState.updateInfo.count { it.value.downloadUrl.isNotBlank() },
        )
    } else {
        NavigationBadgeState()
    }
    val uiMode = LocalUiMode.current
    val surfaceColor = when (uiMode) {
        UiMode.Material -> MaterialTheme.colorScheme.surface // Blur is not used in Material, this is just a placeholder
        UiMode.Miuix, UiMode.MiuixStock -> MiuixTheme.colorScheme.surface
    }
    val blurBackdrop = rememberBlurBackdrop(enableBlur)

    val backdrop = rememberLayerBackdrop {
        drawRect(surfaceColor)
        drawContent()
    }

    val settledPage = mainPagerState.pagerState.settledPage
    LaunchedEffect(settledPage) {
        onPageChanged(settledPage)
    }

    val currentPage = mainPagerState.pagerState.currentPage
    LaunchedEffect(currentPage) {
        mainPagerState.syncPage()
    }

    MainScreenBackHandler(mainPagerState, navController)

    CompositionLocalProvider(
        LocalMainPagerState provides mainPagerState
    ) {
        val contentReady = rememberContentReady()
        val pagerContent = @Composable { bottomInnerPadding: Dp ->
            Box(modifier = if (blurBackdrop != null) Modifier.layerBackdrop(blurBackdrop) else Modifier) {
                HorizontalPager(
                    modifier = Modifier
                        .pagerGestureOverride(
                            pagerState = mainPagerState.pagerState,
                            mode = pagerMode,
                            enabled = userScrollEnabled,
                        )
                        .then(if (enableFloatingBottomBar && enableFloatingBottomBarBlur) Modifier.layerBackdrop(backdrop) else Modifier),
                    state = mainPagerState.pagerState,
                    beyondViewportPageCount = if (contentReady) 3 else 0,
                    overscrollEffect = null,
                    userScrollEnabled = userScrollEnabled && !interceptPagerGestures,
                    pageNestedScrollConnection = if (interceptPagerGestures) {
                        PagerGestureNestedScrollConnection
                    } else {
                        pageNestedScrollConnection(
                            state = mainPagerState.pagerState,
                            orientation = androidx.compose.foundation.gestures.Orientation.Horizontal,
                        )
                    },
                    flingBehavior = flingBehavior(
                        state = mainPagerState.pagerState,
                        snapAnimationSpec = PagerNavigationSpringSpec,
                    ),
                ) { page ->
                    val isCurrentPage = page == settledPage
                    when (page) {
                        0 -> if (contentReady || isCurrentPage) HomePager(navController, bottomInnerPadding, isCurrentPage)
                        1 -> if (contentReady || isCurrentPage) SuperUserPager(navController, bottomInnerPadding, isCurrentPage)
                        2 -> if (contentReady || isCurrentPage) ModulePager(bottomInnerPadding, isCurrentPage)
                        3 -> if (contentReady || isCurrentPage) SettingPager(navController, bottomInnerPadding, isCurrentPage)
                    }
                }
            }
        }

        if (useNavigationRail) {
            val startInsets = WindowInsets.systemBars.union(WindowInsets.displayCutout)
                .only(WindowInsetsSides.Start)
            val navBarBottomPadding = WindowInsets.systemBars.asPaddingValues().calculateBottomPadding()
            val panelInsets = WindowInsets.safeDrawing.only(WindowInsetsSides.Vertical).asPaddingValues()

            when (uiMode) {
                UiMode.Material -> androidx.compose.material3.Scaffold(
                    containerColor = MaterialTheme.colorScheme.surfaceContainer
                ) {
                    Row {
                        SideRail(navigationBadge)
                        Box(
                            modifier = Modifier
                                .weight(1f)
                                .consumeWindowInsets(startInsets)
                        ) {
                            pagerContent(navBarBottomPadding)
                        }
                    }
                }

                UiMode.MiuixStock -> Scaffold { _ ->
                    Row {
                        SideRail(navigationBadge)
                        Box(
                            modifier = Modifier
                                .weight(1f)
                                .consumeWindowInsets(startInsets)
                        ) {
                            pagerContent(navBarBottomPadding)
                        }
                    }
                }
                UiMode.Miuix -> Scaffold { _ ->
                    // The home's backdrop, re-read whenever the picture is replaced.
                    val wallpaperContext = LocalContext.current
                    val wallpaperVersion = HomeWallpaperStore.version
                    val videoPath = remember(wallpaperVersion) {
                        HomeWallpaperStore.videoFile(wallpaperContext).absolutePath.takeIf {
                            HomeWallpaperStore.videoFile(wallpaperContext).exists()
                        }
                    }
                    val homeWallpaper = remember(wallpaperContext, wallpaperVersion) {
                        runCatching {
                            val file = HomeWallpaperStore.file(wallpaperContext)
                            if (!file.exists()) return@runCatching null
                            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                            BitmapFactory.decodeFile(file.absolutePath, bounds)
                            var sample = 1
                            while (bounds.outWidth / sample > 1080) sample *= 2
                            BitmapFactory.decodeFile(
                                file.absolutePath,
                                BitmapFactory.Options().apply { inSampleSize = sample },
                            )?.asImageBitmap()
                        }.getOrNull()
                    }
                    // The rail's pill refracts this frame, so the frame needs a layer of its own:
                    // the pager's backdrop stops at the panel, which is exactly where the rail is not.
                    // This rail glass is independent from the app-wide blur preference.
                    val railBackdrop = rememberLayerBackdrop {
                        drawRect(surfaceColor)
                        drawContent()
                    }
                    // The frame around the page, built the way Aster builds its scene: a backdrop
                    // that is blurred and darkened, a plate under the rail so the icons keep their
                    // contrast, and the page panel on top of all of it — blurred outside, sharp in.
                    Box(
                        modifier = Modifier
                            .fillMaxSize()
                    ) {
                        Box(
                            modifier = Modifier
                                .fillMaxSize()
                                .layerBackdrop(railBackdrop),
                        ) {
                            Box(
                                modifier = Modifier.fillMaxSize().background(
                                    Brush.verticalGradient(
                                        listOf(
                                            MiuixTheme.colorScheme.primaryContainer,
                                            MiuixTheme.colorScheme.secondaryContainer,
                                            MiuixTheme.colorScheme.background,
                                        )
                                    )
                                )
                            )
                            if (homeWallpaper != null) {
                                Image(
                                    bitmap = homeWallpaper,
                                    contentDescription = null,
                                    contentScale = ContentScale.Crop,
                                    // Lighter than before: 32dp blurred a photograph into a smear rather
                                    // than a backdrop.
                                    modifier = Modifier.fillMaxSize().blur(12.dp),
                                )
                                Box(
                                    modifier = Modifier.fillMaxSize()
                                        .background(Color.Black.copy(alpha = BackgroundDim.value.floatValue))
                                )
                            }
                            Box(
                                modifier = Modifier
                                    .fillMaxHeight()
                                    .width(84.dp)
                                    .background(
                                        Brush.horizontalGradient(
                                            listOf(Color.Black.copy(alpha = 0.25f), Color.Transparent)
                                        )
                                    )
                            )
                        }
                    Row {
                        SideRail(navigationBadge, railBackdrop)
                        // The page rides in a rounded panel inset from the top, the end and the
                        // bottom, so the rail's divider reads as the frame the app is wrapped in.
                        Box(
                            modifier = Modifier
                                .weight(1f)
                                .consumeWindowInsets(startInsets)
                                // Nothing the page draws may spill past the frame: a shadow that
                                // escaped below the panel read as a second card poking out.
                                .clipToBounds()
                                // Consumed in full so the pages still see no status bar of their
                                // own; the padding below is what is left once the line is raised.
                                .consumeWindowInsets(WindowInsets.safeDrawing.only(WindowInsetsSides.Vertical)),
                        ) {
                            // Rounded on the rail side only: the panel runs to the end of the screen,
                            // so the two corners out there would be a curve against the bezel for no
                            // reason — the page owns that area all the way over.
                            val pagePanelShape = RoundedCornerShape(
                                topStart = PANEL_CORNER,
                                topEnd = 0.dp,
                                bottomEnd = 0.dp,
                                bottomStart = PANEL_CORNER,
                            )
                            Box(
                                modifier = Modifier
                                    .fillMaxSize()
                                    // The top line sits just under the status-bar icons: left at the
                                    // raw inset it stood 165px off the top against 74px at the bottom.
                                    .padding(
                                        top = panelInsets.calculateTopPadding() + 8.dp - 20.dp,
                                        bottom = panelInsets.calculateBottomPadding() + 8.dp,
                                    )
                                    .clip(pagePanelShape)
                                    .border(
                                        width = 1.dp,
                                        color = MiuixTheme.colorScheme.dividerLine,
                                        shape = pagePanelShape,
                                    )
                                    .onGloballyPositioned { coords ->
                                        PanelMetrics.size.value = coords.size
                                        PanelMetrics.pos.value = coords.positionInWindow()
                                    },
                            ) {
                                // The same picture inside the line, sharp: the blur belongs to the
                                // frame outside it, and the page sits on the untouched photograph.
                                if (videoPath != null) {
                                    // Sharp inside the panel, like the still: the blur belongs to
                                    // the outer frame, not the page the reader looks at.
                                    VideoBackdrop(
                                        path = videoPath,
                                        modifier = Modifier.fillMaxSize(),
                                    )
                                    Box(
                                        modifier = Modifier.fillMaxSize().background(
                                            Color.Black.copy(alpha = BackgroundDim.value.floatValue)
                                        )
                                    )
                                } else if (homeWallpaper != null) {
                                    Image(
                                        bitmap = homeWallpaper,
                                        contentDescription = null,
                                        contentScale = ContentScale.Crop,
                                        modifier = Modifier.fillMaxSize(),
                                    )
                                    // Dimmed a little: light text needs something to sit on, and the
                                    // picture is still the picture at this much.
                                    Box(
                                        modifier = Modifier.fillMaxSize().background(
                                            Color.Black.copy(alpha = BackgroundDim.value.floatValue)
                                        )
                                    )
                                }
                                // One scheme for the whole page instead of a colour at every card:
                                // the Miuix surfaces are what the reader sees, so handing them the
                                // panel's opacity makes every list, row and card follow the slider.
                                val baseColors = MiuixTheme.colorScheme
                                val panelFill = HomeWallpaperStore.PANEL_FILL
                                val translucentColors = baseColors.copy(
                                    surface = baseColors.surfaceContainer.copy(
                                        alpha = panelFill
                                    ),
                                    surfaceContainer = baseColors.surfaceContainer.copy(
                                        alpha = panelFill
                                    ),
                                    surfaceContainerHigh = baseColors.surfaceContainerHigh.copy(
                                        alpha = panelFill
                                    ),
                                )
                                MiuixTheme(
                                    colors = translucentColors,
                                    textStyles = run {
                                        val styles = MiuixTheme.textStyles
                                        val halo = Shadow(
                                            color = Color.Black.copy(alpha = 0.95f),
                                            offset = Offset.Zero,
                                            blurRadius = 1.5f,
                                        )
                                        // The halo is off: an outline around every glyph read worse
                                        // than the glare it was meant to fix.
                                        fun TextStyle.outlined() = this
                                        TextStyles(
                                            styles.main.outlined(),
                                            styles.paragraph.outlined(),
                                            styles.body1.outlined(),
                                            styles.body2.outlined(),
                                            styles.button.outlined(),
                                            styles.footnote1.outlined(),
                                            styles.footnote2.outlined(),
                                            styles.headline1.outlined(),
                                            styles.headline2.outlined(),
                                            styles.subtitle.outlined(),
                                            styles.title1.outlined(),
                                            styles.title2.outlined(),
                                            styles.title3.outlined(),
                                            styles.title4.outlined(),
                                        )
                                    },
                                ) {
                                    pagerContent(0.dp)
                                }
                                // Depth painted straight onto the canvas: one open path down the top,
                                // around the two rounded corners and along the bottom — the right edge
                                // is simply not part of it — stroked with a real blur and clipped to
                                // the panel, so the shading is soft, even, and never leaves the card.
                                Box(
                                    modifier = Modifier
                                        .matchParentSize()
                                        .drawWithContent {
                                            val radius = PANEL_CORNER.toPx()
                                            val shadowPath = Path().apply {
                                                moveTo(size.width, 0f)
                                                lineTo(radius, 0f)
                                                quadraticTo(0f, 0f, 0f, radius)
                                                lineTo(0f, size.height - radius)
                                                quadraticTo(0f, size.height, radius, size.height)
                                                lineTo(size.width, size.height)
                                            }
                                            val paint = android.graphics.Paint().apply {
                                                isAntiAlias = true
                                                style = android.graphics.Paint.Style.STROKE
                                                // One shade wide, not two: a stroke of twice the
                                                // shade with a blur inside it dimmed the whole panel
                                                // instead of just its edge.
                                                strokeWidth = EDGE_SHADE.toPx()
                                                color = android.graphics.Color.BLACK
                                                // Heavier over a photograph: at 0.12 the edge is
                                                // simply not there on a bright picture.
                                                alpha = (
                                                    if (homeWallpaper != null) 0.18f else EDGE_SHADOW.alpha
                                                ).times(255).toInt()
                                                maskFilter = android.graphics.BlurMaskFilter(
                                                    EDGE_SHADE.toPx() * 0.7f,
                                                    android.graphics.BlurMaskFilter.Blur.NORMAL,
                                                )
                                            }
                                            clipPath(shadowPath) {
                                                drawIntoCanvas { canvas ->
                                                    canvas.nativeCanvas.drawPath(
                                                        shadowPath.asAndroidPath(),
                                                        paint,
                                                    )
                                                }
                                            }
                                            drawContent()
                                        },
                                )
                            }
                        }
                    }
                    }
                }
            }
        } else {
            val bottomBar = @Composable {
                Box(
                    modifier = Modifier.fillMaxWidth()
                ) {
                    BottomBar(
                        blurBackdrop = blurBackdrop,
                        backdrop = backdrop,
                        navigationBadge = navigationBadge,
                        modifier = Modifier.align(Alignment.BottomCenter),
                    )
                }
            }

            when (uiMode) {
                UiMode.Material -> androidx.compose.material3.Scaffold(
                    bottomBar = bottomBar,
                    containerColor = MaterialTheme.colorScheme.surfaceContainer
                ) { innerPadding ->
                    pagerContent(innerPadding.calculateBottomPadding())
                }

                UiMode.Miuix, UiMode.MiuixStock -> Scaffold(bottomBar = bottomBar) { innerPadding ->
                    pagerContent(innerPadding.calculateBottomPadding())
                }
            }
        }
    }
}


@Composable
private fun MainScreenBackHandler(
    mainState: MainPagerState,
    navController: Navigator,
) {
    val isPagerBackHandlerEnabled by remember {
        derivedStateOf {
            navController.current() is Route.Main && navController.backStackSize() == 1 && mainState.selectedPage != 0
        }
    }

    val navEventState = rememberNavigationEventState(NavigationEventInfo.None)

    NavigationBackHandler(
        state = navEventState,
        isBackEnabled = isPagerBackHandlerEnabled,
        onBackCompleted = {
            mainState.animateToPage(0)
        }
    )
}
