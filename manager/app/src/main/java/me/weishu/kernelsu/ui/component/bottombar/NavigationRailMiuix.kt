package me.weishu.kernelsu.ui.component.bottombar

import androidx.compose.foundation.LocalIndication
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
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import java.time.LocalDateTime
import kotlinx.coroutines.delay
import androidx.compose.ui.Alignment
import androidx.compose.ui.BiasAlignment
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import me.weishu.kernelsu.Natives
import me.weishu.kernelsu.ui.component.rebootlistpopup.RebootListPopupMiuix
import me.weishu.kernelsu.ui.LocalMainPagerState
import top.yukonga.miuix.kmp.utils.MiuixIndication
import top.yukonga.miuix.kmp.basic.NavigationRail
import top.yukonga.miuix.kmp.basic.NavigationRailDefaults
import top.yukonga.miuix.kmp.basic.NavigationRailItem
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
fun NavigationRailMiuix(
    navigationBadge: NavigationBadgeState,
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
        // over a picture that light block is exactly the box that appears the instant a destination
        // is tapped. Transparent, the tap still works and nothing is drawn on top of the rail.
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
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .onSizeChanged { itemsHeight = with(density) { it.height.toDp() } },
                ) {
                    icons.forEachIndexed { index, icon ->
                        NavigationRailItem(
                            selected = mainState.selectedPage == index,
                            onClick = {
                                mainState.animateToPage(index)
                            },
                            icon = icon,
                            // The rail is icons only; the destinations are named by the expanded state.
                            label = "",
                            badge = navigationBadgeFor(index, navigationBadge),
                        )
                    }
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
 * The time above the destinations: the hour over the minute, the date under both.
 *
 * It turns over on the minute, which is the only unit worth showing here — this is the strip people
 * look at while deciding what to open, not a stopwatch.
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
