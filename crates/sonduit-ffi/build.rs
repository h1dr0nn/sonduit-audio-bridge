//! Emits the UniFFI scaffolding metadata the bindings generator reads.

fn main() {
    uniffi::generate_scaffolding("src/lib.rs").ok();
}
