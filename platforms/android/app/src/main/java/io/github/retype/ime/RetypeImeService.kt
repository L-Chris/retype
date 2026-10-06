package io.github.retype.ime

import android.content.Intent
import android.inputmethodservice.InputMethodService
import android.util.Log
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.platform.ViewCompositionStrategy
import androidx.lifecycle.*
import androidx.savedstate.*
import java.io.File
import java.util.concurrent.Executors
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import org.json.JSONObject

data class KeyboardState(
    val generation: Long = 0,
    val composition: String = "",
    val candidates: List<String> = emptyList(),
    val pageStart: Int = 0,
    val selected: Int = 0,
    val chinese: Boolean = true,
    val ready: Boolean = false,
    val password: Boolean = false,
    val numeric: Boolean = false,
    val enterLabel: String = "换行",
    val error: String? = null,
    val translation: String? = null,
    val translating: Boolean = false,
)

class RetypeImeService :
    InputMethodService(), LifecycleOwner, ViewModelStoreOwner, SavedStateRegistryOwner {
  private val registry = LifecycleRegistry(this)
  private val saved = SavedStateRegistryController.create(this)
  override val lifecycle: Lifecycle
    get() = registry

  override val viewModelStore = ViewModelStore()
  override val savedStateRegistry: SavedStateRegistry
    get() = saved.savedStateRegistry

  private val executor = Executors.newSingleThreadExecutor { task -> Thread(task, "retype-engine") }
  private val dispatcher = executor.asCoroutineDispatcher()
  private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
  private val jobs = Channel<suspend () -> Unit>(Channel.UNLIMITED)
  private var handle = 0L // Only accessed on the engine dispatcher.
  @Volatile private var epoch = 0L
  private var policy = EditorPolicy(false, true, false, false)
  private var connection: InputConnection? = null
  private var composing = false
  private var view: ComposeView? = null
  private var engineJob: Job? = null
  private var state by mutableStateOf(KeyboardState())
  private val activityClock = ActivityClock()
  private var translationJob: Job? = null
  private var noticeJob: Job? = null
  private var modifierTap = 0
  private var modifierUsed = false

  private fun finishStatistics() {
    TypingStatistics.get(applicationContext).record(activityClock.boundary())
  }
  private fun finishEditorStatistics() {
    if (composing && connection?.finishComposingText() == true &&
        !state.chinese && !policy.password && !policy.literal) {
      TypingStatistics.get(applicationContext).record(activityClock.commit(state.composition, false))
    }
    finishStatistics()
    composing = false
  }

  override fun onCreate() {
    super.onCreate()
    saved.performAttach()
    saved.performRestore(null)
    registry.currentState = Lifecycle.State.RESUMED
    engineJob =
        scope.launch(dispatcher) {
          for (job in jobs) try {
            job()
          } catch (e: Exception) {
            Log.e("retype", "IME operation failed", e)
          }
        }
  }

  override fun onCreateInputView(): View {
    view?.disposeComposition()
    // WindowRecomposer resolves the window content root, not only the ComposeView.
    window?.window?.decorView?.let { root ->
      root.setViewTreeLifecycleOwner(this)
      root.setViewTreeViewModelStoreOwner(this)
      root.setViewTreeSavedStateRegistryOwner(this)
    }
    return ComposeView(this).also {
      view = it
      it.setViewTreeLifecycleOwner(this)
      it.setViewTreeViewModelStoreOwner(this)
      it.setViewTreeSavedStateRegistryOwner(this)
      it.setViewCompositionStrategy(ViewCompositionStrategy.DisposeOnViewTreeLifecycleDestroyed)
      it.setContent {
        RetypeTheme {
          Keyboard(
              state,
              ::key,
              ::choose,
              ::toggle,
              ::literal,
              ::openSettings,
              ::translate,
              ::paste,
          )
        }
      }
    }
  }

  override fun onWindowShown() {
    super.onWindowShown()
    LanClipboard.get(this).resume()
    registry.currentState = Lifecycle.State.RESUMED
  }

  override fun onWindowHidden() {
    registry.currentState = Lifecycle.State.STARTED
    super.onWindowHidden()
  }

  override fun onEvaluateFullscreenMode() = false

  override fun onStartInput(attribute: EditorInfo, restarting: Boolean) {
    super.onStartInput(attribute, restarting)
    finishEditorStatistics()
    connection?.finishComposingText()
    connection = currentInputConnection
    composing = false
    val current = ++epoch
    translationJob?.cancel()
    noticeJob?.cancel()
    activityClock.reset()
    policy = EditorPolicy.from(attribute)
    val fieldPolicy = policy
    Log.d(
        "retype",
        "start input epoch=$current restart=$restarting literal=${policy.literal} ascii=${policy.ascii}",
    )
    val preferences = getSharedPreferences("settings", MODE_PRIVATE)
    val chinese = !policy.ascii && preferences.getBoolean("chinese", true)
    val flypy = preferences.getBoolean("flypy", false)
    Log.d("retype", "session scheme=${if (flypy) "flypy" else "full"} epoch=$current")
    state =
        KeyboardState(
            chinese = chinese,
            password = policy.password,
            numeric = policy.literal && !policy.password,
            ready = policy.literal,
            enterLabel =
                when (attribute.imeOptions and EditorInfo.IME_MASK_ACTION) {
                  EditorInfo.IME_ACTION_SEARCH -> "搜索"
                  EditorInfo.IME_ACTION_GO -> "前往"
                  EditorInfo.IME_ACTION_SEND -> "发送"
                  EditorInfo.IME_ACTION_DONE -> "完成"
                  EditorInfo.IME_ACTION_NEXT -> "下一项"
                  else -> "换行"
                },
        )
    jobs.trySend {
      closeSession()
      if (fieldPolicy.literal || epoch != current) return@trySend
      try {
        val file = DictionaryAssets.prepare(applicationContext)
        handle =
            NativeBridge.create(
                file.absolutePath,
                File(filesDir, "learning.db").absolutePath,
                flypy,
                chinese,
                fieldPolicy.learning,
                DictionaryPacks.paths(applicationContext),
            )
        NativeBridge.dispatch(
            handle,
            JSONObject()
                .put("type", "englishOptions")
                .put("enabled", preferences.getBoolean("english", true))
                .put("spelling", preferences.getBoolean("spelling", true))
                .toString(),
        )
        withContext(Dispatchers.Main) { if (epoch == current) state = state.copy(ready = true) }
      } catch (e: Exception) {
        Log.e("retype", "Session initialization failed", e)
        withContext(Dispatchers.Main) {
          if (epoch == current) state = state.copy(error = "词库加载失败，暂用直接输入", ready = true)
        }
      }
    }
  }

  override fun onFinishInput() {
    Log.d("retype", "finish input epoch=$epoch")
    ++epoch
    translationJob?.cancel()
    noticeJob?.cancel()
    finishEditorStatistics()
    activityClock.reset()
    connection?.finishComposingText()
    connection = null
    composing = false
    state = KeyboardState()
    jobs.trySend { closeSession() }
    super.onFinishInput()
  }

  private fun closeSession() {
    if (handle != 0L) {
      NativeBridge.destroy(handle)
      handle = 0L
    }
  }

  private fun command(json: JSONObject, passthrough: (() -> Boolean)? = null) {
    val current = epoch
    val ic = connection ?: return
    val switching = json.optString("type") == "toggle"
    jobs.trySend {
      if (epoch != current) {
        if (switching) Log.d("retype", "mode switch discarded after editor change")
        return@trySend
      }
      if (handle == 0L) {
        withContext(Dispatchers.Main) {
          if (epoch == current && connection === ic) passthrough?.invoke()
        }
        return@trySend
      }
      val update = JSONObject(NativeBridge.dispatch(handle, json.toString()))
      if (switching)
          Log.d(
              "retype",
              "mode result chinese=${update.getBoolean("chinese")} epoch=$current",
          )
      val generation = update.getLong("generation")
      Log.d(
          "retype",
          "candidate update epoch=$current generation=$generation letters=${update.getString("composition").length} candidates=${update.getJSONArray("candidates").length()}")
      var accepted = false
      withContext(Dispatchers.Main) {
        if (epoch != current || connection !== ic) return@withContext
        val commits = update.getJSONArray("commits")
        val committedLanguage = state.chinese
        accepted = true
        ic.beginBatchEdit()
        try {
          for (i in 0 until commits.length()) {
            val text = commits.getString(i)
            val committed = ic.commitText(text, 1)
            accepted = committed && accepted
            if (committed && !policy.password && !policy.literal) {
              TypingStatistics.get(applicationContext)
                  .record(activityClock.commit(text, committedLanguage))
            }
          }
          val composition = update.getString("composition")
          if (composition.isNotEmpty()) {
            accepted = ic.setComposingText(composition, 1) && accepted
            composing = true
          } else {
            if (composing && commits.length() == 0) accepted = ic.commitText("", 1) && accepted
            ic.finishComposingText()
            composing = false
          }
          if (update.getBoolean("passThrough")) {
            passthrough?.invoke()
          }
          val key = json.optString("value")
          if (!policy.password && !policy.literal && accepted &&
              (switching || json.optString("type") == "choose" ||
               key in setOf("space", "enter", "tab", "left", "right", "up", "down"))) {
            finishStatistics()
          }
          val candidates = update.getJSONArray("candidates")
          state =
              state.copy(
                  generation = generation,
                  composition = composition,
                  candidates = List(candidates.length()) { candidates.getString(it) },
                  pageStart = update.getInt("pageStart"),
                  selected = update.getInt("selected"),
                  chinese = update.getBoolean("chinese"),
                  ready = true,
              )
          if (switching)
              getSharedPreferences("settings", MODE_PRIVATE)
                  .edit()
                  .putBoolean("chinese", state.chinese)
                  .apply()
          if (switching)
              Log.d(
                  "retype",
                  "mode UI chinese=${state.chinese} lifecycle=${registry.currentState}",
              )
        } finally {
          ic.endBatchEdit()
        }
      }
      NativeBridge.dispatch(
          handle,
          JSONObject()
              .put("type", "ack")
              .put("generation", generation)
              .put("accepted", accepted)
              .toString(),
      )
    }
  }

  private fun direct(value: String): Boolean {
    val ic = connection ?: return false
    fun commit(text: String): Boolean {
      val accepted = ic.commitText(text, 1)
      if (accepted && !policy.password && !policy.literal) {
        TypingStatistics.get(applicationContext)
            .record(activityClock.commit(text, state.chinese))
      }
      return accepted
    }
    val accepted = when (value) {
      "backspace" -> {
        if (policy.password) sendHostKey(ic, KeyEvent.KEYCODE_DEL)
        else {
          val selected = ic.getSelectedText(0)
          if (!selected.isNullOrEmpty()) ic.commitText("", 1)
          else ic.deleteSurroundingTextInCodePoints(1, 0)
        }
      }
      "enter" -> {
        val info = currentInputEditorInfo
        val action = info?.imeOptions?.and(EditorInfo.IME_MASK_ACTION) ?: EditorInfo.IME_ACTION_NONE
        if (info != null &&
            info.imeOptions and EditorInfo.IME_FLAG_NO_ENTER_ACTION == 0 &&
            action !in setOf(EditorInfo.IME_ACTION_NONE, EditorInfo.IME_ACTION_UNSPECIFIED))
            ic.performEditorAction(action)
        else {
          ic.sendKeyEvent(KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_ENTER))
          ic.sendKeyEvent(KeyEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_ENTER))
        }
      }
      "space" -> commit(" ")
      "tab" -> ic.commitText("\t", 1)
      "up" -> sendHostKey(ic, KeyEvent.KEYCODE_DPAD_UP)
      "down" -> sendHostKey(ic, KeyEvent.KEYCODE_DPAD_DOWN)
      "left" -> sendHostKey(ic, KeyEvent.KEYCODE_DPAD_LEFT)
      "right" -> sendHostKey(ic, KeyEvent.KEYCODE_DPAD_RIGHT)
      "escape" -> sendHostKey(ic, KeyEvent.KEYCODE_ESCAPE)
      else -> commit(value)
    }
    if (accepted && !policy.password && !policy.literal) {
      if (value == "backspace") activityClock.backspace()
      else if (value in setOf("enter", "tab", "left", "right", "up", "down", "escape")) finishStatistics()
    }
    return accepted
  }

  private fun sendHostKey(ic: InputConnection, code: Int): Boolean {
    val down = ic.sendKeyEvent(KeyEvent(KeyEvent.ACTION_DOWN, code))
    return ic.sendKeyEvent(KeyEvent(KeyEvent.ACTION_UP, code)) && down
  }

  private fun key(value: String, mods: Int = 0) {
    if (!policy.password &&
        !policy.literal &&
        (value == "space" || value.firstOrNull()?.isLetterOrDigit() == true && value.length == 1))
        activityClock.tick(state.chinese)
    if (policy.literal || state.error != null) {
      direct(value)
      return
    }
    command(JSONObject().put("type", "key").put("value", value).put("mods", mods)) { direct(value) }
  }

  private fun literal(value: String) {
    if (policy.literal || state.error != null) {
      direct(value)
      return
    }
    command(JSONObject().put("type", "literal").put("text", value)) { direct(value) }
  }

  private fun paste(value: String) {
    val current = epoch
    val ic = connection ?: return
    jobs.trySend {
      if (epoch != current) return@trySend
      if (handle != 0L) NativeBridge.dispatch(handle, JSONObject().put("type", "reset").toString())
      withContext(Dispatchers.Main) {
        if (epoch != current || connection !== ic) return@withContext
        finishEditorStatistics()
        ic.commitText(value, 1)
        state = state.copy(composition = "", candidates = emptyList())
      }
    }
  }

  private fun choose(index: Int, generation: Long) {
    if (!policy.password) activityClock.tick(state.chinese)
    command(JSONObject().put("type", "choose").put("index", index).put("generation", generation))
  }

  private fun toggle() {
    Log.d("retype", "mode switch requested epoch=$epoch literal=${policy.literal}")
    if (policy.literal) return
    command(JSONObject().put("type", "toggle"))
  }

  private fun openSettings() {
    startActivity(
        Intent(this, SettingsActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
  }

  private fun notice(text: String) {
    noticeJob?.cancel()
    state = state.copy(translation = text, translating = false)
    noticeJob =
        scope.launch {
          delay(3000)
          state = state.copy(translation = null)
        }
  }

  private fun translate() {
    if (policy.password) {
      notice("密码框不支持翻译")
      return
    }
    if (state.translating) return
    finishEditorStatistics()
    val current = epoch
    val ic = connection ?: return
    noticeJob?.cancel()
    state = state.copy(translation = "翻译中…", translating = true)
    // Drain preceding keys, then capture the editor on its required main thread.
    jobs.trySend {
      if (epoch != current) return@trySend
      if (handle != 0L) NativeBridge.dispatch(handle, JSONObject().put("type", "reset").toString())
      withContext(Dispatchers.Main) {
        if (epoch != current || connection !== ic) return@withContext
        ic.finishComposingText()
        composing = false
        state = state.copy(composition = "", candidates = emptyList())
        translationJob =
            scope.launch {
              try {
                val source = EditorTranslation.read(ic)
                val translated =
                    withContext(Dispatchers.IO) {
                      val store = AppStore(applicationContext)
                      val config = store.ai()
                      val key =
                          SecretVault(applicationContext).get("ai:${config.getString("provider")}")
                      val operation =
                          JSONObject()
                              .put("type", "translate")
                              .put("config", config)
                              .put("key", key)
                              .put("text", source.text)
                      org.json.JSONTokener(NativeBridge.feature(operation.toString())).nextValue()
                          as String
                    }
                if (epoch != current || connection !== ic) return@launch
                val preview = AppStore(applicationContext).ai().optBoolean("preview")
                if (!preview && EditorTranslation.replace(ic, source, translated)) notice("已翻译")
                else {
                  (getSystemService(CLIPBOARD_SERVICE) as android.content.ClipboardManager)
                      .setPrimaryClip(android.content.ClipData.newPlainText("译文", translated))
                  notice("译文已复制")
                }
              } catch (e: CancellationException) {
                throw e
              } catch (e: Exception) {
                if (epoch == current) notice(e.message ?: "翻译失败")
              }
            }
      }
    }
  }

  override fun onUpdateSelection(
      oldSelStart: Int,
      oldSelEnd: Int,
      newSelStart: Int,
      newSelEnd: Int,
      candidatesStart: Int,
      candidatesEnd: Int,
  ) {
    super.onUpdateSelection(
        oldSelStart,
        oldSelEnd,
        newSelStart,
        newSelEnd,
        candidatesStart,
        candidatesEnd,
    )
    // Cursor movement outside our composition finishes it instead of deleting editor text.
    if (composing && newSelStart != candidatesEnd && oldSelStart != newSelStart) {
      Log.d(
          "retype",
          "composition reset by editor selection epoch=$epoch selection=$newSelStart:$newSelEnd composing=$candidatesStart:$candidatesEnd")
      finishEditorStatistics()
      composing = false
      state = state.copy(composition = "", candidates = emptyList())
      command(JSONObject().put("type", "reset"))
    }
  }

  override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
    val bindings = AppStore(applicationContext).shortcuts()
    if (event.repeatCount == 0 && Shortcuts.matches(bindings.getJSONObject("translate"), event)) {
      translate()
      return true
    }
    if (event.repeatCount == 0 &&
        bindings.getJSONObject("mode").getInt("modifiers") != 0 &&
        Shortcuts.matches(bindings.getJSONObject("mode"), event)) {
      toggle()
      return true
    }
    val tap = bindings.getJSONObject("mode")
    if (tap.getInt("modifiers") == 0 &&
        tap.getInt("vk") != 0 &&
        Shortcuts.vk(keyCode) == tap.getInt("vk")) {
      modifierTap = keyCode
      modifierUsed = false
      return true
    }
    if (modifierTap != 0) modifierUsed = true
    if (keyCode in setOf(KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_SHIFT_RIGHT)) {
      return super.onKeyDown(keyCode, event)
    }
    if (event.isCtrlPressed || event.isAltPressed || event.isMetaPressed) {
      finishEditorStatistics()
      connection?.finishComposingText()
      composing = false
      command(JSONObject().put("type", "reset"))
      return super.onKeyDown(keyCode, event)
    }
    val value =
        when (keyCode) {
          KeyEvent.KEYCODE_DEL -> "backspace"
          KeyEvent.KEYCODE_ENTER -> "enter"
          KeyEvent.KEYCODE_SPACE -> "space"
          KeyEvent.KEYCODE_TAB -> "tab"
          KeyEvent.KEYCODE_ESCAPE -> "escape"
          KeyEvent.KEYCODE_DPAD_UP -> "up"
          KeyEvent.KEYCODE_DPAD_DOWN -> "down"
          KeyEvent.KEYCODE_DPAD_LEFT -> "left"
          KeyEvent.KEYCODE_DPAD_RIGHT -> "right"
          else -> event.unicodeChar.takeIf { it >= 32 }?.toChar()?.toString()
        }
    if (value != null) {
      key(value, if (event.isShiftPressed) 1 else 0)
      return true
    }
    return super.onKeyDown(keyCode, event)
  }

  override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean {
    if (modifierTap == keyCode) {
      if (!modifierUsed) toggle()
      modifierTap = 0
      return true
    }
    if (keyCode in setOf(KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_SHIFT_RIGHT)) {
      return super.onKeyUp(keyCode, event)
    }
    if (keyCode == KeyEvent.KEYCODE_DEL ||
        keyCode == KeyEvent.KEYCODE_ENTER ||
        keyCode == KeyEvent.KEYCODE_SPACE ||
        event.unicodeChar >= 32)
        return true
    return super.onKeyUp(keyCode, event)
  }

  override fun onDestroy() {
    finishEditorStatistics()
    ++epoch
    view?.disposeComposition()
    registry.currentState = Lifecycle.State.DESTROYED
    translationJob?.cancel()
    noticeJob?.cancel()
    viewModelStore.clear()
    jobs.trySend { closeSession() }
    jobs.close()
    // Drain queued operations and close native workers before shutting down the dispatcher.
    engineJob?.invokeOnCompletion {
      dispatcher.close()
      executor.shutdown()
      scope.cancel()
    }
    super.onDestroy()
  }
}
