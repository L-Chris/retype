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
  @Test fun versionMigrationFiltersEveryMetricAndKeepsUnknownHistory() {
    val context=InstrumentationRegistry.getInstrumentation().targetContext
    val name="version-statistics-${UUID.randomUUID()}.db"
    val minute=ZonedDateTime.now().toEpochSecond()/60
    val device="a".repeat(32)
    context.openOrCreateDatabase(name,0,null).use { db ->
      db.execSQL("CREATE TABLE buckets(device TEXT,stream TEXT,minute INTEGER,chinese INTEGER,english INTEGER,chinese_ms INTEGER,english_ms INTEGER,english_words INTEGER NOT NULL DEFAULT 0,english_word_ms INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(device,stream,minute))")
      db.execSQL("INSERT INTO buckets VALUES(?,'android.log',?,0,100,0,10000,5,10000)",arrayOf<Any>(device,minute))
      db.version=2
    }
    val statistics=TypingStatistics(context,name)
    fun row(version:String,words:Int,stream:String="android@$version.log")=JSONObject()
      .put("device",device).put("stream",stream).put("version",version).put("minute",minute)
      .put("counts",JSONObject().put("chinese",0).put("english",100).put("chinese_ms",0).put("english_ms",10000).put("english_words",words).put("english_word_ms",10000))
    try {
      assertEquals("",statistics.export().getJSONObject(0).getString("version"))
      val rows=JSONArray().put(row("0.8.0",5)).put(row("0.9.0",20))
      statistics.merge(rows);statistics.merge(rows)
      assertEquals(setOf("","0.8.0","0.9.0"),statistics.versions().toSet())
      assertEquals(30L,statistics.recent().englishWords)
      assertEquals(60L,statistics.recent().speed(false))
      assertEquals(5L,statistics.recent(version="0.8.0").englishWords)
      assertEquals(5L,statistics.recent(version="").englishWords)
      for(period in 0..3) {
        assertEquals(30L,statistics.summary(period).current.englishWords)
        assertEquals(20L,statistics.summary(period,version="0.9.0").current.englishWords)
      }
      statistics.merge(JSONArray().put(row("0.9.0",2,"android-voice@0.9.0.log")))
      assertEquals(2L,statistics.voiceTotal().englishWords)
      assertEquals(0L,statistics.voiceTotal("0.8.0").englishWords)
      assertEquals(30L,statistics.summary(0).current.englishWords)
    } finally { statistics.close();context.deleteDatabase(name) }
  }

  @Test fun forwardingWindowsMeasurementsRetainsVersionAndCorrectionCounters() {
    val context=InstrumentationRegistry.getInstrumentation().targetContext
    val name="forward-statistics-${UUID.randomUUID()}.db"
    val statistics=TypingStatistics(context,name)
    try {
      val lengths=JSONArray()
      for(i in 0..7) lengths.put(JSONObject().put("inputs",if(i==1) 1 else 0).put("corrected",if(i==1) 1 else 0).put("units",if(i==1) 2 else 0))
      val input=JSONObject().put("chinese",lengths).put("cancelled",2).put("spelling_deletions",3)
      val counts=JSONObject().put("chinese",2).put("english",0).put("chinese_ms",10000).put("english_ms",0).put("input",input)
      val row=JSONObject().put("device","a".repeat(32)).put("stream","desktop@0.8.0.log").put("version","0.8.0").put("minute",ZonedDateTime.now().toEpochSecond()/60).put("counts",counts)
      statistics.merge(JSONArray().put(row))
      val old=JSONObject(row.toString());old.remove("version");old.getJSONObject("counts").remove("input")
      statistics.merge(JSONArray().put(old));statistics.merge(JSONArray().put(row))
      val result=statistics.export()
      assertEquals(1,result.length())
      assertEquals("0.8.0",result.getJSONObject(0).getString("version"))
      assertEquals(1L,result.getJSONObject(0).getJSONObject("counts").getJSONObject("input").getJSONArray("chinese").getJSONObject(1).getLong("corrected"))
      assertEquals(2L,result.getJSONObject(0).getJSONObject("counts").getJSONObject("input").getLong("cancelled"))
    } finally { statistics.close();context.deleteDatabase(name) }
  }

}
