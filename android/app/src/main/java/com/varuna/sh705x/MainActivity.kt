package com.varuna.sh705x

import android.graphics.Color
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import com.varuna.sh705x.ui.App
import com.varuna.sh705x.ui.Sh705xTheme
import com.varuna.sh705x.ui.isDark
import com.varuna.sh705x.ui.loadTheme
import com.varuna.sh705x.ui.saveTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(SystemBarStyle.dark(Color.TRANSPARENT))
        setContent {
            var theme by remember { mutableStateOf(loadTheme(this)) }
            val dark = isDark(theme)
            // The status bar's icons follow the app's theme, not the system's.
            LaunchedEffect(dark) {
                enableEdgeToEdge(
                    if (dark) {
                        SystemBarStyle.dark(Color.TRANSPARENT)
                    } else {
                        SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
                    },
                )
            }
            Sh705xTheme(dark) {
                App(theme) {
                    theme = it
                    saveTheme(this, it)
                }
            }
        }
    }

    override fun onDestroy() {
        // Leaving the app ends the ECU; a rotation does not get here (configChanges).
        if (isFinishing) Native.close()
        super.onDestroy()
    }
}
