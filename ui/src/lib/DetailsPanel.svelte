<!--
  Details of the selection, as the Inspector loads them: a commit's message, signatures, parents and
  files; the staged and unstaged files of uncommitted changes; or the files between two compared
  commits. Clicking a file opens its diff.
-->
<script lang="ts">
  import type { ChangeStatus, FileChange, Oid, Signature } from "./bindings";
  import { formatSignatureTime, shortId } from "./format";
  import { subjectKey, type FileList, type Inspector, type ListKey } from "./inspector.svelte";
  import type { RepoView } from "./view.svelte";

  interface Props {
    /** Where the selection lives; parent links navigate within it. */
    view: RepoView;
    inspector: Inspector;
    /** Opens the diff of `file` from `list`. */
    onopen: (list: FileList, file: FileChange) => void;
    onclose: () => void;
  }

  let { view, inspector, onopen, onclose }: Props = $props();

  /** Files rendered per list before asking; a huge commit would otherwise freeze the panel. */
  const FILE_LIMIT = 500;

  const STATUS_LETTER: Record<ChangeStatus, string> = {
    added: "A",
    modified: "M",
    deleted: "D",
    renamed: "R",
    copied: "C",
    typeChanged: "T",
  };

  const LIST_TITLE: Record<ListKey, string> = {
    commit: "Changed files",
    staged: "Staged",
    unstaged: "Unstaged",
    range: "Changed files",
  };

  const subject = $derived(inspector.subject);
  const key = $derived(subjectKey(subject));
  const details = $derived(inspector.details);
  const selectedRow = $derived(view.selected === null ? undefined : view.row(view.selected));
  const comparison = $derived(view.comparison);
  const message = $derived.by(() => {
    if (!details) return { subject: "", body: "" };
    const first = details.message.split("\n", 1)[0];
    return { subject: first, body: details.message.slice(first.length).replace(/^\s*\n/, "").trimEnd() };
  });

  /** Lists showing all their files, as `subjectKey/listKey`. */
  let expanded = $state.raw(new Set<string>());

  /** Row of each parent in the snapshot on screen: absent while looking up, `null` if not shown. */
  let parentRows = $state.raw(new Map<Oid, number | null>());

  $effect(() => {
    const parents = details?.parents ?? [];
    void view.generation; // look the parents up again whenever the snapshot changes
    let current = true;
    void Promise.all(parents.map((parent) => view.find(parent))).then((rows) => {
      if (current) parentRows = new Map(parents.map((parent, k) => [parent, rows[k]]));
    });
    return () => {
      current = false;
    };
  });

  // Keyboard movement through the file lists, for the app's key router. Focus moves between the
  // real file buttons, so the Staged and Unstaged lists read as one list.
  let root: HTMLElement;

  function fileButtons(): HTMLButtonElement[] {
    return [...root.querySelectorAll<HTMLButtonElement>("button.file")];
  }

  function focusedFile(buttons: HTMLButtonElement[]): number {
    return buttons.indexOf(document.activeElement as HTMLButtonElement);
  }

  function focusFile(buttons: HTMLButtonElement[], index: number): void {
    const button = buttons[Math.max(0, Math.min(buttons.length - 1, index))];
    if (!button) return;
    button.focus();
    button.scrollIntoView({ block: "nearest" });
  }

  /**
   * Moves keyboard focus into the file lists: onto the open file, else the first file. While no
   * files are shown the panel itself takes focus, so list keys work once they load.
   */
  export function focusFiles(): void {
    const buttons = fileButtons();
    if (buttons.length === 0) {
      root.focus();
      return;
    }
    focusFile(buttons, Math.max(0, buttons.findIndex((button) => button.classList.contains("open"))));
  }

  /** Moves focus `by` files down (negative: up), across all lists. */
  export function moveFile(by: number): void {
    const buttons = fileButtons();
    const current = focusedFile(buttons);
    focusFile(buttons, current < 0 ? (by > 0 ? 0 : buttons.length - 1) : current + by);
  }

  /** Focuses file `index` (clamped) or the last file. */
  export function gotoFile(index: number | "last"): void {
    const buttons = fileButtons();
    focusFile(buttons, index === "last" ? buttons.length - 1 : index);
  }

  /** Opens the focused file's diff; without a focused file, focuses the first one. */
  export function openFocused(): void {
    const buttons = fileButtons();
    const current = focusedFile(buttons);
    if (current < 0) focusFile(buttons, 0);
    else buttons[current].click();
  }
</script>

{#snippet signature(label: string, who: Signature)}
  <dt>{label}</dt>
  <dd>
    <span class="name">{who.name}</span>
    <span class="email">&lt;{who.email}&gt;</span>
    <span class="when">{formatSignatureTime(who.time, who.offsetMinutes)}</span>
  </dd>
{/snippet}

{#snippet fileList(list: FileList)}
  {@const listId = `${key}/${list.key}`}
  <h2 class="files-heading">
    {LIST_TITLE[list.key]}
    {#if list.files}<span class="count">{list.files.length}</span>{/if}
  </h2>
  {#if list.files === undefined}
    <p class="note">Loading…</p>
  {:else if list.files.length === 0}
    <p class="note">No changes.</p>
  {:else}
    {@const shown = expanded.has(listId) ? list.files : list.files.slice(0, FILE_LIMIT)}
    <ul class="files">
      {#each shown as file}
        {@const request = inspector.diff?.request}
        {@const isOpen = request?.list === list.key && request.path === file.path}
        <li>
          <button
            class="file"
            class:open={isOpen}
            aria-current={isOpen ? "true" : undefined}
            title={file.oldPath === null ? file.path : `${file.oldPath} → ${file.path}`}
            onclick={() => onopen(list, file)}
          >
            <span class="status status-{file.status}">{STATUS_LETTER[file.status]}</span>
            <span class="path">
              {#if file.oldPath !== null}
                <span class="old-path">{file.oldPath}</span> <span class="arrow">→</span>
              {/if}
              {file.path}
            </span>
            <span class="stat">
              {#if file.additions === null || file.deletions === null}
                <span class="binary">binary</span>
              {:else}
                <span class="additions">+{file.additions}</span>
                <span class="deletions">−{file.deletions}</span>
              {/if}
            </span>
          </button>
        </li>
      {/each}
    </ul>
    {#if shown.length < list.files.length}
      <button class="button show-all" onclick={() => (expanded = new Set(expanded).add(listId))}>
        Show all {list.files.length} files
      </button>
    {/if}
  {/if}
{/snippet}

<aside bind:this={root} class="details" aria-label="Details" tabindex="-1">
  <header class="details-header">
    <span class="title">
      {#if subject.kind === "range"}
        Comparing <span class="mono">{shortId(subject.older)}</span> →
        <span class="mono">{shortId(subject.newer)}</span>
      {:else if subject.kind === "worktree"}
        Uncommitted changes
      {:else if subject.kind === "commit"}
        {selectedRow?.kind === "stash" ? "Stash" : "Commit"} <span class="mono">{shortId(subject.id)}</span>
      {:else}
        Loading…
      {/if}
    </span>
    <button class="icon-button" aria-label="Close details" title="Close details" onclick={onclose}
      >×</button
    >
  </header>

  <div class="details-body">
    {#if subject.kind === "range"}
      {@const older = comparison ? view.row(comparison.older) : undefined}
      {@const newer = comparison ? view.row(comparison.newer) : undefined}
      <p class="note">
        Changes from the older commit (A) to the newer one (B). Click a row or press Esc to stop
        comparing.
      </p>
      <dl class="meta">
        <dt>A</dt>
        <dd><span class="mono">{shortId(subject.older)}</span> {older?.summary ?? ""}</dd>
        <dt>B</dt>
        <dd><span class="mono">{shortId(subject.newer)}</span> {newer?.summary ?? ""}</dd>
      </dl>
      {#each inspector.lists as list (list.key)}
        {@render fileList(list)}
      {/each}
    {:else if subject.kind === "worktree"}
      <p class="note">
        Staged files are what the next commit would contain; unstaged changes exist only in the
        working tree.
      </p>
      {#each inspector.lists as list (list.key)}
        {@render fileList(list)}
      {/each}
    {:else if subject.kind === "commit"}
      {#if details === undefined}
        <p class="subject">{selectedRow?.summary ?? ""}</p>
        <p class="note">Loading details…</p>
      {:else if details === null}
        <p class="note">This row isn't a commit, so there are no details to show.</p>
      {:else}
        <p class="subject">{message.subject}</p>
        {#if message.body}
          <pre class="message">{message.body}</pre>
        {/if}

        <dl class="meta">
          {@render signature("Author", details.author)}
          {@render signature("Committer", details.committer)}
          <dt>Parents</dt>
          <dd class="mono">
            {#each details.parents as parent}
              {@const target = parentRows.get(parent)}
              {#if target === undefined || target === null}
                <span
                  class="parent"
                  class:unavailable={target === null}
                  title={target === null ? "Not in this view" : parent}>{shortId(parent)}</span
                >
              {:else}
                <button class="parent link" title="Go to {parent}" onclick={() => view.goTo(parent)}
                  >{shortId(parent)}</button
                >
              {/if}
            {:else}
              <span class="none">none</span>
            {/each}
          </dd>
          <dt>Commit</dt>
          <dd class="mono id">{details.id}</dd>
        </dl>

        {#each inspector.lists as list (list.key)}
          {@render fileList(list)}
        {/each}
      {/if}
    {:else}
      <p class="note">Loading…</p>
    {/if}
  </div>
</aside>

<style>
  .details {
    display: flex;
    flex: none;
    flex-direction: column;
    width: min(400px, 42%);
    min-height: 0;
    border-left: 1px solid var(--border);
  }

  .details-header {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: space-between;
    height: 26px;
    padding: 0 2px 0 12px;
    border-bottom: 1px solid var(--border);
    color: var(--fg-muted);
    font-size: 11px;
    font-weight: 600;
  }

  .title .mono {
    font-weight: 400;
  }

  .details-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 12px 14px 16px;
    user-select: text;
  }

  .mono {
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .note {
    margin: 0;
    color: var(--fg-muted);
  }

  .subject {
    margin: 0 0 8px;
    font-size: 14px;
    font-weight: 600;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .message {
    margin: 0 0 12px;
    font: inherit;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .meta {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 6px 12px;
    margin: 12px 0 0;
    padding: 10px 0 0;
    border-top: 1px solid var(--border);
    font-size: 12px;
  }

  dt {
    color: var(--fg-muted);
  }

  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .email,
  .none {
    color: var(--fg-muted);
  }

  .when {
    display: block;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }

  .parent + .parent {
    margin-left: 8px;
  }

  .parent.unavailable {
    color: var(--fg-muted);
    cursor: help;
  }

  .link {
    padding: 0;
    border: 0;
    border-radius: 2px;
    background: none;
    color: var(--focus);
    font: inherit;
    text-decoration: underline;
    text-decoration-color: color-mix(in srgb, currentColor 40%, transparent);
    text-underline-offset: 2px;
    cursor: pointer;
  }

  .link:hover {
    text-decoration-color: currentColor;
  }

  .id {
    color: var(--fg-muted);
  }

  .files-heading {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 16px 0 6px;
    color: var(--fg-muted);
    font-size: 11px;
    font-weight: 600;
  }

  .count {
    padding: 0 6px;
    border-radius: 8px;
    background: var(--button-hover);
    font-weight: 400;
    font-variant-numeric: tabular-nums;
  }

  .files {
    margin: 0 -6px;
    padding: 0;
    list-style: none;
    font-size: 12px;
  }

  .file {
    display: grid;
    grid-template-columns: 14px minmax(0, 1fr) auto;
    align-items: baseline;
    gap: 8px;
    width: 100%;
    padding: 2px 6px;
    border: 0;
    border-radius: 4px;
    background: transparent;
    text-align: left;
    cursor: pointer;
  }

  .file:hover {
    background: var(--bg-hover);
  }

  /* The keyboard cursor in the file list. */
  .file:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: -2px;
    background: var(--bg-hover);
  }

  .details:focus {
    outline: none;
  }

  .file.open {
    background: var(--bg-selected);
  }

  .status {
    font-family: var(--font-mono);
    font-weight: 600;
    text-align: center;
  }

  .status-added {
    color: var(--status-added);
  }
  .status-modified {
    color: var(--status-modified);
  }
  .status-deleted {
    color: var(--status-deleted);
  }
  .status-renamed {
    color: var(--status-renamed);
  }
  .status-copied {
    color: var(--status-copied);
  }
  .status-typeChanged {
    color: var(--status-type);
  }

  .path {
    overflow-wrap: anywhere;
  }

  .old-path,
  .arrow {
    color: var(--fg-muted);
  }

  .stat {
    font-family: var(--font-mono);
    font-size: 11px;
    white-space: nowrap;
  }

  .additions {
    color: var(--status-added);
  }

  .deletions {
    color: var(--status-deleted);
  }

  .binary {
    color: var(--fg-muted);
  }

  .show-all {
    margin-top: 8px;
  }
</style>
