package io.github.retype.ime

import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.*

@Composable
fun SettingsScreen(startDictionary: Boolean = false, downloadResources: Boolean = false) {
  val context = LocalContext.current
  val store = remember { AppStore(context) }
  val scope = rememberCoroutineScope()
  var page by rememberSaveable { mutableIntStateOf(if (startDictionary) 1 else -1) }
  LaunchedEffect(Unit) {
    BaseResources.observe(context)
    if (downloadResources) BaseResources.start(context)
  }
  var revision by remember { mutableIntStateOf(0) }
  var busy by remember { mutableStateOf(false) }
  var message by remember { mutableStateOf<String?>(null) }
  val pages = listOf("输入", "词库", "统计", "AI 提供商", "翻译", "云同步", "关于", "跨设备", "语音输入")
  val descriptions =
      listOf(
          "全拼、双拼、英文补全",
          "专业词汇、名人及常用人名",
          "打字数量与平均速度",
          "模型、接口与凭据",
          "目标语言与思考等级",
          "跨设备同步设置与数据",
          "版本与开源许可", "局域网文字剪贴板同步", "识别模型与麦克风")
  BackHandler(page >= 0) {
    page = -1
    message = null
  }
  fun work(operation: suspend () -> String) {
    if (busy) return
    busy = true
    message = null
    scope.launch {
      try {
        message = operation()
        revision++
      } catch (e: CancellationException) {
        throw e
      } catch (e: Exception) {
        message = e.message ?: "操作失败"
      } finally {
        busy = false
      }
    }
  }
  Scaffold(
      containerColor = if (page < 0) Color(0xFFE7F3F1) else Color(0xFFF3F4F4),
      topBar = {
        Box(Modifier.statusBarsPadding().fillMaxWidth().height(80.dp)) {
          if (page < 0) {
            Row(
                Modifier.align(Alignment.CenterStart).padding(horizontal = 20.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                  Image(painterResource(R.drawable.ic_retype), null, Modifier.size(40.dp))
                  Text("retype 输入法", fontSize = 24.sp, fontWeight = FontWeight.SemiBold)
                }
          } else {
            IconButton(
                onClick = {
                  page = -1
                  message = null
                },
                modifier = Modifier.align(Alignment.CenterStart).padding(start = 8.dp)) {
                  Chevron(back = true)
                }
            Text(
                pages[page],
                Modifier.align(Alignment.Center),
                fontSize = 20.sp,
                fontWeight = FontWeight.SemiBold)
          }
        }
      }) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
          if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
          message?.let {
            Text(
                it,
                Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
                style = MaterialTheme.typography.bodySmall,
            )
          }
          // Connection tests must preserve the cloud page's unsaved form and toggle.
          key(page, if (page == 5) 0 else revision) {
            Column(
                Modifier.fillMaxSize()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
              if (page < 0 || page == 1) BaseResourceSection()
              if (page < 0) {
                Text(
                    "设置",
                    Modifier.padding(top = 12.dp, bottom = 4.dp),
                    fontSize = 24.sp,
                    fontWeight = FontWeight.SemiBold)
                pages.chunked(2).forEachIndexed { row, titles ->
                  Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    titles.forEachIndexed { column, title ->
                      val index = row * 2 + column
                      Card(
                          onClick = {
                            page = index
                            message = null
                          },
                          modifier = Modifier.weight(1f),
                          shape = RoundedCornerShape(20.dp),
                          colors = CardDefaults.cardColors(containerColor = Color.White)) {
                            Column(
                                Modifier.fillMaxWidth().padding(18.dp),
                                verticalArrangement = Arrangement.spacedBy(12.dp)) {
                                  SettingsGlyph(if (index == 7) 8 else if (index == 8) 9 else index)
                                  Spacer(Modifier.height(8.dp))
                                  Row(verticalAlignment = Alignment.CenterVertically) {
                                    Text(
                                        title,
                                        Modifier.weight(1f),
                                        fontSize = 17.sp,
                                        fontWeight = FontWeight.SemiBold)
                                    Chevron()
                                  }
                                  Text(
                                      descriptions[index],
                                      Modifier.heightIn(min = 40.dp),
                                      fontSize = 13.sp,
                                      lineHeight = 20.sp,
                                      color = Color(0xFF858D8C))
                                }
                          }
                    }
                    if (titles.size == 1) Spacer(Modifier.weight(1f))
                  }
                }
              } else
                  when (page) {
                    0 -> InputPage(store)
                    1 -> PacksPage(busy, ::work)
                    2 -> StatisticsPage()
                    3 -> ProvidersPage(store, busy, ::work)
                    4 -> TranslationPage(store)
                    5 -> CloudPage(store, busy, ::work)
                    7 -> LanPage()
                    8 -> VoicePage(store)
                    else -> AboutPage()
                  }
              Spacer(Modifier.height(24.dp))
            }
          }
        }
      }
}

@Composable
private fun Chevron(back: Boolean = false, down: Boolean = false) {
  Canvas(
      Modifier.size(if (back) 26.dp else 18.dp).semantics { if (back) contentDescription = "返回" }) {
        val path =
            Path().apply {
              if (down) {
                moveTo(size.width * .25f, size.height * .4f)
                lineTo(size.width * .5f, size.height * .65f)
                lineTo(size.width * .75f, size.height * .4f)
              } else {
                val a = if (back) .65f else .4f
                val b = if (back) .35f else .65f
                moveTo(size.width * a, size.height * .2f)
                lineTo(size.width * b, size.height * .5f)
                lineTo(size.width * a, size.height * .8f)
              }
            }
        drawPath(path, Color(0xFF697271), style = Stroke(2.dp.toPx(), cap = StrokeCap.Round))
      }
}

@Composable
internal fun SettingsGlyph(index: Int, modifier: Modifier = Modifier.size(30.dp)) {
  val ink = MaterialTheme.colorScheme.onSurfaceVariant
  Canvas(modifier) {
    val stroke = 1.8.dp.toPx()
    fun point(x: Float, y: Float) = Offset(size.width * x / 24f, size.height * y / 24f)
    fun line(a: Float, b: Float, c: Float, d: Float) =
        drawLine(ink, point(a, b), point(c, d), stroke, StrokeCap.Round)
    fun box(a: Float, b: Float, c: Float, d: Float) =
        drawRoundRect(
            ink,
            point(a, b),
            androidx.compose.ui.geometry.Size(
                size.width * (c - a) / 24f, size.height * (d - b) / 24f),
            androidx.compose.ui.geometry.CornerRadius(2.dp.toPx()),
            style = Stroke(stroke))
    when (index) {
      9 -> { box(9f, 2f, 15f, 15f); line(6f, 10f, 6f, 16f); line(18f, 10f, 18f, 16f); line(6f, 16f, 12f, 19f); line(18f, 16f, 12f, 19f); line(12f, 19f, 12f, 22f); line(8f, 22f, 16f, 22f) }
      0 -> {
        box(2f, 5f, 22f, 19f)
        for (y in listOf(9f, 12f)) for (x in listOf(6f, 10f, 14f, 18f)) line(x, y, x + .3f, y)
        line(7f, 16f, 17f, 16f)
      }
      1 -> {
        box(4f, 3f, 20f, 21f)
        line(8f, 3f, 8f, 21f)
        line(12f, 8f, 17f, 8f)
        line(12f, 12f, 17f, 12f)
      }
      2 -> {
        line(4f, 20f, 20f, 20f)
        line(6f, 16f, 6f, 12f)
        line(12f, 16f, 12f, 7f)
        line(18f, 16f, 18f, 3f)
      }
      3 -> {
        box(5f, 5f, 19f, 19f)
        for (x in listOf(8f, 12f, 16f)) {
          line(x, 2f, x, 5f)
          line(x, 19f, x, 22f)
          line(2f, x, 5f, x)
          line(19f, x, 22f, x)
        }
        line(9f, 10f, 15f, 10f)
        line(9f, 14f, 13f, 14f)
      }
      4 -> {
        line(2f, 5f, 13f, 5f)
        line(7f, 2f, 7f, 5f)
        line(10f, 5f, 4f, 14f)
        line(4f, 8f, 11f, 14f)
        line(12f, 21f, 17f, 10f)
        line(17f, 10f, 22f, 21f)
        line(14f, 17f, 20f, 17f)
      }
      5 -> {
        val path =
            Path().apply {
              moveTo(size.width * .18f, size.height * .62f)
              cubicTo(0f, 0f, size.width * .65f, 0f, size.width * .72f, size.height * .38f)
              cubicTo(
                  size.width * 1.1f,
                  size.height * .38f,
                  size.width * 1.1f,
                  size.height * .78f,
                  size.width * .72f,
                  size.height * .78f)
            }
        drawPath(path, ink, style = Stroke(stroke, cap = StrokeCap.Round))
        line(5f, 16f, 5f, 22f)
        line(2f, 19f, 5f, 22f)
        line(5f, 22f, 8f, 19f)
        line(13f, 22f, 13f, 16f)
        line(10f, 19f, 13f, 16f)
        line(13f, 16f, 16f, 19f)
      }
      8 -> {
        box(2f, 3f, 17f, 15f)
        line(5f, 19f, 13f, 19f)
        line(9f, 15f, 9f, 19f)
        box(15f, 9f, 22f, 22f)
      }
      7 -> {
        drawCircle(ink, size.width * .3f, style = Stroke(stroke))
        drawCircle(ink, size.width * .11f, style = Stroke(stroke))
        for (i in 0 until 8) {
          val a = i * Math.PI / 4
          val x = kotlin.math.cos(a).toFloat()
          val y = kotlin.math.sin(a).toFloat()
          drawLine(ink, center + Offset(x, y) * size.width * .3f,
              center + Offset(x, y) * size.width * .43f, stroke, StrokeCap.Round)
        }
      }
      else -> {
        drawCircle(ink, size.width * .4f, style = Stroke(stroke))
        line(12f, 11f, 12f, 17f)
        drawCircle(ink, stroke * .65f, point(12f, 7f))
      }
    }
  }
}

enum class ActionIcon {
  Plus,
  Trash,
  Refresh,
  Bolt,
  Check,
  Close,
  Sliders,
  Edit,
}

@Composable
internal fun ActionIconButton(
    icon: ActionIcon,
    description: String,
    enabled: Boolean = true,
    primary: Boolean = false,
    danger: Boolean = false,
    onClick: () -> Unit,
) {
  val bg =
      when {
        primary -> Color(0xFF138F96)
        danger -> Color(0xFFFDF0F0)
        else -> Color(0xFFF4F7F8)
      }
  val fg =
      when {
        primary -> Color.White
        danger -> Color(0xFFD13D3D)
        else -> Color(0xFF142B3B)
      }
  Surface(
      onClick = onClick,
      enabled = enabled,
      shape = RoundedCornerShape(10.dp),
      color = if (enabled) bg else bg.copy(alpha = 0.45f),
      border =
          if (primary) null
          else
              BorderStroke(
                  1.dp,
                  if (danger) Color(0xFFEECECE) else Color(0xFFE3E9EC),
              ),
      modifier = Modifier.size(38.dp).semantics { contentDescription = description },
  ) {
    Box(contentAlignment = Alignment.Center) {
      Canvas(Modifier.size(16.dp)) {
        val color = if (enabled) fg else fg.copy(alpha = 0.45f)
        val stroke = 1.8.dp.toPx()
        fun pt(x: Float, y: Float) = Offset(size.width * x / 16f, size.height * y / 16f)
        fun seg(x1: Float, y1: Float, x2: Float, y2: Float) =
            drawLine(color, pt(x1, y1), pt(x2, y2), stroke, StrokeCap.Round)
        when (icon) {
          ActionIcon.Plus -> {
            seg(3f, 8f, 13f, 8f)
            seg(8f, 3f, 8f, 13f)
          }
          ActionIcon.Trash -> {
            seg(2.5f, 4f, 13.5f, 4f)
            seg(6f, 2.2f, 10f, 2.2f)
            drawRoundRect(
                color,
                pt(4f, 4f),
                androidx.compose.ui.geometry.Size(size.width * 8f / 16f, size.height * 9.5f / 16f),
                androidx.compose.ui.geometry.CornerRadius(1.5.dp.toPx()),
                style = Stroke(stroke),
            )
          }
          ActionIcon.Refresh -> {
            drawCircle(color, size.width * 5f / 16f, style = Stroke(stroke))
            seg(10f, 2.5f, 13.5f, 3f)
            seg(13.5f, 3f, 12.8f, 6.5f)
          }
          ActionIcon.Bolt -> {
            seg(2.5f, 9.5f, 6.2f, 4.5f)
            seg(6.2f, 4.5f, 9.8f, 10.5f)
            seg(9.8f, 10.5f, 13.5f, 5.5f)
          }
          ActionIcon.Check -> {
            seg(3f, 8.5f, 6.5f, 12f)
            seg(6.5f, 12f, 13.5f, 4.5f)
          }
          ActionIcon.Close -> {
            seg(3.5f, 3.5f, 12.5f, 12.5f)
            seg(3.5f, 12.5f, 12.5f, 3.5f)
          }
          ActionIcon.Sliders -> {
            seg(2.5f, 5f, 13.5f, 5f)
            seg(2.5f, 11f, 13.5f, 11f)
            drawCircle(color, 2.2.dp.toPx(), pt(6f, 5f))
            drawCircle(color, 2.2.dp.toPx(), pt(10f, 11f))
          }
          ActionIcon.Edit -> {
            seg(3.5f, 12.5f, 12f, 4f)
            seg(3f, 13f, 6f, 12.5f)
            seg(3f, 13f, 3.5f, 10f)
          }
        }
      }
    }
  }
}

@Composable
fun SettingsCard(content: @Composable ColumnScope.() -> Unit) {
  Card(
      Modifier.fillMaxWidth(),
      shape = RoundedCornerShape(20.dp),
      colors = CardDefaults.cardColors(containerColor = Color.White)) {
        Column(
            Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
            content = content,
        )
      }
}

@Composable
fun ToggleRow(title: String, value: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
  Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
    Text(
        title,
        Modifier.weight(1f).padding(end = 12.dp),
        style = MaterialTheme.typography.titleMedium)
    Switch(value, onCheckedChange = onChange, enabled = enabled)
  }
}

@Composable
fun Choice(
    label: String,
    options: List<Pair<String, String>>,
    value: String,
    onChange: (String) -> Unit,
) {
  var expanded by remember { mutableStateOf(false) }
  Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
    Text(label, style = MaterialTheme.typography.labelLarge)
    Box {
      OutlinedButton(
          onClick = { expanded = true },
          modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp),
      ) {
        Text(
            options.firstOrNull { it.first == value }?.second ?: value.ifBlank { "请选择" },
            Modifier.weight(1f),
        )
        Chevron(down = true)
      }
      DropdownMenu(expanded, onDismissRequest = { expanded = false }) {
        options.forEach { (id, title) ->
          DropdownMenuItem(
              text = { Text(title) },
              onClick = {
                expanded = false
                onChange(id)
              },
          )
        }
      }
    }
  }
}

@Composable
private fun InputPage(store: AppStore) {
  var symbols by remember { mutableStateOf(store.prefs.getBoolean("symbolCompletion", true)) }
  var flypy by remember { mutableStateOf(store.prefs.getBoolean("flypy", false)) }
  var english by remember { mutableStateOf(store.prefs.getBoolean("english", true)) }
  var spelling by remember { mutableStateOf(store.prefs.getBoolean("spelling", true)) }
  SettingsCard {
    Text("拼音方案", style = MaterialTheme.typography.titleMedium)
    listOf(false to "全拼", true to "小鹤双拼").forEach { (value, label) ->
      Row(
          Modifier.fillMaxWidth().clickable {
            flypy = value
            store.prefs.edit().putBoolean("flypy", value).apply()
          },
          verticalAlignment = Alignment.CenterVertically,
      ) {
        RadioButton(
            flypy == value,
            onClick = {
              flypy = value
              store.prefs.edit().putBoolean("flypy", value).apply()
            },
        )
        Text(label)
      }
    }
  }
  SettingsCard {
    ToggleRow("英文单词补全", english) {
      english = it
      store.prefs.edit().putBoolean("english", it).apply()
    }
    ToggleRow("英文拼写建议", spelling, english) {
      spelling = it
      store.prefs.edit().putBoolean("spelling", it).apply()
    }
    ToggleRow("符号自动补全", symbols) {
      symbols = it
      store.prefs.edit().putBoolean("symbolCompletion", it).apply()
    }
  }
}

@Composable
private fun PacksPage(busy: Boolean, work: (suspend () -> String) -> Unit) {
  val context = LocalContext.current
  val store = remember { AppStore(context) }
  DictionaryPacks.catalog(context).forEachIndexed { i, p ->
    SettingsCard {
      ToggleRow(
          p.getString("title"),
          store.prefs.getInt("packs", 0) and (1 shl i) != 0,
          !busy,
      ) { enabled ->
        work {
          DictionaryPacks.set(context, i, enabled)
          if (enabled) "词库已启用" else "词库已停用"
        }
      }
      Text(
          "${p.getString("description")} · %.1f MB".format(p.getLong("bytes") / 1048576.0),
          style = MaterialTheme.typography.bodySmall,
      )
    }
  }
}

@Composable
private fun BaseResourceSection() {
  val context = LocalContext.current
  val resources by BaseResources.state.collectAsState()
  val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
    if (uri != null) {
      runCatching { context.contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
      BaseResources.start(context, uri)
    }
  }
  if (resources.checked && !resources.ready) {
    SettingsCard {
      Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Text(
            if (resources.base == null) "基础词库" else "补全基础词库",
            Modifier.weight(1f),
            style = MaterialTheme.typography.titleMedium,
        )
        if (resources.busy) {
          ActionIconButton(ActionIcon.Close, "取消") { BaseResources.cancel(context) }
        } else {
          Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(onClick = { BaseResources.start(context) }) {
              Text("下载 · %.1f MB".format(resources.bytes / 1048576.0))
            }
            OutlinedButton(onClick = { picker.launch(arrayOf("*/*")) }) { Text("导入") }
          }
        }
      }
      if (resources.busy) {
        LinearProgressIndicator(progress = { resources.progress }, modifier = Modifier.fillMaxWidth())
        if (resources.message.isNotEmpty())
            Text(resources.message, style = MaterialTheme.typography.bodySmall)
      } else if (resources.message.isNotEmpty()) {
        Text(resources.message, style = MaterialTheme.typography.bodySmall)
      }
    }
  }
}

@Composable
private fun AboutPage() {
  val context = LocalContext.current
  var notice by remember { mutableStateOf("") }
  var showNotice by remember { mutableStateOf(false) }
  LaunchedEffect(Unit) {
    notice =
        withContext(Dispatchers.IO) {
          context.assets.open("NOTICE.txt").bufferedReader().use { it.readText() }
        }
  }
  SettingsCard {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
      Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text("retype · ${BuildConfig.VERSION_NAME}", style = MaterialTheme.typography.titleLarge)
        Text(
            if (BuildConfig.DEBUG) "Android 预览版" else "Android",
            style = MaterialTheme.typography.bodySmall,
            color = Color(0xFF697271),
        )
      }
      OutlinedButton(
          onClick = {
            context.startActivity(
                Intent(
                    Intent.ACTION_VIEW,
                    android.net.Uri.parse("https://github.com/L-Chris/retype"),
                ))
          }) {
            Text("主页")
          }
    }
  }
  UpdateCard()
  SettingsCard {
    Row(
        Modifier.fillMaxWidth().clickable { showNotice = !showNotice },
        verticalAlignment = Alignment.CenterVertically,
    ) {
      Text("开源许可与第三方署名", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
      Chevron(down = showNotice)
    }
    if (showNotice && notice.isNotEmpty()) {
      Text(notice, style = MaterialTheme.typography.bodySmall)
    }
  }
}
