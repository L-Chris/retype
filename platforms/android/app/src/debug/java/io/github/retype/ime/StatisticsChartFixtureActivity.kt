package io.github.retype.ime

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.lightColorScheme
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** Deterministic charts for UI tests, without writing statistics or settings. */
class StatisticsChartFixtureActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val rows = MutableList(20) { LengthCounts() }
        listOf(
            1 to LengthCounts(20, 2, 20),
            2 to LengthCounts(30, 3, 60),
            4 to LengthCounts(10, 2, 40),
            5 to LengthCounts(10, 2, 50),
            8 to LengthCounts(10, 2, 80),
            10 to LengthCounts(5, 1, 50),
            11 to LengthCounts(10, 2, 110),
            20 to LengthCounts(5, 1, 130),
        ).forEach { (length, counts) -> rows[length - 1] = counts }
        val fixture = InputMeasurements(rows, rows, cancelled = 7)
        setContent {
            MaterialTheme(colorScheme = lightColorScheme()) {
                Surface(Modifier.fillMaxSize()) {
                    Column(Modifier.safeDrawingPadding().verticalScroll(rememberScrollState()).padding(16.dp)) {
                        InputStatisticsSection(fixture, "今天 · 测试数据")
                    }
                }
            }
        }
    }
}
