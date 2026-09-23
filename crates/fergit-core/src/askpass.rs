//! Credential prompts from git, answered by the user in FerGit's window.
//!
//! When git needs a user name, password, token or SSH passphrase that no credential helper or agent
//! supplies, it runs the program named by `GIT_ASKPASS` (`SSH_ASKPASS` for ssh) with the prompt as
//! its argument and reads the answer from its stdout. FerGit points both at a helper, normally
//! FerGit's own executable, which calls [`helper_main`] first thing. The helper relays the prompt
//! to the [`AskpassServer`] running inside the app, which asks the user through a [`Prompter`].
//!
//! # Security
//!
//! - The server listens on loopback only, on a port chosen by the OS, and serves a request only if
//!   it starts with a 128-bit token generated per server. Port and token reach the helper through
//!   git's environment, so only processes git starts for an operation can use them.
//! - Answers exist only in memory, on their way from the prompter to the helper's stdout. They are
//!   never logged or journaled.

use std::collections::hash_map::RandomState;
use std::ffi::OsString;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Environment variable through which git's child, the helper, finds the server: `<port>:<token>`.
const ENDPOINT_VAR: &str = "FERGIT_ASKPASS_ENDPOINT";
/// The longest request the server reads: a token and a one-line prompt.
const MAX_REQUEST: u64 = 16 * 1024;
/// How long a connection may take to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A prompt git (or ssh) wants answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRequest {
    /// The prompt as git words it, e.g. `Password for 'https://me@example.com': `.
    pub text: String,
    /// Whether the answer is a secret (a password, token or passphrase) and should be masked.
    pub secret: bool,
}

/// Asks the user. Called on a background thread; may block until the user answers.
pub trait Prompter: Send + Sync + 'static {
    /// The user's answer, or `None` if they cancelled, which fails the git operation.
    fn prompt(&self, request: PromptRequest) -> Option<String>;
}

/// What git needs to reach an [`AskpassServer`]: pass it to operations that may prompt.
#[derive(Debug, Clone)]
pub struct Askpass {
    program: PathBuf,
    endpoint: String,
}

impl Askpass {
    /// Environment variables that route git's and ssh's prompts to the server.
    pub(crate) fn env(&self) -> [(&'static str, OsString); 4] {
        [
            ("GIT_ASKPASS", self.program.clone().into()),
            ("SSH_ASKPASS", self.program.clone().into()),
            // ssh otherwise uses SSH_ASKPASS only without a terminal and with DISPLAY set.
            ("SSH_ASKPASS_REQUIRE", "force".into()),
            (ENDPOINT_VAR, self.endpoint.clone().into()),
        ]
    }
}

/// Answers the helper's requests until dropped.
pub struct AskpassServer {
    askpass: Askpass,
    address: SocketAddr,
    stopped: Arc<AtomicBool>,
}

impl AskpassServer {
    /// Starts serving prompts to `prompter`. `program` is the helper git should run: an executable
    /// that calls [`helper_main`] before anything else.
    pub fn start(program: PathBuf, prompter: Arc<dyn Prompter>) -> io::Result<AskpassServer> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let token = random_token();
        let stopped = Arc::new(AtomicBool::new(false));
        let askpass = Askpass { program, endpoint: format!("{}:{token}", address.port()) };

        let stop = Arc::clone(&stopped);
        thread::Builder::new().name("askpass".to_owned()).spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let Ok(stream) = stream else {
                    continue;
                };
                let (token, prompter) = (token.clone(), Arc::clone(&prompter));
                // One thread per prompt: a user taking their time must not block another request.
                let _ = thread::Builder::new()
                    .name("askpass-request".to_owned())
                    .spawn(move || drop(serve(stream, &token, prompter.as_ref())));
            }
        })?;
        Ok(AskpassServer { askpass, address, stopped })
    }

    pub fn askpass(&self) -> &Askpass {
        &self.askpass
    }
}

impl Drop for AskpassServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(1));
    }
}

/// Handles one request: `<token>\n<prompt>`, answered with `ok\n<answer>` or `cancel\n`.
fn serve(mut stream: TcpStream, token: &str, prompter: &dyn Prompter) -> io::Result<()> {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    let mut request = String::new();
    (&mut stream).take(MAX_REQUEST).read_to_string(&mut request)?;
    let Some((sent_token, prompt)) = request.split_once('\n') else {
        return Ok(());
    };
    if !constant_time_eq(sent_token.as_bytes(), token.as_bytes()) {
        return Ok(());
    }
    let answer = prompter.prompt(PromptRequest { text: prompt.to_owned(), secret: is_secret(prompt) });
    let reply = match answer {
        Some(answer) => format!("ok\n{answer}"),
        None => "cancel\n".to_owned(),
    };
    stream.write_all(reply.as_bytes())?;
    stream.shutdown(Shutdown::Write)
}

/// If git started this process as the askpass helper, relays the prompt to the app, prints the
/// answer and returns the exit code to end the process with. Otherwise returns `None` at once.
///
/// Call it first thing in `main` of the program passed to [`AskpassServer::start`].
pub fn helper_main() -> Option<i32> {
    let endpoint = std::env::var(ENDPOINT_VAR).ok()?;
    let prompt = std::env::args().nth(1).unwrap_or_default();
    Some(match ask(&endpoint, &prompt) {
        Ok(Some(answer)) => {
            let mut stdout = io::stdout().lock();
            match writeln!(stdout, "{answer}").and_then(|()| stdout.flush()) {
                Ok(()) => 0,
                Err(_) => 1,
            }
        }
        // Git treats a failing helper as no answer and fails the operation, which is what
        // cancelling means.
        Ok(None) | Err(_) => 1,
    })
}

fn ask(endpoint: &str, prompt: &str) -> io::Result<Option<String>> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "malformed askpass endpoint");
    let (port, token) = endpoint.split_once(':').ok_or_else(invalid)?;
    let port: u16 = port.parse().map_err(|_| invalid())?;
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
    // The prompt is one line; a newline in it would only confuse the protocol.
    stream.write_all(format!("{token}\n{}", prompt.replace(['\r', '\n'], " ")).as_bytes())?;
    stream.shutdown(Shutdown::Write)?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply.strip_prefix("ok\n").map(str::to_owned))
}

/// Whether the answer to `prompt` should be masked. Only prompts known to ask for something
/// public (a user name, a yes/no confirmation) aren't; anything unrecognized is treated as secret.
fn is_secret(prompt: &str) -> bool {
    let prompt = prompt.to_ascii_lowercase();
    !(prompt.starts_with("username") || prompt.contains("(yes/no"))
}

/// 128 unpredictable bits as hex. `RandomState` seeds its keys from the operating system's random
/// source, which is all the randomness a loopback token needs, without another dependency.
fn random_token() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    (0..2u8)
        .map(|i| {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u8(i);
            hasher.write_u128(nanos);
            format!("{:016x}", hasher.finish())
        })
        .collect()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |diff, (x, y)| diff | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Answer(Mutex<Vec<PromptRequest>>, Option<&'static str>);

    impl Prompter for Answer {
        fn prompt(&self, request: PromptRequest) -> Option<String> {
            self.0.lock().unwrap().push(request);
            self.1.map(str::to_owned)
        }
    }

    fn endpoint(server: &AskpassServer) -> String {
        server.askpass().endpoint.clone()
    }

    #[test]
    fn relays_a_prompt_and_its_answer() {
        let prompter = Arc::new(Answer(Mutex::default(), Some("s3cret")));
        let server = AskpassServer::start("helper".into(), prompter.clone()).unwrap();

        let answer = ask(&endpoint(&server), "Password for 'https://me@example.com': ").unwrap();

        assert_eq!(answer.as_deref(), Some("s3cret"));
        let seen = prompter.0.lock().unwrap();
        assert_eq!(seen.as_slice(), [PromptRequest { text: "Password for 'https://me@example.com': ".into(), secret: true }]);
    }

    #[test]
    fn a_cancelled_prompt_has_no_answer() {
        let server = AskpassServer::start("helper".into(), Arc::new(Answer(Mutex::default(), None))).unwrap();
        assert_eq!(ask(&endpoint(&server), "Username for 'https://example.com': ").unwrap(), None);
    }

    #[test]
    fn a_wrong_token_is_ignored() {
        let prompter = Arc::new(Answer(Mutex::default(), Some("s3cret")));
        let server = AskpassServer::start("helper".into(), prompter.clone()).unwrap();
        let port = endpoint(&server).split_once(':').unwrap().0.to_owned();

        assert_eq!(ask(&format!("{port}:{}", "0".repeat(32)), "Password: ").unwrap(), None);
        assert!(prompter.0.lock().unwrap().is_empty(), "the user is never asked");
    }

    #[test]
    fn user_names_and_confirmations_are_not_secret() {
        assert!(!is_secret("Username for 'https://github.com': "));
        assert!(!is_secret("Are you sure you want to continue connecting (yes/no/[fingerprint])? "));
        assert!(is_secret("Password for 'https://me@github.com': "));
        assert!(is_secret("Enter passphrase for key '/home/me/.ssh/id_ed25519': "));
    }

    #[test]
    fn tokens_differ() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
