package me.weishu.kernelsu.ui

import androidx.compose.runtime.staticCompositionLocalOf

enum class UiMode(val value: String, val displayName: String) {
    Miuix("miuix", "miuix(美化版)"),
    MiuixStock("miuix_stock", "miuix(原版)"),
    Material("material", "Material 3");

    companion object {
        fun fromValue(value: String): UiMode = when (value) {
            Material.value -> Material
            MiuixStock.value -> MiuixStock
            else -> Miuix
        }

        val DEFAULT_VALUE = Miuix.value
    }
}

val LocalUiMode = staticCompositionLocalOf { UiMode.Miuix }

/** True for both miuix variants: structural miuix code (scaffolds, components) applies to both. */
val UiMode.isMiuixFamily: Boolean
    get() = this == UiMode.Miuix || this == UiMode.MiuixStock
