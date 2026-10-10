package io.github.retype.ime

import android.view.inputmethod.InputConnection
import org.json.JSONObject

internal interface SymbolEditor {
  fun commit(text: String): Boolean
  fun compose(text: String): Boolean
  fun before(length: Int): String?
  fun after(length: Int): String?
  fun select(position: Int): Boolean
  fun finish(): Boolean
  fun delete(before: Int, after: Int): Boolean
}
private class InputSymbolEditor(private val ic: InputConnection) : SymbolEditor {
  override fun commit(text: String) = ic.commitText(text, 1)
  override fun compose(text: String) = ic.setComposingText(text, 1)
  override fun before(length: Int) = ic.getTextBeforeCursor(length, 0)?.toString()
  override fun after(length: Int) = ic.getTextAfterCursor(length, 0)?.toString()
  override fun select(position: Int) = ic.setSelection(position, position)
  override fun finish() = ic.finishComposingText()
  override fun delete(before: Int, after: Int) = ic.deleteSurroundingText(before, after)
}
internal data class SymbolRule(val left: String?, val right: String?, val close: String?, val pair: Boolean = true)

/** Editor-local ownership; never infer ownership from punctuation already in an app. */
internal class SymbolCompletion(
    private val rules: (String, Boolean, String, String) -> SymbolRule = { key, chinese, before, after ->
      val request = JSONObject().put("type", "symbolRules").put("key", key)
          .put("chinese", chinese).put("before", before).put("after", after)
      val result = JSONObject(NativeBridge.feature(request.toString()))
      fun field(name: String) = if (result.isNull(name)) null else result.optString(name).takeIf { it.isNotEmpty() }
      SymbolRule(field("left"), field("right"), field("close"), result.optBoolean("pair"))
    },
) {
  private data class PairMark(var position: Int, val left: String, val right: String)
  private val pairs = mutableListOf<PairMark>()
  private val expected = ArrayDeque<Pair<Int, Int>>()
  private var start = -1
  private var end = -1
  private var compositionStart = -1
  private var compositionEnd = -1

  fun reset(selectionStart: Int = -1, selectionEnd: Int = selectionStart) {
    pairs.clear(); expected.clear()
    start = selectionStart; end = selectionEnd
    finish()
  }
  fun finish() { compositionStart = -1; compositionEnd = -1 }

  fun selectionChanged(a: Int, b: Int) {
    val selection = a to b
    val own = expected.indexOf(selection)
    if (own >= 0) {
      repeat(own + 1) { expected.removeFirst() }
      return // The latest local position can be ahead of delayed editor callbacks.
    }
    if (a != start || b != end) reset(a, b)
  }

  private fun caret(a: Int, b: Int = a) {
    start = a; end = b
    if (expected.size == 64) expected.removeFirst()
    expected.addLast(a to b)
  }
  private fun replaced(a: Int, b: Int, length: Int) {
    pairs.removeAll { a < it.position + it.right.length && b > it.position }
    val delta = length - (b - a)
    pairs.forEach { if (it.position >= b) it.position += delta }
    caret(a + length)
  }
  private fun write(ic: SymbolEditor, text: String, composing: Boolean): Boolean {
    val a = if (compositionStart >= 0) compositionStart else minOf(start, end)
    val b = if (compositionStart >= 0) compositionEnd else maxOf(start, end)
    val accepted = if (composing) ic.compose(text) else ic.commit(text)
    if (!accepted) { reset(); return false }
    if (a >= 0 && b >= a) replaced(a, b, text.length) else reset()
    if (composing && a >= 0) { compositionStart = a; compositionEnd = a + text.length }
    else finish()
    return true
  }
  fun compose(ic: InputConnection, text: String) = compose(InputSymbolEditor(ic), text)
  fun compose(ic: SymbolEditor, text: String) = write(ic, text, true)

  fun commit(ic: InputConnection, text: String, key: String? = null, chinese: Boolean = true, enabled: Boolean = false) =
      commit(InputSymbolEditor(ic), text, key, chinese, enabled)

  fun commit(ic: SymbolEditor, text: String, key: String? = null, chinese: Boolean = true, enabled: Boolean = false): Boolean {
    if (!enabled || key == null || key.length != 1 || key[0].isLetterOrDigit() || key[0].isWhitespace() || start < 0 || start != end) {
      if (!enabled) pairs.clear()
      return write(ic, text, false)
    }
    // Only direct symbol commits qualify; words committed before them stay ordinary text.
    val mapping = rules(key, chinese, "", "")
    if (mapping.left.isNullOrEmpty() && mapping.close.isNullOrEmpty()) return write(ic, text, false)
    val before = ic.before(2)
    val after = ic.after(1)
    if (before == null || after == null) { pairs.clear(); return write(ic, text, false) }
    val (left, right, close, complete) = rules(key, chinese, before, after)
    if (left.isNullOrEmpty() && close.isNullOrEmpty()) return write(ic, text, false)
    val last = text.lastOrNull()?.toString()
    if (last != key && last != left && last != right && last != close) return write(ic, text, false)
    val prefix = text.dropLast(1)
    if (prefix.isNotEmpty() && !write(ic, prefix, false)) return false
    val owned = pairs.lastOrNull { it.position == end && it.right == close }
    if (owned != null && ic.after(owned.right.length) == owned.right) {
      if (!ic.finish()) { pairs.clear(); return write(ic, last ?: key, false) }
      finish()
      val next = end + owned.right.length
      if (ic.select(next)) { pairs.remove(owned); caret(next); return true }
      pairs.remove(owned)
      return write(ic, close ?: key, false)
    }
    if (left.isNullOrEmpty() || right.isNullOrEmpty()) return write(ic, close ?: last ?: key, false)
    if (!write(ic, left, false)) return false
    if (!complete) return true
    // Recheck after committing a composition; apps may perform their own auto-pairing.
    if (ic.after(right.length) == right) return true
    val inside = end
    if (!write(ic, right, false)) return true // Opener succeeded; do not duplicate it on fallback.
    if (!ic.select(inside)) {
      // We are still after the inserted closer; remove only that verified character.
      if (ic.before(right.length) == right && ic.delete(right.length, 0)) {
        replaced(inside, inside + right.length, 0)
      }
      pairs.clear()
      return true
    }
    caret(inside)
    if (pairs.size == 32) pairs.removeAt(0)
    pairs.add(PairMark(inside, left, right))
    return true
  }

  fun backspace(ic: InputConnection) = backspace(InputSymbolEditor(ic))
  fun deletedBackward(length: Int?) {
    if (length == null || start < length || start != end || compositionStart >= 0) reset()
    else replaced(start - length, end, 0)
  }
  fun backspace(ic: SymbolEditor): Boolean {
    if (compositionStart >= 0 || start < 0 || start != end) return false
    val pair = pairs.lastOrNull { it.position == end } ?: return false
    if (ic.before(pair.left.length) != pair.left || ic.after(pair.right.length) != pair.right) return false
    if (!ic.delete(pair.left.length, pair.right.length)) return false
    pairs.remove(pair)
    replaced(end - pair.left.length, end + pair.right.length, 0)
    return true
  }
}
