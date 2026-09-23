<!--
  The filter picker: which branches (or other refs) to show commits of, and a path the commits must
  change. Edits a draft; nothing changes until Apply.
-->
<script lang="ts">
  import { untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import type { Filter, RefLabel } from "./bindings";
  import { REF_GROUPS, draftFilter, isFiltered, matchRefs } from "./filter";

  interface Props {
    /** The filter applied now; the draft starts from it. */
    current: Filter;
    /** Reads the refs to choose from. */
    loadRefs: () => Promise<RefLabel[]>;
    onapply: (filter: Filter) => void;
    onclose: () => void;
  }

  let { current, loadRefs, onapply, onclose }: Props = $props();

  // The draft starts from the filter applied when the picker opened, and doesn't follow it after.
  const checked = new SvelteSet<string>(untrack(() => current.refs));
  let path = $state(untrack(() => current.path ?? ""));
  let search = $state("");
  let refs = $state.raw<RefLabel[] | null>(null);
  let searchInput: HTMLInputElement;

  const listed = $derived(refs ? matchRefs(refs, search, checked) : { refs: [], total: 0 });
  const groups = $derived(
    REF_GROUPS.map((group) => ({ ...group, refs: listed.refs.filter((ref) => ref.kind === group.kind) })).filter(
      (group) => group.refs.length > 0,
    ),
  );

  $effect(() => {
    searchInput.focus();
    void loadRefs().then((all) => (refs = all));
  });

  function toggle(fullName: string, on: boolean): void {
    if (on) checked.add(fullName);
    else checked.delete(fullName);
  }

  function apply(event: SubmitEvent): void {
    event.preventDefault();
    onapply(draftFilter(checked, path));
  }

  // Esc is for this panel while it has focus; the app's key router leaves text fields alone.
  function onkeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape") return;
    event.preventDefault();
    onclose();
  }
</script>

<!-- Esc from any control inside closes the panel (see `onkeydown`). -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<form class="filter-panel" aria-label="Filter commits" onsubmit={apply} {onkeydown}>
  <header class="filter-header">
    <h2>Filter commits</h2>
    <button type="button" class="icon-button" aria-label="Close filter" title="Close (Esc)" onclick={onclose}
      >×</button
    >
  </header>

  <label class="field">
    <span class="field-label">Branches</span>
    <input
      bind:this={searchInput}
      bind:value={search}
      class="text"
      type="text"
      placeholder="Type to narrow the list"
      spellcheck="false"
      autocomplete="off"
    />
  </label>
  <div class="refs" role="group" aria-label="Refs to show commits of">
    {#if refs === null}
      <p class="note">Loading…</p>
    {:else if groups.length === 0}
      <p class="note">No matching refs.</p>
    {:else}
      {#each groups as group (group.kind)}
        <h3>{group.title}</h3>
        {#each group.refs as ref (ref.fullName)}
          <label class="ref" title={ref.fullName}>
            <input
              type="checkbox"
              checked={checked.has(ref.fullName)}
              onchange={(event) => toggle(ref.fullName, event.currentTarget.checked)}
            />
            <span class="ref-name">{ref.name}</span>
          </label>
        {/each}
      {/each}
      {#if listed.total > listed.refs.length}
        <p class="note">
          {(listed.total - listed.refs.length).toLocaleString()} more; type to narrow the list.
        </p>
      {/if}
    {/if}
  </div>
  <p class="hint">
    {checked.size === 0 ? "No branch chosen: commits of every branch." : `Commits reachable from ${checked.size} chosen.`}
  </p>

  <label class="field">
    <span class="field-label">Only commits changing</span>
    <input
      bind:value={path}
      class="text"
      type="text"
      placeholder="A file or folder, e.g. src/lib"
      spellcheck="false"
      autocomplete="off"
    />
  </label>

  <footer class="actions">
    <button
      type="button"
      class="button"
      disabled={!isFiltered(current) && checked.size === 0 && path.trim() === ""}
      onclick={() => onapply({ refs: [], path: null })}>Show all</button
    >
    <button type="submit" class="button primary">Apply</button>
  </footer>
</form>

<style>
  .filter-panel {
    position: fixed;
    top: 76px;
    right: 10px;
    z-index: 15;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: min(340px, calc(100vw - 20px));
    max-height: calc(100vh - 60px);
    margin: 0;
    padding: 0 12px 12px;
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    background: var(--bg);
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.25);
  }

  .filter-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 0 -4px 0 0;
    padding-top: 8px;
  }

  h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .field-label {
    color: var(--fg-muted);
    font-size: 11px;
    font-weight: 600;
  }

  .text {
    height: 26px;
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-subtle);
    color: var(--fg);
    font: inherit;
  }

  .text:focus {
    border-color: var(--focus);
    outline: none;
  }

  .refs {
    min-height: 80px;
    max-height: 280px;
    overflow: auto;
    padding: 2px 6px 6px;
    border: 1px solid var(--border);
    border-radius: 4px;
  }

  h3 {
    margin: 6px 0 2px;
    color: var(--fg-subtle);
    font-size: 11px;
    font-weight: 600;
  }

  .ref {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 1px 0;
    user-select: none;
  }

  .ref input {
    margin: 0;
    accent-color: var(--primary-bg);
  }

  .ref-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .note,
  .hint {
    margin: 4px 0;
    color: var(--fg-muted);
    font-size: 12px;
  }

  .hint {
    margin: -2px 0 0;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
