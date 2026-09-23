<!--
  A context menu at a point on screen. Arrow keys move between entries, Enter picks one, Escape,
  a click elsewhere or losing focus closes it. Keys pressed while it is open never reach the app's
  key router.
-->
<script lang="ts">
  import { tick } from "svelte";
  import type { MenuAction, MenuEntry } from "./menu";

  interface Props {
    x: number;
    y: number;
    entries: MenuEntry[];
    onpick: (action: MenuAction) => void;
    onclose: () => void;
  }

  let { x, y, entries, onpick, onclose }: Props = $props();

  let menu: HTMLDivElement;
  let left = $state(0);
  let top = $state(0);

  // Open where asked, moved back inside the window if it would overflow.
  $effect(() => {
    const rect = menu.getBoundingClientRect();
    left = Math.max(4, Math.min(x, window.innerWidth - rect.width - 4));
    top = Math.max(4, Math.min(y, window.innerHeight - rect.height - 4));
    void tick().then(() => items()[0]?.focus());
  });

  function items(): HTMLButtonElement[] {
    return Array.from(menu.querySelectorAll<HTMLButtonElement>("button[role=menuitem]"));
  }

  function onkeydown(event: KeyboardEvent): void {
    event.stopPropagation();
    const list = items();
    const index = list.indexOf(document.activeElement as HTMLButtonElement);
    const move = (to: number) => list[(to + list.length) % list.length]?.focus();
    switch (event.key) {
      case "Escape":
        onclose();
        break;
      case "ArrowDown":
      case "j":
        move(index + 1);
        break;
      case "ArrowUp":
      case "k":
        move(index < 0 ? -1 : index - 1);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(-1);
        break;
      case "Tab":
        break;
      default:
        // Enter and Space activate the focused button natively; other keys do nothing here.
        return;
    }
    event.preventDefault();
  }

  function onpointerdown(event: PointerEvent): void {
    if (!menu.contains(event.target as Node)) onclose();
  }
</script>

<svelte:window {onpointerdown} onblur={onclose} onresize={onclose} />

<div
  bind:this={menu}
  class="menu"
  role="menu"
  tabindex="-1"
  style:left="{left}px"
  style:top="{top}px"
  {onkeydown}
  oncontextmenu={(event) => event.preventDefault()}
>
  {#each entries as entry}
    {#if "separator" in entry}
      <div class="separator" role="separator"></div>
    {:else}
      <button
        class="item"
        class:danger={entry.danger}
        role="menuitem"
        onclick={() => {
          onclose();
          onpick(entry.action);
        }}>{entry.label}</button
      >
    {/if}
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 30;
    min-width: 200px;
    max-width: 420px;
    padding: 4px;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    background: var(--bg);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.2);
    outline: none;
  }

  .item {
    display: block;
    width: 100%;
    overflow: hidden;
    padding: 4px 10px;
    border: 0;
    border-radius: 4px;
    background: transparent;
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .item:hover,
  .item:focus {
    background: var(--bg-selected);
    outline: none;
  }

  .item.danger {
    color: var(--danger-fg);
  }

  .separator {
    height: 1px;
    margin: 4px 6px;
    background: var(--border);
  }
</style>
