package io.github.retype.ime

import org.json.JSONArray
import org.json.JSONObject

internal val inputLengthLabels: List<String> by lazy {
    val labels = JSONObject(NativeBridge.feature(JSONObject().put("type", "featurePolicy").toString())).getJSONObject("statistics").getJSONArray("lengthLabels")
    List(labels.length()) { labels.getString(it) }
}

private fun sum(a: Long, b: Long) = if (a > Long.MAX_VALUE - b) Long.MAX_VALUE else a + b

data class LengthCounts(val inputs: Long = 0, val corrected: Long = 0, val units: Long = 0) {
    operator fun plus(other: LengthCounts) = LengthCounts(sum(inputs, other.inputs), sum(corrected, other.corrected), sum(units, other.units))
    fun rate(): Double? = if (inputs == 0L) null else corrected * 100.0 / inputs
    fun mean(): Double? = if (inputs == 0L) null else units.toDouble() / inputs
}

data class InputMeasurements(
    val chinese: List<LengthCounts> = List(20) { LengthCounts() },
    val english: List<LengthCounts> = List(20) { LengthCounts() },
    val cancelled: Long = 0,
    val spellingDeletions: Long = 0,
) {
    init { require(chinese.size == 20 && english.size == 20) }
    private fun numeric(): LongArray = LongArray(122).also { out ->
        (chinese + english).forEachIndexed { i, c -> out[i * 3] = c.inputs; out[i * 3 + 1] = c.corrected; out[i * 3 + 2] = c.units }
        out[120] = cancelled; out[121] = spellingDeletions
    }
    fun grouped(ch: Boolean): List<LengthCounts> = NativeBridge.statisticsMath(if (ch) 2 else 3, numeric()).let { a ->
        List(a.size / 3) { i -> LengthCounts(a[i * 3], a[i * 3 + 1], a[i * 3 + 2]) }
    }
    fun total(ch: Boolean): LengthCounts = NativeBridge.statisticsMath(if (ch) 4 else 5, numeric()).let { LengthCounts(it[0], it[1], it[2]) }
    operator fun plus(other: InputMeasurements): InputMeasurements = fromNative(NativeBridge.statisticsMath(0, numeric(), other.numeric()))!!
    fun mergeMax(other: InputMeasurements): InputMeasurements = fromNative(NativeBridge.statisticsMath(1, numeric(), other.numeric()))!!
    fun toJson(): JSONObject {
        fun rows(values: List<LengthCounts>) = JSONArray().also { a -> values.forEach { a.put(JSONObject().put("inputs", it.inputs).put("corrected", it.corrected).put("units", it.units)) } }
        return JSONObject().put("chinese", rows(chinese)).put("english", rows(english)).put("cancelled", cancelled).put("spelling_deletions", spellingDeletions)
    }
    companion object {
        fun fromNative(values: LongArray?): InputMeasurements? {
            if (values == null) return null
            require(values.size == 122)
            fun rows(offset: Int) = List(20) { i -> LengthCounts(values[offset + i * 3], values[offset + i * 3 + 1], values[offset + i * 3 + 2]) }
            return InputMeasurements(rows(0), rows(60), values[120], values[121])
        }
        fun fromJson(o: JSONObject?): InputMeasurements? {
            if (o == null || o.length() == 0) return null
            fun value(o: JSONObject, key: String) = o.optLong(key, 0).also { require(it in 0..1000000000000000L) }
            fun rows(key: String): List<LengthCounts> {
                val a = o.optJSONArray(key) ?: return List(20) { LengthCounts() }
                require(a.length() == 20)
                return List(20) { val row = a.getJSONObject(it); LengthCounts(value(row, "inputs"), value(row, "corrected"), value(row, "units")) }
            }
            return InputMeasurements(rows("chinese"), rows("english"), value(o, "cancelled"), value(o, "spelling_deletions"))
        }
    }
}
