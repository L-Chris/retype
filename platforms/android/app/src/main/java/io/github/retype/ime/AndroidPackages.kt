package io.github.retype.ime

internal object AndroidPackages {
  fun names(version: String, supportedAbis: List<String>): List<String> =
      supportedAbis.filter { it == "arm64-v8a" || it == "x86_64" }.distinct()
          .map { "retype-$version-android-$it.apk" } + "retype-$version-android.apk"
}
