# Optional compile-time annotations referenced by AndroidX Test, not runtime APIs.
# These rules apply only to the instrumentation APK, never the application.
-dontwarn com.google.errorprone.annotations.CanIgnoreReturnValue
-dontwarn com.google.errorprone.annotations.MustBeClosed
