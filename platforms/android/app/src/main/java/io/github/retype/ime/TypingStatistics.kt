package io.github.retype.ime

import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import android.os.SystemClock
import java.time.*
import kotlinx.coroutines.*
import org.json.JSONArray
import org.json.JSONObject

data class Counts(
    val chinese: Long = 0,
    val english: Long = 0,
    val chineseMs: Long = 0,
    val englishMs: Long = 0,
    val englishWords: Long = 0,
    val englishWordMs: Long = 0,
) {
    operator fun plus(o: Counts) =
        Counts(
            chinese + o.chinese,
            english + o.english,
            chineseMs + o.chineseMs,
            englishMs + o.englishMs,
            englishWords + o.englishWords,
            englishWordMs + o.englishWordMs,
        )

    fun speed(chinese: Boolean): Long? {
        val n = if (chinese) this.chinese else englishWords
        val ms = if (chinese) chineseMs else englishWordMs
        return if (n < (if (chinese) 10 else 5) || ms < 10000) null else kotlin.math.round(n * 60000.0 / ms).toLong()
    }
}

data class StatisticsSummary(
    val current: Counts,
    val previous: Counts,
    val bars: List<Pair<String, Counts>>,
)

class TypingStatistics internal constructor(private val context: Context, name: String = "statistics.db") :
    SQLiteOpenHelper(context, name, null, 2) {
    companion object {
        @Volatile private var instance: TypingStatistics? = null

        fun get(context: Context) =
            instance
                ?: synchronized(this) {
                    instance ?: TypingStatistics(context.applicationContext).also { instance = it }
                }

        private val writes =
            CoroutineScope(
                SupervisorJob() +
                    Dispatchers.IO.limitedParallelism(1) +
                    CoroutineExceptionHandler { _, error ->
                        android.util.Log.e("retype", "Statistics write failed", error)
                    }
            )
    }

    override fun onCreate(db: SQLiteDatabase) {
        db.execSQL(
            "CREATE TABLE buckets(device TEXT,stream TEXT,minute INTEGER,chinese INTEGER,english INTEGER,chinese_ms INTEGER,english_ms INTEGER,english_words INTEGER NOT NULL DEFAULT 0,english_word_ms INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(device,stream,minute))"
        )
    }

    override fun onUpgrade(db: SQLiteDatabase, old: Int, new: Int) {
        if (old < 2) {
            db.execSQL("ALTER TABLE buckets ADD COLUMN english_words INTEGER NOT NULL DEFAULT 0")
            db.execSQL("ALTER TABLE buckets ADD COLUMN english_word_ms INTEGER NOT NULL DEFAULT 0")
        }
    }

    fun record(c: Counts) {
        if (c == Counts()) return
        val minute = System.currentTimeMillis() / 60000
        val device = AppStore(context).deviceId()
        writes.launch {
            val db = writableDatabase
            db.beginTransaction()
            try {
                db.execSQL(
                    "INSERT OR IGNORE INTO buckets VALUES(?, 'android.log', ?,0,0,0,0,0,0)",
                    arrayOf<Any>(device, minute),
                )
                db.execSQL(
                    "UPDATE buckets SET chinese=chinese+?,english=english+?,chinese_ms=chinese_ms+?,english_ms=english_ms+?,english_words=english_words+?,english_word_ms=english_word_ms+? WHERE device=? AND stream='android.log' AND minute=?",
                    arrayOf<Any>(c.chinese, c.english, c.chineseMs, c.englishMs, c.englishWords, c.englishWordMs, device, minute),
                )
                db.setTransactionSuccessful()
            } finally {
                db.endTransaction()
            }
        }
    }

    fun export(): JSONArray {
        val result = JSONArray()
        readableDatabase
            .rawQuery("SELECT * FROM buckets ORDER BY device,stream,minute", null)
            .use { cursor ->
                while (cursor.moveToNext()) {
                    result.put(
                        JSONObject()
                            .put("device", cursor.getString(0))
                            .put("stream", cursor.getString(1))
                            .put("minute", cursor.getLong(2))
                            .put(
                                "counts",
                                JSONObject()
                                    .put("chinese", cursor.getLong(3))
                                    .put("english", cursor.getLong(4))
                                    .put("chinese_ms", cursor.getLong(5))
                                    .put("english_ms", cursor.getLong(6))
                                    .put("english_words", cursor.getLong(7))
                                    .put("english_word_ms", cursor.getLong(8)),
                            )
                    )
                }
            }
        return result
    }

    fun merge(buckets: JSONArray) {
        require(buckets.length() <= 200000)
        val db = writableDatabase
        db.beginTransaction()
        try {
            for (i in 0 until buckets.length()) {
                val b = buckets.getJSONObject(i)
                val c = b.getJSONObject("counts")
                require(
                    b.getString("device").matches(Regex("[a-f0-9]{32}")) &&
                        b.getString("stream").endsWith(".log") &&
                        b.getLong("minute") in 0..50000000
                )
                db.execSQL(
                    "INSERT OR IGNORE INTO buckets VALUES(?,?,?,0,0,0,0,0,0)",
                    arrayOf(b.getString("device"), b.getString("stream"), b.getLong("minute")),
                )
                require(
                    listOf("chinese", "english", "chinese_ms", "english_ms").all {
                        c.getLong(it) in 0..1000000000000000L
                    }
                )
                require(listOf("english_words", "english_word_ms").all { c.optLong(it, 0) in 0..1000000000000000L })
                db.execSQL(
                    "UPDATE buckets SET chinese=MAX(chinese,?),english=MAX(english,?),chinese_ms=MAX(chinese_ms,?),english_ms=MAX(english_ms,?),english_words=MAX(english_words,?),english_word_ms=MAX(english_word_ms,?) WHERE device=? AND stream=? AND minute=?",
                    arrayOf(
                        c.getLong("chinese"),
                        c.getLong("english"),
                        c.getLong("chinese_ms"),
                        c.getLong("english_ms"),
                        c.optLong("english_words", 0),
                        c.optLong("english_word_ms", 0),
                        b.getString("device"),
                        b.getString("stream"),
                        b.getLong("minute"),
                    ),
                )
            }
            db.setTransactionSuccessful()
        } finally {
            db.endTransaction()
        }
    }

    fun recent(now: ZonedDateTime = ZonedDateTime.now()): Counts {
        val minute = now.toEpochSecond() / 60
        readableDatabase
            .rawQuery(
                "SELECT COALESCE(SUM(chinese),0),COALESCE(SUM(english),0),COALESCE(SUM(chinese_ms),0),COALESCE(SUM(english_ms),0),COALESCE(SUM(english_words),0),COALESCE(SUM(english_word_ms),0) FROM buckets WHERE stream='android.log' AND minute BETWEEN ? AND ?",
                arrayOf((minute - 4).toString(), minute.toString()),
            )
            .use { c ->
                c.moveToFirst()
                return Counts(c.getLong(0), c.getLong(1), c.getLong(2), c.getLong(3), c.getLong(4), c.getLong(5))
            }
    }

    fun summary(period: Int, now: ZonedDateTime = ZonedDateTime.now()): StatisticsSummary {
        val date = now.toLocalDate()
        val zone = now.zone
        val start =
            when (period) {
                0 -> date
                1 -> date.minusDays((date.dayOfWeek.value - 1).toLong())
                2 -> date.withDayOfMonth(1)
                else -> date.withDayOfYear(1)
            }
        fun previous(d: LocalDate) =
            when (period) {
                0 -> d.minusDays(1)
                1 -> d.minusWeeks(1)
                2 -> d.minusMonths(1)
                else -> d.minusYears(1)
            }
        val prev = previous(start)
        val startMinute = start.atStartOfDay(zone).toEpochSecond() / 60
        var total = Counts()
        var old = Counts()
        val bars = linkedMapOf<String, Counts>()
        if (period == 0) (0..23).forEach { bars["$it"] = Counts() }
        else if (period == 3) (1..12).forEach { bars["$it 月"] = Counts() }
        else {
            var d = start
            val end = if (period == 1) start.plusWeeks(1) else start.plusMonths(1)
            while (d < end) {
                bars["${d.monthValue}/${d.dayOfMonth}"] = Counts()
                d = d.plusDays(1)
            }
        }
        val params =
            arrayOf(
                (prev.atStartOfDay(zone).toEpochSecond() / 60).toString(),
                (now.toEpochSecond() / 60).toString(),
            )
        readableDatabase
            .rawQuery(
                "SELECT minute,SUM(chinese),SUM(english),SUM(chinese_ms),SUM(english_ms),SUM(english_words),SUM(english_word_ms) FROM buckets WHERE stream='android.log' AND minute>=? AND minute<=? GROUP BY minute",
                params,
            )
            .use { cursor ->
                while (cursor.moveToNext()) {
                    val minute = cursor.getLong(0)
                    val c =
                        Counts(
                            cursor.getLong(1),
                            cursor.getLong(2),
                            cursor.getLong(3),
                            cursor.getLong(4),
                            cursor.getLong(5),
                            cursor.getLong(6),
                        )
                    if (minute < startMinute) old += c
                    else {
                        total += c
                        val d = Instant.ofEpochSecond(minute * 60).atZone(zone)
                        val key =
                            when (period) {
                                0 -> "${d.hour}"
                                3 -> "${d.monthValue} 月"
                                else -> "${d.monthValue}/${d.dayOfMonth}"
                            }
                        bars[key] = (bars[key] ?: Counts()) + c
                    }
                }
            }
        return StatisticsSummary(total, old, bars.toList())
    }
}

class ActivityClock {
    private var last = 0L
    private var language = true
    private var pending = 0L
    private var wordState = JSONArray()
    private var wordCounterAvailable = true

    fun tick(chinese: Boolean, now: Long = SystemClock.elapsedRealtime()) {
        if (language != chinese) pending = 0
        if (last != 0L && language == chinese && now - last in 0..15000) pending += now - last
        last = now
        language = chinese
    }

    private fun words(text: String = "", boundary: Boolean = false, backspace: Boolean = false): Long {
        if (!wordCounterAvailable || (text.isEmpty() && wordState.length() == 0)) return 0
        return try {
            val result = JSONObject(NativeBridge.feature(JSONObject().put("type", "countWords")
                .put("state", wordState).put("text", text).put("boundary", boundary)
                .put("backspace", backspace).toString()))
            wordState = result.getJSONArray("state")
            result.getLong("words")
        } catch (error: LinkageError) {
            wordCounterAvailable = false
            android.util.Log.w("retype", "Word counter unavailable; keeping literal input", error)
            0
        } catch (error: Exception) {
            wordCounterAvailable = false
            android.util.Log.w("retype", "Word counter failed; keeping literal input", error)
            0
        }
    }

    fun boundary(): Counts = Counts(englishWords = words(boundary = true))

    fun backspace() { words(backspace = true) }

    fun commit(text: String, chinese: Boolean): Counts {
        val count = text.codePoints().filter { !Character.isWhitespace(it) && !Character.isISOControl(it) }.count()
        val ms = if (language == chinese) pending else 0
        pending = 0
        return if (chinese) Counts(chinese = count, chineseMs = ms)
        else Counts(english = count, englishMs = ms, englishWords = words(text), englishWordMs = ms)
    }

    fun reset() {
        last = 0
        pending = 0
        wordState = JSONArray()
    }
}
