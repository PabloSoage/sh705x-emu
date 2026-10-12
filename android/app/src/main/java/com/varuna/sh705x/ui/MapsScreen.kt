package com.varuna.sh705x.ui

import android.content.Context
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.varuna.sh705x.MapData
import com.varuna.sh705x.Native
import com.varuna.sh705x.R
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.cos
import kotlin.math.sin
import kotlin.math.sqrt

/** Rows of the list shown at once. */
private const val ROWS = 80

/** How a map is drawn. */
private enum class Mode { HEAT, SURFACE, TABLE }

/** A colour for [t] in 0..1: blue, teal, green, yellow, red (as the desktop window's). */
private fun heat(t: Double): Color {
    val stops = listOf(
        0.0 to Color(0xFF283CA0),
        0.25 to Color(0xFF1E96AA),
        0.5 to Color(0xFF46B450),
        0.75 to Color(0xFFE6C832),
        1.0 to Color(0xFFD23C28),
    )
    val x = if (t.isNaN()) 0.0 else t.coerceIn(0.0, 1.0)
    val k = stops.indexOfFirst { it.first >= x }.coerceIn(1, stops.size - 1)
    val (t0, a) = stops[k - 1]
    val (t1, b) = stops[k]
    val f = ((x - t0) / (t1 - t0)).toFloat()
    return Color(
        red = a.red + (b.red - a.red) * f,
        green = a.green + (b.green - a.green) * f,
        blue = a.blue + (b.blue - a.blue) * f,
    )
}

/** A short number: few decimals for big values. */
private fun num(v: Double): String = when {
    !v.isFinite() -> "-"
    kotlin.math.abs(v) >= 1000 -> "%.0f".format(v)
    kotlin.math.abs(v) >= 100 -> "%.1f".format(v)
    kotlin.math.abs(v) >= 1 -> "%.2f".format(v)
    else -> "%.3f".format(v)
}

/** One row of the list. */
private data class Row(val desc: Int, val dims: Int, val nx: Int, val ny: Int, val calls: Long, val last: Double, val name: String)

private fun rows(text: String): List<Row> = text.lineSequence().mapNotNull { l ->
    val c = l.split('\t')
    if (c.size < 7) return@mapNotNull null
    Row(
        c[0].toIntOrNull(16) ?: return@mapNotNull null,
        c[1].toIntOrNull() ?: 0,
        c[2].toIntOrNull() ?: 0,
        c[3].toIntOrNull() ?: 0,
        c[4].toLongOrNull() ?: 0,
        c[5].toDoubleOrNull() ?: Double.NaN,
        c[6],
    )
}.toList()

private suspend fun readText(context: Context, uri: Uri): String = withContext(Dispatchers.IO) {
    context.contentResolver.openInputStream(uri)?.use { it.readBytes().decodeToString() }.orEmpty()
}

/** The calibration maps the firmware reads, as it reads them. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun MapsScreen() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var filter by remember { mutableStateOf("") }
    var follow by remember { mutableStateOf(false) }
    var selected by remember { mutableStateOf<Int?>(null) }
    var mode by remember { mutableStateOf(Mode.HEAT) }
    var status by remember { mutableStateOf("") }
    var list by remember { mutableStateOf(emptyList<Row>()) }
    var data by remember { mutableStateOf<MapData?>(null) }
    var text by remember { mutableStateOf("") }
    var links by remember { mutableStateOf(emptyList<Int>()) }
    var note by remember { mutableStateOf<String?>(null) }
    var profileText by remember { mutableStateOf<String?>(null) }
    var profiles by remember { mutableStateOf(emptyList<String>()) }
    var tapped by remember { mutableStateOf<String?>(null) }
    var stopping by remember { mutableStateOf(false) }
    // While naming: the names part of the status it started from; the note goes when it changes.
    var namingFrom by remember { mutableStateOf<String?>(null) }
    val namesOf = { st: String -> st.substringAfter("names:", "") }

    LaunchedEffect(Unit) {
        var tick = 0
        while (true) {
            status = Native.mapsStatus()
            namingFrom?.let { from ->
                if (namesOf(status) != from) {
                    namingFrom = null
                    note = null
                }
            }
            list = rows(Native.mapList(filter))
            if (follow) list.firstOrNull()?.let { selected = it.desc }
            selected?.let { d ->
                val m = MapData(Native.mapData(d))
                data = m.takeIf { it.valid }
                if (tick % 4 == 0) {
                    text = Native.mapText(d)
                    links = Native.mapSources(d).lines().mapNotNull { it.toIntOrNull(16) }
                }
            }
            tick++
            delay(150)
        }
    }

    val pickProfile = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) scope.launch {
            val t = readText(context, uri)
            profileText = t
            profiles = Native.profileNames(t).lines().filter { it.isNotBlank() }
            note = context.getString(R.string.profiles_read, profiles.size)
        }
    }
    val pickLabels = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) scope.launch {
            val n = Native.labelMaps(readText(context, uri))
            note = context.getString(R.string.labels_read, n)
        }
    }

    Text(status.ifEmpty { stringResource(R.string.maps_starting) }, style = MaterialTheme.typography.bodySmall)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedButton(onClick = {
            Native.nameSensors()
            note = context.getString(R.string.naming)
                    namingFrom = namesOf(status)
        }) { Text(stringResource(R.string.find_sensors)) }
        OutlinedButton(onClick = { pickProfile.launch(arrayOf("*/*")) }) { Text(stringResource(R.string.read_profile)) }
        OutlinedButton(onClick = { pickLabels.launch(arrayOf("*/*")) }) { Text(stringResource(R.string.read_labels)) }
    }
    if (profiles.isNotEmpty()) {
        Text(stringResource(R.string.name_from), style = MaterialTheme.typography.labelLarge)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            profiles.forEachIndexed { k, name ->
                FilterChip(selected = false, onClick = {
                    profileText?.let { Native.nameFromProfile(it, k) }
                    note = context.getString(R.string.naming)
                    namingFrom = namesOf(status)
                }, label = { Text(name) })
            }
        }
    }
    note?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
    Text(stringResource(R.string.names_private), style = MaterialTheme.typography.bodySmall)

    val m = data
    val sel = selected
    if (sel != null && m != null) {
        Text("%06X · %s".format(sel, if (m.dims == 3) "${m.nx}×${m.ny}" else "${m.nx}"), style = MaterialTheme.typography.titleMedium)
        Text(
            if (m.dims == 3) stringResource(R.string.last_lookup_3d, num(m.lastX), num(m.lastY), num(m.lastOut))
            else stringResource(R.string.last_lookup_2d, num(m.lastX), num(m.lastOut)),
            style = MaterialTheme.typography.bodyMedium,
        )
        text.lines().filter { it.isNotBlank() }.forEach { Text(it, style = MaterialTheme.typography.bodySmall) }
        if (links.isNotEmpty()) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                links.forEach { d ->
                    OutlinedButton(onClick = { selected = d; follow = false }) { Text("%06X".format(d)) }
                }
            }
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Mode.entries.filter { m.dims == 3 || it != Mode.SURFACE }.forEach { md ->
                FilterChip(
                    selected = mode == md,
                    onClick = { mode = md },
                    label = {
                        Text(
                            stringResource(
                                when (md) {
                                    Mode.HEAT -> if (m.dims == 3) R.string.heat_map else R.string.curve
                                    Mode.SURFACE -> R.string.surface
                                    Mode.TABLE -> R.string.table
                                },
                            ),
                        )
                    },
                )
            }
            Button(onClick = {
                if (stopping) {
                    Native.breakOnMap(-1)
                } else {
                    Native.breakOnMap(sel)
                    Native.run(true)
                }
                stopping = !stopping
            }) { Text(stringResource(if (stopping) R.string.stop_cancel else R.string.stop_at_lookup)) }
        }
        when {
            mode == Mode.TABLE -> Table(m)
            m.dims == 2 -> Curve(m)
            mode == Mode.SURFACE -> Surface(m)
            else -> HeatMap(m) { tapped = it }
        }
        tapped?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
    }

    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(
            value = filter,
            onValueChange = { filter = it },
            label = { Text(stringResource(R.string.filter)) },
            singleLine = true,
            modifier = Modifier.weight(1f),
        )
        Text(stringResource(R.string.follow))
        Switch(checked = follow, onCheckedChange = { follow = it })
    }
    list.take(ROWS).forEach { r ->
        val size = if (r.dims == 3) "${r.nx}×${r.ny}" else "${r.nx}"
        Text(
            "%06X %6s %7d → %s  %s".format(r.desc, size, r.calls, num(r.last), r.name),
            fontFamily = FontFamily.Monospace,
            fontSize = 12.sp,
            modifier = Modifier
                .fillMaxWidth()
                .background(if (r.desc == selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent)
                .clickable {
                    selected = r.desc
                    follow = false
                    tapped = null
                }
                .padding(vertical = 3.dp),
        )
    }
}

/** The cells coloured by value, where the firmware is and has been; a tap shows a cell. */
@Composable
private fun HeatMap(m: MapData, onCell: (String) -> Unit) {
    val (lo, hi) = m.span()
    Canvas(
        Modifier
            .fillMaxWidth()
            .aspectRatio(if (m.dims == 3) 1.1f else 2f)
            .pointerInput(m.nx, m.rows) {
                detectTapGestures { p ->
                    val i = (p.x / size.width * m.nx).toInt().coerceIn(0, m.nx - 1)
                    val j = (p.y / size.height * m.rows).toInt().coerceIn(0, m.rows - 1)
                    onCell(
                        if (m.dims == 3) "X ${num(m.x(i))} · Y ${num(m.y(j))}: ${num(m.z(i, j))}"
                        else "X ${num(m.x(i))}: ${num(m.z(i, j))}",
                    )
                }
            },
    ) {
        val cw = size.width / m.nx
        val ch = size.height / m.rows
        for (j in 0 until m.rows) for (i in 0 until m.nx) {
            drawRect(heat((m.z(i, j) - lo) / (hi - lo)), Offset(i * cw, j * ch), Size(cw - 1f, ch - 1f))
        }
        fun at(x: Double, y: Double): Offset {
            val px = MapData.position(m.nx, m::x, x)
            val py = if (m.dims == 3) MapData.position(m.ny, m::y, y) else 0.0
            return Offset(((px + 0.5) * cw).toFloat(), ((py + 0.5) * ch).toFloat())
        }
        var prev: Offset? = null
        for (k in 0 until m.trail) {
            val (x, y, _) = m.trailAt(k)
            val p = at(x, y)
            prev?.let { drawLine(Color.White.copy(alpha = 0.55f), it, p, 3f) }
            prev = p
        }
        if (m.lastX.isFinite()) {
            val c = at(m.lastX, m.lastY)
            drawCircle(Color.White, 9.dp.toPx(), c, style = Stroke(3.dp.toPx()))
        }
    }
}

/** A curve: its points, where the firmware is and has been. */
@Composable
private fun Curve(m: MapData) {
    val (lo, hi) = m.span()
    val tm = rememberTextMeasurer()
    Canvas(Modifier.fillMaxWidth().aspectRatio(1.8f).background(Color(0xFF161616))) {
        val x0 = m.x(0)
        val x1 = m.x(m.nx - 1)
        val xs = if (x1 > x0) x1 - x0 else 1.0
        fun pos(x: Double, z: Double) = Offset(
            ((x.coerceIn(x0, x1) - x0) / xs * size.width).toFloat(),
            (size.height - (z - lo) / (hi - lo) * size.height).toFloat(),
        )
        var prev: Offset? = null
        for (i in 0 until m.nx) {
            val p = pos(m.x(i), m.z(i, 0))
            prev?.let { drawLine(Color(0xFF6EAAFF), it, p, 4f) }
            drawCircle(Color(0xFF6EAAFF), 4f, p)
            prev = p
        }
        for (k in 0 until m.trail) {
            val (x, _, z) = m.trailAt(k)
            drawCircle(Color.White.copy(alpha = 0.5f), 4f, pos(x, z))
        }
        if (m.lastX.isFinite()) drawCircle(Color.White, 9.dp.toPx(), pos(m.lastX, m.lastOut), style = Stroke(3.dp.toPx()))
        val style = TextStyle(color = Color.Gray, fontSize = 10.sp)
        drawText(textMeasurer = tm, text = num(hi), topLeft = Offset(4f, 2f), style = style)
        drawText(textMeasurer = tm, text = num(lo), topLeft = Offset(4f, size.height - 34f), style = style)
        drawText(
            textMeasurer = tm,
            text = "X ${num(x0)} … ${num(x1)}",
            topLeft = Offset(size.width / 2, size.height - 34f),
            style = style,
        )
    }
}

/** The map as a surface, to turn by dragging. */
@Composable
private fun Surface(m: MapData) {
    var yaw by remember { mutableFloatStateOf(-0.6f) }
    var pitch by remember { mutableFloatStateOf(0.55f) }
    val (lo, hi) = m.span()
    Canvas(
        Modifier
            .fillMaxWidth()
            .aspectRatio(1.1f)
            .background(Color(0xFF121212))
            .pointerInput(Unit) {
                detectDragGestures { change, d ->
                    change.consume()
                    yaw += d.x * 0.01f
                    pitch = (pitch + d.y * 0.01f).coerceIn(0.05f, 1.45f)
                }
            },
    ) {
        val (cy, sy, cp, sp) = listOf(cos(yaw), sin(yaw), cos(pitch), sin(pitch))
        val scale = minOf(size.width, size.height * 1.4f) * 0.42f
        val centre = Offset(size.width / 2, size.height / 2)
        fun project(x: Float, y: Float, z: Float): Pair<Offset, Float> {
            val (px, py, pz) = Triple(x - 0.5f, y - 0.5f, (z - 0.5f) * 0.6f)
            val rx = px * cy - py * sy
            val ry = px * sy + py * cy
            return Offset(centre.x + rx * scale, centre.y + (ry * sp - pz * cp) * scale) to (ry * cp + pz * sp)
        }
        val fx = { i: Int -> if (m.nx > 1) i.toFloat() / (m.nx - 1) else 0.5f }
        val fy = { j: Int -> if (m.ny > 1) j.toFloat() / (m.ny - 1) else 0.5f }
        val fz = { z: Double -> ((z - lo) / (hi - lo)).toFloat() }
        data class Quad(val depth: Float, val pts: List<Offset>, val colour: Color)
        val quads = ArrayList<Quad>()
        for (j in 0 until m.ny - 1) for (i in 0 until m.nx - 1) {
            val corners = listOf(i to j, i + 1 to j, i + 1 to j + 1, i to j + 1)
            var depth = 0f
            var zs = 0.0
            val pts = corners.map { (a, b) ->
                val z = m.z(a, b)
                zs += z
                val (q, d) = project(fx(a), fy(b), fz(z))
                depth += d
                q
            }
            // Shaded by how the cell faces the light.
            val dx = (fz(m.z(i + 1, j)) - fz(m.z(i, j))) * 0.6f / (fx(i + 1) - fx(i)).coerceAtLeast(1e-6f)
            val dy = (fz(m.z(i, j + 1)) - fz(m.z(i, j))) * 0.6f / (fy(j + 1) - fy(j)).coerceAtLeast(1e-6f)
            val light = 0.55f + 0.45f / sqrt(1f + dx * dx + dy * dy)
            val c = heat((zs / 4 - lo) / (hi - lo))
            quads += Quad(depth, pts, Color(c.red * light, c.green * light, c.blue * light))
        }
        quads.sortByDescending { it.depth }
        val edge = Color.Black.copy(alpha = 0.35f)
        for (q in quads) {
            val path = Path().apply {
                moveTo(q.pts[0].x, q.pts[0].y)
                for (k in 1..3) lineTo(q.pts[k].x, q.pts[k].y)
                close()
            }
            drawPath(path, q.colour)
            drawPath(path, edge, style = Stroke(1f))
        }
        if (m.lastX.isFinite()) {
            val px = (MapData.position(m.nx, m::x, m.lastX) / (maxOf(m.nx, 2) - 1)).toFloat()
            val py = (MapData.position(m.ny, m::y, m.lastY) / (maxOf(m.ny, 2) - 1)).toFloat()
            val (top, _) = project(px, py, fz(m.lastOut))
            val (base, _) = project(px, py, 0f)
            drawLine(Color.White, base, top, 3f)
            drawCircle(Color.White, 8.dp.toPx(), top, style = Stroke(3.dp.toPx()))
        }
    }
}

/** The numbers, with the cells the last lookup interpolated between marked. */
@Composable
private fun Table(m: MapData) {
    fun near(n: Int, at: (Int) -> Double, v: Double): Pair<Int, Int> {
        val p = MapData.position(n, at, v)
        val i = p.toInt()
        return i to if (p > i) i + 1 else i
    }
    val (i0, i1) = near(m.nx, m::x, m.lastX)
    val (j0, j1) = if (m.dims == 3) near(m.ny, m::y, m.lastY) else 0 to 0
    val mono = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 11.sp)
    Column(Modifier.horizontalScroll(rememberScrollState())) {
        Row {
            Text(if (m.dims == 3) "Y \\ X" else "X", style = mono, modifier = Modifier.padding(4.dp))
            for (i in 0 until m.nx) Text(num(m.x(i)), style = mono.copy(color = Color.Gray), modifier = Modifier.padding(4.dp))
        }
        for (j in 0 until m.rows) {
            Row {
                Text(if (m.dims == 3) num(m.y(j)) else "Z", style = mono.copy(color = Color.Gray), modifier = Modifier.padding(4.dp))
                for (i in 0 until m.nx) {
                    val used = m.lastX.isFinite() && (i == i0 || i == i1) && (j == j0 || j == j1)
                    Text(
                        num(m.z(i, j)),
                        style = mono,
                        modifier = Modifier
                            .background(if (used) Color(0xFF5A5014) else Color.Transparent)
                            .padding(4.dp),
                    )
                }
            }
        }
    }
}
