package io.github.retype.ime

import android.content.Context
import android.util.AtomicFile
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import org.json.JSONArray
import org.json.JSONObject

class AppStore(private val context: Context) {
    val prefs = context.getSharedPreferences("settings", Context.MODE_PRIVATE)

    companion object {
        private val lock = Any()
    }

    private fun read(name: String, fallback: () -> JSONObject): JSONObject =
        synchronized(lock) {
            val file = AtomicFile(File(context.filesDir, name))
            if (!file.baseFile.exists()) fallback()
            else JSONObject(file.openRead().use { it.readBytes().toString(Charsets.UTF_8) })
        }

    private fun write(name: String, value: JSONObject) =
        synchronized(lock) {
            val file = AtomicFile(File(context.filesDir, name))
            val out = file.startWrite()
            try {
                out.write(value.toString().toByteArray())
                file.finishWrite(out)
            } catch (e: Exception) {
                file.failWrite(out)
                throw e
            }
        }

    fun ai(): JSONObject =
        read("ai.json") {
            JSONObject(
                """{"version":1,"providers":[],"provider":"","model":"","target":"English","preview":false,"instructions":"","reasoning":"none","timeout_seconds":60}"""
            )
        }.also { if (!it.has("voice")) it.put("voice", voiceDefaults()) }

    fun voiceDefaults() = JSONObject().put("provider", "").put("model", "").put("language", "auto").put("tidy", false)
    fun saveAi(value: JSONObject) {
        write("ai.json", value)
    }

    fun deviceId(): String =
        synchronized(lock) {
            prefs.getString("deviceId", null)
                ?: UUID.randomUUID().toString().replace("-", "").also {
                    prefs.edit().putString("deviceId", it).commit()
                }
        }

    fun cloud(): JSONObject =
        read("cloud.json") {
            JSONObject()
                .put("version", 1)
                .put("enabled", false)
                .put("provider", "坚果云")
                .put("url", "https://dav.jianguoyun.com/dav/")
                .put("username", "")
                .put("device_id", deviceId())
                .put("device_name", android.os.Build.MODEL)
        }

    fun saveCloud(value: JSONObject) {
        write("cloud.json", value)
    }

    fun cloudAccount(value: JSONObject): String =
        sha256(
            (value.getString("url").trim().trimEnd('/') + "\n" + value.getString("username").trim())
                .toByteArray()
        )

    fun shortcuts(): JSONObject =
        JSONObject(
            prefs.getString("shortcuts", null)
                ?: """{"mode":{"vk":16,"modifiers":0},"translate":{"vk":48,"modifiers":3}}"""
        )

    fun values(): JSONObject =
        synchronized(lock) {
            val ai = ai()
            val result =
                JSONObject()
                    .put("input.scheme", if (prefs.getBoolean("flypy", false)) 1 else 0)
                    .put("input.english", prefs.getBoolean("english", true))
                    .put("input.english_spelling", prefs.getBoolean("spelling", true))
                    .put("input.symbol_completion", prefs.getBoolean("symbolCompletion", true))
                    .put("dictionary.enabled", prefs.getInt("packs", 0))
                    .put("updates.auto_check", prefs.getBoolean("updatesAutoCheck", true))
                    .put("shortcuts", shortcuts())
                    .put("voice.settings", JSONObject(ai.getJSONObject("voice").toString()).also { it.remove("microphone") })
                    .put(
                        "translation.model",
                        JSONObject()
                            .put("provider", ai.getString("provider"))
                            .put("model", ai.getString("model")),
                    )
            mapOf(
                    "target" to "target",
                    "preview" to "preview",
                    "instructions" to "instructions",
                    "reasoning" to "reasoning",
                    "timeout" to "timeout_seconds",
                )
                .forEach { (k, a) -> result.put("translation.$k", ai.get(a)) }
            val providers = ai.getJSONArray("providers")
            for (i in 0 until providers.length()) {
                val p = providers.getJSONObject(i)
                result.put("providers/${p.getString("id")}", p)
            }
            result
        }

    fun applyValues(values: JSONObject) =
        synchronized(lock) {
            // Validate everything before changing local state. Unknown future keys are retained by
            // Rust sync.
            val scheme = values.getInt("input.scheme")
            require(scheme in 0..1)
            val packs = values.getInt("dictionary.enabled")
            require(packs in 0..127)
            val shortcuts = values.getJSONObject("shortcuts")
            Shortcuts.validate(shortcuts)
            val ai = ai()
            val providers = JSONArray()
            values.keys().forEach { k ->
                if (k.startsWith("providers/") && !values.isNull(k)) {
                    val p = values.getJSONObject(k)
                    require(p.getString("id").let { "providers/$it" } == k)
                    require(!p.getString("base_url").contains(Regex("[@?#]")))
                    providers.put(p)
                }
            }
            values.optJSONObject("voice.settings")?.let { v ->
                require(v.optString("language", "auto") in listOf("auto", "zh", "en"))
                require(v.optString("model").length <= 512 && v.optString("provider").length <= 128)
                ai.put("voice", JSONObject(v.toString()).also { it.remove("microphone") })
            }
            ai.put("providers", providers)
            values.getJSONObject("translation.model").let {
                ai.put("provider", it.getString("provider")).put("model", it.getString("model"))
            }
            mapOf(
                    "target" to "target",
                    "preview" to "preview",
                    "instructions" to "instructions",
                    "reasoning" to "reasoning",
                    "timeout" to "timeout_seconds",
                )
                .forEach { (k, a) -> ai.put(a, values.get("translation.$k")) }
            require(
                ai.getString("reasoning") in
                    listOf("none", "default", "minimal", "low", "medium", "high")
            )
            require(ai.getLong("timeout_seconds") in 5..180)
            require(
                listOf("provider", "model", "target", "instructions", "reasoning").all {
                    ai.get(it) is String
                }
            )
            require(ai.get("preview") is Boolean)
            saveAi(ai)
            prefs
                .edit()
                .putBoolean("updatesAutoCheck", values.optBoolean("updates.auto_check", true))
                .commit()
            prefs
                .edit()
                .putBoolean("flypy", scheme == 1)
                .putInt("packs", packs)
                .putString("shortcuts", shortcuts.toString())
                .putBoolean("english", values.optBoolean("input.english", true))
                .putBoolean("spelling", values.optBoolean("input.english_spelling", true))
                .putBoolean("symbolCompletion", values.optBoolean("input.symbol_completion", true))
                .commit()
        }
}

fun sha256(bytes: ByteArray): String =
    MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

object Shortcuts {
    fun validate(value: JSONObject) {
        val mode = value.getJSONObject("mode")
        val translate = value.getJSONObject("translate")
        for ((binding, isMode) in listOf(mode to true, translate to false)) {
            val vk = binding.getInt("vk")
            val mods = binding.getInt("modifiers")
            require(
                (vk == 0 && mods == 0) ||
                    (isMode && vk in listOf(16, 17) && mods == 0) ||
                    (vk in 32..90 && mods in 1..7 && Integer.bitCount(mods) <= 2)
            ) {
                "快捷键需要 Ctrl / Alt 等修饰键"
            }
        }
        require(mode.toString() != translate.toString() || mode.getInt("vk") == 0) { "快捷键不能重复" }
    }

    fun vk(code: Int): Int =
        when (code) {
            in android.view.KeyEvent.KEYCODE_A..android.view.KeyEvent.KEYCODE_Z ->
                code - android.view.KeyEvent.KEYCODE_A + 65
            in android.view.KeyEvent.KEYCODE_0..android.view.KeyEvent.KEYCODE_9 ->
                code - android.view.KeyEvent.KEYCODE_0 + 48
            android.view.KeyEvent.KEYCODE_SPACE -> 32
            android.view.KeyEvent.KEYCODE_SHIFT_LEFT,
            android.view.KeyEvent.KEYCODE_SHIFT_RIGHT -> 16
            android.view.KeyEvent.KEYCODE_CTRL_LEFT,
            android.view.KeyEvent.KEYCODE_CTRL_RIGHT -> 17
            else -> 0
        }

    fun matches(binding: JSONObject, event: android.view.KeyEvent): Boolean {
        val mods =
            (if (event.isCtrlPressed) 1 else 0) or
                (if (event.isAltPressed) 2 else 0) or
                (if (event.isShiftPressed) 4 else 0)
        return binding.getInt("vk") != 0 &&
            binding.getInt("vk") == vk(event.keyCode) &&
            binding.getInt("modifiers") == mods
    }

    fun label(binding: JSONObject): String {
        val vk = binding.getInt("vk")
        if (vk == 0) return "未设置"
        return listOfNotNull(
                if (binding.getInt("modifiers") and 1 != 0) "Ctrl" else null,
                if (binding.getInt("modifiers") and 2 != 0) "Alt" else null,
                if (binding.getInt("modifiers") and 4 != 0) "Shift" else null,
                when (vk) {
                    16 -> "Shift"
                    17 -> "Ctrl"
                    32 -> "Space"
                    else -> vk.toChar().toString()
                },
            )
            .joinToString(" + ")
    }
}
