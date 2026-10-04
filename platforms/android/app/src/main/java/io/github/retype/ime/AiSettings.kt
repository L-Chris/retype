package io.github.retype.ime

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.*
import org.json.JSONArray
import org.json.JSONObject

private val presets =
    listOf(
        Triple("自定义", "", "Compatible"),
        Triple("OpenAI", "https://api.openai.com/v1", "Compatible"),
        Triple("DeepSeek", "https://api.deepseek.com/v1", "Compatible"),
        Triple("OpenRouter", "https://openrouter.ai/api/v1", "Compatible"),
        Triple("硅基流动", "https://api.siliconflow.cn/v1", "Compatible"),
        Triple("Anthropic", "https://api.anthropic.com", "Anthropic"),
        Triple("Gemini", "https://generativelanguage.googleapis.com/v1beta", "Gemini"),
        Triple("Ollama", "http://localhost:11434", "Ollama"),
    )

@Composable
fun ProvidersPage(store: AppStore, busy: Boolean, work: (suspend () -> String) -> Unit) {
    val context = LocalContext.current
    var editing by remember { mutableStateOf<JSONObject?>(null) }
    val array = store.ai().getJSONArray("providers")
    for (i in 0 until array.length()) {
        val p = array.getJSONObject(i)
        SettingsCard {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(p.getString("name"))
                    Text(
                        "${p.getJSONArray("models").length()} 个模型",
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                TextButton(enabled = !busy, onClick = { editing = JSONObject(p.toString()) }) {
                    Text("编辑")
                }
            }
        }
    }
    Button(
        enabled = !busy,
        onClick = {
            editing =
                JSONObject()
                    .put("id", java.util.UUID.randomUUID().toString())
                    .put("preset", "自定义")
                    .put("name", "自定义")
                    .put("base_url", "")
                    .put("kind", "Compatible")
                    .put("models", JSONArray())
        },
    ) {
        Text("添加提供商")
    }
    editing?.let { p ->
        ProviderDialog(
            p,
            onDismiss = { editing = null },
            onSave = { provider, key ->
                work {
                    withContext(Dispatchers.IO) {
                        SecretVault(context).save("ai:${provider.getString("id")}", key)
                        val ai = store.ai()
                        val old = ai.getJSONArray("providers")
                        val next = JSONArray()
                        for (i in 0 until old.length()) if (
                            old.getJSONObject(i).getString("id") != provider.getString("id")
                        )
                            next.put(old.getJSONObject(i))
                        next.put(provider)
                        ai.put("providers", next)
                        store.saveAi(ai)
                    }
                    "提供商已保存"
                }
                editing = null
            },
        )
    }
}

@Composable
private fun ProviderDialog(
    provider: JSONObject,
    onDismiss: () -> Unit,
    onSave: (JSONObject, String) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var name by remember { mutableStateOf(provider.getString("name")) }
    var preset by remember { mutableStateOf(provider.getString("preset")) }
    var url by remember { mutableStateOf(provider.getString("base_url")) }
    var kind by remember { mutableStateOf(provider.getString("kind")) }
    var models by remember {
        mutableStateOf(
            (0 until provider.getJSONArray("models").length()).joinToString("\n") {
                provider.getJSONArray("models").getString(it)
            }
        )
    }
    var key by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        try {
            key =
                withContext(Dispatchers.IO) {
                    SecretVault(context).get("ai:${provider.getString("id")}")
                }
        } catch (e: Exception) {
            error = "无法读取凭据，请重新填写"
        }
    }
    fun draft() =
        JSONObject(provider.toString())
            .put("name", name.trim())
            .put("preset", preset)
            .put("base_url", url.trim())
            .put("kind", kind)
            .put(
                "models",
                JSONArray(
                    models
                        .split(Regex("[\\n,，]"))
                        .map { it.trim() }
                        .filter { it.isNotEmpty() }
                        .distinct()
                ),
            )
    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        title = { Text("AI 提供商") },
        text = {
            Column(
                Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Choice("预设", presets.map { it.first to it.first }, preset) { v ->
                    preset = v
                    presets
                        .first { it.first == v }
                        .let {
                            name = it.first
                            url = it.second
                            kind = it.third
                        }
                }
                OutlinedTextField(
                    name,
                    { name = it },
                    label = { Text("名称") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Choice(
                    "接口类型",
                    listOf(
                        "Compatible" to "OpenAI 兼容",
                        "Anthropic" to "Anthropic",
                        "Gemini" to "Gemini",
                        "Ollama" to "Ollama",
                    ),
                    kind,
                ) {
                    kind = it
                }
                OutlinedTextField(
                    url,
                    { url = it },
                    label = { Text("接口地址") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    key,
                    { key = it },
                    label = { Text("API Key") },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    models,
                    { models = it },
                    label = { Text("模型 ID（每行一个）") },
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedButton(
                    enabled = !busy,
                    onClick = {
                        busy = true
                        error = null
                        scope.launch {
                            try {
                                val p = draft()
                                val secret = key
                                val result =
                                    withContext(Dispatchers.IO) {
                                        JSONArray(
                                            NativeBridge.feature(
                                                JSONObject()
                                                    .put("type", "models")
                                                    .put("provider", p)
                                                    .put("key", secret)
                                                    .toString()
                                            )
                                        )
                                    }
                                check(draft().toString() == p.toString() && key == secret) {
                                    "配置已变化，请重新获取模型"
                                }
                                models =
                                    (0 until result.length()).joinToString("\n") {
                                        result.getString(it)
                                    }
                            } catch (e: CancellationException) {
                                throw e
                            } catch (e: Exception) {
                                error = e.message
                            } finally {
                                busy = false
                            }
                        }
                    },
                ) {
                    Text(if (busy) "获取中…" else "获取模型")
                }
                if (error != null) Text(error!!, style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = {
            TextButton(
                enabled = !busy,
                onClick = {
                    if (name.isBlank() || !url.startsWith("http") || url.contains(Regex("[@?#]")))
                        error = "请填写名称和有效接口地址"
                    else onSave(draft(), key)
                },
            ) {
                Text("保存")
            }
        },
        dismissButton = { TextButton(enabled = !busy, onClick = onDismiss) { Text("取消") } },
    )
}

@Composable
fun TranslationPage(store: AppStore) {
    var ai by remember { mutableStateOf(store.ai()) }
    var saved by remember { mutableStateOf(false) }
    fun change(name: String, value: Any) {
        ai = JSONObject(ai.toString()).put(name, value)
        saved = false
    }
    val providers = ai.getJSONArray("providers")
    val options =
        (0 until providers.length()).map {
            providers.getJSONObject(it).let { p -> p.getString("id") to p.getString("name") }
        }
    val provider =
        (0 until providers.length())
            .map { providers.getJSONObject(it) }
            .firstOrNull { it.getString("id") == ai.getString("provider") }
    SettingsCard {
        Choice("提供商", options, ai.getString("provider")) {
            change("provider", it)
            change("model", "")
        }
        val models = provider?.getJSONArray("models") ?: JSONArray()
        Choice(
            "模型",
            (0 until models.length()).map { models.getString(it) to models.getString(it) },
            ai.getString("model"),
        ) {
            change("model", it)
        }
        Choice(
            "目标语言",
            listOf(
                "English" to "英语",
                "简体中文" to "简体中文",
                "繁體中文" to "繁体中文",
                "日本語" to "日语",
                "한국어" to "韩语",
                "Français" to "法语",
                "Deutsch" to "德语",
                "Español" to "西班牙语",
            ),
            ai.getString("target"),
        ) {
            change("target", it)
        }
        Choice(
            "思考等级",
            listOf(
                "none" to "不思考",
                "default" to "模型默认",
                "minimal" to "最低",
                "low" to "低",
                "medium" to "中",
                "high" to "高",
            ),
            ai.getString("reasoning"),
        ) {
            change("reasoning", it)
        }
        OutlinedTextField(
            ai.getString("instructions"),
            { change("instructions", it) },
            label = { Text("附加要求") },
            modifier = Modifier.fillMaxWidth(),
        )
        Button(
            onClick = {
                store.saveAi(ai)
                saved = true
            }
        ) {
            Text(if (saved) "已保存" else "保存")
        }
    }
}

