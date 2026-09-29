package me.weishu.kernelsu.ui.component.bottombar

import androidx.compose.foundation.Image
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.selection.selectable
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.platform.ViewConfiguration
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import androidx.compose.ui.util.fastCoerceIn
import java.time.LocalDateTime
import kotlinx.coroutines.delay
import androidx.compose.ui.Alignment
import androidx.compose.ui.BiasAlignment
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import me.weishu.kernelsu.Natives
import me.weishu.kernelsu.ui.component.rebootlistpopup.RebootListPopupMiuix
import me.weishu.kernelsu.ui.LocalMainPagerState
import me.weishu.kernelsu.ui.component.iosIndicatorSpecular
import me.weishu.kernelsu.ui.component.liquid.InnerShadow
import me.weishu.kernelsu.ui.component.liquid.innerShadow
import me.weishu.kernelsu.ui.component.liquid.lens
import me.weishu.kernelsu.ui.component.liquid.vibrancy
import me.weishu.kernelsu.ui.component.miuix.animation.DampedDragAnimation
import me.weishu.kernelsu.ui.component.rememberGravityRotatedHighlight
import me.weishu.kernelsu.ui.theme.isInDarkTheme
import top.yukonga.miuix.kmp.utils.MiuixIndication
import top.yukonga.miuix.kmp.basic.BadgedBox
import top.yukonga.miuix.kmp.basic.NavigationRail
import top.yukonga.miuix.kmp.basic.NavigationRailDefaults
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.blur.LayerBackdrop
import top.yukonga.miuix.kmp.blur.blur
import top.yukonga.miuix.kmp.blur.drawBackdrop
import top.yukonga.miuix.kmp.blur.isRuntimeShaderSupported
import top.yukonga.miuix.kmp.shader.isRenderEffectSupported
import top.yukonga.miuix.kmp.theme.MiuixTheme
import kotlin.math.roundToInt

@Composable
fun NavigationRailMiuix(
    navigationBadge: NavigationBadgeState,
    backdrop: LayerBackdrop? = null,
    modifier: Modifier = Modifier,
) {
    val fullFeatured = Natives.isFullFeatured()
    if (!fullFeatured) return

    val mainState = LocalMainPagerState.current

    val icons = BottomBarDestination.entries.map { it.icon }

    // The destinations belong at the bottom of the column: a rail that starts at the top leaves the
    // lower half of the screen empty and parks the last destination — settings — in the middle of
    // nowhere.
    //
    // The rail's own column is Arrangement.Top and scrolls, so there is no slot to align with and a
    // weight would be measured against an unbounded height. What is left is a spacer of exactly the
    // leftover height, and the leftover is knowable: the column's height, minus the insets it pads
    // itself with, its vertical padding, the expand toggle, and the items — the items measured,
    // because their height follows the label font.
    val insetsPadding = WindowInsets.statusBars.union(WindowInsets.navigationBars).asPaddingValues()
    val overhead = NavigationRailDefaults.VerticalPadding * 2 +
        insetsPadding.calculateTopPadding() +
        insetsPadding.calculateBottomPadding()
    val density = LocalDensity.current
    var itemsHeight by remember { mutableStateOf(0.dp) }

    BoxWithConstraints(modifier = modifier.fillMaxHeight()) {
        val spacer: Dp = (maxHeight - overhead - itemsHeight).coerceAtLeast(0.dp)

        // A tapped destination is fed back with a block filled in the theme's `onBackground` colour —
        // over a picture that light block is exactly the box that appears the instant a destination is
        // tapped. Transparent, the tap still works and nothing is drawn on top of the rail.
        val noPressBlock = remember {
            MiuixIndication(color = Color.Transparent)
        }
        CompositionLocalProvider(LocalIndication provides noPressBlock) {
            NavigationRail(
                // Fixed width, and therefore no expand/collapse button: the library draws that button
                // itself whenever the rail is given a state, and this one is icons in a single column.
                expanded = false,
                // The frame around the page is painted once, by the screen: the rail only draws what is
                // on top of it, or the layer underneath would show through twice.
                color = Color.Transparent,
                minWidth = 64.dp,
                // No straight divider of its own: the page panel's rounded border takes that role and
                // wraps the content instead of running past it at both ends.
                showDivider = false,
            ) {
                Spacer(modifier = Modifier.height(spacer))
                val systemViewConfiguration = LocalViewConfiguration.current
                val railViewConfiguration = remember(systemViewConfiguration) {
                    object : ViewConfiguration {
                        override val longPressTimeoutMillis = 300L
                        override val doubleTapTimeoutMillis =
                            systemViewConfiguration.doubleTapTimeoutMillis
                        override val doubleTapMinTimeMillis =
                            systemViewConfiguration.doubleTapMinTimeMillis
                        override val touchSlop = systemViewConfiguration.touchSlop
                    }
                }
                CompositionLocalProvider(LocalViewConfiguration provides railViewConfiguration) {
                    RailItems(
                        icons = icons,
                        selectedIndex = mainState.selectedPage,
                        onSelect = { mainState.animateToPage(it) },
                        badgeFor = { index -> navigationBadgeFor(index, navigationBadge) },
                        backdrop = backdrop,
                        onItemsHeight = { itemsHeight = it },
                    )
                }
            }
        }

        // The reboot control belongs to the app, not to the page, and this strip is where the app's
        // own furniture lives — it also fills the stretch above the destinations that used to be
        // the expand button's.
        Box(
            modifier = Modifier
                .align(Alignment.TopCenter)
                .windowInsetsPadding(WindowInsets.statusBars.only(WindowInsetsSides.Top))
                .fillMaxHeight(),
        ) {
            Box(modifier = Modifier.align(Alignment.TopCenter).padding(top = 8.dp)) {
                // Darker than the default: over a blurred photo the icon has to hold its own.
                RebootListPopupMiuix(iconTint = MiuixTheme.colorScheme.onSurface)
            }
            SceneClock(modifier = Modifier.align(BiasAlignment(0f, -0.35f)))
        }
    }
}

/**
 * The destinations and the selection pill that slides between them.
 *
 * The pill is drawn once for the whole group instead of once per item so it can travel continuously
 * under a held finger; the items carry only their icons. A hold grows the pill ([DampedDragAnimation]
 * presses it), the drag walks it across the items, and the release snaps it to the destination under
 * the finger — a plain tap still selects, animated by the same spring.
 */
@Composable
private fun RailItems(
    icons: List<ImageVector>,
    selectedIndex: Int,
    onSelect: (Int) -> Unit,
    badgeFor: (Int) -> (@Composable () -> Unit)?,
    backdrop: LayerBackdrop?,
    onItemsHeight: (Dp) -> Unit,
) {
    val count = icons.size
    val currentSelectedIndex by rememberUpdatedState(selectedIndex)
    val animationScope = rememberCoroutineScope()
    val dampedDrag = remember(animationScope, count) {
        DampedDragAnimation(
            animationScope = animationScope,
            initialValue = selectedIndex.toFloat(),
            valueRange = 0f..(count - 1).toFloat(),
            visibilityThreshold = 0.001f,
            initialScale = 1f,
            // Same growth the floating bar grows to: past a fifth again, the pill reads as picked up.
            pressedScale = 78f / 56f,
            canDrag = { true },
            // Driven by the gestures below rather than by the class's own modifier.
            onDragStarted = {},
            onDragStopped = {},
            onDragCancelled = {},
            onDrag = { _, _ -> },
        )
    }

    // A tap, a swipe between pages, or a released drag all land here: glide the pill to the page
    // that is now showing, with the press-and-release flourish on the way.
    LaunchedEffect(selectedIndex) {
        if (dampedDrag.value.roundToInt() != selectedIndex) {
            dampedDrag.animateToValue(selectedIndex.toFloat())
        }
    }

    val density = LocalDensity.current
    var itemHeightPx by remember { mutableStateOf(0f) }
    val iconCenterOffsetPx = with(density) {
        (
            NavigationRailDefaults.ItemVerticalPadding +
                NavigationRailDefaults.CollapsedIndicatorVerticalPadding +
                11.dp
            ).toPx()
    }
    val pillShape = remember { androidx.compose.foundation.shape.RoundedCornerShape(16.dp) }
    // The lens shader needs API 33; below it the pill falls back to the solid block it replaced.
    val glass = backdrop != null && isRenderEffectSupported() && isRuntimeShaderSupported()

    Box(
        modifier = Modifier
            .fillMaxWidth()
            .onSizeChanged { size ->
                itemHeightPx = size.height.toFloat() / count
                onItemsHeight(with(density) { size.height.toDp() })
            }
            // Keep the gesture alive while selection changes; restarting it on selectedIndex
            // would cancel the active drag during the release callback.
            .pointerInput(count) {
                var dragPositionY = 0f
                var dragTargetIndex = currentSelectedIndex
                detectDragGesturesAfterLongPress(
                    onDragStart = { position ->
                        dragPositionY = position.y
                        dampedDrag.press()
                        if (itemHeightPx > 0f) {
                            dragTargetIndex =
                                (position.y / itemHeightPx).roundToInt().coerceIn(0, count - 1)
                            dampedDrag.updateValue(
                                dragTargetIndex.toFloat(),
                            )
                        }
                    },
                    onDrag = { change, _ ->
                        change.consume()
                        dragPositionY = change.position.y
                        if (itemHeightPx > 0f) {
                            dragTargetIndex =
                                (dragPositionY / itemHeightPx).roundToInt().coerceIn(0, count - 1)
                            dampedDrag.updateValue(
                                (dragPositionY / itemHeightPx).coerceIn(0f, (count - 1).toFloat()),
                            )
                        }
                    },
                    onDragEnd = {
                        val target = dragTargetIndex.coerceIn(0, count - 1)
                        dampedDrag.updateValue(target.toFloat())
                        dampedDrag.release()
                        onSelect(target)
                    },
                    onDragCancel = {
                        dampedDrag.updateValue(currentSelectedIndex.toFloat())
                        dampedDrag.release()
                    },
                )
            },
    ) {
        // First in the box, so the icons draw over it: the pill sits behind, as the per-item block
        // it replaced did.
        SelectionPill(
            dampedDrag = dampedDrag,
            itemHeightPx = itemHeightPx,
            iconCenterOffsetPx = iconCenterOffsetPx,
            glass = glass,
            backdrop = backdrop,
            shape = pillShape,
            modifier = Modifier.align(Alignment.TopCenter),
        )
        Column(modifier = Modifier.fillMaxWidth()) {
            icons.forEachIndexed { index, icon ->
                RailItem(
                    selected = index == selectedIndex,
                    onClick = { onSelect(index) },
                    icon = icon,
                    badge = badgeFor(index),
                )
            }
        }
    }
}

/** One destination: icon over an empty label, sized so every item in the column is the same height. */
@Composable
private fun RailItem(
    selected: Boolean,
    onClick: () -> Unit,
    icon: ImageVector,
    badge: (@Composable () -> Unit)?,
) {
    val tint = MiuixTheme.colorScheme.onSurfaceContainer
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .selectable(
                selected = selected,
                onClick = onClick,
                role = Role.Tab,
            )
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(vertical = NavigationRailDefaults.ItemVerticalPadding),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Box(
                modifier = Modifier.padding(
                    horizontal = NavigationRailDefaults.ExpandedItemContentHorizontalPadding,
                    vertical = NavigationRailDefaults.CollapsedIndicatorVerticalPadding,
                ),
            ) {
                val iconContent: @Composable () -> Unit = {
                    Image(
                        // A shade under the library's 24dp: four icons in a column read heavy at full size.
                        modifier = Modifier.size(22.dp),
                        imageVector = icon,
                        // Decorative: the adjacent label already names the item; avoids TalkBack double-read.
                        contentDescription = null,
                        colorFilter = ColorFilter.tint(tint),
                    )
                }
                if (badge != null) {
                    BadgedBox(badge = { badge() }) { iconContent() }
                } else {
                    iconContent()
                }
            }
            Spacer(modifier = Modifier.height(NavigationRailDefaults.IconTextSpacing))
            Text(
                text = "",
                color = tint,
                fontSize = NavigationRailDefaults.LabelFontSize,
                fontWeight = FontWeight.Medium,
            )
        }
    }
}

/**
 * The liquid-glass selection pill: one backdrop-refracting block that rides the held finger between
 * destinations, growing while the hold lasts and easing back once the destination is picked.
 */
@Composable
private fun SelectionPill(
    dampedDrag: DampedDragAnimation,
    itemHeightPx: Float,
    iconCenterOffsetPx: Float,
    glass: Boolean,
    backdrop: LayerBackdrop?,
    shape: androidx.compose.foundation.shape.RoundedCornerShape,
    modifier: Modifier = Modifier,
) {
    val isInDark = isInDarkTheme()
    val pillTint = MiuixTheme.colorScheme.surfaceContainerHigh
    val specular = rememberGravityRotatedHighlight(iosIndicatorSpecular, extraDegrees = 90f)

    Box(
        modifier = modifier
            // Hidden until the items have been measured, so the pill never flashes at rest.
            .alpha(if (itemHeightPx > 0f) 1f else 0f)
            .then(
                if (itemHeightPx > 0f) {
                    Modifier.graphicsLayer {
                        translationY =
                            dampedDrag.value * itemHeightPx + iconCenterOffsetPx - size.height / 2f
                    }
                } else {
                    Modifier
                },
            )
            .size(width = 52.dp, height = 32.dp)
            .dropShadow(
                shape = shape,
                shadow = Shadow(
                    radius = 8.dp,
                    color = Color.Black,
                    alpha = if (isInDark) 0.2f else 0.1f,
                ),
            )
            .then(
                if (glass && backdrop != null) {
                    Modifier
                        .drawBackdrop(
                            backdrop = backdrop,
                            shape = { shape },
                            effects = {
                                val progress = dampedDrag.pressProgress
                                vibrancy()
                                blur(4.dp.toPx(), 4.dp.toPx())
                                lens(
                                    // A shallow bend at rest so the block reads as glass before it is
                                    // ever touched; the hold deepens it the way the floating bar does.
                                    refractionHeight = 3.dp.toPx() + 7.dp.toPx() * progress,
                                    refractionAmount = 3.dp.toPx() + 11.dp.toPx() * progress,
                                    depthEffect = true,
                                    chromaticAberration = 0.5f,
                                )
                            },
                            highlight = {
                                specular.value.copy(alpha = 0.3f + 0.7f * dampedDrag.pressProgress)
                            },
                            layerBlock = {
                                scaleX = dampedDrag.scaleX
                                scaleY = dampedDrag.scaleY
                                // Squash across the travel and stretch along it: the pill leans the
                                // way it is going instead of sliding rigid.
                                val velocity = dampedDrag.velocity / 10f
                                scaleY /= 1f - (velocity * 0.75f).fastCoerceIn(-0.2f, 0.2f)
                                scaleX *= 1f - (velocity * 0.25f).fastCoerceIn(-0.2f, 0.2f)
                            },
                            onDrawSurface = {
                                drawRect(pillTint.copy(alpha = 0.55f))
                                val progress = dampedDrag.pressProgress
                                drawRect(
                                    color = if (!isInDark) Color.Black.copy(alpha = 0.1f)
                                    else Color.White.copy(alpha = 0.1f),
                                    alpha = 1f - progress,
                                )
                                drawRect(Color.Black.copy(alpha = 0.03f * progress))
                            },
                        )
                        .innerShadow(shape = shape) {
                            InnerShadow(
                                radius = 8.dp * dampedDrag.pressProgress,
                                color = Color.Black.copy(alpha = 0.15f),
                                alpha = dampedDrag.pressProgress,
                            )
                        }
                } else {
                    Modifier.graphicsLayer {
                        scaleX = dampedDrag.scaleX
                        scaleY = dampedDrag.scaleY
                        val velocity = dampedDrag.velocity / 10f
                        scaleY /= 1f - (velocity * 0.75f).fastCoerceIn(-0.2f, 0.2f)
                        scaleX *= 1f - (velocity * 0.25f).fastCoerceIn(-0.2f, 0.2f)
                    }.background(pillTint, shape)
                },
            ),
    )
}

/**
 * The time above the destinations: the hour over the minute, the date under both.
 *
 * It turns over on the minute, which is the only unit worth showing here — not a stopwatch.
 */
@Composable
private fun SceneClock(modifier: Modifier = Modifier) {
    var now by remember { mutableStateOf(LocalDateTime.now()) }
    LaunchedEffect(Unit) {
        while (true) {
            now = LocalDateTime.now()
            delay(60_000L - System.currentTimeMillis() % 60_000L)
        }
    }

    Column(modifier = modifier, horizontalAlignment = Alignment.CenterHorizontally) {
        Text(
            text = "%02d".format(now.hour),
            fontSize = 30.sp,
            fontWeight = FontWeight.Medium,
            color = MiuixTheme.colorScheme.onBackground,
        )
        Text(
            text = "%02d".format(now.minute),
            fontSize = 30.sp,
            fontWeight = FontWeight.Medium,
            color = MiuixTheme.colorScheme.onBackground,
        )
        Text(
            text = "%d/%d".format(now.monthValue, now.dayOfMonth),
            fontSize = MiuixTheme.textStyles.body2.fontSize,
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            modifier = Modifier.padding(top = 2.dp),
        )
    }
}
