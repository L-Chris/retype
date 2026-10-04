package io.github.retype.ime

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.*
import org.json.JSONArray
import org.json.JSONObject

@Composable
fun CloudPage(store: AppStore, busy: Boolean, work: (suspend () -> String) -> Unit) {
    val context = LocalContext.current
    var config by remember { mutableStateOf(store.cloud()) }
    var password by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var status by remember {
        mutableStateOf(JSONObject(store.prefs.getString("syncStatus", "{}")!!))
    }
    LaunchedEffect(Unit) {
        try {
            password =
                withContext(Dispatchers.IO) {
                    SecretVault(context).get("cloud:${store.cloudAccount(config)}")
                }
        } catch (e: Exception) {
            error = "请重新填写云盘密码"
        }
    }
    fun change(name: String, value: Any) {
        config = JSONObject(config.toString()).put(name, value)
    }
    fun save() {
        SecretVault(context).save("cloud:${store.cloudAccount(config)}", password)
        store.saveCloud(config)
        CloudSync.schedule(context)
    }
    SettingsCard {
        ToggleRow("云同步", config.optBoolean("enabled"), !busy) {
            change("enabled", it)
            if (!it) {
                store.saveCloud(config)
                CloudSync.schedule(context)
            }
        }
        Choice(
            "云盘",
            listOf("坚果云", "cstcloud", "InfiniCLOUD", "Koofr", "HiDrive", "Yandex Disk", "自定义").map {
                it to it
            },
            config.getString("provider"),
        ) { p ->
            change("provider", p)
            change(
                "url",
                when (p) {
                    "坚果云" -> "https://dav.jianguoyun.com/dav/"
                    "cstcloud" -> "https://data.cstcloud.cn/dav/"
                    "Koofr" -> "https://app.koofr.net/dav/Koofr/"
                    "HiDrive" -> "https://webdav.hidrive.strato.com/"
                    "Yandex Disk" -> "https://webdav.yandex.com/"
                    else -> ""
                },
            )
            password = ""
        }
        OutlinedTextField(
            config.getString("url"),
            {
                change("url", it)
                password = ""
            },
            label = { Text("WebDAV 地址") },
            singleLine = true,
            enabled = !busy,
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            config.getString("username"),
            {
                change("username", it)
                password = ""
            },
            label = { Text("用户名") },
            singleLine = true,
            enabled = !busy,
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            password,
            { password = it },
            label = { Text("应用密码") },
            singleLine = true,
            enabled = !busy,
            visualTransformation = PasswordVisualTransformation(),
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            config.getString("device_name"),
            { change("device_name", it) },
            label = { Text("设备名称") },
            singleLine = true,
            enabled = !busy,
            modifier = Modifier.fillMaxWidth(),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedButton(
                enabled = !busy,
                onClick = {
                    work {
                        val draft = JSONObject(config.toString())
                        val secret = password
                        withContext(Dispatchers.IO) {
                            JSONObject(
                                    NativeBridge.feature(
                                        JSONObject()
                                            .put("type", "syncTest")
                                            .put("config", draft)
                                            .put("password", secret)
                                            .toString()
                                    )
                                )
                                .getString("message")
                        }
                    }
                },
            ) {
                Text("测试连接")
            }
            Button(
                enabled = !busy,
                onClick = {
                    work {
                        withContext(Dispatchers.IO) { save() }
                        CloudSync.run(context).also { status = it }.getString("message")
                    }
                },
            ) {
                Text("保存并同步")
            }
        }
        Text("同步设置、个人词库和统计数据；API Key 与云盘密码仅保存在本机。", style = MaterialTheme.typography.bodySmall)
        if (error != null) Text(error!!, style = MaterialTheme.typography.bodySmall)
    }
    if (status.optBoolean("awaitingMerge"))
        Button(
            enabled = !busy,
            onClick = {
                work {
                    CloudSync.run(context, confirm = true).also { status = it }.getString("message")
                }
            },
        ) {
            Text("确认首次合并")
        }
    val conflicts = status.optJSONArray("conflicts") ?: JSONArray()
    for (i in 0 until conflicts.length()) {
        val c = conflicts.getJSONObject(i)
        SettingsCard {
            Text(c.getString("key"))
            Text("来自 ${c.getString("device_name")}", style = MaterialTheme.typography.bodySmall)
            Row {
                listOf(false to "保留本机", true to "使用云端").forEach { (remote, label) ->
                    TextButton(
                        enabled = !busy,
                        onClick = {
                            work {
                                CloudSync.run(
                                        context,
                                        resolve =
                                            JSONObject()
                                                .put("key", c.getString("key"))
                                                .put(
                                                    "stamp",
                                                    c.getJSONObject("remote").getJSONObject("stamp"),
                                                )
                                                .put("remote", remote),
                                    )
                                    .also { status = it }
                                    .getString("message")
                            }
                        },
                    ) {
                        Text(label)
                    }
                }
            }
        }
    }
}
