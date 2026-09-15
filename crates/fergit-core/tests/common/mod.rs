//! Throwaway repositories built with the git CLI, isolated from the user's git configuration.

// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use fergit_core::Oid;
use tempfile::TempDir;

/// Commit time used when a test doesn't care: 2023-11-14T22:13:20Z.
pub const T0: i64 = 1_700_000_000;
pub const AUTHOR_NAME: &str = "Ada Author";
pub const AUTHOR_EMAIL: &str = "ada@example.com";

/// A temp directory holding the repositories of one test. Deleted on drop.
pub struct Fixture {
    dir: TempDir,
}

impl Fixture {
    pub fn new() -> Fixture {
        isolated_home();
        Fixture { dir: tempfile::tempdir().expect("create a temp dir") }
    }

    /// `name` inside the fixture directory (not created).
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Creates a repository with a worktree at `name`.
    pub fn init(&self, name: &str) -> PathBuf {
        git(self.dir.path(), &["init", "--quiet", name]);
        self.path(name)
    }

    /// Creates a bare repository at `name`.
    pub fn init_bare(&self, name: &str) -> PathBuf {
        git(self.dir.path(), &["init", "--quiet", "--bare", name]);
        self.path(name)
    }
}

/// Runs git in `cwd` with author and committer time [`T0`] and returns its stdout.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    git_at(cwd, args, T0)
}

/// Runs git in `cwd` with author and committer time `time` (UTC) and returns its stdout.
pub fn git_at(cwd: &Path, args: &[&str], time: i64) -> String {
    git_dated(cwd, args, &format!("{time} +0000"))
}

/// Runs git in `cwd` with author and committer date `date` in git's internal format
/// (`<seconds> <±hhmm>`) and returns its stdout. Panics if git fails.
pub fn git_dated(cwd: &Path, args: &[&str], date: &str) -> String {
    let home = isolated_home();
    let output = Command::new("git")
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("GIT_AUTHOR_NAME", AUTHOR_NAME)
        .env("GIT_AUTHOR_EMAIL", AUTHOR_EMAIL)
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_NAME", "Carl Committer")
        .env("GIT_COMMITTER_EMAIL", "carl@example.com")
        .env("GIT_COMMITTER_DATE", date)
        .args(["-c", "init.defaultBranch=main", "-c", "commit.gpgSign=false", "-c", "tag.gpgSign=false"])
        .args(["-c", "core.autocrlf=false", "-c", "core.fsmonitor=false"])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed in {}:\n{}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git output is UTF-8")
}

/// Writes `contents` to `file` (a `/`-separated path relative to `repo`), creating directories.
pub fn write(repo: &Path, file: &str, contents: impl AsRef<[u8]>) {
    let path = repo.join(file);
    std::fs::create_dir_all(path.parent().expect("file has a parent")).expect("create directories");
    std::fs::write(path, contents).expect("write file");
}

/// Writes `file`, stages it and commits with `message` at `time`. Returns the new commit.
pub fn commit(repo: &Path, file: &str, contents: impl AsRef<[u8]>, message: &str, time: i64) -> Oid {
    write(repo, file, contents);
    git(repo, &["add", "--", file]);
    git_at(repo, &["commit", "--quiet", "-m", message], time);
    rev_parse(repo, "HEAD")
}

pub fn rev_parse(repo: &Path, rev: &str) -> Oid {
    git(repo, &["rev-parse", "--verify", rev]).trim().parse().expect("git prints an object id")
}

/// A home directory with an empty global git config, shared by all tests in this process.
///
/// Git commands get it through their environment. gix, which runs inside the test process, reads
/// the process environment, so that is pointed at it too, before any test touches a repository.
fn isolated_home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        // A fixed location, so repeated runs reuse it instead of leaking a directory each time.
        let home = std::env::temp_dir().join("fergit-core-tests-home");
        std::fs::create_dir_all(&home).expect("create the isolated home");
        std::fs::write(home.join("gitconfig"), "").expect("write an empty global config");
        // SAFETY: every test starts by creating a `Fixture`, which calls this first. `OnceLock`
        // makes concurrent callers wait until initialization returns, so no other thread of this
        // process reads the environment (to spawn git or open a repository) while it's changed.
        unsafe {
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            std::env::set_var("GIT_CONFIG_GLOBAL", home.join("gitconfig"));
            std::env::set_var("HOME", &home);
            std::env::set_var("USERPROFILE", &home);
            std::env::set_var("XDG_CONFIG_HOME", &home);
            std::env::set_var("GIT_TERMINAL_PROMPT", "0");
            for inherited in [
                "GIT_DIR",
                "GIT_WORK_TREE",
                "GIT_INDEX_FILE",
                "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                "GIT_CEILING_DIRECTORIES",
                "GIT_NAMESPACE",
                "GIT_CONFIG",
                "GIT_CONFIG_COUNT",
                "GIT_CONFIG_PARAMETERS",
            ] {
                std::env::remove_var(inherited);
            }
        }
        home
    })
}
