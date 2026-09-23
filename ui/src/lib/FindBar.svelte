<!--
  The find bar, floating over the top of the commit list. Typing searches; Enter, Shift+Enter and
  Esc reach the app's key router like every other key (its `find` context), so this component only
  shows the finder's state and forwards clicks.
-->
<script lang="ts">
  import type { Finder } from "./find.svelte";

  interface Props {
    finder: Finder;
    onclose: () => void;
  }

  let { finder, onclose }: Props = $props();

  let input: HTMLInputElement;

  /** Focuses the text field with its text selected, so typing replaces the last query. */
  export function focus(): void {
    input.focus();
    input.select();
  }
</script>

<div class="find-bar" role="search">
  <input
    bind:this={input}
    class="find-input"
    type="text"
    placeholder="Find by message, author or id"
    aria-label="Find commits"
    spellcheck="false"
    autocomplete="off"
    value={finder.query}
    oninput={(event) => finder.setQuery(event.currentTarget.value)}
  />
  <span
    class="find-status"
    class:none={finder.status === "No matches"}
    aria-live="polite"
    title={finder.total > finder.listed
      ? `Only the first ${finder.listed.toLocaleString()} matches can be stepped through`
      : undefined}>{finder.status}</span
  >
  <button
    class="icon-button"
    aria-label="Previous match"
    title="Previous match (Shift+Enter, N)"
    disabled={finder.listed === 0}
    onclick={() => finder.step(-1)}>↑</button
  >
  <button
    class="icon-button"
    aria-label="Next match"
    title="Next match (Enter, n)"
    disabled={finder.listed === 0}
    onclick={() => finder.step(1)}>↓</button
  >
  <button class="icon-button" aria-label="Close find" title="Close (Esc)" onclick={onclose}>×</button>
</div>

<style>
  .find-bar {
    position: absolute;
    top: 32px;
    right: 18px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 4px 4px 6px;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    background: var(--bg);
    box-shadow: 0 4px 16px rgb(0 0 0 / 0.15);
  }

  .find-input {
    width: 240px;
    height: 24px;
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-subtle);
    color: var(--fg);
    font: inherit;
  }

  .find-input:focus {
    border-color: var(--focus);
    outline: none;
  }

  .find-status {
    min-width: 72px;
    color: var(--fg-muted);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    text-align: center;
    white-space: nowrap;
  }

  .find-status.none {
    color: var(--danger-fg);
  }

  .icon-button:disabled {
    opacity: 0.4;
  }
</style>
