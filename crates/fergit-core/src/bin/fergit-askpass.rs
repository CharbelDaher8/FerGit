//! A standalone askpass helper, for exercising the credential prompt path without the desktop app
//! (whose own executable doubles as the helper). See `fergit_core::askpass`.

fn main() {
    match fergit_core::askpass::helper_main() {
        Some(code) => std::process::exit(code),
        None => {
            eprintln!("fergit-askpass: run by git as GIT_ASKPASS for FerGit, not directly");
            std::process::exit(2);
        }
    }
}
