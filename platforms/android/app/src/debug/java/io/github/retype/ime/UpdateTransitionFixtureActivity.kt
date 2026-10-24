package io.github.retype.ime

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.unit.dp

/** Changes update state just before drawing, without network or installer access. */
class UpdateTransitionFixtureActivity : ComponentActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    setContent {
      val status = remember { mutableStateOf(UpdateStatus(busy = true, progress = .5f, message = "测试下载中")) }
      var next by remember { mutableStateOf<UpdateStatus?>(null) }
      MaterialTheme {
        Surface(Modifier.fillMaxSize()) {
          Column(Modifier.safeDrawingPadding().padding(16.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
              Button(onClick = { next = UpdateStatus(busy = true, progress = .5f, message = "测试下载中") }) { Text("开始") }
              Button(onClick = { next = UpdateStatus(ready = true, message = "测试下载完成") }) { Text("完成") }
              Button(onClick = { next = UpdateStatus(message = "测试已取消") }) { Text("取消") }
              Button(onClick = { next = UpdateStatus(message = "测试下载失败") }) { Text("失败") }
            }
            Button(onClick = { next = UpdateStatus(busy = true, message = "测试检查中") }) { Text("检查") }
            Box(Modifier.drawWithContent {
              // Simulate a download completing after composition but before an
              // existing progress indicator's draw callback executes.
              next?.let { status.value = it; next = null }
              drawContent()
            }) {
              UpdateCard(status)
            }
          }
        }
      }
    }
  }
}
