package io.github.retype.ime

import android.util.Log

/** Numeric diagnostics only: never include keys, editor contents or candidates. */
internal object InputPerformance {
  private var lastSlowLog = 0L // Called only by the serialized engine worker.

  fun record(queued: Long, nativeStarted: Long, nativeFinished: Long,
      editorStarted: Long, editorFinished: Long, candidateCount: Int) {
    if (editorStarted == 0L) return
    val now = System.nanoTime()
    if (now - queued < 16_000_000 || now - lastSlowLog < 250_000_000) return
    lastSlowLog = now
    Log.i("retype-input", "input_timing queue_us=${(nativeStarted - queued) / 1000} native_us=${(nativeFinished - nativeStarted) / 1000} editor_queue_us=${(editorStarted - nativeFinished) / 1000} editor_us=${(editorFinished - editorStarted) / 1000} total_us=${(now - queued) / 1000} candidates=$candidateCount")
  }

  fun startup(started: Long, dictionaryReady: Long, sessionReady: Long) {
    Log.i("retype-input", "startup_timing dictionary_us=${(dictionaryReady - started) / 1000} session_us=${(sessionReady - dictionaryReady) / 1000}")
  }
}
