package com.varuna.sh705x.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val Amber = Color(0xFFFFC850)
private val AmberInk = Color(0xFF8A5A00)

/** Material 3, light or dark as the system is; the charts keep their own dark plot. */
@Composable
fun Sh705xTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = if (isSystemInDarkTheme()) {
            darkColorScheme(primary = Amber, onPrimary = Color(0xFF1A1400))
        } else {
            lightColorScheme(primary = AmberInk)
        },
        content = content,
    )
}
