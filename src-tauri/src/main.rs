// Release builds are GUI-only; debug builds keep the console for log output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    sentinel_app::run();
}
