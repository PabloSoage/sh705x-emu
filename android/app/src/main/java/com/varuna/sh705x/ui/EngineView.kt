package com.varuna.sh705x.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.varuna.sh705x.Frame
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.sin

// The engine view, as the desktop window draws it (crates/gui view.rs).

/** Cam pulses over the 720° cycle and their width (engine::wheel's defaults). */
private val CAM = floatArrayOf(0f, 180f, 360f, 540f, 690f)
private const val CAM_WIDTH = 10f

/**
 * Firing top dead centres of cylinders 1, 2, 3, 4 over 720°, firing order 1-3-4-2, with
 * cylinder 1 put at 0°: where it really is relative to the gap is not known.
 */
private val TDC = floatArrayOf(0f, 540f, 180f, 360f)

/** Colours of the first, second and third trace of a chart. */
val TRACE_COLOURS = listOf(Color(0xFF6EAAFF), Color(0xFFFFA03C), Color(0xFF78DC78))
private val PlotBack = Color(0xFF161616)
private val Line = Color(0xFF787878)
private val Light = Color(0xFFC8C8C8)
private val SensorRed = Color(0xFFFF5A5A)
private val CamAmber = Color(0xFFFFC850)
private val InjectorBlue = Color(0xFF50C8FF)

/** A point at [deg] clockwise from the top, [r] from [c]. */
private fun polar(c: Offset, deg: Float, r: Float): Offset {
    val t = deg / 180f * PI.toFloat()
    return Offset(c.x + r * sin(t), c.y - r * cos(t))
}

private fun DrawScope.centredText(tm: TextMeasurer, text: String, at: Offset, style: TextStyle) {
    val r = tm.measure(text, style)
    drawText(r, topLeft = Offset(at.x - r.size.width / 2f, at.y - r.size.height / 2f))
}

/**
 * The 60-4 crank wheel and the cam wheel turning under their sensors (top, red), with the
 * angle over the 720° of a cam cycle and the tooth at the sensor.
 */
@Composable
fun Wheels(crank: Double?, words: WheelWords, modifier: Modifier = Modifier) {
    val tm = rememberTextMeasurer()
    Canvas(modifier) {
        // On the charts' dark plot, so the colours read in a light theme too.
        drawRect(PlotBack)
        val c = center
        val big = size.minDimension / 2f * 0.86f
        val a = (crank ?: 0.0).toFloat()
        drawCircle(Line, big, c, style = Stroke(1.dp.toPx()))
        // Tooth k reaches the sensor (top) when the crank is at k·6°.
        for (k in 0 until 56) {
            val at = k * 6f - a.mod(360f)
            drawLine(Light, polar(c, at, big), polar(c, at, big * 1.1f), 3.dp.toPx())
        }
        val small = big * 0.62f
        drawCircle(Line, small, c, style = Stroke(1.dp.toPx()))
        for (cam in CAM) {
            // The cam turns at half speed.
            val from = (cam - a) / 2f
            for (i in 0 until 6) {
                val d0 = from + i * CAM_WIDTH / 2f / 6f
                val d1 = from + (i + 1) * CAM_WIDTH / 2f / 6f
                drawLine(CamAmber, polar(c, d0, small * 1.12f), polar(c, d1, small * 1.12f), 5.dp.toPx())
            }
        }
        drawLine(SensorRed, polar(c, 0f, big * 1.14f), polar(c, 0f, big * 0.98f), 2.dp.toPx())
        drawLine(SensorRed, polar(c, 0f, small * 1.25f), polar(c, 0f, small * 0.95f), 2.dp.toPx())
        val text = if (crank == null) {
            words.stopped
        } else {
            val tooth = floor(crank.mod(360.0) / 6.0).toInt()
            "%.1f° / 720\n%s".format(crank, if (tooth < 56) words.tooth.format(tooth) else words.gap)
        }
        centredText(tm, text, c, TextStyle(color = Light, fontSize = 12.sp))
    }
}

/** What the wheels say, in the app's language. */
data class WheelWords(val stopped: String, val tooth: String, val gap: String)

/**
 * The four cylinders: piston height and stroke from the crank angle, and the injector lit
 * while its output (TO8I–L) pulses.
 */
@Composable
fun Cylinders(frame: Frame, strokes: List<String>, modifier: Modifier = Modifier) {
    val tm = rememberTextMeasurer()
    val crank = frame.crank?.toFloat()
    val now = frame.nowUs
    val injecting = BooleanArray(4)
    for (i in 0 until frame.pulses) {
        val p = frame.pulse(i)
        if (p.output in 8..11 && p.startUs <= now && now <= p.endUs) injecting[p.output - 8] = true
    }
    Canvas(modifier) {
        drawRect(PlotBack)
        val w = size.width / 4f
        val label = TextStyle(color = Light, fontSize = 11.sp)
        for (i in 0 until 4) {
            val left = w * i + 10.dp.toPx()
            val top = 22.dp.toPx()
            val bore = Size(w - 20.dp.toPx(), size.height - 44.dp.toPx())
            drawRect(Line, Offset(left, top), bore, style = Stroke(1.dp.toPx()))
            if (crank != null) {
                val ph = (crank - TDC[i]).mod(720f)
                // 0 at top dead centre, 1 at bottom.
                val down = (1f - cos(ph / 180f * PI.toFloat())) / 2f
                val piston = 14.dp.toPx()
                val y = top + 3.dp.toPx() + down * (bore.height - piston - 6.dp.toPx())
                drawRect(Color(0xFFAAAAAA), Offset(left + 2.dp.toPx(), y), Size(bore.width - 4.dp.toPx(), piston))
                val stroke = strokes[(ph / 180f).toInt().coerceIn(0, 3)]
                centredText(tm, stroke, Offset(left + bore.width / 2f, top + bore.height + 11.dp.toPx()), label)
            }
            val x = left + bore.width / 2f
            drawCircle(if (injecting[i]) InjectorBlue else Color(0xFF3C3C3C), 6.dp.toPx(), Offset(x, 10.dp.toPx()))
            centredText(tm, "${i + 1}", Offset(x + 16.dp.toPx(), 10.dp.toPx()), label)
        }
    }
}

/** A strip chart's legend: each trace in its colour, with its last value. */
@Composable
fun Legend(frame: Frame, traces: IntArray, names: List<String>, units: List<String>) {
    Text(
        buildAnnotatedString {
            traces.forEachIndexed { n, k ->
                if (n > 0) append("   ")
                withStyle(SpanStyle(color = TRACE_COLOURS[n])) {
                    val v = frame.last(k)
                    append(names[k])
                    append(if (v.isNaN()) "  —" else "  %.1f %s".format(v, units[k]))
                }
            }
        },
        style = MaterialTheme.typography.bodySmall,
    )
}

/**
 * One strip chart: [traces] over the window, scaled to what they span. Each column is drawn
 * from its minimum to its maximum, so a move shorter than a column still shows.
 */
@Composable
fun Strip(
    frame: Frame,
    title: String,
    traces: IntArray,
    names: List<String>,
    units: List<String>,
    windowUs: Double,
) {
    val tm = rememberTextMeasurer()
    Column(Modifier.padding(vertical = 4.dp)) {
        Text(title, style = MaterialTheme.typography.labelLarge)
        Legend(frame, traces, names, units)
        Canvas(Modifier.fillMaxWidth().height(88.dp)) {
            drawRect(PlotBack)
            var lo = Double.POSITIVE_INFINITY
            var hi = Double.NEGATIVE_INFINITY
            for (c in 0 until frame.columns) for (k in traces) {
                val a = frame.min(c, k)
                val b = frame.max(c, k)
                if (a.isFinite()) lo = minOf(lo, a)
                if (b.isFinite()) hi = maxOf(hi, b)
            }
            if (!lo.isFinite() || !hi.isFinite()) return@Canvas
            if (hi - lo < 1e-6) {
                lo -= 1.0
                hi += 1.0
            } else {
                val pad = (hi - lo) * 0.06
                lo -= pad
                hi += pad
            }
            val t0 = frame.nowUs - windowUs
            fun x(t: Double) = ((t - t0) / windowUs * size.width).toFloat()
            fun y(v: Double) = (size.height - (v - lo) / (hi - lo) * size.height).toFloat()
            traces.forEachIndexed { n, k ->
                val colour = TRACE_COLOURS[n]
                var prev: Offset? = null
                for (c in 0 until frame.columns) {
                    val a = frame.min(c, k)
                    val b = frame.max(c, k)
                    if (a.isNaN() || b.isNaN()) {
                        prev = null
                        continue
                    }
                    val xc = x(frame.time(c))
                    drawLine(colour, Offset(xc, y(b)), Offset(xc, y(a) + 0.5f), 2f)
                    val mid = Offset(xc, y((a + b) / 2))
                    prev?.let { drawLine(colour, it, mid, 1.5f) }
                    prev = mid
                }
            }
            val scale = TextStyle(color = Line, fontSize = 9.sp)
            drawText(textMeasurer = tm, text = "%.1f".format(hi), topLeft = Offset(3f, 1f), style = scale)
            val bottom = tm.measure("%.1f".format(lo), scale)
            drawText(bottom, topLeft = Offset(3f, size.height - bottom.size.height - 1f))
        }
    }
}

/**
 * The crank and cam signals, rebuilt from the crank angle of each column, and the one-shot
 * pulses of the ATU-II's channel 8, where the injectors are (TO8I–L).
 */
@Composable
fun Signals(frame: Frame, windowUs: Double, words: SignalWords) {
    val tm = rememberTextMeasurer()
    val outputs = sortedSetOf(8, 9, 10, 11)
    for (i in 0 until frame.pulses) outputs.add(frame.pulse(i).output)
    // Teeth come every 6°: past this window they would be more than one per column.
    val teeth = windowUs <= 200_000.0
    val lanes = outputs.size + if (teeth) 2 else 0
    val lane = 16.dp
    Column(Modifier.padding(vertical = 4.dp)) {
        Text(words.title, style = MaterialTheme.typography.labelLarge)
        Canvas(Modifier.fillMaxWidth().height(lane * lanes + 4.dp)) {
            val l = lane.toPx()
            val gutter = 44.dp.toPx()
            val plotW = size.width - gutter
            drawRect(PlotBack, Offset(gutter, 0f), Size(plotW, size.height))
            val t0 = frame.nowUs - windowUs
            fun x(t: Double) = (gutter + (t - t0) / windowUs * plotW).toFloat()
            val label = TextStyle(color = Light, fontSize = 10.sp)
            var row = 0
            fun name(text: String) {
                val r = tm.measure(text, label)
                drawText(r, topLeft = Offset(gutter - r.size.width - 4.dp.toPx(), 2f + row * l + (l - r.size.height) / 2f))
            }
            if (teeth) {
                for ((n, level) in listOf<(Double) -> Boolean>(
                    { a -> val d = a.mod(360.0); floor(d / 6.0) < 56 && d.mod(6.0) < 3.0 },
                    { a -> CAM.any { (a - it).mod(720.0) < CAM_WIDTH } },
                ).withIndex()) {
                    name(if (n == 0) words.crank else words.cam)
                    val top = 2f + row * l + 2f
                    val bottom = 2f + (row + 1) * l - 3f
                    var prev: Offset? = null
                    for (c in 0 until frame.columns) {
                        val a = frame.crankAt(c)
                        if (a.isNaN()) {
                            prev = null
                            continue
                        }
                        val p = Offset(x(frame.time(c)), if (level(a)) top else bottom)
                        prev?.let {
                            drawLine(Light, it, Offset(p.x, it.y), 1.5f)
                            if (it.y != p.y) drawLine(Light, Offset(p.x, it.y), p, 1.5f)
                        }
                        prev = p
                    }
                    row++
                }
            }
            for (o in outputs) {
                name(if (o in 8..11) words.injector.format(o - 7) else "TO8" + ('A' + o))
                val top = 2f + row * l + 1f
                for (i in 0 until frame.pulses) {
                    val p = frame.pulse(i)
                    if (p.output != o) continue
                    val xa = maxOf(x(p.startUs), gutter)
                    val xb = maxOf(minOf(x(p.endUs), size.width), xa + 1f)
                    drawRect(InjectorBlue, Offset(xa, top), Size(xb - xa, l - 4f))
                }
                row++
            }
        }
    }
}

/** What the signal lanes say, in the app's language. */
data class SignalWords(val title: String, val crank: String, val cam: String, val injector: String)
