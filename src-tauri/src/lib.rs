//! FerGit desktop shell: exposes fergit-core [`Sessions`], one per tab, to the webview over typed
//! IPC.
//!
//! Commands are thin: they move work off the async runtime and translate errors. Anything that
//! knows about git or the graph belongs in fergit-core.

mod operations;

use std::path::Path;
use std::sync::Arc;

use fergit_core::repo::RepoError;
use fergit_core::session::{Session, Sessions};
use fergit_core::{
    CommitDetails, DiffSide, FileChange, FileDiff, Filter, Oid, RefLabel, RepoInfo, RowLocation, RowsPage, SearchResult,
    SessionId,
};
use serde::Serialize;
use specta_typescript::Typescript;
use tauri::{AppHandle, Manager, State};
use tauri_specta::{Builder, ErrorHandlingMode, Event, collect_commands, collect_events};

/// The error every command rejects with. The UI shows `message`; `kind` only picks wording.
#[derive(Debug, Serialize, specta::Type, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct AppError {
    kind: ErrorKind,
    message: String,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// A command named a session that isn't open (any more).
    NoRepository,
    NotARepository,
    Git,
    /// A bug in FerGit.
    Internal,
}

impl AppError {
    fn new(kind: ErrorKind, message: impl Into<String>) -> AppError {
        AppError { kind, message: message.into() }
    }
}

impl From<RepoError> for AppError {
    fn from(error: RepoError) -> AppError {
        let kind = match error {
            RepoError::NotARepository { .. } => ErrorKind::NotARepository,
            RepoError::Git(_) => ErrorKind::Git,
        };
        AppError::new(kind, error.to_string())
    }
}

/// Sent when an open repository changed on disk and a newer snapshot is current. `info` is what
/// `refresh` would return for `session`, so the UI handles both the same way.
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
pub struct RepoChanged {
    session: SessionId,
    info: RepoInfo,
}

/// A repository `open_repo` opened, or found already open.
#[derive(Debug, Serialize, specta::Type)]
pub struct OpenedRepo {
    session: SessionId,
    info: RepoInfo,
}

/// App-wide state: the open sessions, one per tab.
#[derive(Default)]
struct AppState {
    sessions: Arc<Sessions>,
}

impl AppState {
    fn session(&self, id: SessionId) -> Result<Arc<Session>, AppError> {
        self.sessions
            .get(id)
            .ok_or_else(|| AppError::new(ErrorKind::NoRepository, "That repository is no longer open."))
    }
}

/// Runs blocking repository work on Tauri's blocking thread pool.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, RepoError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| AppError::new(ErrorKind::Internal, format!("background task failed: {e}")))?
        .map_err(AppError::from)
}

/// Opens the repository containing `path` in a new session that emits `repoChanged` events, or
/// returns the session that already has that repository open.
#[tauri::command]
#[specta::specta]
async fn open_repo(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<OpenedRepo, AppError> {
    let sessions = Arc::clone(&state.sessions);
    blocking(move || {
        // Emitting fails only while the app shuts down, when nobody is listening anyway.
        let on_change = move |session, info| drop(RepoChanged { session, info }.emit(&app));
        let (session, opened) = sessions.open(Path::new(&path), on_change)?;
        Ok(OpenedRepo { session, info: opened.info() })
    })
    .await
}

/// Closes a session and stops watching its repository. Closing a closed session does nothing.
#[tauri::command]
#[specta::specta]
async fn close_repo(state: State<'_, AppState>, session: SessionId) -> Result<(), AppError> {
    let sessions = Arc::clone(&state.sessions);
    // Stopping the watcher can wait on its thread, so it stays off the async runtime.
    blocking(move || {
        sessions.close(session);
        Ok(())
    })
    .await
}

/// Re-reads a session's repository. The generation changes only if something visible changed.
#[tauri::command]
#[specta::specta]
async fn refresh(state: State<'_, AppState>, session: SessionId) -> Result<RepoInfo, AppError> {
    let session = state.session(session)?;
    blocking(move || session.refresh()).await
}

/// Rows `start..start + len` of the current snapshot, clamped to the rows that exist.
#[tauri::command]
#[specta::specta]
async fn rows(state: State<'_, AppState>, session: SessionId, start: u32, len: u32) -> Result<RowsPage, AppError> {
    let session = state.session(session)?;
    blocking(move || session.rows(start, len)).await
}

/// Where the row showing `id` (a commit, a stash, or the all-zero id for uncommitted changes) is
/// in the current snapshot; `row` is `null` if no row shows it.
#[tauri::command]
#[specta::specta]
async fn locate(state: State<'_, AppState>, session: SessionId, id: Oid) -> Result<RowLocation, AppError> {
    let session = state.session(session)?;
    blocking(move || Ok::<_, RepoError>(session.locate(id))).await
}

/// Full details of one commit; `null` if `id` isn't a commit.
#[tauri::command]
#[specta::specta]
async fn commit_details(
    state: State<'_, AppState>,
    session: SessionId,
    id: Oid,
) -> Result<Option<CommitDetails>, AppError> {
    let session = state.session(session)?;
    blocking(move || session.commit_details(id)).await
}

/// Files that differ between `from` and `to`, sorted by path; a `from` of `null` compares against
/// nothing. Reads the index and worktree as they are now.
#[tauri::command]
#[specta::specta]
async fn changes(
    state: State<'_, AppState>,
    session: SessionId,
    from: Option<DiffSide>,
    to: DiffSide,
) -> Result<Vec<FileChange>, AppError> {
    let session = state.session(session)?;
    blocking(move || session.changes(from, to)).await
}

/// How one file differs between `from` (`null`: nothing) and `to`. `path` names the file on the
/// `to` side; pass `oldPath` when it was renamed or copied from another path.
#[tauri::command]
#[specta::specta]
async fn file_diff(
    state: State<'_, AppState>,
    session: SessionId,
    from: Option<DiffSide>,
    to: DiffSide,
    path: String,
    old_path: Option<String>,
) -> Result<FileDiff, AppError> {
    let session = state.session(session)?;
    blocking(move || session.file_diff(from, to, &path, old_path.as_deref())).await
}

/// The rows of a session's current snapshot whose commit message, author or id `query` matches,
/// ignoring case. Check `generation`: rows of an older snapshot mean nothing now.
#[tauri::command]
#[specta::specta]
async fn search(state: State<'_, AppState>, session: SessionId, query: String) -> Result<SearchResult, AppError> {
    let session = state.session(session)?;
    blocking(move || session.search(&query)).await
}

/// Shows only the commits `filter` selects in a session (the default filter shows all). Returns
/// what `refresh` would: the generation changes unless the filter was already applied.
#[tauri::command]
#[specta::specta]
async fn set_filter(state: State<'_, AppState>, session: SessionId, filter: Filter) -> Result<RepoInfo, AppError> {
    let session = state.session(session)?;
    blocking(move || session.set_filter(filter)).await
}

/// Every ref of a session's repository, for choosing what to filter on.
#[tauri::command]
#[specta::specta]
async fn refs(state: State<'_, AppState>, session: SessionId) -> Result<Vec<RefLabel>, AppError> {
    let session = state.session(session)?;
    blocking(move || Ok::<_, RepoError>(session.refs())).await
}

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            open_repo,
            close_repo,
            refresh,
            rows,
            locate,
            commit_details,
            changes,
            file_diff,
            search,
            set_filter,
            refs,
            operations::run_operation,
            operations::answer_prompt,
            operations::undoable,
        ])
        .events(collect_events![RepoChanged, operations::OpProgress, operations::CredentialPrompt])
        .error_handling(ErrorHandlingMode::Throw)
}

const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/src/lib/bindings.ts");

/// Writes the TypeScript bindings the UI imports. Run via `cargo test -p fergit export_bindings`,
/// and automatically on every debug launch.
fn export_bindings(builder: &Builder<tauri::Wry>) {
    let path = Path::new(BINDINGS_PATH);
    std::fs::create_dir_all(path.parent().expect("bindings path has a parent"))
        .expect("failed to create the bindings directory");
    builder
        .export(Typescript::default().header("// Generated by tauri-specta from src-tauri. Do not edit.\n"), path)
        .expect("failed to export TypeScript bindings");
}

pub fn run() {
    let builder = specta_builder();
    #[cfg(debug_assertions)]
    export_bindings(&builder);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            app.manage(operations::Operations::new(app.handle()));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running FerGit");
}

#[cfg(test)]
mod tests {
    #[test]
    fn export_bindings() {
        super::export_bindings(&super::specta_builder());
    }
}
