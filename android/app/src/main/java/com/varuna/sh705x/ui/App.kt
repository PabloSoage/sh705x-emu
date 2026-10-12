package com.varuna.sh705x.ui

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringArrayResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import com.varuna.sh705x.Frame
import com.varuna.sh705x.Native
import com.varuna.sh705x.R
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** History windows, ms (as the desktop view's). */
private val WINDOWS_MS = doubleArrayOf(10.0, 50.0, 200.0, 1000.0, 5000.0, 20000.0)

/** ECU time per wall-clock time to choose from. */
private val SPEEDS = doubleArrayOf(1.0, 0.1, 0.01, 0.001)

/** Columns a frame asks for: about one per two pixels of a phone's width. */
private const val COLUMNS = 480

/** The traces in each chart, in `session::scope::TRACES` order (as the desktop view's). */
private val PANELS = listOf(
    intArrayOf(0),
    intArrayOf(1, 2, 3),
    intArrayOf(4, 5),
    intArrayOf(6, 9),
    intArrayOf(7, 8),
    intArrayOf(10, 11),
    intArrayOf(12),
    intArrayOf(13),
    intArrayOf(14),
)

/** A dump picked from the phone's storage: its name and bytes, in memory only. */
class Picked(val name: String, val bytes: ByteArray)

private suspend fun read(context: Context, uri: Uri): Picked = withContext(Dispatchers.IO) {
    val name = context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
        ?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
        ?: uri.lastPathSegment.orEmpty()
    val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: ByteArray(0)
    Picked(name, bytes)
}

/** The app: the load screen, then the ECU running; [theme] and [onTheme] for the theme row. */
@Composable
fun App(theme: ThemeChoice, onTheme: (ThemeChoice) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var flash by remember { mutableStateOf<Picked?>(null) }
    var eeprom by remember { mutableStateOf<Picked?>(null) }
    var on by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var frame by remember { mutableStateOf(Frame.EMPTY) }
    var window by remember { mutableIntStateOf(1) }

    val pickFlash = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) scope.launch { flash = read(context, uri) }
    }
    val pickEeprom = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) scope.launch { eeprom = read(context, uri) }
    }

    // Out of sight, the ECU pauses rather than run on in the background.
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) {
        if (on) Native.run(false)
    }

    LaunchedEffect(on, window) {
        while (on) {
            val f = Frame(Native.frame(WINDOWS_MS[window] * 1000.0, COLUMNS))
            if (f.valid) frame = f
            delay(33)
        }
    }

    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(
            Modifier
                .safeDrawingPadding()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(stringResource(R.string.app_name), style = MaterialTheme.typography.titleLarge)
            if (!on) {
                Load(
                    flash = flash,
                    eeprom = eeprom,
                    error = error,
                    onFlash = { pickFlash.launch(arrayOf("*/*")) },
                    onEeprom = { pickEeprom.launch(arrayOf("*/*")) },
                    onPowerOn = {
                        val f = flash ?: return@Load
                        scope.launch {
                            val e = withContext(Dispatchers.Default) { Native.open(f.bytes, eeprom?.bytes) }
                            if (e.isEmpty()) {
                                error = null
                                on = true
                            } else {
                                error = e
                            }
                        }
                    },
                )
            } else {
                Engine(frame, window, onWindow = { window = it }, onOff = {
                    Native.close()
                    on = false
                    frame = Frame.EMPTY
                })
            }
            ThemeRow(theme, onTheme)
        }
    }
}

/** Picking the dumps and powering the ECU on. */
@Composable
private fun Load(
    flash: Picked?,
    eeprom: Picked?,
    error: String?,
    onFlash: () -> Unit,
    onEeprom: () -> Unit,
    onPowerOn: () -> Unit,
) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.load_intro), style = MaterialTheme.typography.bodyMedium)
            OutlinedButton(onClick = onFlash) {
                Text(flash?.let { stringResource(R.string.picked, it.name, it.bytes.size) } ?: stringResource(R.string.pick_flash))
            }
            OutlinedButton(onClick = onEeprom) {
                Text(eeprom?.let { stringResource(R.string.picked, it.name, it.bytes.size) } ?: stringResource(R.string.pick_eeprom))
            }
            Button(onClick = onPowerOn, enabled = flash != null) { Text(stringResource(R.string.power_on)) }
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            Text(stringResource(R.string.no_firmware), style = MaterialTheme.typography.bodySmall)
        }
    }
}

/** The ECU running: status, controls and the engine view. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Engine(frame: Frame, window: Int, onWindow: (Int) -> Unit, onOff: () -> Unit) {
    var rpm by remember { mutableFloatStateOf(750f) }
    var pedal by remember { mutableFloatStateOf(0f) }
    val names = stringArrayResource(R.array.trace_names).toList()
    val units = remember { Native.traces().lines().map { it.substringAfter('\t') } }
    val panels = stringArrayResource(R.array.panels)

    Text(
        stringResource(
            R.string.status,
            frame.nowUs / 1e6,
            frame.rpm,
            frame.realTime,
        ),
        style = MaterialTheme.typography.bodyMedium,
    )
    Native.stop().takeIf { it.isNotEmpty() }?.let {
        Text(stringResource(R.string.stopped_by, it), color = MaterialTheme.colorScheme.error)
    }

    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(onClick = { Native.run(!frame.running) }) {
            Text(stringResource(if (frame.running) R.string.pause else R.string.run))
        }
        OutlinedButton(onClick = { Native.nextTooth() }) { Text(stringResource(R.string.next_tooth)) }
        OutlinedButton(onClick = { Native.nextEdge() }) { Text(stringResource(R.string.next_edge)) }
        OutlinedButton(onClick = {
            Native.preset(true)
            Native.plants(Frame.PLANTS_TURNING)
            Native.run(true)
            rpm = 750f
        }) { Text(stringResource(R.string.idle_on_torque)) }
        OutlinedButton(onClick = onOff) { Text(stringResource(R.string.power_off)) }
    }

    Text(stringResource(R.string.speed), style = MaterialTheme.typography.labelLarge)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        for (s in SPEEDS) {
            FilterChip(
                selected = frame.speed == s,
                onClick = { Native.speed(s) },
                label = { Text(if (s == 1.0) stringResource(R.string.real_time) else "×$s") },
            )
        }
    }

    // The engine, or the maps it reads (traced only while shown: about 6 % slower).
    var tab by remember { mutableIntStateOf(0) }
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        FilterChip(selected = tab == 0, onClick = { tab = 0 }, label = { Text(stringResource(R.string.tab_engine)) })
        FilterChip(selected = tab == 1, onClick = { tab = 1 }, label = { Text(stringResource(R.string.tab_maps)) })
        FilterChip(selected = tab == 2, onClick = { tab = 2 }, label = { Text(stringResource(R.string.tab_faults)) })
    }
    // The maps and the codes' tests are traced together, while either tab is open.
    val tracing = tab != 0
    DisposableEffect(tracing) {
        if (tracing) Native.traceMaps(true)
        onDispose { if (tracing) Native.traceMaps(false) }
    }
    var openMap by remember { mutableStateOf<Int?>(null) }
    if (tab == 1) {
        MapsScreen(openMap)
        Spacer(Modifier.height(24.dp))
        return
    }
    if (tab == 2) {
        FaultsScreen { d ->
            openMap = d
            tab = 1
        }
        Spacer(Modifier.height(24.dp))
        return
    }

    Text(stringResource(R.string.plants), style = MaterialTheme.typography.labelLarge)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        listOf(
            Frame.PLANTS_OFF to R.string.plants_off,
            Frame.PLANTS_HELD to R.string.plants_held,
            Frame.PLANTS_TURNING to R.string.plants_turning,
        ).forEach { (mode, label) ->
            FilterChip(
                selected = frame.plants == mode,
                onClick = { Native.plants(mode) },
                label = { Text(stringResource(label)) },
            )
        }
    }

    if (frame.plants != Frame.PLANTS_TURNING) {
        Text(stringResource(R.string.engine_speed, rpm), style = MaterialTheme.typography.labelLarge)
        Slider(
            value = rpm,
            onValueChange = {
                rpm = it
                Native.rpm(it.toDouble())
            },
            valueRange = 0f..4500f,
        )
    }
    Text(stringResource(R.string.pedal, pedal), style = MaterialTheme.typography.labelLarge)
    Slider(
        value = pedal,
        onValueChange = {
            pedal = it
            Native.sensor("pedal", it.toDouble())
        },
        valueRange = 0f..100f,
    )

    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Wheels(
            frame.crank,
            WheelWords(
                stringResource(R.string.wheel_stopped),
                stringResource(R.string.wheel_tooth),
                stringResource(R.string.wheel_gap),
            ),
            Modifier.weight(1f).aspectRatio(1f),
        )
        Cylinders(
            frame,
            stringArrayResource(R.array.strokes).toList(),
            Modifier.weight(1f).height(170.dp),
        )
    }
    Text(stringResource(R.string.phase_assumed), style = MaterialTheme.typography.bodySmall)

    Text(stringResource(R.string.window), style = MaterialTheme.typography.labelLarge)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        WINDOWS_MS.forEachIndexed { k, ms ->
            FilterChip(
                selected = window == k,
                onClick = { onWindow(k) },
                label = { Text(if (ms < 1000) "${ms.toInt()} ms" else "${(ms / 1000).toInt()} s") },
            )
        }
    }

    val windowUs = WINDOWS_MS[window] * 1000.0
    PANELS.forEachIndexed { k, traces ->
        Strip(frame, panels[k], traces, names, units, windowUs)
    }
    Signals(
        frame,
        windowUs,
        SignalWords(
            stringResource(R.string.signals),
            stringResource(R.string.crank),
            stringResource(R.string.cam),
            stringResource(R.string.injector),
        ),
    )
    Text(stringResource(R.string.no_injection), style = MaterialTheme.typography.bodySmall)
    Spacer(Modifier.height(24.dp))
}

/** Dark (the default), light, or as the system is. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ThemeRow(theme: ThemeChoice, onTheme: (ThemeChoice) -> Unit) {
    Text(stringResource(R.string.theme), style = MaterialTheme.typography.labelLarge)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        listOf(
            ThemeChoice.DARK to R.string.theme_dark,
            ThemeChoice.LIGHT to R.string.theme_light,
            ThemeChoice.SYSTEM to R.string.theme_system,
        ).forEach { (choice, label) ->
            FilterChip(
                selected = theme == choice,
                onClick = { onTheme(choice) },
                label = { Text(stringResource(label)) },
            )
        }
    }
}
