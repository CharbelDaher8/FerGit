//! Running operations on open repositories, and asking the user for credentials git needs.
//!
//! `run_operation` resolves with the outcome; progress arrives meanwhile as `opProgress` events for
//! the session and the operation's id. When git needs a password, token or passphrase, a
//! `credentialPrompt` event asks the UI, which replies with `answer_prompt`. Prompts are app-wide:
//! whichever session's operation asked, the answer goes back to it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use fergit_core::askpass::{AskpassServer, PromptRequest, Prompter};
use fergit_core::session::OpContext;
use fergit_core::session::journal::Journal;
use fergit_core::{OpId, OpOutcome, Operation, SessionId, Undoable};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use crate::{AppError, AppState, ErrorKind};

/// How long a credential prompt waits for the user before git is told there's no answer.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// A line of git's progress for the operation `id` running on `session`, e.g.
/// `Receiving objects: 45% (9/20)`.
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
pub struct OpProgress {
    session: SessionId,
    id: OpId,
    text: String,
}

/// Git needs the user to type something. Answer with `answer_prompt(id, …)`.
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct CredentialPrompt {
    id: u32,
    /// Git's prompt, e.g. `Password for 'https://me@github.com': `.
    text: String,
    /// Mask the input: the answer is a password, token or passphrase.
    secret: bool,
}

/// App-wide state for operations: where they are journaled and how git reaches the user.
pub struct Operations {
    context: OpContext,
    prompts: Arc<Prompts>,
    /// Keeps the askpass server running for as long as the app runs.
    _askpass: Option<AskpassServer>,
}

impl Operations {
    /// Sets up the journal in the app's data directory and the askpass server. Either can fail
    /// without stopping the app: operations then go unjournaled, or fail when they need a prompt.
    pub fn new(app: &AppHandle) -> Operations {
        let journal = app
            .path()
            .app_local_data_dir()
            .ok()
            .and_then(|dir| Journal::open(dir.join("journal.jsonl")).ok())
            .map(Arc::new);
        let prompts = Arc::new(Prompts { app: app.clone(), next: AtomicU32::new(1), pending: Mutex::default() });
        // FerGit's own executable is the helper: `main` hands over to it when git runs it.
        let askpass = std::env::current_exe()
            .ok()
            .and_then(|exe| AskpassServer::start(exe, Arc::clone(&prompts) as Arc<dyn Prompter>).ok());
        Operations {
            context: OpContext { journal, askpass: askpass.as_ref().map(|server| server.askpass().clone()) },
            prompts,
            _askpass: askpass,
        }
    }
}

/// Credential prompts waiting for the user's answer, by prompt id.
struct Prompts {
    app: AppHandle,
    next: AtomicU32,
    pending: Mutex<HashMap<u32, Sender<Option<String>>>>,
}

impl Prompter for Prompts {
    fn prompt(&self, request: PromptRequest) -> Option<String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = mpsc::channel();
        self.pending.lock().unwrap_or_else(PoisonError::into_inner).insert(id, answer);
        let asked = CredentialPrompt { id, text: request.text, secret: request.secret }.emit(&self.app).is_ok();
        let reply = if asked { answered.recv_timeout(PROMPT_TIMEOUT).ok().flatten() } else { None };
        self.pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
        reply
    }
}

/// Runs `operation` on the repository of `session`. `id` identifies this request: running an id
/// that already ran on that session returns that run's outcome instead of running again. Failures
/// of the operation itself are an outcome, not a rejection; the call rejects only if the session
/// isn't open.
#[tauri::command]
#[specta::specta]
pub async fn run_operation(
    app: AppHandle,
    state: State<'_, AppState>,
    operations: State<'_, Operations>,
    session: SessionId,
    id: OpId,
    operation: Operation,
) -> Result<OpOutcome, AppError> {
    let session_id = session;
    let session = state.session(session_id)?;
    let context = operations.context.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let progress_id = id.clone();
        let mut progress = |text: &str| {
            // Emitting fails only while the app shuts down.
            let _ = OpProgress { session: session_id, id: progress_id.clone(), text: text.to_owned() }.emit(&app);
        };
        session.run(id, operation, &context, &mut progress)
    })
    .await
    .map_err(|e| AppError::new(ErrorKind::Internal, format!("background task failed: {e}")))
}

/// What undo would restore now on the repository of `session`: the most recent operation on it
/// that can be undone, why it can't, or that there is nothing to undo. Run it with the `undo`
/// operation, naming the entry this returned.
#[tauri::command]
#[specta::specta]
pub async fn undoable(
    state: State<'_, AppState>,
    operations: State<'_, Operations>,
    session: SessionId,
) -> Result<Undoable, AppError> {
    let session = state.session(session)?;
    let Some(journal) = operations.context.journal.clone() else {
        return Ok(Undoable::Nothing);
    };
    tauri::async_runtime::spawn_blocking(move || session.undoable(&journal))
        .await
        .map_err(|e| AppError::new(ErrorKind::Internal, format!("background task failed: {e}")))?
        .map_err(|e| AppError::new(ErrorKind::Internal, format!("Can't read the operation journal: {e}")))
}

/// Answers the credential prompt `id`; `null` cancels it, failing the operation that asked.
/// Answering a prompt that timed out or was already answered does nothing.
#[tauri::command]
#[specta::specta]
pub fn answer_prompt(operations: State<'_, Operations>, id: u32, answer: Option<String>) {
    let pending = operations.prompts.pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
    if let Some(reply) = pending {
        let _ = reply.send(answer);
    }
}
