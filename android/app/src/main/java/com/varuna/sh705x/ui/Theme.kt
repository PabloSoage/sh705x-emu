package com.varuna.sh705x.ui

import android.content.Context
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val Amber = Color(0xFFFFC850)
private val AmberInk = Color(0xFF8A5A00)

/** Dark (the default: the charts and wheels are drawn on a dark plot), light, or as the system is. */
enum class ThemeChoice { DARK, LIGHT, SYSTEM }

private const val PREFS = "settings"
private const val THEME = "theme"

/** The theme chosen last, dark if none. */
fun loadTheme(context: Context): ThemeChoice =
    context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(THEME, null)
        ?.let { runCatching { ThemeChoice.valueOf(it) }.getOrNull() }
        ?: ThemeChoice.DARK

/** Remembers [choice] for the next start. */
fun saveTheme(context: Context, choice: ThemeChoice) {
    context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(THEME, choice.name).apply()
}

/** Whether [choice] is dark here and now. */
@Composable
fun isDark(choice: ThemeChoice): Boolean = when (choice) {
    ThemeChoice.DARK -> true
    ThemeChoice.LIGHT -> false
    ThemeChoice.SYSTEM -> isSystemInDarkTheme()
}

/** Material 3, dark or light; the charts keep their own dark plot either way. */
@Composable
fun Sh705xTheme(dark: Boolean, content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = if (dark) {
            darkColorScheme(primary = Amber, onPrimary = Color(0xFF1A1400))
        } else {
            lightColorScheme(primary = AmberInk)
        },
        content = content,
    )
}
