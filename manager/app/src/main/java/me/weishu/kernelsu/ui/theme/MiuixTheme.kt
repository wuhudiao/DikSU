package me.weishu.kernelsu.ui.theme

import android.app.Activity
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.platform.LocalContext
import androidx.core.view.WindowInsetsControllerCompat
import com.materialkolor.dynamiccolor.ColorSpec
import me.weishu.kernelsu.ui.util.rememberWallpaperSet
import me.weishu.kernelsu.ui.webui.MonetColorsProvider
import top.yukonga.miuix.kmp.theme.ColorSchemeMode
import top.yukonga.miuix.kmp.theme.LocalContentColor
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.ThemeColorSpec
import top.yukonga.miuix.kmp.theme.ThemeController
import top.yukonga.miuix.kmp.theme.ThemePaletteStyle

@Composable
fun MiuixKernelSUTheme(
    appSettings: AppSettings,
    content: @Composable () -> Unit
) {
    val context = LocalContext.current
    val systemDarkTheme = isSystemInDarkTheme()
    val hasPicture = rememberWallpaperSet()
    val darkTheme = appSettings.colorMode.isDark || (appSettings.colorMode.isSystem && systemDarkTheme)
    val colorStyle = appSettings.paletteStyle
    val colorSpec = appSettings.colorSpec

    val miuixPaletteStyle = try {
        ThemePaletteStyle.valueOf(colorStyle.name)
    } catch (_: Exception) {
        ThemePaletteStyle.TonalSpot
    }

    val miuixColorSpec = if (colorSpec.effectiveFor(colorStyle) == ColorSpec.SpecVersion.SPEC_2025) {
        ThemeColorSpec.Spec2025
    } else {
        ThemeColorSpec.Spec2021
    }

    val resolvedKeyColor: Color? = when {
        appSettings.keyColor != 0 -> Color(appSettings.keyColor)
        appSettings.colorMode.isMonet ->
            if (darkTheme) dynamicDarkColorScheme(context).primary
            else dynamicLightColorScheme(context).primary

        else -> null
    }

    val controller = ThemeController(
        when (appSettings.colorMode) {
            ColorMode.SYSTEM -> ColorSchemeMode.System
            ColorMode.LIGHT -> ColorSchemeMode.Light
            ColorMode.DARK -> ColorSchemeMode.Dark
            ColorMode.MONET_SYSTEM -> ColorSchemeMode.MonetSystem
            ColorMode.MONET_LIGHT -> ColorSchemeMode.MonetLight
            ColorMode.MONET_DARK, ColorMode.DARK_AMOLED -> ColorSchemeMode.MonetDark
        },
        keyColor = resolvedKeyColor,
        isDark = darkTheme,
        paletteStyle = miuixPaletteStyle,
        colorSpec = miuixColorSpec,
    )

    MiuixTheme(
        controller = controller,
        content = {
            LaunchedEffect(darkTheme) {
                val window = (context as? Activity)?.window ?: return@LaunchedEffect
                WindowInsetsControllerCompat(window, window.decorView).apply {
                    isAppearanceLightStatusBars = !darkTheme
                    isAppearanceLightNavigationBars = !darkTheme
                }
            }
            MonetColorsProvider.UpdateCss()
            // The picture decides how the text reads, and it does so for everything the app draws —
            // a page, a pushed screen, a dialog. With one the app is the dark one, where the greys
            // the theme hands out are too dim: the text climbs to the bright tint the home's own
            // text uses, white with the key colour still in it. With no picture the app is the light
            // one, and there the text is plain black rather than the theme's own brown-black. Only
            // the text colours move; a component that paints its own container keeps the colour
            // meant for it, so the bright ones stay bright.
            val base = MiuixTheme.colorScheme
            val litText = lerp(base.primary, Color.White, 0.65f)
            val litSummary = lerp(base.primary, Color.White, 0.8f)
            val textColors = if (hasPicture) {
                base.copy(
                    onSurface = lerp(base.onSurface, litText, 0.5f),
                    onBackground = lerp(base.onBackground, litText, 0.5f),
                    onSurfaceVariantSummary = lerp(base.onSurfaceVariantSummary, litSummary, 0.6f),
                )
            } else {
                base.copy(
                    onSurface = Color.Black,
                    onBackground = Color.Black,
                    onSurfaceVariantSummary = Color.Black,
                )
            }
            MiuixTheme(colors = textColors, textStyles = MiuixTheme.textStyles) {
                CompositionLocalProvider(
                    LocalContentColor provides MiuixTheme.colorScheme.onBackground,
                ) {
                    content()
                }
            }
        }
    )
}
