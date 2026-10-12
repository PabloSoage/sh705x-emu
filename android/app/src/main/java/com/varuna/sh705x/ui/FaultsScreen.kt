package com.varuna.sh705x.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.varuna.sh705x.Native
import com.varuna.sh705x.R
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** One code of the list. */
private data class Code(
    val index: Int,
    val code: String,
    val enabled: Boolean,
    val fails: Long,
    val tests: Long,
    val lastFailed: Boolean,
    val status: String,
    val text: String,
)

private fun codes(text: String): List<Code> = text.lineSequence().mapNotNull { l ->
    val c = l.split('\t')
    if (c.size < 8) return@mapNotNull null
    Code(
        c[0].toIntOrNull() ?: return@mapNotNull null,
        c[1],
        c[2] == "1",
        c[3].toLongOrNull() ?: 0,
        c[4].toLongOrNull() ?: 0,
        c[5] == "1",
        c[6],
        c[7],
    )
}.toList()

/** The calibration's fault codes: their tests live, which the ECU reports, and where each is tested. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun FaultsScreen(onMap: (Int) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var filter by remember { mutableStateOf("") }
    var failing by remember { mutableStateOf(false) }
    var disabled by remember { mutableStateOf(false) }
    var status by remember { mutableStateOf("") }
    var list by remember { mutableStateOf(emptyList<Code>()) }
    var selected by remember { mutableStateOf<Int?>(null) }
    var text by remember { mutableStateOf("") }
    var maps by remember { mutableStateOf(emptyList<Int>()) }
    var note by remember { mutableStateOf<String?>(null) }
    var stopping by remember { mutableStateOf(false) }

    LaunchedEffect(Unit) {
        while (true) {
            status = Native.codesStatus()
            list = codes(Native.codeList(filter, failing, disabled))
            selected?.let {
                text = Native.codeText(it)
                maps = Native.codeMaps(it).lines().mapNotNull { m -> m.toIntOrNull(16) }
            }
            delay(300)
        }
    }

    val pickTexts = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) scope.launch {
            val t = withContext(Dispatchers.IO) {
                context.contentResolver.openInputStream(uri)?.use { it.readBytes().decodeToString() }.orEmpty()
            }
            note = context.getString(R.string.texts_read, Native.describeCodes(t))
        }
    }

    if (status.isEmpty()) {
        Text(stringResource(R.string.codes_unknown), style = MaterialTheme.typography.bodySmall)
        return
    }
    Text(status, style = MaterialTheme.typography.bodySmall)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedButton(onClick = { Native.readCodes() }) { Text(stringResource(R.string.read_codes)) }
        OutlinedButton(onClick = { pickTexts.launch(arrayOf("*/*")) }) { Text(stringResource(R.string.read_texts)) }
    }
    note?.let { Text(it, style = MaterialTheme.typography.bodySmall) }

    val sel = selected
    if (sel != null && text.isNotEmpty()) {
        text.lines().forEachIndexed { k, l ->
            Text(l, style = if (k == 0) MaterialTheme.typography.titleMedium else MaterialTheme.typography.bodySmall)
        }
        if (maps.isNotEmpty()) {
            Text(stringResource(R.string.its_maps), style = MaterialTheme.typography.labelLarge)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                maps.forEach { d -> OutlinedButton(onClick = { onMap(d) }) { Text("%06X".format(d)) } }
            }
        }
        Button(onClick = {
            if (stopping) {
                Native.breakOnTest(-1)
            } else {
                Native.breakOnTest(sel)
                Native.run(true)
            }
            stopping = !stopping
        }) { Text(stringResource(if (stopping) R.string.stop_test_cancel else R.string.stop_at_test)) }
    }

    OutlinedTextField(
        value = filter,
        onValueChange = { filter = it },
        label = { Text(stringResource(R.string.code_filter)) },
        singleLine = true,
        modifier = Modifier.fillMaxWidth(),
    )
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        FilterChip(selected = failing, onClick = { failing = !failing }, label = { Text(stringResource(R.string.only_failing)) })
        FilterChip(selected = disabled, onClick = { disabled = !disabled }, label = { Text(stringResource(R.string.only_disabled)) })
    }
    list.forEach { c ->
        val colour = when {
            c.status.isNotEmpty() -> Color(0xFFFFAA50)
            c.lastFailed -> Color(0xFFE6D26E)
            !c.enabled -> MaterialTheme.colorScheme.onSurface.copy(alpha = 0.5f)
            else -> MaterialTheme.colorScheme.onSurface
        }
        Text(
            "%s %3d %s %5d/%-5d %s %s".format(
                c.code,
                c.index,
                if (c.enabled) "on " else "OFF",
                c.fails,
                c.tests,
                c.status,
                c.text,
            ),
            color = colour,
            fontFamily = FontFamily.Monospace,
            fontSize = 12.sp,
            modifier = Modifier
                .fillMaxWidth()
                .background(if (c.index == selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent)
                .clickable {
                    selected = c.index
                    stopping = false
                }
                .padding(vertical = 3.dp),
        )
    }
}
