package io.github.retype.ime

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp

@Composable
internal fun UpdateCard() {
  val context = LocalContext.current
  val prefs = remember { AppStore(context).prefs }
  val status by AppUpdates.status.collectAsState()
  var auto by remember { mutableStateOf(prefs.getBoolean("updatesAutoCheck", true)) }
  var notes by remember { mutableStateOf(false) }
  SettingsCard {
    Text("应用更新", style = MaterialTheme.typography.titleMedium)
    ToggleRow("每天自动检查更新", auto) {
      auto = it
      prefs.edit().putBoolean("updatesAutoCheck", it).apply()
      AppUpdates.schedule(context.applicationContext)
    }
    if (status.message.isNotBlank())
        Text(status.message, style = MaterialTheme.typography.bodyMedium)
    if (status.busy) {
      if (status.progress != null)
          LinearProgressIndicator(
              progress = { status.progress!! }, modifier = Modifier.fillMaxWidth())
      else LinearProgressIndicator(Modifier.fillMaxWidth())
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
      OutlinedButton(enabled = !status.busy, onClick = { AppUpdates.checkNow(context) }) {
        Text("检查更新")
      }
      if (status.release != null && !status.busy) {
        Button(
            onClick = {
              if (status.ready) AppUpdates.install(context) else AppUpdates.download(context)
            }) {
              Text(if (status.ready) "安装更新" else "下载更新")
            }
      }
      if (status.busy && status.progress != null)
          TextButton(onClick = AppUpdates::cancel) { Text("取消") }
    }
    status.release?.let { release ->
      TextButton(onClick = { notes = !notes }) { Text(if (notes) "收起更新说明" else "更新说明") }
      if (notes)
          Text(release.notes.ifBlank { "此版本未提供更新说明" }, style = MaterialTheme.typography.bodySmall)
    }
  }
}
