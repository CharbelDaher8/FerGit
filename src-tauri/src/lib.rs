//! FerGit desktop shell: exposes a fergit-core [`Session`] to the webview over typed IPC.
//!
//! Commands are thin: they move work off the async runtime and translate errors. Anything that
//! knows about git or the graph belongs in fergit-core.

mod operations;

use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use fergit_core::repo::{RepoError, RepoWatcher};
use fergit_core::session::Session;
use fergit_core::{CommitDetails, DiffSide, FileChange, FileDiff, Oid, RepoInfo, RowLocation, RowsPage};
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
    /// A command needed an open repository and none is open.
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

/// Sent when the open repository changed on disk and a newer snapshot is current. Carries what
/// `refresh` would return, so the UI handles both the same way.
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
pub struct RepoChanged(RepoInfo);

struct OpenRepo {
    session: Arc<Session>,
    /// Emits [`RepoChanged`]; `None` if watching failed, in which case changes show up on the next
    /// explicit refresh instead.
    _watcher: Option<RepoWatcher>,
}

/// App-wide state. One repository is open at a time for now.
#[derive(Default)]
struct AppState {
    open: RwLock<Option<OpenRepo>>,
}

impl AppState {
    fn session(&self) -> Result<Arc<Session>, AppError> {
        self.open
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|open| Arc::clone(&open.session))
            .ok_or_else(|| AppError::new(ErrorKind::NoRepository, "No repository is open."))
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

/// Opens the repository containing `path`, replacing any open repository, and starts emitting
/// `repoChanged` events for it.
#[tauri::command]
#[specta::specta]
async fn open_repo(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<RepoInfo, AppError> {
    let session = Arc::new(blocking(move || Session::open(Path::new(&path))).await?);
    // Emitting fails only while the app shuts down, when nobody is listening anyway.
    let watcher = session.watch(move |info| drop(RepoChanged(info).emit(&app))).ok();
    let info = session.info();
    // Replacing the previous repository drops its watcher. An event it already sent carries an
    // older generation than any of this repository's, so the UI ignores it.
    *state.open.write().unwrap_or_else(PoisonError::into_inner) = Some(OpenRepo { session, _watcher: watcher });
    Ok(info)
}

/// Re-reads the open repository. The generation changes only if something visible changed.
#[tauri::command]
#[specta::specta]
async fn refresh(state: State<'_, AppState>) -> Result<RepoInfo, AppError> {
    let session = state.session()?;
    blocking(move || session.refresh()).await
}

/// Rows `start..start + len` of the current snapshot, clamped to the rows that exist.
#[tauri::command]
#[specta::specta]
async fn rows(state: State<'_, AppState>, start: u32, len: u32) -> Result<RowsPage, AppError> {
    let session = state.session()?;
    blocking(move || session.rows(start, len)).await
}

/// Where the row showing `id` (a commit, a stash, or the all-zero id for uncommitted changes) is
/// in the current snapshot; `row` is `null` if no row shows it.
#[tauri::command]
#[specta::specta]
async fn locate(state: State<'_, AppState>, id: Oid) -> Result<RowLocation, AppError> {
    let session = state.session()?;
    blocking(move || Ok::<_, RepoError>(session.locate(id))).await
}

/// Full details of one commit; `null` if `id` isn't a commit.
#[tauri::command]
#[specta::specta]
async fn commit_details(state: State<'_, AppState>, id: Oid) -> Result<Option<CommitDetails>, AppError> {
    let session = state.session()?;
    blocking(move || session.commit_details(id)).await
}

/// Files that differ between `from` and `to`, sorted by path; a `from` of `null` compares against
/// nothing. Reads the index and worktree as they are now.
#[tauri::command]
#[specta::specta]
async fn changes(state: State<'_, AppState>, from: Option<DiffSide>, to: DiffSide) -> Result<Vec<FileChange>, AppError> {
    let session = state.session()?;
    blocking(move || session.changes(from, to)).await
}

/// How one file differs between `from` (`null`: nothing) and `to`. `path` names the file on the
/// `to` side; pass `oldPath` when it was renamed or copied from another path.
#[tauri::command]
#[specta::specta]
async fn file_diff(
    state: State<'_, AppState>,
    from: Option<DiffSide>,
    to: DiffSide,
    path: String,
    old_path: Option<String>,
) -> Result<FileDiff, AppError> {
    let session = state.session()?;
    blocking(move || session.file_diff(from, to, &path, old_path.as_deref())).await
}

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            open_repo,
            refresh,
            rows,
            locate,
            commit_details,
            changes,
            file_diff,
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
