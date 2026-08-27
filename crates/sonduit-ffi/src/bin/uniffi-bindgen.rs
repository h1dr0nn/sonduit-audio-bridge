//! Generates the Kotlin bindings the Android app compiles against.
//!
//! A binary rather than a build script step, because the generator has to run
//! against the built cdylib and Gradle is what knows where that ended up.

fn main() {
    uniffi::uniffi_bindgen_main();
}
