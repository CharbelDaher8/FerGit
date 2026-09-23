<!--
  What the running operations are doing, and why the last one failed: git's message in words, with
  git's own output a click away.
-->
<script lang="ts">
  import type { Operations } from "./operations.svelte";

  interface Props {
    operations: Operations;
  }

  let { operations }: Props = $props();

  let showOutput = $state(false);
  const failure = $derived(operations.failure);

  // A new failure starts with the output folded away.
  $effect(() => {
    void failure;
    showOutput = false;
  });
</script>

{#each operations.active as op (op.id)}
  <div class="status" role="status" aria-live="polite">
    <span class="spinner" aria-hidden="true"></span>
    <span class="title">{op.title}…</span>
    {#if op.progress}
      <span class="progress">{op.progress}</span>
    {/if}
  </div>
{/each}

{#if failure}
  <div class="failure" role="alert">
    <div class="failure-head">
      <div class="failure-text">
        <strong>{failure.title} failed.</strong>
        <span>{failure.error.message}</span>
      </div>
      {#if failure.error.output}
        <button class="button" onclick={() => (showOutput = !showOutput)}>
          {showOutput ? "Hide git output" : "Show git output"}
        </button>
      {/if}
      <button
        class="icon-button"
        aria-label="Dismiss"
        title="Dismiss"
        onclick={() => operations.dismissFailure()}>×</button
      >
    </div>
    {#if showOutput}
      <pre class="output">{failure.error.output}</pre>
    {/if}
  </div>
{/if}

<style>
  .status {
    display: flex;
    flex: none;
    align-items: center;
    gap: 8px;
    min-width: 0;
    padding: 4px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-subtle);
    font-size: 12px;
  }

  .title {
    flex: none;
    font-weight: 600;
  }

  .progress {
    min-width: 0;
    overflow: hidden;
    color: var(--fg-muted);
    font-family: var(--font-mono);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .spinner {
    flex: none;
    width: 10px;
    height: 10px;
    border: 2px solid var(--border-strong);
    border-top-color: var(--primary-bg);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .failure {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: 6px;
    padding: 6px 6px 6px 12px;
    border-bottom: 1px solid var(--danger-border);
    background: var(--danger-bg);
    color: var(--danger-fg);
  }

  .failure-head {
    display: flex;
    align-items: flex-start;
    gap: 8px;
  }

  .failure-text {
    display: flex;
    flex: 1;
    flex-wrap: wrap;
    gap: 0 6px;
    padding-top: 3px;
    overflow-wrap: anywhere;
  }

  .failure .icon-button {
    color: inherit;
  }

  .output {
    max-height: 12em;
    margin: 0 6px 0 0;
    overflow: auto;
    padding: 6px 8px;
    border-radius: 4px;
    background: var(--bg);
    color: var(--fg);
    font-family: var(--font-mono);
    font-size: 11px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }
</style>
