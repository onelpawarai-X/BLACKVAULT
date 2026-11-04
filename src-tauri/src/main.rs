// BLACKVAULT — Windows desktop entry point.
// Prevents a console window from appearing alongside the GUI on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    blackvault_lib::run()
}
