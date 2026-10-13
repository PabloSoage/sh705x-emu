package com.varuna.sh705x.update

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.varuna.sh705x.R
import java.io.File
import kotlin.concurrent.thread

private sealed interface Download {
    data object Idle : Download
    class Running(val progress: Float) : Download
    class Ready(val file: File) : Download
    /** Downloaded, and more than one app can install it: the user picks. */
    class Choose(val file: File, val installers: List<AppUpdate.Installer>) : Download
    data object Failed : Download
}

/** The release notes, then download and install the APK for this device, or open the release page. */
@Composable
fun UpdateDialog(update: Releases.Update, onDismiss: () -> Unit) {
    val context = LocalContext.current
    var state by remember { mutableStateOf<Download>(Download.Idle) }
    val running = state is Download.Running

    fun install(file: File) {
        // Without "install unknown apps" every installer refuses. The file
        // stays in the cache, so the next tap after granting it installs.
        if (!AppUpdate.canInstall(context)) {
            state = Download.Ready(file)
            AppUpdate.requestInstallPermission(context)
            return
        }
        val installers = AppUpdate.installers(context, file)
        if (installers.size >= 2) {
            state = Download.Choose(file, installers)
        } else {
            state = Download.Ready(file)
            AppUpdate.install(context, file)
        }
    }

    AlertDialog(
        onDismissRequest = { if (!running) onDismiss() },
        title = { Text(stringResource(R.string.update_available_title, update.title)) },
        text = {
            Column {
                Text(
                    Releases.plainNotes(update.body).ifBlank { update.tag },
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.heightIn(max = 320.dp).verticalScroll(rememberScrollState()),
                )
                when (val s = state) {
                    is Download.Running -> if (s.progress >= 0f) {
                        LinearProgressIndicator(
                            progress = { s.progress },
                            modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
                        )
                    } else {
                        LinearProgressIndicator(Modifier.fillMaxWidth().padding(top = 12.dp))
                    }
                    is Download.Choose -> Column(Modifier.padding(top = 12.dp)) {
                        Text(
                            stringResource(R.string.update_choose_installer),
                            style = MaterialTheme.typography.labelLarge,
                        )
                        s.installers.forEach { installer ->
                            FilledTonalButton(
                                onClick = { AppUpdate.installWith(context, s.file, installer) },
                                modifier = Modifier.fillMaxWidth().padding(top = 6.dp),
                            ) { Text(installer.label) }
                        }
                        TextButton(onClick = { AppUpdate.install(context, s.file) }) {
                            Text(stringResource(R.string.update_other_installer))
                        }
                    }
                    Download.Failed -> Text(
                        stringResource(R.string.update_download_failed),
                        color = MaterialTheme.colorScheme.error,
                        modifier = Modifier.padding(top = 12.dp),
                    )
                    else -> Unit
                }
            }
        },
        confirmButton = {
            val apk = update.apk
            when {
                apk == null -> Button(onClick = { AppUpdate.openReleasePage(context, update) }) {
                    Text(stringResource(R.string.update_open_github))
                }
                // While picking an installer the list is the action.
                state is Download.Choose -> Unit
                else -> Button(
                    enabled = !running,
                    onClick = {
                        (state as? Download.Ready)?.let { install(it.file); return@Button }
                        state = Download.Running(-1f)
                        thread(name = "update-download", isDaemon = true) {
                            val file = runCatching {
                                AppUpdate.download(context, apk) { state = Download.Running(it) }
                            }.getOrNull()
                            if (file == null) state = Download.Failed else install(file)
                        }
                    },
                ) {
                    Text(
                        stringResource(
                            when (state) {
                                is Download.Ready -> R.string.update_install
                                Download.Failed -> R.string.update_retry
                                else -> R.string.update_download
                            },
                        ),
                    )
                }
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss, enabled = !running) { Text(stringResource(R.string.update_later)) }
        },
    )
}
