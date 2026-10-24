package io.github.retype.ime

object NativeBridge {
    init {
        System.loadLibrary("retype_android")
    }

    @JvmStatic
    external fun create(
        dictionary: String,
        database: String,
        flypy: Boolean,
        chinese: Boolean,
        learn: Boolean,
        packs: String = "[]",
    ): Long

    @JvmStatic external fun feature(operation: String): String

    @JvmStatic external fun dispatch(handle: Long, command: String): String

    @JvmStatic external fun dispatchInput(handle: Long, operation: Int, text: String, argument: Int, generation: Long, flags: Int): NativeUpdate

    internal fun send(handle: Long, command: NativeCommand): NativeUpdate =
        dispatchInput(handle, command.operation, command.text, command.argument, command.generation, command.flags)

    @JvmStatic external fun candidates(handle: Long, generation: Long, start: Int, limit: Int): Array<String>?

    @JvmStatic external fun acknowledge(handle: Long, generation: Long, accepted: Boolean)

    @JvmStatic external fun statisticsCreate(): Long
    @JvmStatic external fun statisticsEdit(handle: Long, before: Int, after: Int, flags: Int)
    @JvmStatic external fun statisticsUpdate(handle: Long, operation: Int, text: String): LongArray?
    @JvmStatic external fun statisticsDestroy(handle: Long)

    @JvmStatic external fun destroy(handle: Long)
}
