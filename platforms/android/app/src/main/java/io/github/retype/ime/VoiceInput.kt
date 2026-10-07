package io.github.retype.ime

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.util.Base64
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.*
import org.json.JSONObject

data class VoiceState(val phase: String = "Recording", val text: String = "", val level: Float = 0f, val seconds: Int = 0, val error: String? = null)

/** No editor access: recording and recognition only. The IME validates its original target. */
class VoiceInput(private val context: Context, private val scope: CoroutineScope, private val changed: (VoiceState) -> Unit, private val completed: (String) -> Unit) {
  private val id = UUID.randomUUID().toString()
  private val cancelled = AtomicBoolean(false)
  private val stopping = AtomicBoolean(false)
  @Volatile private var recorder: AudioRecord? = null
  private var job: Job? = null
  private var last = VoiceState()
  private suspend fun operation(type: String, extra: (JSONObject) -> Unit = {}): VoiceState = withContext(Dispatchers.IO) {
    val request = JSONObject().put("type", type).put("id", id).also(extra)
    val result = JSONObject(NativeBridge.feature(request.toString()))
    VoiceState(result.getString("phase"), result.optString("text"), result.optDouble("level").toFloat(), result.optInt("seconds"), result.optString("error").takeIf { it.isNotEmpty() && it != "null" })
  }
  fun start() {
    if (job != null) return
    job = scope.launch {
      try {
        require(context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) { "请先在语音输入设置中允许麦克风权限" }
        val store = AppStore(context); val config = store.ai(); val settings = config.getJSONObject("voice")
        val key = SecretVault(context).get("ai:${settings.getString("provider")}")
        operation("voiceStart") { it.put("config",config).put("settings",settings).put("key",key) }
        if (cancelled.get()) { operation("voiceCancel"); return@launch }
        coroutineScope {
          val audio = launch(Dispatchers.IO) {
            val size = maxOf(AudioRecord.getMinBufferSize(16000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT), 6400)
            val mic = AudioRecord(MediaRecorder.AudioSource.VOICE_RECOGNITION,16000,AudioFormat.CHANNEL_IN_MONO,AudioFormat.ENCODING_PCM_16BIT,size)
            try {
              require(mic.state == AudioRecord.STATE_INITIALIZED) { "麦克风不可用" }
              recorder = mic
              if (cancelled.get()) return@launch
              mic.startRecording()
              val buffer = ByteArray(3200); var frames = 0
              while (!cancelled.get() && !stopping.get()) {
                val count = mic.read(buffer,0,buffer.size,AudioRecord.READ_BLOCKING)
                if (count < 0) { if (stopping.get() || cancelled.get()) break; error("麦克风录音中断") }
                if (count == 0) continue
                operation("voicePush") { it.put("pcm",Base64.encodeToString(buffer.copyOf(count),Base64.NO_WRAP)) }
                frames += count
                if (frames >= 16000 * 2 * 120) stopping.set(true)
              }
            } finally {
              recorder = null
              runCatching { mic.stop() }; mic.release()
            }
            if (!cancelled.get()) operation("voiceStop")
          }
          val deadline = android.os.SystemClock.elapsedRealtime() + 210000
          while (!cancelled.get()) {
            delay(150)
            val snapshot = operation("voicePoll")
            if (cancelled.get()) break
            last = snapshot
            changed(snapshot)
            if (snapshot.phase == "Done") { stopping.set(true); completed(snapshot.text); break }
            if (snapshot.phase in listOf("Error","Cancelled")) break
            if (android.os.SystemClock.elapsedRealtime() > deadline) error("语音识别超时，请重试")
          }
          stopping.set(true); runCatching { recorder?.stop() }; audio.join()
        }
      } catch (e: CancellationException) { throw e }
      catch (e: Exception) { if (!cancelled.get()) changed(last.copy(phase="Error",error=e.message ?: "语音输入失败")) }
      finally {
        stopping.set(true); runCatching { recorder?.stop() }
        withContext(NonCancellable + Dispatchers.IO) { runCatching { operation("voiceCancel") } }
      }
    }
  }
  fun finish() { stopping.set(true); runCatching { recorder?.stop() } }
  fun cancel() { cancelled.set(true); stopping.set(true); runCatching { recorder?.stop() }; job?.cancel() }
}
