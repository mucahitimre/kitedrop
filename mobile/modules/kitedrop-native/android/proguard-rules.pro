# JNA (used by the UniFFI Kotlin bindings) loads classes and fields by reflection.
-dontwarn java.awt.**
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
# UniFFI-generated bindings: structures and callbacks are looked up by name through JNA.
-keep class uniffi.kitedrop_ffi.** { *; }
