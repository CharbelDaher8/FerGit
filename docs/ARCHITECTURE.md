# FerGit — Architecture Plan

A Git Graph replacement built with Rust + Tauri 2.

- **Status:** plan, nothing built yet
- **Written:** 2026-09-14
- **Grounded in:** *A Philosophy of Software Design* (APOSD) and *Designing Data-Intensive Applications* (DDIA). Chapter references are given inline.

---

## 0. The main idea

**The `.git` directory is the only real source of truth.** Everything FerGit keeps in memory is **derived data**: the commit index, graph layout, ref labels and avatars. It's a cache that can be thrown away and rebuilt from git (DDIA ch10–12).

FerGit never keeps its own copy of "which branches exist" and patches it after an operation. It runs the operation, then reads git again. That rules out a whole class of out-of-sync bugs (DDIA ch11, dual writes). It also means a change made in a terminal or VS Code shows up exactly like a change made in FerGit.

---

## 1. Layers

Each layer works with different concepts from the one below it (APOSD ch7):

| Layer | Works in terms of | Hides |
|---|---|---|
| `ui/` (Svelte + TS) | pixels, rows, clicks, user intents | everything about git |
| `src-tauri` | IPC messages | Tauri itself; it's only a thin dispatcher |
| `session` | snapshots, generations, operations | concurrency, file watching, refresh, the operation journal |
| `graph` | lanes, columns, edges | the layout algorithm, color assignment |
| `repo` | commits, refs, diffs, `Operation` | gix vs git CLI, argument quoting, output parsing, locks, locale |

Use three crates, not ten. Splitting too early adds interfaces without hiding anything (APOSD ch4/ch9):

```
FerGit/
├─ Cargo.toml              # workspace
├─ package.json            # frontend deps + Tauri CLI (at the root so the CLI finds src-tauri)
├─ crates/fergit-graph/    # pure layout: no I/O, no gix
├─ crates/fergit-core/     # no Tauri dependency, so it can be tested headless
│  └─ src/{repo/, session/, types.rs}
├─ src-tauri/              # commands, events, capabilities, askpass helper
└─ ui/                     # Svelte 5 + TS, generated src/lib/bindings.ts
```

**Why `graph` is its own crate** (changed 2026-09-15 when building started): the interface stays the same, but the compiler now enforces that layout does no I/O and never touches gix. It also builds in seconds, and work in `repo` can't break the layout tests. Split `repo` and `session` apart only if compile times demand it.

---

## 2. Core abstractions

### 2.1 `repo`: the only module that knows about git

Three designs compared (APOSD ch11, design it twice):

| | libgit2 (`git2`) | Only shell out to `git` | **gix for reads + `git` CLI for writes** |
|---|---|---|---|
| Reads (the hot path) | good | slow: a process spawn plus parsing per call, and spawning is expensive on Windows | fastest; reads the commit-graph file directly |
| Writes (rebase, merge, push) | no hooks, partial config support, its own credential handling | behaves exactly like the user's git: hooks, signing, credential helpers, SSH | same as the CLI option |

**Decision: gix for reads, the `git` CLI for writes.** Callers never know which backend answered (APOSD ch5). If gix is missing a feature, the module falls back to the CLI internally and the interface doesn't change.

Everything specific to the git CLI lives only in this module:

- `Command::arg`, never a shell string
- `--end-of-options` before any ref the user supplied
- machine-readable output: `-z`, `--porcelain=v2`
- `LC_ALL=C`, `GIT_TERMINAL_PROMPT=0`, and `GIT_ASKPASS` pointing to our own helper
- a short retry when git's `index.lock` is briefly held

Operations describe what the user wants, not git commands (APOSD ch6):

```rust
pub enum Operation {
    Checkout     { target: Rev },
    CreateBranch { name: RefName, at: Oid },
    DeleteBranch { name: RefName, force: bool },      // means "ensure it's gone": already gone → Ok
    Merge        { from: Rev, mode: MergeMode },
    Rebase       { onto: Rev },
    CherryPick   { commits: Vec<Oid> },
    Revert       { commit: Oid },
    Reset        { branch: RefName, to: Oid, mode: ResetMode, expected: Oid },
    Push         { branch: RefName, remote: String, force: ForceMode }, // lease-based force only
    Fetch        { remote: Option<String>, prune: bool },
    Tag { .. }, Stash { .. },
}
```

The `expected: Oid` field is a **compare-and-set**. The operation carries the branch tip the user was looking at. If the branch has moved since then, the operation fails instead of overwriting someone's work (DDIA ch7, lost updates).

- It maps to `git update-ref <ref> <new> <old>` and `--force-with-lease=<ref>:<expected>`.
- A plain `--force` is never offered.

### 2.2 `graph`: a pure function

`layout(commits in topological order, refs) -> rows`.

- It does no I/O, reads no clock, and has no hash-map ordering in its output.
- Because it's deterministic (DDIA ch10), it's easy to test with snapshots (`insta`) and property tests (`proptest`).
- Properties to test: every parent edge connects, and no two nodes share a cell.

**Algorithm.** It processes one commit at a time and costs O(rows × lanes). It keeps a list of lanes, each waiting for a particular commit (`lanes: Vec<Option<Oid>>`). For each commit:

1. Its column is the leftmost lane waiting for it. Other lanes waiting for it are merge-ins; free them.
2. The first parent takes over that column and its color, so a branch line keeps one color.
3. For each other parent, reuse a lane already waiting for it. Otherwise take the leftmost free lane.

Rows 0..N never depend on anything below N, so the graph can be loaded page by page.

**No special cases.** Normal commits, the uncommitted-changes row and stashes (`Commit | WorkingTree | Stash`) are all ordinary nodes. The working-tree row is simply a node whose parent is HEAD, so there's no separate rendering path (APOSD ch10).

**Where the layout runs.** Doing it in TypeScript would send less data over IPC, but graph logic would be split across two languages. **Decision: Rust.** One owner, faster, and testable. The UI knows how to *draw* a lane, not how to *work one out*.

### 2.3 `session`: snapshots, one writer, and a journal

```rust
/// An open repository. Reads see an immutable, consistent snapshot; mutations are
/// serialized, and every mutation or external change produces a new snapshot.
impl RepoSession {
    pub fn open(path: &Path) -> Result<Self, OpenError>;
    pub fn snapshot(&self) -> Arc<Snapshot>;                        // cheap
    pub async fn run(&self, id: OpId, op: Operation) -> OpOutcome;  // single-writer queue
    pub fn changes(&self) -> watch::Receiver<Generation>;
}

impl Snapshot {
    pub fn generation(&self) -> Generation;               // logical counter, not a timestamp
    pub fn row_count(&self) -> u32;
    pub fn rows(&self, range: Range<u32>) -> Vec<Row>;    // clamps; past the end → empty
    pub fn locate(&self, oid: Oid) -> Option<u32>;        // keeps the scroll position across refreshes
    pub fn commit(&self, oid: Oid) -> Option<CommitDetails>;
    pub fn diff(&self, spec: DiffSpec) -> Result<Diff, ReadError>;
    pub fn state(&self) -> RepoState;                     // Clean | Merging{..} | Rebasing{..}
}
```

- **Refresh by watching git's files.** Watch `.git/HEAD`, `refs/`, `packed-refs`, `index`, `logs/` and the working tree, with debouncing.
  - Build the new snapshot in the background and swap it in all at once, then emit `repo_changed`.
  - The old snapshot keeps answering scroll requests until the new one is ready (DDIA ch10).
- **Order responses with generations.** Every response carries its generation number, and the UI ignores out-of-date ones. Use a counter rather than wall-clock time, which can't order these reliably (DDIA ch8).
- **One writer, many readers.** Operations go through a single queue per repo. Readers hold `Arc<Snapshot>`, so a long fetch never blocks scrolling (DDIA ch3).
- **Duplicate-safe operations.** The UI generates the operation ID and the session ignores repeats, so double-clicking Push runs one push (DDIA ch12).
- **Operation journal: append-only JSONL, never rewritten.** Each entry records:
  - the operation ID and operation
  - ref tips before and after
  - exit status and duration
  - stderr, with credentials scrubbed

  The journal gives you:
  - an audit trail
  - **Undo**, done as a *new* operation that restores the recorded tips; the log itself is never edited (DDIA ch11)
  - an optional, off-by-default enterprise feature that ships it to Azure (Log Analytics, or Blob storage with an immutability/WORM policy)
- **Compact in-memory layout (APOSD ch20, DDIA ch3).** Keep the commit index as separate arrays:
  - `oids: Vec<[u8;20]>`
  - `parent_offsets: Vec<u32>` and `parents: Vec<u32>` (indices, not oids)
  - `times: Vec<i64>`
  - `author: Vec<u32>` (interned)

  Load commit messages only for visible rows. That's about 50 MB for 1.3M commits, and none of it shows in the interface.

---

## 3. IPC contract (Tauri 2)

A few general commands beat 30 specific ones:

```
open_repo(path)                                 -> RepoInfo { id, generation, row_count }
rows(repo, generation, start, len)              -> RowsPage       // clamped, never errors
locate(repo, generation, oid)                   -> Option<u32>
commit_details(repo, oid)                       -> CommitDetails
diff(repo, DiffSpec)                            -> Diff
search(repo, query, Channel<Hit>)               -> ()             // streamed
run(repo, op_id, Operation, Channel<Progress>)  -> OpOutcome
event repo_changed { repo, generation, row_count }
```

- **One diff mechanism.** `DiffSpec { from: Side, to: Side, paths }`, with `Side = Commit(oid) | Index | WorkTree`, covers the commit view, comparing two commits, uncommitted changes and stashes (APOSD ch6).
- **Generate the TypeScript types** from Rust with `tauri-specta`. The schema is defined once, so a hand-written `types.ts` can't drift from the Rust structs (APOSD ch5).
- **Compatibility, only where it matters (DDIA ch4).** The UI and backend ship in the same binary, so the IPC schema needs no versioning. What does need it is data that **outlives the binary**: `settings.json`, the journal, any cache.
  - Store a `schema_version`.
  - Only ever add fields, with `#[serde(default)]`.
  - Keep a `#[serde(flatten)] extra: serde_json::Map`, so an older FerGit rewriting the settings file doesn't delete fields a newer version added.
- **Commit IDs travel as hex strings**, never as numbers.
- **Start with JSON over `Channel`.** Switch `rows()` to raw bytes (`tauri::ipc::Response`) only if measurements show it's the bottleneck. The call signature stays the same (APOSD ch20, measure first).

---

## 4. Errors (APOSD ch10)

| Situation | How it's handled |
|---|---|
| Merge, rebase or cherry-pick stops with conflicts | **Not an error.** It's a normal state, `RepoState::Merging{conflicts}`, shown as a banner with Continue / Abort. |
| Empty repo, unborn HEAD, detached HEAD | Normal: zero rows, or HEAD is just another label |
| `rows()` past the end, or deleting a branch that's already gone | Defined away: returns an empty page or `Ok` |
| `index.lock` briefly held | Handled inside `repo` with a short retry |
| Auth failure, rejected push, stale lease, failing hook | One result type, `OpOutcome::Failed { kind, message, stderr }`, handled in one place in the UI (a toast plus "show git output"). The message is written where the failure is detected. |
| Graph layout's internal consistency check fails | A bug: `debug_assert!`, plus an error on that view only, with a report button |

---

## 5. Performance (DDIA ch1, APOSD ch20)

- **What drives load:**
  - commit count (Linux is about 1.3M)
  - ref count (some monorepos have 100k+ tags)
  - how wide the graph gets
  - size of a single diff
  - how often files change underneath you (an IDE saving constantly)
- **Targets, as percentiles.** Measure them in the app with `hdrhistogram`.
  - First page of the graph: p99 < 300 ms on a 1M-commit repo that has a commit-graph file
  - Scrolling: p99 < 16 ms per frame
  - Refresh after an external change: p95 < 500 ms
- **UI:**
  - a virtualized list
  - one `<canvas>` drawing only the visible rows, rather than one SVG for every loaded commit
  - pages of about 200 rows, fetched ahead of the scroll
- **Backend:**
  - no `git` process spawned on hot paths
  - suggest `git commit-graph write` / `fetch.writeCommitGraph=true` when a repo lacks a commit-graph file
  - no on-disk cache of your own until measurements show a slow cold start; if one is added, it's disposable and keyed by ref tips
- **Benchmarks from week one.** Build a harness using linux, chromium and a synthetic repo with very wide merges, and run it in CI.

---

## 6. Security review

- **Tauri capabilities.**
  - The webview can call only FerGit's own commands: no `fs`, `shell` or `http` plugin permissions.
  - Strict CSP.
  - No remote content in the webview. Avatars are served through a Rust custom URI scheme (`avatar://`).
- **Treat repo content as untrusted.** Anyone can author a commit, so commit messages, names, branch names and paths can be hostile.
  - Render text only, never `innerHTML`.
  - Validate ref names and put `--end-of-options` before them, so a branch named `--upload-pack=…` can't become a git option.
- **Malicious repos.** A repo's `.git/config` can run commands (`core.fsmonitor`, `core.sshCommand`, hooks).
  - Background reads go through gix, which doesn't run those.
  - Respect `safe.directory`.
  - Any background CLI read gets `-c core.fsmonitor=false`.
  - Hooks run only on operations the user starts, as with git itself.
- **Authentication.**
  - FerGit never stores credentials. It uses git credential helpers (Git Credential Manager on Windows) and ssh-agent.
  - Password prompts appear in our `GIT_ASKPASS` dialog. FerGit's own executable is the helper; it relays each prompt to the app over loopback TCP, guarded by a per-run 128-bit token.
  - Git Credential Manager runs with `GCM_INTERACTIVE=never`: it can still supply stored credentials, but it can't open its own sign-in windows. Any `GIT_ASKPASS`/`SSH_ASKPASS` inherited from the environment is dropped. The trade-off is no browser OAuth sign-in from FerGit; use a token in the dialog instead.
  - The journal lives at `<app local data dir>/journal.jsonl`, one file for all repositories. Each entry records the repository's root.
  - Tokens and `https://user:token@` URLs are scrubbed before anything is journaled.
- **SSO, MQTT, certificates.**
  - A local app needs none of these.
  - If the Azure journal upload ships, authenticate with Entra ID or certificates issued only to enrolled machines, over TLS.
- **Updates.** Use Tauri's updater with signed artifacts, and Authenticode signing on Windows.

---

## 7. Data usage

- **Reads:** local repo contents only.
- **Stores locally:** `settings.json`, the recent-repos list, the operation journal (immutable, append-only) and an avatar cache.
- **Sends off the machine only:**
  - git network operations the user starts (fetch, pull, push)
  - avatar lookups, **opt-in** because they send email hashes to Gravatar or GitHub
  - the optional Azure journal upload
- **No telemetry by default.**

---

## 8. Milestones

1. **Spike (about a week).**
   - Walk the Linux repo's history with gix and run the layout; time it.
   - Push 10k rows through a Tauri `Channel` to decide JSON vs binary.
   - Prototype canvas scrolling.
   - *Throw the code away, keep the numbers.*
2. **Read-only viewer.** Open a repo; graph, ref labels, working-tree row, commit details, diffs, refresh when files change, scroll position kept across refreshes.
3. **Safe operations + journal.** Checkout, create/delete branches and tags, fetch/pull, push with lease, password prompt dialog.
4. **History-changing operations.** Merge, rebase, cherry-pick, revert, reset, stash. Conflicts as a normal state; undo from the journal.
5. **Git Graph feature parity.** Find, compare commits, branch filter, several repos in tabs, opt-in avatars, themes.
6. **Hardening.**
   - Property tests and fuzzing for the layout.
   - Integration tests that *actually trigger* the failure paths: a push rejected by a local bare remote, forced conflicts, a held `index.lock`.
   - Performance CI and a security pass.

### Testing

- **`graph`:** pure unit, snapshot (`insta`) and property (`proptest`) tests.
- **`repo` / `session`:** integration tests against temporary repos built with scripted `git`, with no Tauri involved (which is why `fergit-core` has no Tauri dependency).
- **UI:** a few smoke tests with `tauri-driver`.

---

## Development notes

- **Keep the checkout out of Windows protected folders** (Documents, Desktop, Pictures…). Where Defender's Controlled Folder Access is on, it blocks `rustc`, build scripts and test binaries from writing there. It shows up as "The system cannot find the file specified (os error 2)", and the Defender log records event 1123. Per-program allowances don't last, because Rust build scripts and test binaries get new hashed paths on every rebuild. The project lives in `C:\Users\CharbelAbiHannaDaher\dev\FerGit` for this reason.
- **Regenerate TypeScript bindings** with `cargo test -p fergit --lib export_bindings`. They are also rewritten on every debug launch of the app.

---

## Open decisions

- [ ] **UI framework.** Leaning Svelte 5, which is small and needs little boilerplate. React works just as well if the team knows it; the architecture stays the same.
- [ ] **Standalone or integrated.** Should FerGit also open from VS Code through a `fergit://open?path=` link? That only affects deep-link registration, not the core.
