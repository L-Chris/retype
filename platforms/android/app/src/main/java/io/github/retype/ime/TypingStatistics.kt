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
) {
    operator fun plus(o: Counts) =
        Counts(
            chinese + o.chinese,
            english + o.english,
            chineseMs + o.chineseMs,
            englishMs + o.englishMs,
        )

    fun speed(chinese: Boolean): Long? {
        val n = if (chinese) this.chinese else english
        val ms = if (chinese) chineseMs else englishMs
        return if (n < 10 || ms < 1000) null else n * 60000 / ms
    }
}

data class StatisticsSummary(
    val current: Counts,
    val previous: Counts,
    val bars: List<Pair<String, Counts>>,
)

class TypingStatistics private constructor(private val context: Context) :
    SQLiteOpenHelper(context, "statistics.db", null, 1) {
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
            "CREATE TABLE buckets(device TEXT,stream TEXT,minute INTEGER,chinese INTEGER,english INTEGER,chinese_ms INTEGER,english_ms INTEGER,PRIMARY KEY(device,stream,minute))"
        )
    }

    override fun onUpgrade(db: SQLiteDatabase, old: Int, new: Int) {
        error("Unsupported statistics version")
    }

    fun record(c: Counts) {
        if (c.chinese + c.english == 0L) return
        val minute = System.currentTimeMillis() / 60000
        val device = AppStore(context).deviceId()
        writes.launch {
            val db = writableDatabase
            db.beginTransaction()
            try {
                db.execSQL(
                    "INSERT OR IGNORE INTO buckets VALUES(?, 'android.log', ?,0,0,0,0)",
                    arrayOf(device, minute),
                )
                db.execSQL(
                    "UPDATE buckets SET chinese=chinese+?,english=english+?,chinese_ms=chinese_ms+?,english_ms=english_ms+? WHERE device=? AND stream='android.log' AND minute=?",
                    arrayOf(c.chinese, c.english, c.chineseMs, c.englishMs, device, minute),
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
                                    .put("english_ms", cursor.getLong(6)),
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
                    "INSERT OR IGNORE INTO buckets VALUES(?,?,?,0,0,0,0)",
                    arrayOf(b.getString("device"), b.getString("stream"), b.getLong("minute")),
                )
                require(
                    listOf("chinese", "english", "chinese_ms", "english_ms").all {
                        c.getLong(it) in 0..1000000000000000L
                    }
                )
                db.execSQL(
                    "UPDATE buckets SET chinese=MAX(chinese,?),english=MAX(english,?),chinese_ms=MAX(chinese_ms,?),english_ms=MAX(english_ms,?) WHERE device=? AND stream=? AND minute=?",
                    arrayOf(
                        c.getLong("chinese"),
                        c.getLong("english"),
                        c.getLong("chinese_ms"),
                        c.getLong("english_ms"),
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
                "SELECT COALESCE(SUM(chinese),0),COALESCE(SUM(english),0),COALESCE(SUM(chinese_ms),0),COALESCE(SUM(english_ms),0) FROM buckets WHERE stream='android.log' AND minute BETWEEN ? AND ?",
                arrayOf((minute - 4).toString(), minute.toString()),
            )
            .use { c ->
                c.moveToFirst()
                return Counts(c.getLong(0), c.getLong(1), c.getLong(2), c.getLong(3))
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
                "SELECT minute,SUM(chinese),SUM(english),SUM(chinese_ms),SUM(english_ms) FROM buckets WHERE stream='android.log' AND minute>=? AND minute<=? GROUP BY minute",
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

    fun tick(chinese: Boolean, now: Long = SystemClock.elapsedRealtime()) {
        if (language != chinese) pending = 0
        if (last != 0L && language == chinese && now - last in 0..15000) pending += now - last
        last = now
        language = chinese
    }

    fun commit(count: Long, chinese: Boolean): Counts {
        val ms = if (language == chinese) pending else 0
        pending = 0
        return if (chinese) Counts(chinese = count, chineseMs = ms)
        else Counts(english = count, englishMs = ms)
    }

    fun reset() {
        last = 0
        pending = 0
    }
}
