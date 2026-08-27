# JNA looks its classes up reflectively, so R8 cannot see the references and
# strips them. UniFFI's generated bindings are built on JNA.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }

# The generated UniFFI bindings are called from native code by name. The
# package comes from the crate name, not from the app namespace.
-keep class uniffi.sonduit_ffi.** { *; }
