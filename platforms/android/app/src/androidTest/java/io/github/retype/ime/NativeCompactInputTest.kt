package io.github.retype.ime

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class NativeCompactInputTest {
  private val context = InstrumentationRegistry.getInstrumentation().targetContext
  private fun open(flypy: Boolean = false): Long = NativeBridge.create(
      runBlocking { ResourceFixtures.prepare(context) }.absolutePath,
      File(context.cacheDir, "compact-input-test.db").absolutePath, flypy, true, false)

  @Test fun typedJniKeepsEveryCandidateAndRejectsStalePagesAndTaps() {
    for (flypy in listOf(false, true)) {
      val handle = open(flypy)
      try {
        NativeBridge.send(handle, NativeCommand.key("n"))
        val first = NativeBridge.send(handle, NativeCommand.key("i"))
        assertTrue(first.candidateTotal > 8)
        assertEquals(8, first.candidates!!.size)
        val all = mutableListOf<String>()
        for (start in 0 until first.candidateTotal step 64) {
          val page = NativeBridge.candidates(handle, first.generation, start, 64)!!
          assertTrue(page.size <= 64)
          all.addAll(page)
        }
        assertEquals(first.candidateTotal, all.size)
        assertEquals(first.candidates!!.toList(), all.take(8))
        assertNull(NativeBridge.candidates(handle, first.generation, 0, 65))
        val unchanged = NativeBridge.send(handle, NativeCommand.choose(-1, first.generation))
        // Invalid operations must throw rather than becoming an unsigned index.
        fail("negative candidate index accepted: ${unchanged.generation}")
      } catch (e: IllegalStateException) {
        assertTrue(e.message!!.contains("invalid input operation"))
      } finally { NativeBridge.destroy(handle) }
    }
  }

  @Test fun selectedPageAndLearningAcknowledgementUseAbsoluteIndexes() {
    val handle = open()
    try {
      val first = NativeBridge.send(handle, NativeCommand.key("n"))
      val second = NativeBridge.send(handle, NativeCommand.key("i"))
      assertNull(NativeBridge.candidates(handle, first.generation, 0, 64))
      val offset = 16
      val expected = NativeBridge.candidates(handle, second.generation, offset, 1)!![0]
      val stale = NativeBridge.send(handle, NativeCommand.choose(offset, first.generation))
      assertTrue(stale.commits.isEmpty())
      assertNull(stale.candidates)
      val selected = NativeBridge.send(handle, NativeCommand.choose(offset, second.generation))
      assertEquals(expected, selected.commits.single())
      NativeBridge.acknowledge(handle, selected.generation, true)
      assertEquals(0, selected.candidateTotal)
    } finally { NativeBridge.destroy(handle) }
  }

  @Test fun typedAndLegacyCommandsProduceTheSameInputAndCommits() {
    for (flypy in listOf(false, true)) {
      val typed = open(flypy)
      val legacy = open(flypy)
      try {
        for (value in listOf("n", "i", "h", "c", "backspace", "backspace", "space", "h", "e", "l", "l", "o", "enter")) {
          val next = NativeBridge.send(typed, NativeCommand.key(value))
          val previous = JSONObject(NativeBridge.dispatch(legacy, JSONObject().put("type", "key").put("value", value).toString()))
          assertEquals(previous.getString("composition"), next.composition)
          assertEquals(previous.getLong("generation"), next.generation)
          val words = previous.getJSONArray("candidates")
          assertEquals(words.length(), next.candidateTotal)
          val actual = mutableListOf<String>()
          for (start in 0 until next.candidateTotal step 64) actual.addAll(NativeBridge.candidates(typed, next.generation, start, 64)!!)
          assertEquals(List(words.length()) { words.getString(it) }, actual)
          val commits = previous.getJSONArray("commits")
          assertEquals(List(commits.length()) { commits.getString(it) }, next.commits.toList())
          NativeBridge.acknowledge(typed, next.generation, false)
          NativeBridge.dispatch(legacy, JSONObject().put("type", "ack").put("generation", next.generation).put("accepted", false).toString())
        }
      } finally { NativeBridge.destroy(typed); NativeBridge.destroy(legacy) }
    }
  }
}
