#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    xpu_command_lib::run();
}
