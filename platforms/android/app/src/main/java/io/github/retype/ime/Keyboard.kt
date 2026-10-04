package io.github.retype.ime

import androidx.compose.foundation.*
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.grid.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private val Accent = Color(0xFF11939B)

@Composable
fun RetypeTheme(content: @Composable () -> Unit) {
  val colors =
      if (isSystemInDarkTheme())
          darkColorScheme(
              primary = Accent,
              background = Color(0xFF15232A),
              surface = Color(0xFF1E303A),
              surfaceVariant = Color(0xFF293E49),
              surfaceContainerHighest = Color(0xFF243640),
          )
      else
          lightColorScheme(
              primary = Accent,
              background = Color(0xFFF6F9FA),
              surface = Color.White,
              surfaceVariant = Color(0xFFE4EDEE),
              surfaceContainerHighest = Color.White,
              surfaceTint = Accent,
          )
  MaterialTheme(colorScheme = colors, content = content)
}

@Composable
fun Keyboard(
    state: KeyboardState,
    onKey: (String, Int) -> Unit,
    onChoose: (Int, Long) -> Unit,
    onToggle: () -> Unit,
    onLiteral: (String) -> Unit,
    onSettings: () -> Unit,
    onTranslate: () -> Unit,
) {
  var uppercase by remember { mutableStateOf(false) }
  var symbols by remember { mutableStateOf(false) }
  var expanded by remember { mutableStateOf(false) }
  LaunchedEffect(state.password, state.numeric) {
    symbols = state.numeric
    uppercase = false
    expanded = false
  }
  LaunchedEffect(state.composition) { if (state.composition.isEmpty()) expanded = false }
  val height = 46.dp
  val haptics = LocalHapticFeedback.current
  fun press(value: String) {
    haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
    if (symbols && value !in setOf("backspace", "enter", "space")) onLiteral(value)
    else
        onKey(
            value,
            if (uppercase && state.chinese && value.firstOrNull()?.isLetter() == true) 1 else 0,
        )
    if (uppercase && value.firstOrNull()?.isLetter() == true) uppercase = false
  }
  Surface(color = MaterialTheme.colorScheme.surfaceVariant) {
    Column(
        Modifier.fillMaxWidth()
            .navigationBarsPadding()
            .padding(horizontal = 4.dp, vertical = 4.dp)) {
          Row(
              Modifier.fillMaxWidth().height(44.dp),
              verticalAlignment = Alignment.CenterVertically,
          ) {
            if (expanded) {
              Text(state.composition, Modifier.weight(1f).padding(start = 12.dp), fontSize = 15.sp)
            } else if (state.candidates.isEmpty()) {
              Row(
                  Modifier.weight(1f).padding(start = 8.dp),
                  verticalAlignment = Alignment.CenterVertically,
                  horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Image(painterResource(R.drawable.ic_retype), "retype", Modifier.size(30.dp))
                    val status =
                        when {
                          state.password -> "安全输入"
                          state.error != null -> state.error
                          !state.ready -> "加载词库…"
                          else -> null
                        }
                    if (status != null)
                        Text(
                            status,
                            fontSize = 12.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                  }
            } else {
              Row(
                  Modifier.weight(1f).horizontalScroll(rememberScrollState()),
                  verticalAlignment = Alignment.CenterVertically,
              ) {
                state.candidates.drop(state.pageStart).take(8).forEachIndexed { index, text ->
                  Candidate(text, index, state.pageStart + index == state.selected) {
                    onChoose(state.pageStart + index, state.generation)
                  }
                }
              }
            }
            if (state.candidates.isNotEmpty()) {
              IconButton(
                  onClick = { expanded = !expanded },
                  modifier =
                      Modifier.size(44.dp).semantics {
                        contentDescription = if (expanded) "收起候选" else "展开候选"
                      },
              ) {
                Text(if (expanded) "⌃" else "⌄", fontSize = 22.sp)
              }
            }
            if (!expanded) {
              ToolbarButton("翻译", 4, !state.password && !state.translating, onTranslate)
              ToolbarButton("设置", 7, true, onSettings)
            }
          }
          if (state.translation != null)
              Row(
                  Modifier.padding(horizontal = 10.dp, vertical = 3.dp),
                  verticalAlignment = Alignment.CenterVertically,
                  horizontalArrangement = Arrangement.spacedBy(8.dp),
              ) {
                if (state.translating)
                    CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp)
                Text(
                    state.translation,
                    fontSize = 12.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
              }
          if (expanded) {
            Box(Modifier.fillMaxWidth().height(205.dp)) {
              LazyVerticalGrid(
                  columns = GridCells.Adaptive(72.dp),
                  modifier = Modifier.fillMaxSize(),
                  contentPadding = PaddingValues(bottom = 48.dp)) {
                    itemsIndexed(state.candidates) { index, text ->
                      Box(
                          Modifier.heightIn(min = 48.dp)
                              .semantics { contentDescription = "candidate-$text" }
                              .clickable {
                                expanded = false
                                onChoose(index, state.generation)
                              }
                              .padding(horizontal = 8.dp, vertical = 12.dp),
                          contentAlignment = Alignment.Center) {
                            Text(text, fontSize = 18.sp)
                          }
                    }
                  }
              KeyCap(
                  "⌫",
                  Modifier.align(Alignment.BottomEnd).width(64.dp).height(44.dp),
                  { press("backspace") },
                  repeat = true,
                  function = true)
            }
          } else {
            val rows =
                if (symbols)
                    listOf(
                        "1234567890",
                        "@#￥%&*()-+",
                        if (state.chinese) "，。？！:;/=" else ",.?!:;/=",
                        "[]{}<>_~",
                    )
                else listOf("qwertyuiop", "asdfghjkl", "zxcvbnm")
            rows.forEachIndexed { rowIndex, row ->
              Row(
                  Modifier.fillMaxWidth()
                      .padding(
                          vertical = 3.dp,
                          horizontal = if (!symbols && rowIndex == 1) 14.dp else 0.dp,
                      ),
                  horizontalArrangement = Arrangement.spacedBy(4.dp),
              ) {
                if (!symbols && rowIndex == 2)
                    KeyCap(
                        if (uppercase) "⇧" else "↑",
                        Modifier.weight(1.3f).height(height),
                        { uppercase = !uppercase },
                        function = true,
                    )
                row.forEach { c ->
                  val value = if (uppercase && !symbols) c.uppercase() else c.toString()
                  KeyCap(value, Modifier.weight(1f).height(height), { press(value) })
                }
                if (rowIndex == rows.lastIndex)
                    KeyCap(
                        "⌫",
                        Modifier.weight(1.3f).height(height),
                        { press("backspace") },
                        repeat = true,
                        function = true,
                    )
              }
            }
            Row(
                Modifier.fillMaxWidth().padding(top = 3.dp),
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
              KeyCap(
                  if (symbols) "ABC" else "123",
                  Modifier.weight(1.2f).height(height),
                  {
                    if (!symbols) onLiteral("")
                    symbols = !symbols
                  },
                  function = true,
              )
              KeyCap(
                  if (state.chinese) "，" else ",",
                  Modifier.weight(.9f).height(height),
                  { onKey(",", 0) },
              )
              KeyCap("空格", Modifier.weight(3.8f).height(height), { press("space") })
              KeyCap(
                  "",
                  Modifier.weight(.9f).height(height).semantics {
                    contentDescription = if (state.chinese) "切换至英文" else "切换至中文"
                  },
                  onToggle,
                  content = { LanguageIcon(state.chinese) },
              )
              KeyCap(
                  state.enterLabel,
                  Modifier.weight(1.6f).height(height),
                  { press("enter") },
                  function = true,
              )
            }
          }
        }
  }
}

@Composable
private fun LanguageIcon(chinese: Boolean) {
  Box(Modifier.size(30.dp)) {
    Text(
        "中",
        Modifier.align(Alignment.TopStart),
        fontSize = 13.sp,
        color = MaterialTheme.colorScheme.onSurface.copy(alpha = if (chinese) 1f else .4f))
    Text(
        "A",
        Modifier.align(Alignment.BottomEnd),
        fontSize = 13.sp,
        color = MaterialTheme.colorScheme.onSurface.copy(alpha = if (chinese) .4f else 1f))
    val ink = MaterialTheme.colorScheme.onSurfaceVariant
    Canvas(Modifier.fillMaxSize()) {
      val w = size.width
      val h = size.height
      val stroke = 1.3.dp.toPx()
      drawLine(ink, Offset(w * .55f, h * .25f), Offset(w * .9f, h * .25f), stroke, StrokeCap.Round)
      drawLine(ink, Offset(w * .9f, h * .25f), Offset(w * .8f, h * .15f), stroke, StrokeCap.Round)
      drawLine(ink, Offset(w * .45f, h * .75f), Offset(w * .1f, h * .75f), stroke, StrokeCap.Round)
      drawLine(ink, Offset(w * .1f, h * .75f), Offset(w * .2f, h * .85f), stroke, StrokeCap.Round)
    }
  }
}

@Composable
private fun ToolbarButton(label: String, glyph: Int, enabled: Boolean, onClick: () -> Unit) {
  IconButton(
      onClick = onClick,
      enabled = enabled,
      modifier = Modifier.size(44.dp).semantics { contentDescription = label }) {
        Surface(
            shape = RoundedCornerShape(50),
            color =
                if (enabled) MaterialTheme.colorScheme.surface
                else MaterialTheme.colorScheme.surface.copy(alpha = .5f)) {
              Box(
                  Modifier.size(36.dp).alpha(if (enabled) 1f else .4f),
                  contentAlignment = Alignment.Center) {
                    SettingsGlyph(glyph, Modifier.size(24.dp))
                  }
            }
      }
}

@Composable
private fun Candidate(text: String, index: Int, selected: Boolean, onClick: () -> Unit) {
  Surface(
      shape = RoundedCornerShape(8.dp),
      color = if (selected) Accent else Color.Transparent,
      modifier =
          Modifier.padding(horizontal = 3.dp, vertical = 2.dp)
              .semantics { contentDescription = "candidate-$text" }
              .clickable(onClick = onClick),
  ) {
    Row(
        Modifier.padding(horizontal = 10.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
      if (index < 8)
          Text(
              "${index + 1}",
              color =
                  if (selected) Color.White.copy(alpha = .8f)
                  else MaterialTheme.colorScheme.onSurfaceVariant,
              fontSize = 11.sp,
              modifier = Modifier.padding(end = 5.dp),
          )
      Text(
          text,
          maxLines = 1,
          fontSize = 17.sp,
          color = if (selected) Color.White else MaterialTheme.colorScheme.onSurface,
      )
    }
  }
}

@Composable
private fun KeyCap(
    text: String,
    modifier: Modifier,
    onClick: () -> Unit,
    repeat: Boolean = false,
    accent: Boolean = false,
    function: Boolean = false,
    content: (@Composable () -> Unit)? = null,
) {
  val current by rememberUpdatedState(onClick)
  val scope = rememberCoroutineScope()
  val interaction =
      if (repeat)
          Modifier.pointerInput(Unit) {
            detectTapGestures(
                onPress = {
                  current()
                  val repeating =
                      scope.launch {
                        delay(400)
                        while (true) {
                          current()
                          delay(65)
                        }
                      }
                  try {
                    tryAwaitRelease()
                  } finally {
                    repeating.cancel()
                  }
                })
          }
      else Modifier.clickable { current() }
  Surface(
      modifier.then(interaction),
      shape = RoundedCornerShape(7.dp),
      shadowElevation = 1.dp,
      color =
          if (accent) Accent
          else if (function)
              lerp(
                  MaterialTheme.colorScheme.surfaceVariant,
                  MaterialTheme.colorScheme.onSurface,
                  .14f)
          else MaterialTheme.colorScheme.surface,
  ) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
      if (content != null) content()
      else
          Text(
              text,
              fontSize = if (text.length > 2) 12.sp else 20.sp,
              color = if (accent) Color.White else MaterialTheme.colorScheme.onSurface,
          )
    }
  }
}
