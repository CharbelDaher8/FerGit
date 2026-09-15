<!-- Details of the selected row: message, signatures, parents and changed files. -->
<script lang="ts">
  import { untrack } from "svelte";
  import { commands, type ChangeStatus, type CommitDetails, type Row, type Signature } from "./bindings";
  import { formatSignatureTime, shortId } from "./format";

  interface Props {
    /** The selected row; `undefined` while its page is loading. */
    row: Row | undefined;
    onclose: () => void;
  }

  let { row, onclose }: Props = $props();

  /** Files rendered before asking; a huge commit would otherwise freeze the panel. */
  const FILE_LIMIT = 500;
  /** Holding an arrow key moves the selection faster than details need to load. */
  const LOAD_DELAY_MS = 60;

  const STATUS_LETTER: Record<ChangeStatus, string> = {
    added: "A",
    modified: "M",
    deleted: "D",
    renamed: "R",
    copied: "C",
    typeChanged: "T",
  };

  /** Details of the last commit loaded; `details` is `null` if the id wasn't a commit. */
  let loaded = $state.raw<{ id: string; details: CommitDetails | null } | null>(null);
  let showAllFiles = $state(false);

  const wantedId = $derived(row && row.kind !== "workingTree" ? row.id : null);
  /** `undefined` while loading. */
  const details = $derived(loaded && loaded.id === wantedId ? loaded.details : undefined);
  const subject = $derived(details ? details.message.split("\n", 1)[0] : "");
  const body = $derived(details ? details.message.slice(subject.length).replace(/^\s*\n/, "").trimEnd() : "");
  const files = $derived(
    details && !showAllFiles ? details.files.slice(0, FILE_LIMIT) : (details?.files ?? []),
  );

  $effect(() => {
    const id = wantedId;
    if (id === null || untrack(() => loaded?.id) === id) return;
    const timer = setTimeout(() => {
      void commands.commitDetails(id).then((result) => {
        // The selection may have moved on while this was loading.
        if (wantedId !== id) return;
        loaded = { id, details: result };
        showAllFiles = false;
      });
    }, LOAD_DELAY_MS);
    return () => clearTimeout(timer);
  });
</script>

{#snippet signature(label: string, who: Signature)}
  <dt>{label}</dt>
  <dd>
    <span class="name">{who.name}</span>
    <span class="email">&lt;{who.email}&gt;</span>
    <span class="when">{formatSignatureTime(who.time, who.offsetMinutes)}</span>
  </dd>
{/snippet}

<aside class="details" aria-label="Commit details">
  <header class="details-header">
    <span class="title">
      {#if row?.kind === "workingTree"}
        Uncommitted changes
      {:else if row}
        {row.kind === "stash" ? "Stash" : "Commit"} <span class="mono">{shortId(row.id)}</span>
      {:else}
        Loading…
      {/if}
    </span>
    <button class="icon-button" aria-label="Close details" title="Close details" onclick={onclose}
      >×</button
    >
  </header>

  <div class="details-body">
    {#if !row}
      <p class="note">Loading…</p>
    {:else if row.kind === "workingTree"}
      <p class="note">
        This row stands for changes in the working tree and index that aren't committed yet.
        Viewing them isn't supported yet.
      </p>
    {:else if details === undefined}
      <p class="subject">{row.summary}</p>
      <p class="note">Loading details…</p>
    {:else if details === null}
      <p class="note">This row isn't a commit, so there are no details to show.</p>
    {:else}
      <p class="subject">{subject}</p>
      {#if body}
        <pre class="message">{body}</pre>
      {/if}

      <dl class="meta">
        {@render signature("Author", details.author)}
        {@render signature("Committer", details.committer)}
        <dt>Parents</dt>
        <dd class="mono">
          {#each details.parents as parent}
            <span class="parent" title={parent}>{shortId(parent)}</span>
          {:else}
            <span class="none">none</span>
          {/each}
        </dd>
        <dt>Commit</dt>
        <dd class="mono id">{details.id}</dd>
      </dl>

      <h2 class="files-heading">
        {details.files.length === 1 ? "1 file changed" : `${details.files.length} files changed`}
      </h2>
      <ul class="files">
        {#each files as file}
          <li class="file">
            <span class="status status-{file.status}" title={file.status}>{STATUS_LETTER[file.status]}</span>
            <span class="path" title={file.oldPath === null ? file.path : `${file.oldPath} → ${file.path}`}>
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
          </li>
        {/each}
      </ul>
      {#if files.length < details.files.length}
        <button class="button show-all" onclick={() => (showAllFiles = true)}>
          Show all {details.files.length} files
        </button>
      {/if}
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
    margin: 12px 0 16px;
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

  .id {
    color: var(--fg-muted);
  }

  .files-heading {
    margin: 0 0 6px;
    color: var(--fg-muted);
    font-size: 11px;
    font-weight: 600;
  }

  .files {
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: 12px;
  }

  .file {
    display: grid;
    grid-template-columns: 14px minmax(0, 1fr) auto;
    align-items: baseline;
    gap: 8px;
    padding: 2px 0;
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
