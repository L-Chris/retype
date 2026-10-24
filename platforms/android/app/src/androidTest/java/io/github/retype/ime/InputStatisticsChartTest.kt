package io.github.retype.ime

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class InputStatisticsChartTest {
    @Test
    fun shareChartsShowCountsAndExactMeansInGroupedTooltips() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val device = UiDevice.getInstance(instrumentation)
        context.startActivity(Intent().setClassName(context, "io.github.retype.ime.StatisticsChartFixtureActivity").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        try {
            val chart = device.wait(Until.findObject(By.descContains("5–10 字/次")), 10000)
            assertNotNull(chart)
            val bounds = chart.visibleBounds
            val margin = 44 * context.resources.displayMetrics.density
            fun tap(index: Int) {
                device.click((bounds.left + margin + (bounds.width() - margin * 2) * (index + .5) / 6).toInt(), (bounds.top + bounds.height() * .7).toInt())
            }
            tap(4)
            assertTrue(device.wait(Until.hasObject(By.textContains("平均上屏长度 7.2 字")), 5000))
            assertTrue(device.hasObject(By.textContains("输入 25 次 · 占比 25.0%")))
            assertFalse(device.hasObject(By.text("次数")))
            device.takeScreenshot(File(context.cacheDir, "input-chinese-share.png"))
            tap(2)
            assertTrue(device.wait(Until.hasObject(By.textContains("回改率 —")), 5000))
        } finally {
            device.pressBack()
        }
    }
}
