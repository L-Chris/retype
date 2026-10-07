package io.github.retype.ime

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
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
  var permitted by remember { mutableStateOf(context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) }
  val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { permitted = it }
  var voice by remember { mutableStateOf<VoiceInput?>(null) }
  var preview by remember { mutableStateOf<VoiceState?>(null) }
  DisposableEffect(Unit) { onDispose { voice?.cancel() } }
  fun save(key: String, value: Any) { settings.put(key,value); store.saveAi(config); config = JSONObject(config.toString()) }
  Card(colors=CardDefaults.cardColors(containerColor=MaterialTheme.colorScheme.surface)) {
    Column(Modifier.padding(20.dp),verticalArrangement=Arrangement.spacedBy(16.dp)) {
      Text("语音模型")
      var expanded by remember { mutableStateOf(false) }
      val selected = settings.optString("model").ifEmpty { "请选择支持音频的模型" }
      OutlinedButton(onClick={expanded=true},modifier=Modifier.fillMaxWidth()) { Text(selected) }
      DropdownMenu(expanded,onDismissRequest={expanded=false}) {
        val providers=config.getJSONArray("providers")
        for (i in 0 until providers.length()) {
          val p=providers.getJSONObject(i)
          if (p.getString("kind") !in listOf("Compatible", "Gemini")) continue
          val models=p.getJSONArray("models")
          for (m in 0 until models.length()) {
            val model=models.getString(m)
            DropdownMenuItem(text={Text("${p.getString("name")} / $model")},onClick={settings.put("provider",p.getString("id")); save("model",model);expanded=false})
          }
        }
      }
      val providers = config.getJSONArray("providers")
      val provider = (0 until providers.length()).map { providers.getJSONObject(it) }
        .firstOrNull { it.getString("id") == settings.optString("provider") }
      provider?.let { p ->
        val model = settings.optString("model")
        val default = if (p.getString("kind") == "Gemini" && model == "gemini-3.5-transcribe-live") "GeminiLive" else "File"
        val protocol = p.optJSONObject("voice_protocols")?.optString(model, default) ?: default
        Text("识别方式")
        Row(horizontalArrangement=Arrangement.spacedBy(8.dp)) {
          for ((value,label) in listOf("File" to "录音后识别", "GeminiLive" to "实时识别（Gemini Live）")) {
            FilterChip(selected=protocol==value,onClick={
              val modes = p.optJSONObject("voice_protocols") ?: JSONObject().also { p.put("voice_protocols",it) }
              modes.put(model,value);store.saveAi(config);config=JSONObject(config.toString())
            },label={Text(label)})
          }
        }
      }
      Text("识别语言")
      Row(horizontalArrangement=Arrangement.spacedBy(8.dp)) {
        for ((value,label) in listOf("auto" to "自动", "zh" to "中文", "en" to "英文")) {
          FilterChip(selected=settings.optString("language","auto")==value,onClick={save("language",value)},label={Text(label)})
        }
      }
      Row(Modifier.fillMaxWidth(),horizontalArrangement=Arrangement.SpaceBetween) {
        Text("整理口头语与段落"); Switch(checked=settings.optBoolean("tidy"),onCheckedChange={save("tidy",it)})
      }
    }
  }
  Card(colors=CardDefaults.cardColors(containerColor=MaterialTheme.colorScheme.surface)) {
    Column(Modifier.padding(20.dp),verticalArrangement=Arrangement.spacedBy(12.dp)) {
      Text("麦克风")
      if (!permitted) Button(onClick={permission.launch(Manifest.permission.RECORD_AUDIO)}) { Text("允许麦克风权限") }
      else if (voice == null || preview?.phase in listOf("Done","Error","Cancelled")) {
        Button(onClick={voice?.cancel();preview=VoiceState();voice=VoiceInput(context,scope,{preview=it},{}).also{it.start()}}) { Text("开始录音测试") }
      } else Row(horizontalArrangement=Arrangement.spacedBy(12.dp)) {
        Button(onClick={voice?.finish()}) { Text("结束录音") }
        TextButton(onClick={voice?.cancel();voice=null;preview=null}) { Text("取消") }
      }
      preview?.let { Text(it.error ?: when(it.phase) {"Recording"->"录音中…";"Recognizing"->"识别中…";else->"识别完成"}); if(it.text.isNotEmpty()) Text(it.text) }
    }
  }
}
