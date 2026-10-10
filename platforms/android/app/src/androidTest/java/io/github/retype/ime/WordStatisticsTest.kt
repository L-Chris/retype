package io.github.retype.ime

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.time.ZonedDateTime
import java.util.UUID
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class WordStatisticsTest {
  @Test fun englishChosenInChineseModeKeepsEnglishCountsAndSpeed() {
    val clock = ActivityClock()
    clock.tick(true, 1000)
    clock.tick(true, 3400)
    val counts = clock.commit("你Hello", true)
    assertEquals(1L, counts.chinese)
    assertEquals(5L, counts.english)
    assertEquals(1L, counts.englishWords)
    assertEquals(400L, counts.chineseMs)
    assertEquals(2000L, counts.englishWordMs)
    assertEquals(0L, clock.boundary().englishWords)
    clock.tick(true, 4000)
    val next = clock.commit("world", true)
    assertEquals(0L, next.chinese)
    assertEquals(1L, next.englishWords)
    assertEquals(600L, next.englishWordMs)
  }
  @Test fun committedFragmentsAndBoundariesUseSharedCounter() {
    val clock = ActivityClock()
    clock.tick(false, 1000)
    assertEquals(0L, clock.commit("hel", false).englishWords)
    clock.backspace()
    clock.tick(false, 1500)
    val committed = clock.commit("llo ", false)
    assertEquals(1L, committed.englishWords)
    assertEquals(500L, committed.englishWordMs)
    assertEquals(0L, clock.boundary().englishWords)
    assertEquals(0L, clock.commit("don’", false).englishWords)
    assertEquals(1L, clock.commit("t ", false).englishWords)
    clock.commit("word", false)
    assertEquals(1L, clock.boundary().englishWords)
    assertEquals(0L, clock.boundary().englishWords)
    clock.commit("x", false)
    clock.backspace()
    assertEquals(0L, clock.boundary().englishWords)
    clock.reset()
    assertEquals(0L, clock.boundary().englishWords)
    assertNull(Counts(englishWords = 4, englishWordMs = 60000).speed(false))
    assertEquals(30L, Counts(english = 5000, englishMs = 600000,
      englishWords = 5, englishWordMs = 10000).speed(false))
  }

  @Test fun databaseUpgradeAndOldSnapshotsPreserveWordMetrics() {
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    val name = "word-statistics-test-${UUID.randomUUID()}.db"
    val minute = ZonedDateTime.now().toEpochSecond() / 60
    val device = "a".repeat(32)
    context.openOrCreateDatabase(name, 0, null).use { db ->
      db.execSQL("CREATE TABLE buckets(device TEXT,stream TEXT,minute INTEGER,chinese INTEGER,english INTEGER,chinese_ms INTEGER,english_ms INTEGER,PRIMARY KEY(device,stream,minute))")
      db.execSQL("INSERT INTO buckets VALUES(?, 'android.log', ?,10,2000,10000,600000)", arrayOf<Any>(device, minute))
      db.version = 1
    }
    val statistics = TypingStatistics(context, name)
    try {
      val old = statistics.export()
      assertEquals(2000L, old.getJSONObject(0).getJSONObject("counts").getLong("english"))
      assertEquals(0L, statistics.recent().englishWords)
      val row = JSONObject(old.getJSONObject(0).toString())
      row.getJSONObject("counts").put("english_words", 5).put("english_word_ms", 10000)
      statistics.merge(JSONArray().put(row))
      statistics.merge(old)
      statistics.merge(JSONArray().put(row))
      assertEquals(5L, statistics.recent().englishWords)
      assertEquals(30L, statistics.recent().speed(false))
      val desktop = JSONObject(row.toString()).put("stream", "desktop.log")
      desktop.getJSONObject("counts").put("english_words", 900)
      statistics.merge(JSONArray().put(desktop))
      assertEquals(5L, statistics.summary(0).current.englishWords)
      assertEquals(2, statistics.export().length())
    } finally {
      statistics.close()
      context.deleteDatabase(name)
    }
  }
}
