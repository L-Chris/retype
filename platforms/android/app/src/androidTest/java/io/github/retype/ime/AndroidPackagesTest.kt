package io.github.retype.ime

import org.junit.Assert.assertEquals
import org.junit.Test

class AndroidPackagesTest {
  @Test fun deviceOrderAndUniversalFallbackArePreserved() {
    assertEquals(listOf("retype-1.2.3-android-arm64-v8a.apk", "retype-1.2.3-android.apk"),
        AndroidPackages.names("1.2.3", listOf("arm64-v8a", "armeabi-v7a")))
    assertEquals(listOf("retype-1.2.3-android-x86_64.apk", "retype-1.2.3-android-arm64-v8a.apk", "retype-1.2.3-android.apk"),
        AndroidPackages.names("1.2.3", listOf("x86_64", "arm64-v8a", "x86_64")))
    assertEquals(listOf("retype-1.2.3-android.apk"), AndroidPackages.names("1.2.3", emptyList()))
  }
}
