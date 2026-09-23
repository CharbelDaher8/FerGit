<!--
  A merge, rebase, cherry-pick or revert that stopped: what stopped, which files conflict, and the
  ways on. The files are resolved in the user's own editor; the list follows as they are saved,
  because the backend reports every change it sees.
-->
<script lang="ts">
  import type { RepoState } from "./bindings";
  import { summarize } from "./repoState";

  interface Props {
    repoState: RepoState;
    /** An operation is running: the buttons wait for it. */
    busy: boolean;
    onaction: (kind: "continue" | "abort" | "skip") => void;
  }

  let { repoState, busy, onaction }: Props = $props();

  const summary = $derived(summarize(repoState));
  const conflicts = $derived(repoState.kind === "clean" ? [] : repoState.conflicts);
</script>

{#if summary}
  <div class="banner state" role="status" aria-live="polite">
    <div class="head">
      <div class="text">
        <strong>{summary.title}.</strong>
        <span>{summary.hint}</span>
      </div>
      <div class="actions">
        <button
          class="button primary"
          disabled={busy || summary.unresolved > 0}
          title={summary.unresolved > 0 ? "Resolve every conflict first" : "Stage the files and carry on"}
          onclick={() => onaction("continue")}>Continue</button
        >
        {#if summary.canSkip}
          <button class="button" disabled={busy} title="Leave this commit out and carry on" onclick={() => onaction("skip")}
            >Skip commit</button
          >
        {/if}
        <button class="button" disabled={busy} title="Put everything back as it was before" onclick={() => onaction("abort")}
          >Abort</button
        >
      </div>
    </div>
    {#if conflicts.length > 0}
      <ul class="files">
        {#each conflicts as file (file.path)}
          <li class:resolved={file.resolved} title={file.resolved ? "No conflict markers left" : "Has conflict markers"}>
            <span class="mark" aria-hidden="true">{file.resolved ? "✓" : "●"}</span>
            <span class="path">{file.path}</span>
            <span class="visually-hidden">{file.resolved ? "resolved" : "has conflicts"}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}

<style>
  .state {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: 6px;
    padding: 6px 8px 6px 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--compare) 45%, var(--bg));
    background: color-mix(in srgb, var(--compare) 12%, var(--bg));
  }

  .head {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .text {
    display: flex;
    flex: 1;
    flex-wrap: wrap;
    gap: 0 6px;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    flex: none;
    gap: 6px;
  }

  .files {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 14px;
    max-height: 5.5em;
    margin: 0;
    overflow: auto;
    padding: 0;
    list-style: none;
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .files li {
    display: flex;
    gap: 5px;
    min-width: 0;
  }

  .mark {
    color: var(--status-deleted);
  }

  .resolved .mark {
    color: var(--status-added);
  }

  .resolved .path {
    color: var(--fg-muted);
  }

  .path {
    overflow-wrap: anywhere;
  }

  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
</style>
