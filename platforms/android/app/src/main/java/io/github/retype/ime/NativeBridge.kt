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

    @JvmStatic external fun destroy(handle: Long)
}
