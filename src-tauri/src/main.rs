// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // FerGit's executable is also its askpass helper: when git runs it to ask for a password, it
    // relays the prompt to the running app and exits without starting a window.
    if let Some(code) = fergit_core::askpass::helper_main() {
        std::process::exit(code);
    }
    fergit_lib::run()
}
