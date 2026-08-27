// The window subsystem hides the console for release builds. Debug builds keep
// it, because a panic with no console is a silent disappearance.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    sonduit_desktop::run();
}
