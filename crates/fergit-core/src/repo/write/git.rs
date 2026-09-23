//! Running the git CLI for mutations.
//!
//! Every write goes through [`Git::run`], which pins down how git is started so that no caller has
//! to remember it:
//! - arguments are passed one by one (`Command::arg`), never through a shell;
//! - output is in English (`LC_ALL=C`), so failures can be recognized from git's messages;
//! - git never waits on a terminal: stdin is closed, `GIT_TERMINAL_PROMPT=0`, and credential and
//!   passphrase prompts go to FerGit's askpass helper when one is given;
//! - a lock file another git process briefly holds (`index.lock`, a ref's `.lock`) is waited out
//!   with a few short retries instead of failing the operation.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use super::scrub::scrub;
use crate::askpass::Askpass;

/// How many times a command that failed on a held lock file is retried, and how long to wait
/// before the first retry (each later retry waits one step longer).
const LOCK_RETRIES: u32 = 5;
const LOCK_RETRY_STEP: Duration = Duration::from_millis(100);

/// Runs git commands in one repository.
pub(super) struct Git<'a> {
    pub root: &'a Path,
    pub askpass: Option<&'a Askpass>,
}

/// What a finished git command printed. Credentials are already scrubbed from both texts.
#[derive(Debug)]
pub(super) struct Output {
    /// The exit code; `None` if git was killed by a signal.
    pub code: Option<i32>,
    /// Git's messages without the progress updates that were overwritten in place (lines ending in
    /// `\r`), which are only meaningful live.
    pub stderr: String,
    pub stdout: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Everything git printed, errors first; what the user sees as "git output".
    pub fn text(&self) -> String {
        match (self.stderr.trim_end(), self.stdout.trim_end()) {
            (err, "") => err.to_owned(),
            ("", out) => out.to_owned(),
            (err, out) => format!("{err}\n{out}"),
        }
    }
}

impl Git<'_> {
    /// Runs `git <args>` to completion, passing each line of progress git reports (with credentials
    /// scrubbed) to `progress` as it arrives. Fails only if git couldn't be started.
    pub fn run(&self, args: &[&str], progress: &mut dyn FnMut(&str)) -> std::io::Result<Output> {
        let mut attempt = 0;
        loop {
            let output = self.run_once(args, progress)?;
            if output.success() || !holds_lock(&output.stderr) || attempt == LOCK_RETRIES {
                return Ok(output);
            }
            attempt += 1;
            thread::sleep(LOCK_RETRY_STEP * attempt);
        }
    }

    fn run_once(&self, args: &[&str], progress: &mut dyn FnMut(&str)) -> std::io::Result<Output> {
        let mut command = Command::new("git");
        command
            .current_dir(self.root)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            // A pull that fast-forwards never needs a message, but never open an editor regardless.
            .env("GIT_MERGE_AUTOEDIT", "no");
        if let Some(askpass) = self.askpass {
            command.envs(askpass.env());
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Without this, every git started from the windowed app flashes a console window.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command.spawn()?;
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let stdout = thread::spawn(move || {
            let mut bytes = Vec::new();
            // A read error leaves what was read; the exit status still says how git fared.
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        });
        let stderr = read_stderr(child.stderr.take().expect("stderr is piped"), progress);
        let status = child.wait()?;
        let stdout = stdout.join().unwrap_or_default();
        Ok(Output {
            code: status.code(),
            stderr: scrub(&stderr),
            stdout: scrub(&String::from_utf8_lossy(&stdout)),
        })
    }
}

/// Reads git's stderr to the end, reporting each line or progress update as it completes. Returns
/// the lines that ended in `\n`: a progress update ending in `\r` is replaced by the next one.
fn read_stderr(mut stderr: impl Read, progress: &mut dyn FnMut(&str)) -> String {
    let mut kept = String::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = match stderr.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &byte in &buf[..n] {
            if byte != b'\r' && byte != b'\n' {
                pending.push(byte);
                continue;
            }
            let line = String::from_utf8_lossy(&pending);
            if !line.trim().is_empty() {
                progress(&scrub(line.trim_end()));
            }
            if byte == b'\n' {
                kept.push_str(&line);
                kept.push('\n');
            }
            pending.clear();
        }
    }
    let rest = String::from_utf8_lossy(&pending);
    if !rest.trim().is_empty() {
        progress(&scrub(rest.trim_end()));
        kept.push_str(&rest);
    }
    kept
}

/// Whether git failed because another process holds one of its lock files, as in
/// `fatal: Unable to create '/repo/.git/index.lock': File exists.`
fn holds_lock(stderr: &str) -> bool {
    stderr.contains(".lock': File exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_updates_are_reported_but_only_finished_lines_kept() {
        let raw = "Counting objects:  50% (1/2)\rCounting objects: 100% (2/2), done.\nTo origin\n! rejected";
        let mut seen = Vec::new();
        let kept = read_stderr(raw.as_bytes(), &mut |line| seen.push(line.to_owned()));
        assert_eq!(
            seen,
            ["Counting objects:  50% (1/2)", "Counting objects: 100% (2/2), done.", "To origin", "! rejected"]
        );
        assert_eq!(kept, "Counting objects: 100% (2/2), done.\nTo origin\n! rejected");
    }

    #[test]
    fn progress_is_scrubbed() {
        let mut seen = Vec::new();
        read_stderr("To https://me:hunter2@example.com/r.git\n".as_bytes(), &mut |line| seen.push(line.to_owned()));
        assert_eq!(seen, ["To https://***@example.com/r.git"]);
    }

    #[test]
    fn recognizes_a_held_lock() {
        assert!(holds_lock("fatal: Unable to create 'C:/r/.git/index.lock': File exists.\n\nAnother git process…"));
        assert!(!holds_lock("fatal: invalid reference: nosuch"));
    }
}
