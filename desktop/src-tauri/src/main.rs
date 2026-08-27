// A GUI application must never open a console window, in any build. The
// previous rule kept one for debug builds so a panic would be visible, which
// meant every artifact handed to a tester came with a stray black window
// beside it. Panics go to the log file the log plugin already writes, which is
// where a user can actually find them afterwards.
#![windows_subsystem = "windows"]

fn main() {
    sonduit_desktop::run();
}
