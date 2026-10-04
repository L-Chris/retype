package io.github.retype.ime

import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.*

@Composable
fun StatisticsPage() {
    val context = LocalContext.current
    var period by remember { mutableIntStateOf(0) }
    var summary by remember { mutableStateOf<StatisticsSummary?>(null) }
    var recent by remember { mutableStateOf(Counts()) }
    LaunchedEffect(period) {
        summary = withContext(Dispatchers.IO) { TypingStatistics.get(context).summary(period) }
        recent = withContext(Dispatchers.IO) { TypingStatistics.get(context).recent() }
    }
    Text("手机端统计", style = MaterialTheme.typography.titleMedium)
    Row {
        listOf("天", "周", "月", "年").forEachIndexed { i, s ->
            TextButton(onClick = { period = i }) {
                Text(
                    s,
                    color =
                        if (period == i) MaterialTheme.colorScheme.primary
                        else MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
    SettingsCard {
        Text("最近 5 分钟", style = MaterialTheme.typography.titleMedium)
        Text("中文 ${recent.speed(true) ?: "—"} 字/分钟 · 英文 ${recent.speed(false) ?: "—"} 字符/分钟")
    }
    summary?.let { s ->
        SettingsCard {
            Text("打字数量", style = MaterialTheme.typography.titleMedium)
            Row {
                Column(Modifier.weight(1f)) {
                    Text("中文")
                    Text("${s.current.chinese}", style = MaterialTheme.typography.headlineMedium)
                }
                Column(Modifier.weight(1f)) {
                    Text("英文")
                    Text("${s.current.english}", style = MaterialTheme.typography.headlineMedium)
                }
            }
        }
        SettingsCard {
            var help by remember { mutableStateOf(false) }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("平均速度", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
                TextButton(onClick = { help = !help }) { Text("ⓘ") }
            }
            if (help)
                Text(
                    "均速 = 上屏字符总数 ÷ 有效输入时间；连续按键间隔超过 15 秒不计入时间，样本不足时不显示速度。",
                    style = MaterialTheme.typography.bodySmall,
                )
            Row {
                listOf(true, false).forEach { ch ->
                    Column(Modifier.weight(1f)) {
                        Text(if (ch) "中文" else "英文")
                        Text(
                            s.current.speed(ch)?.toString() ?: "—",
                            style = MaterialTheme.typography.headlineMedium,
                        )
                        Text(
                            if (ch) "字/分钟" else "字符/分钟",
                            style = MaterialTheme.typography.bodySmall,
                        )
                        val old = s.previous.speed(ch)
                        val current = s.current.speed(ch)
                        Text(
                            if (old != null && old > 0 && current != null)
                                "较上期 %+.0f%%".format((current - old) * 100.0 / old)
                            else "上期无可比数据",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
            val max =
                s.bars
                    .maxOfOrNull { maxOf(it.second.speed(true) ?: 0, it.second.speed(false) ?: 0) }
                    ?.coerceAtLeast(1) ?: 1
            var selected by remember { mutableStateOf<Pair<String, Counts>?>(null) }
            Row(
                Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).height(150.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.Bottom,
            ) {
                s.bars.forEach { b ->
                    Column(
                        Modifier.width(32.dp).clickable { selected = b },
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        Row(
                            Modifier.height(120.dp),
                            verticalAlignment = Alignment.Bottom,
                            horizontalArrangement = Arrangement.spacedBy(2.dp),
                        ) {
                            Box(
                                Modifier.width(12.dp)
                                    .height(((b.second.speed(true) ?: 0) * 120f / max).dp)
                                    .background(MaterialTheme.colorScheme.primary)
                            )
                            Box(
                                Modifier.width(12.dp)
                                    .height(((b.second.speed(false) ?: 0) * 120f / max).dp)
                                    .background(androidx.compose.ui.graphics.Color(0xFF73BAD2))
                            )
                        }
                        Text(b.first, style = MaterialTheme.typography.labelSmall)
                    }
                }
            }
            selected?.let {
                Text(
                    "${it.first} · 中文 ${it.second.speed(true) ?: "—"} · 英文 ${it.second.speed(false) ?: "—"}",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}
