<!-- The keyboard shortcuts, as an overlay. `?` and Esc close it through the app's key router. -->
<script lang="ts">
  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  interface Group {
    title: string;
    /** Each entry: the keys (alternatives separated by two spaces) and what they do. */
    keys: [string, string][];
  }

  const GROUPS: Group[] = [
    {
      title: "Commit graph",
      keys: [
        ["j  k", "Next / previous commit (↓ ↑ too)"],
        ["5j", "Move 5 rows (any count)"],
        ["gg  G", "First / last commit"],
        ["20G", "Row 20"],
        ["Ctrl+d  Ctrl+u", "Half a page down / up"],
        ["Ctrl+f  Ctrl+b", "A page down / up (PgDn PgUp too)"],
        ["l  Enter", "Into the file list"],
        ["Ctrl+click", "Compare with the selected commit"],
      ],
    },
    {
      title: "File list",
      keys: [
        ["j  k", "Next / previous file"],
        ["gg  G", "First / last file"],
        ["l  Enter", "Open the file's diff"],
        ["h", "Back to the graph"],
      ],
    },
    {
      title: "Diff",
      keys: [
        ["j  k", "Scroll a line"],
        ["Ctrl+d  Ctrl+u", "Scroll half a page"],
        ["gg  G", "Top / bottom"],
        ["h  l", "Scroll sideways"],
        ["0  $", "Start / end of the longest line"],
        ["q  Esc", "Close the diff"],
      ],
    },
    {
      title: "Anywhere",
      keys: [
        ["?", "Show or hide these shortcuts"],
        ["Esc", "Cancel a half-typed count or g, else close the diff, else stop comparing"],
      ],
    },
  ];
</script>

<div class="backdrop">
  <div class="help" role="dialog" aria-label="Keyboard shortcuts">
    <header class="help-header">
      <h2>Keyboard shortcuts</h2>
      <button class="icon-button" aria-label="Close keyboard shortcuts" title="Close (Esc)" onclick={onclose}
        >×</button
      >
    </header>
    <div class="groups">
      {#each GROUPS as group}
        <div class="group">
          <h3>{group.title}</h3>
          <dl>
            {#each group.keys as [keys, action]}
              <dt>
                {#each keys.split("  ") as key}
                  <kbd>{key}</kbd>
                {/each}
              </dt>
              <dd>{action}</dd>
            {/each}
          </dl>
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: grid;
    place-items: center;
    padding: 24px;
    background: color-mix(in srgb, var(--bg) 40%, transparent);
  }

  .help {
    width: min(760px, 100%);
    max-height: 100%;
    overflow: auto;
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    background: var(--bg);
    box-shadow: 0 12px 40px var(--shadow);
  }

  .help-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 10px 10px 16px;
    border-bottom: 1px solid var(--border);
  }

  h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }

  .groups {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
    gap: 4px 24px;
    padding: 12px 16px 16px;
  }

  h3 {
    margin: 8px 0 6px;
    color: var(--fg-muted);
    font-size: 11px;
    font-weight: 600;
  }

  dl {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 4px 12px;
    margin: 0;
    font-size: 12px;
  }

  dt {
    display: flex;
    gap: 4px;
    white-space: nowrap;
  }

  dd {
    margin: 0;
    color: var(--fg-muted);
  }

  kbd {
    padding: 0 5px;
    border: 1px solid var(--border-strong);
    border-bottom-width: 2px;
    border-radius: 4px;
    background: var(--bg-subtle);
    color: var(--fg);
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 16px;
  }
</style>
