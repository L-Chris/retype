package io.github.retype.ime

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.lightColorScheme
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

class SettingsActivity : ComponentActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    CloudSync.schedule(applicationContext)
    AppUpdates.schedule(applicationContext)
    setContent {
      MaterialTheme(
          colorScheme =
              lightColorScheme(
                  primary = Color(0xFF11939B),
                  onPrimary = Color.White,
                  onSurface = Color(0xFF202827),
                  onSurfaceVariant = Color(0xFF788381),
                  surface = Color.White,
                  surfaceContainerHighest = Color.White,
                  outline = Color(0xFFD4DDDB),
                  secondaryContainer = Color(0xFFE7F3F1),
              ),
          shapes =
              Shapes(
                  small = RoundedCornerShape(12.dp),
                  medium = RoundedCornerShape(16.dp),
                  large = RoundedCornerShape(20.dp)),
      ) {
        SettingsScreen()
      }
    }
  }

  override fun onResume() {
    super.onResume()
    val prefs = AppStore(this).prefs
    if (prefs.getBoolean("pendingUpdateInstall", false)) {
      prefs.edit().putBoolean("pendingUpdateInstall", false).apply()
      if (packageManager.canRequestPackageInstalls()) AppUpdates.install(this)
    }
  }
}
