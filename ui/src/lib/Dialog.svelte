<!--
  The current modal dialog from `dialogs`. Enter confirms (from a single-line field or a checkbox),
  Escape cancels. Keys pressed inside never reach the app's key router.
-->
<script lang="ts">
  import { tick, untrack } from "svelte";
  import { canConfirm, initialValues, type DialogSpec, type DialogValues } from "./dialogs.svelte";

  interface Props {
    spec: DialogSpec;
    onclose: (values: DialogValues | null) => void;
  }

  let { spec, onclose }: Props = $props();

  // Starts from the values the dialog was opened with; a new spec means a new component.
  let values = $state(untrack(() => initialValues(spec)));
  const ready = $derived(canConfirm(spec, values));
  let form: HTMLFormElement;

  $effect(() => {
    void tick().then(() => form.querySelector<HTMLElement>("input, textarea, button.confirm")?.focus());
  });

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    if (ready) onclose($state.snapshot(values));
  }

  function onkeydown(event: KeyboardEvent): void {
    event.stopPropagation();
    if (event.key === "Escape") {
      event.preventDefault();
      onclose(null);
    } else if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && ready) {
      // Ctrl+Enter confirms from a multi-line field too.
      event.preventDefault();
      onclose($state.snapshot(values));
    }
  }
</script>

<div class="backdrop">
  <div class="dialog" role="dialog" aria-modal="true" aria-label={spec.title} tabindex="-1" {onkeydown}>
    <form bind:this={form} onsubmit={submit}>
      <h2>{spec.title}</h2>
      {#if spec.message}
        <p class="message">{spec.message}</p>
      {/if}
      {#each spec.fields as field (field.key)}
        {#if field.kind === "checkbox"}
          <label class="checkbox">
            <input type="checkbox" bind:checked={values[field.key] as boolean} />
            {field.label}
          </label>
        {:else if field.kind === "choice"}
          <fieldset class="choice">
            <legend>{field.label}</legend>
            {#each field.options as option (option.value)}
              <label class="option">
                <input type="radio" name={field.key} value={option.value} bind:group={values[field.key] as string} />
                <span>
                  <span class="option-label">{option.label}</span>
                  {#if option.hint}
                    <span class="option-hint">{option.hint}</span>
                  {/if}
                </span>
              </label>
            {/each}
          </fieldset>
        {:else}
          <label class="text">
            <span>{field.label}</span>
            {#if field.multiline}
              <textarea rows="3" bind:value={values[field.key] as string} placeholder={field.placeholder}></textarea>
            {:else}
              <input
                type={field.secret ? "password" : "text"}
                autocomplete="off"
                spellcheck="false"
                bind:value={values[field.key] as string}
                placeholder={field.placeholder}
              />
            {/if}
          </label>
        {/if}
      {/each}
      <div class="buttons">
        {#if spec.cancel !== null}
          <button type="button" class="button" onclick={() => onclose(null)}>{spec.cancel ?? "Cancel"}</button>
        {/if}
        <button type="submit" class="button primary confirm" class:danger={spec.danger} disabled={!ready}>
          {spec.confirm}
        </button>
      </div>
    </form>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 40;
    display: grid;
    place-items: center;
    padding: 24px;
    background: color-mix(in srgb, var(--bg) 40%, transparent);
  }

  .dialog {
    width: min(440px, 100%);
    max-height: 100%;
    overflow: auto;
    padding: 16px;
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    background: var(--bg);
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.25);
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }

  .message {
    margin: 0;
    color: var(--fg-muted);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .text {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--fg-muted);
  }

  input[type="text"],
  input[type="password"],
  textarea {
    padding: 5px 8px;
    border: 1px solid var(--border-strong);
    border-radius: 5px;
    background: var(--bg);
    color: var(--fg);
    font: 13px var(--font-ui);
    resize: vertical;
  }

  .checkbox {
    display: flex;
    align-items: center;
    gap: 6px;
    user-select: none;
  }

  .checkbox input {
    margin: 0;
    accent-color: var(--primary-bg);
  }

  .choice {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding: 0;
    border: 0;
  }

  legend {
    margin-bottom: 4px;
    padding: 0;
    color: var(--fg-muted);
    font-size: 12px;
  }

  .option {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    user-select: none;
  }

  .option input {
    margin: 2px 0 0;
    accent-color: var(--primary-bg);
  }

  .option-label {
    font-weight: 600;
  }

  .option-hint {
    color: var(--fg-muted);
  }

  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }

  .button.primary.danger {
    background: var(--ref-head);
  }
</style>
