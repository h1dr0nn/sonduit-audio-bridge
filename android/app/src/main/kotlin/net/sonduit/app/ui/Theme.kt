package net.sonduit.app.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * The same palette the desktop shell uses.
 *
 * Values are copied from `desktop/src/index.css` rather than approximated, so
 * a phone next to the laptop looks like the same product. Dynamic colour is
 * deliberately not used: it would make the two disagree on every device.
 */
private val Accent = Color(0xFF7C93E8)

private val LightScheme = lightColorScheme(
    primary = Accent,
    onPrimary = Color.White,
    background = Color(0xFFF7F6F3),
    onBackground = Color(0xFF17171A),
    surface = Color(0xFFFFFFFF),
    onSurface = Color(0xFF17171A),
    surfaceVariant = Color(0xFFF1F0ED),
    onSurfaceVariant = Color(0xFF6F6F76),
    outline = Color(0x1F17171A),
    error = Color(0xFFD9503F),
)

private val DarkScheme = darkColorScheme(
    primary = Accent,
    onPrimary = Color.White,
    background = Color(0xFF1A1A1E),
    onBackground = Color(0xFFF4F4F2),
    surface = Color(0xFF2A2A31),
    onSurface = Color(0xFFF4F4F2),
    surfaceVariant = Color(0xFF1F1F25),
    onSurfaceVariant = Color(0xFFA2A2AA),
    outline = Color(0x24FFFFFF),
    error = Color(0xFFD9503F),
)

/** Semantic colours Material does not carry. */
data class SonduitColors(
    val ok: Color,
    val warn: Color,
    val danger: Color,
    val faint: Color,
)

val LocalSonduitColors = staticCompositionLocalOf {
    SonduitColors(
        ok = Color(0xFF2FA96B),
        warn = Color(0xFFD99B28),
        danger = Color(0xFFD9503F),
        faint = Color(0xFF9B9BA2),
    )
}

/** Corner radii, matching the desktop card and inner-surface values. */
object Radius {
    val card = 22.dp
    val inner = 16.dp
    val pill = 999.dp
}

private val SonduitTypography = Typography(
    headlineLarge = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.SemiBold,
        fontSize = 30.sp,
        letterSpacing = (-0.5).sp,
    ),
    titleMedium = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Medium,
        fontSize = 16.sp,
    ),
    bodyMedium = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontSize = 14.sp,
    ),
    labelSmall = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontSize = 11.sp,
        letterSpacing = 0.6.sp,
    ),
    // Numbers are read as numbers, not as prose. Monospace stops a telemetry
    // value from shifting sideways every time a digit changes.
    bodySmall = TextStyle(
        fontFamily = FontFamily.Monospace,
        fontSize = 13.sp,
    ),
)

@Composable
fun SonduitTheme(
    dark: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    val colors = if (dark) {
        SonduitColors(
            ok = Color(0xFF3FBF7C),
            warn = Color(0xFFE0A93C),
            danger = Color(0xFFE4604F),
            faint = Color(0xFF74747C),
        )
    } else {
        SonduitColors(
            ok = Color(0xFF2FA96B),
            warn = Color(0xFFD99B28),
            danger = Color(0xFFD9503F),
            faint = Color(0xFF9B9BA2),
        )
    }

    CompositionLocalProvider(LocalSonduitColors provides colors) {
        MaterialTheme(
            colorScheme = if (dark) DarkScheme else LightScheme,
            typography = SonduitTypography,
            content = content,
        )
    }
}
