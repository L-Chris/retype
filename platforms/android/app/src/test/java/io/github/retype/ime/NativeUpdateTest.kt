package io.github.retype.ime

import org.junit.Assert.*
import org.junit.Test

class NativeUpdateTest {
  @Test fun unchangedPreviewIsReusedWhileOldExpandedPagesAreInvalidated() {
    val preview = listOf("你", "呢")
    val pages = mapOf(0 to listOf("旧候选"))
    val previous = KeyboardState(generation = 4, candidates = preview, candidateTotal = 100, candidatePages = pages)
    val current = previous.withNativeUpdate(NativeUpdate(5, "ni", null, 0, 100, 0, true, emptyArray(), false))
    assertSame(preview, current.candidates)
    assertTrue(current.candidatePages.isEmpty())
    val same = previous.withNativeUpdate(NativeUpdate(4, "n", null, 0, 100, 0, true, emptyArray(), false))
    assertSame(pages, same.candidatePages)
    val cleared = current.withNativeUpdate(NativeUpdate(6, "", emptyArray(), 0, 0, 0, true, arrayOf("你"), false))
    assertTrue(cleared.candidates.isEmpty())
    assertEquals(0, cleared.candidateTotal)
    assertEquals("", cleared.composition)
  }
}
