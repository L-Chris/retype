package io.github.retype.ime

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay

@Composable
fun LanPage() {
  val context = LocalContext.current
  val clipboard = remember { LanClipboard.get(context) }
  val state by clipboard.state.collectAsState()
  var showJoin by remember { mutableStateOf(false) }
  var code by remember { mutableStateOf("") }
  var now by remember { mutableStateOf(System.currentTimeMillis()) }
  LaunchedEffect(state.expires) {
    now = System.currentTimeMillis()
    while (state.expires > now) {
      delay(1000)
      now = System.currentTimeMillis()
    }
  }
  DisposableEffect(clipboard) {
    val stop = clipboard.discover()
    onDispose { stop() }
  }
  Card {
    Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
      Row(verticalAlignment = Alignment.CenterVertically) {
        Text("文字剪贴板同步", Modifier.weight(1f))
        Switch(state.enabled, clipboard::enable)
      }
      Text(state.message, style = MaterialTheme.typography.bodySmall)
      if (state.name.isNotEmpty()) {
        Text(state.name)
        TextButton(onClick = clipboard::unlink) { Text("解除关联") }
      }
      if (state.enabled) {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
          OutlinedButton(
              onClick = {
                showJoin = false
                clipboard.showCode()
              }
          ) {
            Text("查看匹配码")
          }
          Button(
              onClick = {
                clipboard.cancelPair()
                showJoin = true
              }
          ) {
            Text("关联设备")
          }
        }
      }
    }
  }
  state.code?.let { matchingCode ->
    Card {
      Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(matchingCode, style = MaterialTheme.typography.headlineLarge)
        Text("请在另一台设备输入此码 · ${maxOf(0, (state.expires - now) / 1000)} 秒后失效")
        Row {
          TextButton(onClick = clipboard::showCode) { Text("刷新") }
          TextButton(onClick = { clipboard.cancelPair() }) { Text("取消") }
        }
      }
    }
  }
  if (showJoin && state.enabled) {
    Card {
      Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("输入另一台设备的匹配码")
        OutlinedTextField(
            code,
            { code = it.filter { c -> c in '0'..'9' }.take(6) },
            label = { Text("6 位匹配码") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            modifier = Modifier.fillMaxWidth(),
        )
        Row {
          Button(
              onClick = {
                clipboard.join(code)
                code = ""
                showJoin = false
              },
              enabled = code.matches(Regex("[0-9]{6}")),
          ) {
            Text("关联")
          }
          TextButton(
              onClick = {
                showJoin = false
                code = ""
                clipboard.cancelPair()
              }
          ) {
            Text("取消")
          }
        }
      }
    }
  }
}
