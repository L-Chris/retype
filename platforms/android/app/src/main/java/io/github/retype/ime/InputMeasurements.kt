package io.github.retype.ime

import org.json.JSONArray
import org.json.JSONObject

internal val inputLengthLabels = listOf("1", "2", "3", "4", "5–10", "11+")
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
    fun grouped(ch: Boolean): List<LengthCounts> {
        val rows = MutableList(6) { LengthCounts() }
        (if (ch) chinese else english).forEachIndexed { i, value ->
            val group = when (i + 1) { 1 -> 0; 2 -> 1; 3 -> 2; 4 -> 3; in 5..10 -> 4; else -> 5 }
            rows[group] = rows[group] + value
        }
        return rows
    }
    fun total(ch: Boolean) = (if (ch) chinese else english).fold(LengthCounts(), LengthCounts::plus)
    operator fun plus(other: InputMeasurements) = combine(other, ::sum)
    fun mergeMax(other: InputMeasurements) = combine(other, ::maxOf)
    private fun combine(other: InputMeasurements, f: (Long, Long) -> Long): InputMeasurements {
        fun rows(a: List<LengthCounts>, b: List<LengthCounts>) = a.zip(b) { x, y -> LengthCounts(f(x.inputs, y.inputs), f(x.corrected, y.corrected), f(x.units, y.units)) }
        return InputMeasurements(rows(chinese, other.chinese), rows(english, other.english), f(cancelled, other.cancelled), f(spellingDeletions, other.spellingDeletions))
    }
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
