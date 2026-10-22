package io.github.retype.ime

import android.content.Context
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File

/** Test resources belong to the instrumentation APK, never the application APK. */
object ResourceFixtures {
  suspend fun prepare(context: Context): File {
    if (DictionaryAssets.available(context) == null || !BaseResources.refresh(context).ready) {
      val bundle = File(context.cacheDir, "fixture.retype-resources")
      try {
        InstrumentationRegistry.getInstrumentation().context.assets.open("retype-base.retype-resources").use { input ->
          bundle.outputStream().use { input.copyTo(it) }
        }
        BaseResources.install(context, "resourceImport", bundle)
      } finally { bundle.delete() }
    }
    return DictionaryAssets.prepare(context)
  }
}
