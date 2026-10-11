package com.varuna.sh705x

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import com.varuna.sh705x.ui.App
import com.varuna.sh705x.ui.Sh705xTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent { Sh705xTheme { App() } }
    }

    override fun onDestroy() {
        // Leaving the app ends the ECU; a rotation does not get here (configChanges).
        if (isFinishing) Native.close()
        super.onDestroy()
    }
}
