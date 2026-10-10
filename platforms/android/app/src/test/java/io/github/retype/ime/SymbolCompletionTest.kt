package io.github.retype.ime

import org.junit.Assert.*
import org.junit.Test

class SymbolCompletionTest {
  private class Editor(initial: String = "", position: Int = initial.length) : SymbolEditor {
    var text = initial
    var start = position
    var end = position
    var composingStart: Int? = null
    var composingEnd = position
    var readable = true
    var movable = true
    var writable = true
    private fun write(value: String, composing: Boolean): Boolean {
      if (!writable) return false
      val a = composingStart ?: start
      val b = if (composingStart == null) end else composingEnd
      text = text.substring(0,a) + value + text.substring(b)
      start = a + value.length; end = start
      composingStart = if (composing) a else null
      composingEnd = start
      return true
    }
    override fun commit(text: String) = write(text, false)
    override fun compose(text: String) = write(text, true)
    override fun before(length: Int): String? { check(length <= 2); return if (readable) text.substring(0,start).takeLast(length) else null }
    override fun after(length: Int): String? { check(length <= 2); return if (readable) text.substring(end).take(length) else null }
    override fun select(position: Int): Boolean { if (!movable) return false; start = position; end = position; return true }
    override fun finish(): Boolean { composingStart = null; return true }
    override fun delete(before: Int, after: Int): Boolean {
      if (!writable) return false
      text = text.substring(0,start-before) + text.substring(end+after)
      start -= before; end = start
      return true
    }
  }
  // Adapter tests inject rules; the exact cross-platform mappings are tested in Rust.
  private fun adapter() = SymbolCompletion { key, chinese, before, after ->
    val pair = when(key) {
      "(" -> if (chinese) "（" to "）" else "(" to ")"
      "[" -> if (chinese) "【" to "】" else "[" to "]"
      "\"" -> if (chinese) "“" to "”" else "\"" to "\""
      "'" -> "'" to "'"
      else -> null
    }
    val close = when(key) { ")" -> if(chinese) "）" else ")"; "]" -> if(chinese) "】" else "]"; "\"", "'" -> pair?.second; else -> null }
    val eligible = pair != null && after != pair.second && !(key == "'" && before.lastOrNull()?.isLetterOrDigit() == true)
    SymbolRule(pair?.first, pair?.second, close, eligible)
  }
  @Test fun nestedPairsComposeCommitAndSkipOnlyOwnedClosers() {
    val e = Editor(); val a = adapter(); a.reset(0)
    assertTrue(a.commit(e,"(","(",enabled=true))
    assertEquals("（）",e.text); assertEquals(1,e.start)
    a.commit(e,"[","[",enabled=true)
    a.compose(e,"ni"); a.compose(e,"nihao")
    a.commit(e,"你好",enabled=true)
    a.commit(e,"]","]",enabled=true)
    assertEquals("（【你好】）",e.text); assertEquals(5,e.start)
    assertFalse(a.backspace(e)) // Nonempty pairs leave ordinary deletion to the editor.
    a.commit(e,")",")",enabled=true)
    assertEquals("（【你好】）",e.text); assertEquals(6,e.start)
    a.commit(e,")",")",enabled=true)
    assertEquals("（【你好】））",e.text)
  }
  @Test fun emptyPairDeletionAndCompositionCancellation() {
    val e = Editor(); val a = adapter(); a.reset(0)
    a.commit(e,"(","(",enabled=true)
    a.compose(e,"ni"); a.commit(e,"",enabled=true); a.finish(); e.finish()
    assertEquals("（）",e.text)
    assertTrue(a.backspace(e)); assertEquals("",e.text)
  }
  @Test fun ordinaryDeletionRebasesClosersUntilPairIsEmpty() {
    val e = Editor(); val a = adapter(); a.reset(0)
    a.commit(e,"(","(",enabled=true)
    a.commit(e,"[","[",enabled=true)
    a.commit(e,"你好",enabled=true)
    repeat(2) { e.delete(1,0); a.deletedBackward(1) }
    assertEquals("（【】）",e.text)
    assertTrue(a.backspace(e)); assertEquals("（）",e.text)
    assertTrue(a.backspace(e)); assertEquals("",e.text)
  }
  @Test fun wordApostrophesAndTextFromOtherSourcesStayLiteral() {
    val e = Editor(); val a = adapter(); a.reset(0)
    a.compose(e,"don"); a.compose(e,"don't")
    a.commit(e,"don't",enabled=true)
    a.commit(e,"'","'",chinese=false,enabled=true)
    assertEquals("don't'",e.text)
    a.commit(e,"(paste)",enabled=true)
    assertEquals("don't'(paste)",e.text)
    a.commit(e,"\"","\"",chinese=false,enabled=true)
    a.commit(e,"hello",enabled=true)
    a.commit(e,"\"","\"",chinese=false,enabled=true)
    assertEquals("don't'(paste)\"hello\"",e.text)
  }
  @Test fun chineseQuotesNormalizeAlternatingKernelOutputWithWordCommit() {
    val e = Editor(); val a = adapter(); a.reset(0)
    a.commit(e,"”","\"",enabled=true)
    assertEquals("“”",e.text)
    a.compose(e,"nihao")
    a.commit(e,"你好“","\"",enabled=true)
    assertEquals("“你好”",e.text); assertEquals(4,e.start)
  }
  @Test fun unsupportedEditorsSelectionsAndDisabledSettingDegradeToSingleSymbol() {
    for (kind in 0..3) {
      val e = Editor("abc",0); val a = adapter(); a.reset(0)
      when(kind) { 0 -> e.readable=false; 1 -> e.movable=false; 2 -> { e.end=3; a.reset(0,3) } }
      assertTrue(a.commit(e,"(","(",chinese=false,enabled=kind!=3))
      assertEquals(if(kind==2) "(" else "(abc",e.text)
      assertFalse(a.backspace(e))
    }
    val e = Editor(")",0); val a = adapter(); a.reset(0)
    a.commit(e,"(","(",chinese=false,enabled=true)
    assertEquals("()",e.text); assertFalse(a.backspace(e))
    a.commit(e,")",")",chinese=false,enabled=true)
    assertEquals("())",e.text) // The pre-existing closer is not retype-owned.
  }
  @Test fun delayedOwnCallbacksRetainOwnershipButUserMovementInvalidatesIt() {
    val e = Editor(); val a = adapter(); a.reset(0)
    a.commit(e,"(","(",enabled=true)
    a.selectionChanged(1,1); a.selectionChanged(2,2); a.selectionChanged(1,1)
    assertTrue(a.backspace(e)); assertEquals("",e.text)
    a.commit(e,"(","(",enabled=true)
    a.selectionChanged(1,1); a.selectionChanged(2,2); a.selectionChanged(1,1)
    e.select(0); a.selectionChanged(0,0)
    e.select(1); a.selectionChanged(1,1)
    assertFalse(a.backspace(e)); assertEquals("（）",e.text)
  }
}
