package io.github.retype.ime

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay

private fun formatMatchingCode(code: String): String {
  if (code.length != 6) return code
  return "${code[0]} ${code[1]} ${code[2]}   ${code[3]} ${code[4]} ${code[5]}"
}

@Composable
fun LanPage() {
  val context = LocalContext.current
  val clipboard = remember { LanClipboard.get(context) }
  val state by clipboard.state.collectAsState()
  var showJoin by remember { mutableStateOf(false) }
  var code by remember { mutableStateOf("") }
  var address by remember { mutableStateOf("") }
  var editAddress by remember { mutableStateOf(false) }
  var savedAddresses by remember(state.addresses) { mutableStateOf(state.addresses) }
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

  // Card 1: Master Switch & Pairing
  SettingsCard {
    ToggleRow("剪贴板同步", state.enabled, onChange = clipboard::enable)
    if (state.enabled) {
      HorizontalDivider(color = Color(0xFFEDF1F3))
      Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Box(
            Modifier.size(8.dp)
                .background(
                    if (state.connection.isNotEmpty()) Color(0xFF1FA866) else Color(0xFF138F96),
                    CircleShape,
                )
        )
        Spacer(Modifier.width(8.dp))
        Text(
            state.message.ifEmpty { "等待连接" },
            Modifier.weight(1f),
            style = MaterialTheme.typography.bodyMedium,
            color = Color(0xFF657781),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
          OutlinedButton(
              onClick = {
                showJoin = false
                clipboard.showCode()
              }
          ) {
            Text("匹配码")
          }
          Button(
              onClick = {
                clipboard.cancelPair()
                showJoin = !showJoin
              }
          ) {
            Text("关联")
          }
        }
      }

      state.code?.let { matchingCode ->
        Surface(
            shape = RoundedCornerShape(12.dp),
            color = Color(0xFFE8F6F6),
            modifier = Modifier.fillMaxWidth(),
        ) {
          Row(
              Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
              verticalAlignment = Alignment.CenterVertically,
          ) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
              Text(
                  formatMatchingCode(matchingCode),
                  fontSize = 22.sp,
                  fontWeight = FontWeight.Bold,
                  fontFamily = FontFamily.Monospace,
                  color = Color(0xFF138F96),
              )
              Text(
                  "${maxOf(0, (state.expires - now) / 1000)}s 后失效",
                  style = MaterialTheme.typography.bodySmall,
                  color = Color(0xFF657781),
              )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
              ActionIconButton(ActionIcon.Refresh, "刷新匹配码", onClick = clipboard::showCode)
              ActionIconButton(ActionIcon.Close, "关闭匹配码", onClick = clipboard::cancelPair)
            }
          }
        }
      }

      if (showJoin) {
        HorizontalDivider(color = Color(0xFFEDF1F3))
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
          Row(
              Modifier.fillMaxWidth(),
              verticalAlignment = Alignment.CenterVertically,
              horizontalArrangement = Arrangement.spacedBy(8.dp),
          ) {
            OutlinedTextField(
                code,
                { code = it.filter { c -> c in '0'..'9' }.take(6) },
                label = { Text("6 位匹配码") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                modifier = Modifier.weight(1f),
            )
            ActionIconButton(
                ActionIcon.Check,
                "确认关联",
                enabled = code.matches(Regex("[0-9]{6}")),
                primary = true,
            ) {
              if (clipboard.join(code, address)) {
                code = ""
                showJoin = false
              }
            }
            ActionIconButton(ActionIcon.Close, "取消") {
              showJoin = false
              code = ""
              clipboard.cancelPair()
            }
          }
          OutlinedTextField(
              address,
              { address = it },
              label = { Text("指定地址（选填，局域网留空）") },
              placeholder = { Text("如 100.64.0.1:50206") },
              singleLine = true,
              modifier = Modifier.fillMaxWidth(),
          )
        }
      }
    }
  }

  // Card 2: Associated Peer
  if (state.name.isNotEmpty()) {
    SettingsCard {
      Text("已关联设备", style = MaterialTheme.typography.labelLarge, color = Color(0xFF657781))
      HorizontalDivider(color = Color(0xFFEDF1F3))
      val online = state.connection.isNotEmpty()
      val endpoint = state.connection.ifEmpty { state.target.ifEmpty { "自动发现" } }
      Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
          Row(
              verticalAlignment = Alignment.CenterVertically,
              horizontalArrangement = Arrangement.spacedBy(6.dp),
          ) {
            Box(
                Modifier.size(8.dp)
                    .background(
                        if (online) Color(0xFF1FA866) else Color(0xFFBEC8CE),
                        CircleShape,
                    )
            )
            Text(state.name, fontWeight = FontWeight.SemiBold)
            Text(
                if (online) "已连接" else "离线",
                style = MaterialTheme.typography.bodySmall,
                color = if (online) Color(0xFF138F96) else Color(0xFF657781),
            )
          }
          SelectionContainer {
            Text(
                endpoint,
                style = MaterialTheme.typography.bodySmall,
                fontFamily = FontFamily.Monospace,
                color = Color(0xFF657781),
            )
          }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
          ActionIconButton(
              ActionIcon.Sliders,
              "连接地址",
              primary = editAddress,
              onClick = { editAddress = !editAddress },
          )
          ActionIconButton(ActionIcon.Trash, "解除关联", danger = true, onClick = clipboard::unlink)
        }
      }
      if (editAddress) {
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
          OutlinedTextField(
              savedAddresses,
              { savedAddresses = it },
              label = { Text("连接地址（多个用逗号分隔）") },
              singleLine = true,
              modifier = Modifier.weight(1f),
          )
          ActionIconButton(ActionIcon.Check, "保存地址", primary = true) {
            clipboard.setAddresses(savedAddresses)
            editAddress = false
          }
          ActionIconButton(ActionIcon.Close, "取消") {
            savedAddresses = state.addresses
            editAddress = false
          }
        }
      }
    }
  }
}
