//! FerGit core: reading git repositories ([`repo`]) and serving consistent, refreshable views of
//! them ([`session`]). No Tauri dependency; the desktop shell lives in `src-tauri`.

pub mod askpass;
mod id_map;
pub mod repo;
pub mod session;
mod types;

pub use fergit_graph::{Edge, GraphRow, Half};
pub use types::*;
