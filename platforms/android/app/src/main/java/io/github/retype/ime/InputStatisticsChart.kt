package io.github.retype.ime

import android.graphics.Paint
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.input.key.*
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

private val revisionColor = Color(0xFFDA7B2F)
private fun percent(value: Double?) = value?.let { "%.1f%%".format(it) } ?: "—"
private fun bucketDetails(label: String, unit: String, row: LengthCounts, total: Long): String =
    "$label $unit/次\n输入 ${row.inputs} 次 · 占比 ${percent(if (total == 0L) 0.0 else row.inputs * 100.0 / total)}\n回改 ${row.corrected} 次 · 回改率 ${percent(row.rate())}\n平均上屏长度 ${row.mean()?.let { "%.1f $unit".format(it) } ?: "—"}${if (row.inputs in 1..4) "\n样本较少" else ""}"

@Composable
private fun Information(text: String) {
    var visible by remember { mutableStateOf(false) }
    Box {
        TextButton(onClick = { visible = !visible }, contentPadding = PaddingValues(4.dp)) { Text("ⓘ") }
        DropdownMenu(expanded = visible, onDismissRequest = { visible = false }) {
            Text(text, Modifier.widthIn(max = 280.dp).padding(12.dp), style = MaterialTheme.typography.bodySmall)
        }
    }
}

@Composable
internal fun InputStatisticsSection(input: InputMeasurements?, scope: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text("回改与上屏长度", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
        Information("当前时间与版本范围内取消输入 ${input?.cancelled ?: 0} 次。取消没有最终长度，不分配到图中。\n仅包含新口径启用后采集的样本。")
    }
    for (chinese in listOf(true, false)) {
        val rows = input?.grouped(chinese) ?: List(6) { LengthCounts() }
        val total = rows.fold(LengthCounts(), LengthCounts::plus)
        val unit = if (chinese) "字" else "词"
        SettingsCard {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(if (chinese) "中文输入 · 字/次" else "英文输入 · 词/次", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
                Information("$scope\n成功输入 ${total.inputs} 次 · 回改 ${total.corrected} 次\n总体回改率 ${percent(total.rate())}\n平均上屏长度 ${total.mean()?.let { "%.1f $unit/次".format(it) } ?: "—"}\n上屏前修改拼写，不代表输错率。")
            }
            InputLengthChart(rows, unit)
        }
    }
}

@Composable
internal fun InputLengthChart(rows: List<LengthCounts>, unit: String) {
    val barColor = MaterialTheme.colorScheme.primary
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    val total = rows.sumOf { it.inputs }
    var selected by remember(rows) { mutableStateOf<Int?>(null) }
    var tipAtTop by remember { mutableStateOf(true) }
    val description = rows.mapIndexed { i, row -> bucketDetails(inputLengthLabels[i], unit, row, total) }.joinToString("；")
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val chartWidth = maxOf(maxWidth, 300.dp)
        Box(Modifier.horizontalScroll(rememberScrollState())) {
            Box(Modifier.width(chartWidth).height(220.dp)) {
                Canvas(Modifier.fillMaxSize().semantics { contentDescription = description }.focusable()
                    .onPreviewKeyEvent { event ->
                        if (event.type != KeyEventType.KeyUp) false else when (event.key) {
                            Key.DirectionLeft -> { selected = ((selected ?: 1) - 1).coerceAtLeast(0); true }
                            Key.DirectionRight -> { selected = ((selected ?: -1) + 1).coerceAtMost(5); true }
                            Key.Escape -> { selected = null; true }
                            else -> false
                        }
                    }
                    .pointerInput(rows) {
                        fun select(point: Offset) {
                            val margin = 44.dp.toPx()
                            if (point.x < margin || point.x > size.width - margin || point.y < 30.dp.toPx() || point.y > size.height - 30.dp.toPx()) selected = null
                            else { selected = (((point.x - margin) * 6 / (size.width - margin * 2)).toInt()).coerceIn(0, 5); tipAtTop = point.y > size.height / 2 }
                        }
                        detectTapGestures(onTap = { select(it) })
                    }
                    .pointerInput(rows) {
                        fun select(point: Offset) {
                            val margin = 44.dp.toPx()
                            selected = (((point.x - margin) * 6 / (size.width - margin * 2)).toInt()).coerceIn(0, 5)
                            tipAtTop = point.y > size.height / 2
                        }
                        detectDragGestures(onDragStart = { select(it) }) { change, _ -> change.consume(); select(change.position) }
                    }) {
                    val margin = 44.dp.toPx(); val top = 30.dp.toPx(); val bottom = size.height - top
                    val width = size.width - margin * 2; val height = bottom - top; val step = width / 6
                    val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply { textSize = 10.sp.toPx() }
                    fun text(value: String, x: Float, y: Float, color: Color, align: Paint.Align) {
                        paint.color = color.toArgb(); paint.textAlign = align; drawContext.canvas.nativeCanvas.drawText(value, x, y, paint)
                    }
                    text("输入占比", 0f, 12.sp.toPx(), barColor, Paint.Align.LEFT)
                    text("回改率", size.width, 12.sp.toPx(), revisionColor, Paint.Align.RIGHT)
                    for (tick in 0..4) {
                        val f = tick / 4f; val y = bottom - height * f
                        drawLine(muted.copy(alpha = .14f), Offset(margin, y), Offset(size.width - margin, y))
                        val label = "%.0f%%".format(f * 100)
                        text(label, margin - 6.dp.toPx(), y + 3.sp.toPx(), muted, Paint.Align.RIGHT)
                        text("%.0f%%".format(f * 100), size.width - margin + 6.dp.toPx(), y + 3.sp.toPx(), revisionColor, Paint.Align.LEFT)
                    }
                    var previous: Offset? = null
                    rows.forEachIndexed { i, row ->
                        val x = margin + step * (i + .5f)
                        if (selected == i) drawRect(barColor.copy(alpha = .06f), Offset(x - step / 2, top), Size(step, height))
                        if (row.inputs > 0) {
                            val h = height * (row.inputs.toDouble() / total.coerceAtLeast(1)).toFloat()
                            drawRoundRect(barColor.copy(alpha = if (selected == i) 1f else .7f), Offset(x - step * .24f, bottom - h), Size(step * .48f, h), CornerRadius(3.dp.toPx()))
                            val point = Offset(x, bottom - height * row.corrected.toFloat() / row.inputs)
                            previous?.let { drawLine(revisionColor, it, point, 2.dp.toPx()) }
                            drawCircle(revisionColor, (if (selected == i) 4.5f else 3f).dp.toPx(), point); previous = point
                        } else previous = null
                        text(inputLengthLabels[i], x, size.height - 8.dp.toPx(), muted, Paint.Align.CENTER)
                    }
                    if (total == 0L) text("暂无新口径样本", size.width / 2, size.height / 2, muted, Paint.Align.CENTER)
                }
                selected?.let { i ->
                    Surface(Modifier.align(if (tipAtTop) Alignment.TopCenter else Alignment.BottomCenter).padding(vertical = 28.dp, horizontal = 32.dp), shape = MaterialTheme.shapes.small, shadowElevation = 3.dp, tonalElevation = 3.dp) {
                        Text(bucketDetails(inputLengthLabels[i], unit, rows[i], total), Modifier.padding(10.dp), style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        }
    }
}
