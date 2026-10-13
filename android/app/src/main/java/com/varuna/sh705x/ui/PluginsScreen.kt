package com.varuna.sh705x.ui

import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.Checkbox
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.varuna.sh705x.Native
import com.varuna.sh705x.R
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** A plugin running: its line, then its log. */
private data class Running(
    val index: Int,
    val name: String,
    val version: String,
    val file: String,
    val heard: String,
    val sent: String,
    val error: String,
    val log: List<String>,
)

private fun running(text: String): Pair<List<Running>, String?> {
    val out = mutableListOf<Running>()
    var note: String? = null
    text.lineSequence().forEach { l ->
        val c = l.split('\t')
        when (c[0]) {
            "P" -> if (c.size >= 8) out += Running(c[1].toIntOrNull() ?: 0, c[2], c[3], c[4], c[5], c[6], c[7], emptyList())
            "L" -> if (out.isNotEmpty()) out[out.size - 1] = out.last().copy(log = out.last().log + c.drop(1).joinToString(" "))
            "N" -> note = c.drop(1).joinToString(" ")
        }
    }
    return out to note
}

/** A plugin installed. */
private data class Installed(
    val file: String,
    val name: String,
    val version: String,
    val enabled: Boolean,
    val description: String,
    val error: String,
)

private fun installed(text: String) = text.lineSequence().mapNotNull { l ->
    val c = l.split('\t')
    if (c.size < 6) null else Installed(c[0], c[1], c[2], c[3] == "1", c[4], c[5])
}.toList()

/** A source, without its token. */
private data class SourceRow(val name: String, val kind: String, val location: String, val ref: String, val private: Boolean)

private fun sources(text: String) = text.lineSequence().mapNotNull { l ->
    val c = l.split('\t')
    if (c.size < 5) null else SourceRow(c[0], c[1], c[2], c[3], c[4] == "1")
}.toList()

/** The kinds of source, as the native side names them. */
private val KINDS = listOf("GitHub", "GitLab", "Gitea", "HTTPS")

/**
 * The plugins: those running beside the ECU, those installed in the app's storage, and
 * the sources they come from — a repository of your own with its token, as in opendash.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun PluginsScreen() {
    val context = LocalContext.current
    val dir = remember { context.filesDir.absolutePath + "/plugins-library" }
    val scope = rememberCoroutineScope()
    var live by remember { mutableStateOf(emptyList<Running>() to (null as String?)) }
    var library by remember { mutableStateOf(emptyList<Installed>()) }
    var srcs by remember { mutableStateOf(emptyList<SourceRow>()) }
    var note by remember { mutableStateOf<String?>(null) }
    val listed = remember { mutableStateMapOf<Int, String>() }
    var busy by remember { mutableIntStateOf(0) }

    fun refresh() {
        library = installed(Native.pluginsInstalled(dir))
        srcs = sources(Native.sourcesList(dir))
    }

    LaunchedEffect(Unit) {
        refresh()
        while (true) {
            live = running(Native.pluginsText())
            delay(300)
        }
    }

    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri != null) scope.launch {
            note = withContext(Dispatchers.IO) {
                val name = context.contentResolver.query(uri, null, null, null, null)?.use { c ->
                    val i = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    if (i >= 0 && c.moveToFirst()) c.getString(i) else null
                } ?: "plugin.wasm"
                val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: ByteArray(0)
                Native.pluginAdd(dir, name, bytes)
            }
            refresh()
        }
    }

    Text(stringResource(R.string.plugins_intro), style = MaterialTheme.typography.bodySmall)

    Text(stringResource(R.string.plugins_running), style = MaterialTheme.typography.titleMedium)
    if (live.first.isEmpty()) Text(stringResource(R.string.none), style = MaterialTheme.typography.bodySmall)
    live.first.forEach { p ->
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("${p.name} ${p.version}", style = MaterialTheme.typography.titleSmall)
                Text(stringResource(R.string.plugin_counts, p.file, p.heard, p.sent), style = MaterialTheme.typography.bodySmall)
                if (p.error.isNotEmpty()) Text(p.error, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                p.log.takeLast(8).forEach { Text(it, fontFamily = FontFamily.Monospace, fontSize = 11.sp) }
                OutlinedButton(onClick = { Native.pluginUnload(p.index) }) { Text(stringResource(R.string.plugin_unload)) }
            }
        }
    }
    live.second?.let { Text(it, style = MaterialTheme.typography.bodySmall) }

    Text(stringResource(R.string.plugins_installed), style = MaterialTheme.typography.titleMedium)
    if (library.isEmpty()) Text(stringResource(R.string.none), style = MaterialTheme.typography.bodySmall)
    library.forEach { p ->
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(if (p.name.isEmpty()) p.file else "${p.name} ${p.version}", style = MaterialTheme.typography.titleSmall)
                Text(p.file, style = MaterialTheme.typography.bodySmall)
                if (p.description.isNotEmpty()) Text(p.description, style = MaterialTheme.typography.bodySmall)
                if (p.error.isNotEmpty()) Text(p.error, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.Center) {
                    if (p.error.isEmpty()) Button(onClick = { note = Native.pluginLoad(dir, p.file).ifEmpty { null } }) {
                        Text(stringResource(R.string.plugin_load))
                    }
                    OutlinedButton(onClick = {
                        note = Native.pluginRemove(dir, p.file).ifEmpty { null }
                        refresh()
                    }) { Text(stringResource(R.string.plugin_remove)) }
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(checked = p.enabled, onCheckedChange = {
                            note = Native.pluginEnable(dir, p.file, it).ifEmpty { null }
                            refresh()
                        })
                        Text(stringResource(R.string.plugin_at_power_on), style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        }
    }
    OutlinedButton(onClick = { pick.launch(arrayOf("*/*")) }) { Text(stringResource(R.string.plugin_from_file)) }
    note?.let { Text(it, style = MaterialTheme.typography.bodySmall) }

    Text(stringResource(R.string.plugin_sources), style = MaterialTheme.typography.titleMedium)
    Text(stringResource(R.string.plugin_sources_intro), style = MaterialTheme.typography.bodySmall)
    srcs.forEachIndexed { i, s ->
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(s.name, style = MaterialTheme.typography.titleSmall)
                Text(
                    (if (s.kind == "HTTPS") "${s.kind} ${s.location}" else "${s.kind} ${s.location} @ ${s.ref}") + " · " +
                        stringResource(if (s.private) R.string.with_token else R.string.public_source),
                    style = MaterialTheme.typography.bodySmall,
                )
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = {
                        busy++
                        scope.launch {
                            listed[i] = withContext(Dispatchers.IO) { Native.sourceIndex(dir, i) }
                            busy--
                        }
                    }) { Text(stringResource(R.string.source_list)) }
                    OutlinedButton(onClick = {
                        note = Native.sourceRemove(dir, i).ifEmpty { null }
                        listed.clear()
                        refresh()
                    }) { Text(stringResource(R.string.plugin_remove)) }
                }
                listed[i]?.let { text ->
                    if (text.startsWith("!")) {
                        Text(text.drop(1), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                    } else if (text.isBlank()) {
                        Text(stringResource(R.string.source_empty), style = MaterialTheme.typography.bodySmall)
                    } else text.lineSequence().forEach { l ->
                        val c = l.split('\t')
                        val file = c[0]
                        Text("${c.getOrElse(1) { "" }} ${c.getOrElse(2) { "" }} · $file", style = MaterialTheme.typography.bodyMedium)
                        c.getOrNull(3)?.takeIf { it.isNotEmpty() }?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
                        OutlinedButton(onClick = {
                            busy++
                            scope.launch {
                                note = withContext(Dispatchers.IO) { Native.sourceInstall(dir, i, file) }
                                busy--
                                refresh()
                            }
                        }) { Text(stringResource(R.string.source_install)) }
                    }
                }
            }
        }
    }
    if (busy > 0) Text(stringResource(R.string.fetching), style = MaterialTheme.typography.bodySmall)

    AddSource(dir) { e ->
        note = e.ifEmpty { null }
        refresh()
    }
}

/** The form that adds a source. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun AddSource(dir: String, onAdded: (String) -> Unit) {
    var name by remember { mutableStateOf("") }
    var kind by remember { mutableStateOf(KINDS[0]) }
    var location by remember { mutableStateOf("") }
    var ref by remember { mutableStateOf("main") }
    var token by remember { mutableStateOf("") }
    Text(stringResource(R.string.source_add), style = MaterialTheme.typography.titleSmall)
    OutlinedTextField(name, { name = it }, label = { Text(stringResource(R.string.source_name)) }, singleLine = true, modifier = Modifier.fillMaxWidth())
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        KINDS.forEach { k -> FilterChip(selected = kind == k, onClick = { kind = k }, label = { Text(k) }) }
    }
    OutlinedTextField(
        location, { location = it },
        label = { Text(stringResource(R.string.source_location)) },
        placeholder = { Text(if (kind == "GitHub" || kind == "GitLab") "owner/repository" else "https://…") },
        singleLine = true, modifier = Modifier.fillMaxWidth(),
    )
    if (kind != "HTTPS") {
        OutlinedTextField(ref, { ref = it }, label = { Text(stringResource(R.string.source_ref)) }, singleLine = true, modifier = Modifier.fillMaxWidth())
    }
    OutlinedTextField(
        token, { token = it },
        label = { Text(stringResource(R.string.source_token)) },
        visualTransformation = PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
        singleLine = true, modifier = Modifier.fillMaxWidth(),
    )
    Button(enabled = name.isNotBlank() && location.isNotBlank(), onClick = {
        val e = Native.sourceAdd(dir, name, kind, location, ref, token)
        if (e.isEmpty()) {
            name = ""; location = ""; ref = "main"; token = ""
        }
        onAdded(e)
    }) { Text(stringResource(R.string.source_add_button)) }
}
