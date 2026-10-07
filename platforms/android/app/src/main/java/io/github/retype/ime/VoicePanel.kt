package io.github.retype.ime

import androidx.compose.foundation.*
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

@Composable
internal fun VoicePanel(state: VoiceState, finish: () -> Unit, cancel: () -> Unit) {
  val ink = MaterialTheme.colorScheme.primary
  Column(Modifier.fillMaxWidth().height(205.dp).padding(horizontal=20.dp), horizontalAlignment=Alignment.CenterHorizontally) {
    Row(Modifier.fillMaxWidth(),horizontalArrangement=Arrangement.Center,verticalAlignment=Alignment.CenterVertically) {
      if (state.phase == "Recognizing") CircularProgressIndicator(Modifier.size(14.dp),strokeWidth=1.5.dp)
      Text(state.error ?: when(state.phase) { "Recording"->"录音中… ${state.seconds}s";"Recognizing"->"识别中…";else->"识别完成" },fontSize=13.sp,modifier=Modifier.padding(8.dp))
    }
    Canvas(Modifier.fillMaxWidth().height(34.dp).pointerInput(cancel) {
      var drag = 0f
      detectVerticalDragGestures(onDragStart={drag=0f},onVerticalDrag={change,amount->change.consume();drag+=amount;if(drag < -60.dp.toPx()) { cancel(); drag=0f }})
    }) {
      for (i in 0 until 35) {
        val x=size.width*(i+1)/36f
        val height = 3.dp.toPx() + size.height*.8f*state.level*(.3f+(i*13%11)/15f)
        drawLine(ink,Offset(x,(size.height-height)/2),Offset(x,(size.height+height)/2),2.dp.toPx(),StrokeCap.Round)
      }
    }
    Text(state.text.ifEmpty { if(state.phase == "Recording") "请开始说话" else "" },Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(vertical=8.dp),fontSize=17.sp)
    Row(Modifier.fillMaxWidth(),horizontalArrangement=Arrangement.SpaceBetween,verticalAlignment=Alignment.CenterVertically) {
      TextButton(onClick=cancel) { Text(if(state.phase in listOf("Error","Done")) "返回键盘" else "取消") }
      if(state.phase == "Recording") Button(onClick=finish) { Text("完成") }
    }
  }
}
