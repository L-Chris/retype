package io.github.retype.ime

/** Constructor and field names are retained for the JNI boundary by R8 rules. */
class NativeUpdate(
    val generation: Long,
    val composition: String,
    val candidates: Array<String>?,
    val candidateStart: Int,
    val candidateTotal: Int,
    val selected: Int,
    val chinese: Boolean,
    val commits: Array<String>,
    val passThrough: Boolean,
)

internal fun KeyboardState.withNativeUpdate(update: NativeUpdate): KeyboardState = copy(
    generation = update.generation,
    composition = update.composition,
    candidates = update.candidates?.asList() ?: candidates,
    candidateTotal = update.candidateTotal,
    candidatePages = if (update.generation == generation) candidatePages else emptyMap(),
    pageStart = update.candidateStart,
    selected = update.selected,
    chinese = update.chinese,
    ready = true,
)

internal class NativeCommand private constructor(
    val operation: Int,
    val text: String = "",
    val argument: Int = 0,
    val generation: Long = 0,
    val flags: Int = 0,
) {
  companion object {
    fun key(value: String, mods: Int = 0) = NativeCommand(1, value, flags = mods)
    fun literal(value: String) = NativeCommand(2, value)
    fun choose(index: Int, generation: Long) = NativeCommand(3, argument = index, generation = generation)
    fun toggle() = NativeCommand(4)
    fun reset() = NativeCommand(5)
    fun english(enabled: Boolean, spelling: Boolean) = NativeCommand(6, flags = (if (enabled) 1 else 0) or (if (spelling) 2 else 0))
    fun voiceLearn(text: String) = NativeCommand(7, text)
  }
}
