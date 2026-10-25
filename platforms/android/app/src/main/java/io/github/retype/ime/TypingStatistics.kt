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
    val input: InputMeasurements? = null,
) {
    operator fun plus(o: Counts) =
        Counts(
            chinese + o.chinese,
            english + o.english,
            chineseMs + o.chineseMs,
            englishMs + o.englishMs,
            englishWords + o.englishWords,
            englishWordMs + o.englishWordMs,
            if(input==null) o.input else if(o.input==null) input else input+o.input,
        )

    fun speed(chinese: Boolean): Long? {
        val n = if (chinese) this.chinese else englishWords
        val ms = if (chinese) chineseMs else englishWordMs
        return NativeBridge.statisticsSpeed(n, ms, if (chinese) 10 else 5).takeIf { it >= 0 }
    }
}

data class StatisticsSummary(
    val current: Counts,
    val previous: Counts,
    val bars: List<Pair<String, Counts>>,
)

class TypingStatistics internal constructor(private val context: Context, name: String = "statistics.db") :
    SQLiteOpenHelper(context, name, null, 4) {
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
            "CREATE TABLE buckets(device TEXT,stream TEXT,minute INTEGER,chinese INTEGER,english INTEGER,chinese_ms INTEGER,english_ms INTEGER,english_words INTEGER NOT NULL DEFAULT 0,english_word_ms INTEGER NOT NULL DEFAULT 0,version TEXT NOT NULL DEFAULT '',input TEXT NOT NULL DEFAULT '{}',PRIMARY KEY(device,stream,minute,version))"
        )
    }

    override fun onUpgrade(db: SQLiteDatabase, old: Int, new: Int) {
        if (old < 2) {
            db.execSQL("ALTER TABLE buckets ADD COLUMN english_words INTEGER NOT NULL DEFAULT 0")
            db.execSQL("ALTER TABLE buckets ADD COLUMN english_word_ms INTEGER NOT NULL DEFAULT 0")
        }
        if (old < 3) {
            db.execSQL("ALTER TABLE buckets RENAME TO buckets_legacy")
            onCreate(db)
            db.execSQL("INSERT INTO buckets(device,stream,minute,chinese,english,chinese_ms,english_ms,english_words,english_word_ms) SELECT device,stream,minute,chinese,english,chinese_ms,english_ms,english_words,english_word_ms FROM buckets_legacy")
            db.execSQL("DROP TABLE buckets_legacy")
        }
        if(old<4) db.execSQL("UPDATE buckets SET input='{}'")
    }

    fun record(c: Counts, stream: String = "android.log") {
        if (c == Counts()) return
        val minute = System.currentTimeMillis() / 60000
        val version=BuildConfig.VERSION_NAME
        val source=stream.removeSuffix(".log")+"@"+version+".log"
        val device = AppStore(context).deviceId()
        writes.launch {
            val db = writableDatabase
            db.beginTransaction()
            try {
                db.execSQL(
                    "INSERT OR IGNORE INTO buckets(device,stream,minute,version,chinese,english,chinese_ms,english_ms) VALUES(?,?,?,?,0,0,0,0)",
                    arrayOf<Any>(device, source, minute, version),
                )
                db.execSQL(
                    "UPDATE buckets SET chinese=chinese+?,english=english+?,chinese_ms=chinese_ms+?,english_ms=english_ms+?,english_words=english_words+?,english_word_ms=english_word_ms+? WHERE device=? AND stream=? AND minute=? AND version=?",
                    arrayOf<Any>(c.chinese, c.english, c.chineseMs, c.englishMs, c.englishWords, c.englishWordMs, device, source, minute, version),
                )
                c.input?.let { delta ->
                    val keys=arrayOf(device,source,minute.toString(),version)
                    db.rawQuery("SELECT input FROM buckets WHERE device=? AND stream=? AND minute=? AND version=?",keys).use { cursor ->
                        cursor.moveToFirst()
                        val old=InputMeasurements.fromJson(JSONObject(cursor.getString(0)))
                        db.execSQL("UPDATE buckets SET input=? WHERE device=? AND stream=? AND minute=? AND version=?",arrayOf((old?.plus(delta)?:delta).toJson().toString(),*keys))
                    }
                }
                db.setTransactionSuccessful()
            } finally {
                db.endTransaction()
            }
        }
    }

    fun export(): JSONArray {
        val result = JSONArray()
        readableDatabase
            .rawQuery("SELECT * FROM buckets ORDER BY device,stream,minute,version", null)
            .use { cursor ->
                while (cursor.moveToNext()) {
                    result.put(
                        JSONObject()
                            .put("device", cursor.getString(0))
                            .put("stream", cursor.getString(1))
                            .put("minute", cursor.getLong(2))
                            .put("version",cursor.getString(9))
                            .put(
                                "counts",
                                JSONObject()
                                    .put("chinese", cursor.getLong(3))
                                    .put("english", cursor.getLong(4))
                                    .put("chinese_ms", cursor.getLong(5))
                                    .put("english_ms", cursor.getLong(6))
                                    .put("english_words", cursor.getLong(7))
                                    .put("english_word_ms", cursor.getLong(8))
                                    .put("input_detail",JSONObject(cursor.getString(10))),
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
                val stream=b.getString("stream")
                val version=b.optString("version", "").ifEmpty { if ('@' in stream) stream.removeSuffix(".log").substringAfterLast('@') else "" }
                require(version.length<=64 && version.matches(Regex("[a-zA-Z0-9.+-]*")) && stream.length<=128)
                val keys=arrayOf(b.getString("device"),stream,b.getLong("minute").toString(),version)
                require(
                    b.getString("device").matches(Regex("[a-f0-9]{32}")) &&
                        b.getString("stream").endsWith(".log") &&
                        b.getLong("minute") in 0..50000000
                )
                db.execSQL(
                    "INSERT OR IGNORE INTO buckets(device,stream,minute,version,chinese,english,chinese_ms,english_ms) VALUES(?,?,?,?,0,0,0,0)",
                    keys,
                )
                require(
                    listOf("chinese", "english", "chinese_ms", "english_ms").all {
                        c.getLong(it) in 0..1000000000000000L
                    }
                )
                require(listOf("english_words", "english_word_ms").all { c.optLong(it, 0) in 0..1000000000000000L })
                db.execSQL(
                    "UPDATE buckets SET chinese=MAX(chinese,?),english=MAX(english,?),chinese_ms=MAX(chinese_ms,?),english_ms=MAX(english_ms,?),english_words=MAX(english_words,?),english_word_ms=MAX(english_word_ms,?) WHERE device=? AND stream=? AND minute=? AND version=?",
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
                        version,
                    ),
                )
                db.rawQuery("SELECT input FROM buckets WHERE device=? AND stream=? AND minute=? AND version=?",keys).use { cursor ->
                    cursor.moveToFirst()
                    val input=mergeInput(JSONObject(cursor.getString(0)),c.optJSONObject("input_detail")?:JSONObject())
                    db.execSQL("UPDATE buckets SET input=? WHERE device=? AND stream=? AND minute=? AND version=?",arrayOf(input.toString(),*keys))
                }
            }
            db.setTransactionSuccessful()
        } finally {
            db.endTransaction()
        }
    }

    fun versions(): List<String> = readableDatabase.rawQuery("SELECT DISTINCT version FROM buckets WHERE stream='android.log' OR stream LIKE 'android@%.log' OR stream='android-voice.log' OR stream LIKE 'android-voice@%.log' ORDER BY version DESC",null).use { c -> buildList { while(c.moveToNext()) add(c.getString(0)) } }

    fun recent(now: ZonedDateTime = ZonedDateTime.now(), version:String?=null): Counts {
        val minute = now.toEpochSecond() / 60
        readableDatabase
            .rawQuery(
                "SELECT COALESCE(SUM(chinese),0),COALESCE(SUM(english),0),COALESCE(SUM(chinese_ms),0),COALESCE(SUM(english_ms),0),COALESCE(SUM(english_words),0),COALESCE(SUM(english_word_ms),0) FROM buckets WHERE (stream='android.log' OR stream LIKE 'android@%.log') AND minute BETWEEN ? AND ?${if(version==null) "" else " AND version=?"}",
                (listOf((minute - 4).toString(), minute.toString())+listOfNotNull(version)).toTypedArray(),
            )
            .use { c ->
                c.moveToFirst()
                return Counts(c.getLong(0), c.getLong(1), c.getLong(2), c.getLong(3), c.getLong(4), c.getLong(5))
            }
    }

    fun summary(period: Int, now: ZonedDateTime = ZonedDateTime.now(), version:String?=null): StatisticsSummary {
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
            (listOf(
                (prev.atStartOfDay(zone).toEpochSecond() / 60).toString(),
                (now.toEpochSecond() / 60).toString(),
            )+listOfNotNull(version)).toTypedArray()
        readableDatabase
            .rawQuery(
                "SELECT minute,chinese,english,chinese_ms,english_ms,english_words,english_word_ms,input FROM buckets WHERE (stream='android.log' OR stream LIKE 'android@%.log') AND minute>=? AND minute<=?${if(version==null) "" else " AND version=?"} ORDER BY minute",
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
                            InputMeasurements.fromJson(JSONObject(cursor.getString(7))),
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

class ActivityClock : AutoCloseable {
    private var measurements=0L
    private var measurementsAvailable=true
    private fun handle():Long {
        if(measurements==0L) measurements=NativeBridge.statisticsCreate()
        return measurements
    }
    private fun measure(operation:Int,text:String=""):InputMeasurements? {
        if(!measurementsAvailable)return null
        return try { InputMeasurements.fromNative(NativeBridge.statisticsUpdate(handle(),operation,text)) }
        catch(error:LinkageError){measurementsAvailable=false;null}
        catch(error:Exception){measurementsAvailable=false;android.util.Log.w("retype","Input measurement unavailable",error);null}
    }
    fun compositionEdit(before:Boolean,beforeLength:Int,after:Boolean,afterLength:Int,backspace:Boolean) {
        if(!measurementsAvailable)return
        try { NativeBridge.statisticsEdit(handle(),beforeLength,afterLength,(if(before)1 else 0) or (if(after)2 else 0) or (if(backspace)4 else 0)) }
        catch(error:LinkageError){measurementsAvailable=false}
        catch(error:Exception){measurementsAvailable=false;android.util.Log.w("retype","Input measurement unavailable",error)}
    }
    fun cancelComposition()=Counts(input=measure(2))
    fun endEditor()=Counts(englishWords=words(boundary=true),input=measure(2))
    override fun close() { if(measurements!=0L) { NativeBridge.statisticsDestroy(measurements);measurements=0L } }

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

    fun boundary(): Counts = Counts(englishWords = words(boundary = true),input=measure(1))

    fun backspace():Counts { words(backspace = true);return Counts(input=measure(3)) }

    fun commit(text: String, chinese: Boolean): Counts {
        val ms = if (language == chinese) pending else 0
        pending = 0
        val counts = NativeBridge.statisticsCounts(text, ms)
        return Counts(chinese = counts[0], english = counts[1],
            chineseMs = counts[2], englishMs = counts[3],
            englishWords = words(text, boundary = chinese), englishWordMs = counts[3], input = measure(0, text))
    }

    fun reset() {
        close()
        last = 0
        pending = 0
        wordState = JSONArray()
    }
}

// Voice counts use a separate stream so speed summaries never include them.
fun TypingStatistics.recordVoice(text: String) {
    val result = JSONObject(NativeBridge.feature(JSONObject().put("type","voiceCounts").put("text",text).toString()))
    record(Counts(chinese=result.getLong("chinese"),englishWords=result.getLong("english")), "android-voice.log")
}
fun TypingStatistics.voiceTotal(version:String?=null): Counts {
    readableDatabase.rawQuery("SELECT COALESCE(SUM(chinese),0),COALESCE(SUM(english_words),0) FROM buckets WHERE (stream='android-voice.log' OR stream LIKE 'android-voice@%.log')${if(version==null) "" else " AND version=?"}",version?.let { arrayOf(it) }).use { c->c.moveToFirst();return Counts(chinese=c.getLong(0),englishWords=c.getLong(1)) }
}

// Preserve Windows measurements when Android forwards cloud snapshots. Only
// bounded numeric counters are accepted; unknown fields are not retained.
private fun mergeInput(old:JSONObject,incoming:JSONObject):JSONObject {
    val left=InputMeasurements.fromJson(old)
    val right=InputMeasurements.fromJson(incoming)
    return (if(left==null) right else if(right==null) left else left.mergeMax(right))?.toJson()?:JSONObject()
}
