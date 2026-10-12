package com.varuna.sh705x.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.varuna.sh705x.Native
import com.varuna.sh705x.OscFrame
import com.varuna.sh705x.R
import kotlinx.coroutines.delay

/** Microseconds a division, as the desktop window's. */
private val TIMEBASES = doubleArrayOf(10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0, 50_000.0)

private const val DIVS = 10

/** Lane colours, as the desktop window's. */
private val LANES = listOf(
    Color(0xFFFFDC5A), Color(0xFF6EC8FF), Color(0xFFFF78C8),
    Color(0xFF78E68C), Color(0xFFFFA050), Color(0xFFBEA0FF),
)

private fun time(us: Double): String =
    if (kotlin.math.abs(us) >= 1000) "%.2f ms".format(us / 1000) else "%.0f µs".format(us)

/** The ECU's pins as waveforms, held still on an edge of one of them. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ScopeScreen() {
    val names = remember { Native.oscChannels().lines() }
    // Crank, SCV, VGT, EGR open, injector 1: what fits a phone.
    var mask by remember { mutableIntStateOf((1 shl 0) or (1 shl 3) or (1 shl 7) or (1 shl 10) or (1 shl 11)) }
    var timebase by remember { mutableIntStateOf(6) }
    var trigger by remember { mutableIntStateOf(0) }
    var rising by remember { mutableStateOf(true) }
    var held by remember { mutableStateOf(false) }
    var frame by remember { mutableStateOf<OscFrame?>(null) }

    LaunchedEffect(mask, timebase, trigger, rising, held) {
        while (!held) {
            val f = OscFrame(Native.oscFrame(TIMEBASES[timebase] * DIVS, trigger, rising, 0.2, mask))
            if (f.valid) frame = f
            delay(100)
        }
    }

    frame?.let { f -> Picture(f, names) }

    Text(stringResource(R.string.osc_channels), style = MaterialTheme.typography.labelLarge)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        names.forEachIndexed { k, n ->
            FilterChip(selected = mask shr k and 1 == 1, onClick = { mask = mask xor (1 shl k) }, label = { Text(n) })
        }
    }
    Text(stringResource(R.string.osc_timebase), style = MaterialTheme.typography.labelLarge)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        TIMEBASES.forEachIndexed { k, tb ->
            FilterChip(selected = timebase == k, onClick = { timebase = k }, label = { Text(time(tb)) })
        }
    }
    Text(stringResource(R.string.osc_trigger), style = MaterialTheme.typography.labelLarge)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        names.forEachIndexed { k, n ->
            if (mask shr k and 1 == 1) {
                FilterChip(selected = trigger == k, onClick = { trigger = k }, label = { Text(n) })
            }
        }
        FilterChip(selected = rising, onClick = { rising = true }, label = { Text(stringResource(R.string.osc_rising)) })
        FilterChip(selected = !rising, onClick = { rising = false }, label = { Text(stringResource(R.string.osc_falling)) })
        FilterChip(selected = held, onClick = { held = !held }, label = { Text(stringResource(R.string.osc_hold)) })
    }

}

/** The traces of one frame, with the trigger and the time from it. */
@Composable
private fun Picture(f: OscFrame, names: List<String>) {
    Text(
        if (f.triggered) stringResource(R.string.osc_triggered) else stringResource(R.string.osc_free),
        style = MaterialTheme.typography.bodySmall,
    )
    val tm = rememberTextMeasurer()
    val lane = 52.dp
    Canvas(
        Modifier
            .fillMaxWidth()
            .height(lane * maxOf(f.traces.size, 1) + 24.dp)
            .background(Color(0xFF0A0E0C)),
    ) {
        val plotH = size.height - 24.dp.toPx()
        val grid = Color(0xFF1E3228)
        for (d in 0..DIVS) {
            val x = size.width * d / DIVS
            drawLine(grid, Offset(x, 0f), Offset(x, plotH), 1f)
        }
        val l = lane.toPx()
        fun x(us: Double) = (us / f.spanUs * size.width).toFloat()
        f.traces.forEachIndexed { n, t ->
            val colour = LANES[n % LANES.size]
            val top = n * l + 18.dp.toPx()
            val hi = top + 2.dp.toPx()
            val lo = top + l - 22.dp.toPx()
            fun y(level: Boolean) = if (level) hi else lo
            val path = Path().apply {
                var level = t.initial
                moveTo(0f, y(level))
                for ((at, lv) in t.edges) {
                    lineTo(x(at), y(level))
                    lineTo(x(at), y(lv))
                    level = lv
                }
                lineTo(size.width, y(level))
            }
            drawPath(path, colour, style = Stroke(2.dp.toPx()))
            val label = names.getOrElse(t.channel) { "?" } + if (t.periodUs.isFinite()) {
                "  %.1f Hz · %.1f %%".format(1e6 / t.periodUs, t.duty * 100)
            } else ""
            drawText(textMeasurer = tm, text = label, topLeft = Offset(4f, n * l), style = TextStyle(color = colour, fontSize = 11.sp))
        }
        if (f.triggerUs.isFinite()) {
            val tx = x(f.triggerUs)
            drawLine(Color.White.copy(alpha = 0.4f), Offset(tx, 0f), Offset(tx, plotH), 1.5f)
        }
        val axis = TextStyle(color = Color.Gray, fontSize = 9.sp, fontFamily = FontFamily.Monospace)
        val zero = if (f.triggerUs.isFinite()) f.triggerUs else f.spanUs
        // Every other division, the last one left out: it would not fit at the edge.
        for (d in 0 until DIVS step 2) {
            val us = f.spanUs * d / DIVS - zero
            drawText(textMeasurer = tm, text = time(us), topLeft = Offset(size.width * d / DIVS, plotH + 4f), style = axis)
        }
    }
}
