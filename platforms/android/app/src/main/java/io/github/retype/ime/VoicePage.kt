package io.github.retype.ime

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import org.json.JSONObject

@Composable
internal fun VoicePage(store: AppStore) {
  val context = LocalContext.current
  val scope = rememberCoroutineScope()
  var config by remember { mutableStateOf(store.ai()) }
  val settings = config.getJSONObject("voice")
  var permitted by remember {
    mutableStateOf(
        context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED
    )
  }
  val permission =
      rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        permitted = it
      }
  var voice by remember { mutableStateOf<VoiceInput?>(null) }
  var preview by remember { mutableStateOf<VoiceState?>(null) }
  DisposableEffect(Unit) { onDispose { voice?.cancel() } }
  fun save(key: String, value: Any) {
    settings.put(key, value)
    store.saveAi(config)
    config = JSONObject(config.toString())
  }
  val providers = config.getJSONArray("providers")
  val modelOptions = buildList {
    for (i in 0 until providers.length()) {
      val p = providers.getJSONObject(i)
      if (p.getString("kind") !in listOf("Compatible", "Gemini")) continue
      val pid = p.getString("id")
      val pname = p.getString("name")
      val models = p.getJSONArray("models")
      for (m in 0 until models.length()) {
        val model = models.getString(m)
        add("$pid|$model" to "$pname / $model")
      }
    }
  }
  val currentSelection = "${settings.optString("provider")}|${settings.optString("model")}"

  SettingsCard {
    Choice("语音模型", modelOptions, currentSelection) { combined ->
      val parts = combined.split("|", limit = 2)
      if (parts.size == 2) {
        settings.put("provider", parts[0])
        save("model", parts[1])
      }
    }
    val provider =
        (0 until providers.length())
            .map { providers.getJSONObject(it) }
            .firstOrNull { it.getString("id") == settings.optString("provider") }
    provider?.let { p ->
      val model = settings.optString("model")
      val default =
          if (p.getString("kind") == "Gemini" && model == "gemini-3.5-transcribe-live")
              "GeminiLive"
          else "File"
      val protocol = p.optJSONObject("voice_protocols")?.optString(model, default) ?: default
      Text("识别方式", style = MaterialTheme.typography.labelLarge)
      Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        for ((value, label) in
            listOf("File" to "录完后识别", "GeminiLive" to "实时识别（Gemini Live）")) {
          FilterChip(
              selected = protocol == value,
              onClick = {
                val modes =
                    p.optJSONObject("voice_protocols")
                        ?: JSONObject().also { p.put("voice_protocols", it) }
                modes.put(model, value)
                store.saveAi(config)
                config = JSONObject(config.toString())
              },
              label = { Text(label) },
          )
        }
      }
    }
    Text("识别语言", style = MaterialTheme.typography.labelLarge)
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
      for ((value, label) in listOf("auto" to "自动识别", "zh" to "中文", "en" to "英文")) {
        FilterChip(
            selected = settings.optString("language", "auto") == value,
            onClick = { save("language", value) },
            label = { Text(label) },
        )
      }
    }
    ToggleRow("整理口语", settings.optBoolean("tidy")) { save("tidy", it) }
  }

  SettingsCard {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
      Text("录音测试", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
      if (!permitted) {
        Button(onClick = { permission.launch(Manifest.permission.RECORD_AUDIO) }) { Text("授权") }
      } else if (voice == null || preview?.phase in listOf("Done", "Error", "Cancelled")) {
        Button(
            onClick = {
              voice?.cancel()
              preview = VoiceState()
              voice = VoiceInput(context, scope, { preview = it }, {}).also { it.start() }
            }
        ) {
          Text("录音")
        }
      } else {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
          Button(onClick = { voice?.finish() }) { Text("完成") }
          ActionIconButton(ActionIcon.Close, "取消") {
            voice?.cancel()
            voice = null
            preview = null
          }
        }
      }
    }
    preview?.let {
      Text(
          it.error
              ?: when (it.phase) {
                "Recording" -> "录音中…"
                "Recognizing" -> "识别中…"
                else -> "识别完成"
              },
          style = MaterialTheme.typography.bodySmall,
      )
      if (it.text.isNotEmpty()) Text(it.text)
    }
  }
}
