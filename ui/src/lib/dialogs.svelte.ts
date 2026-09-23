/** A modal question: a title, some fields to fill in, and a button that confirms. */
export interface DialogSpec {
  title: string;
  /** Plain text shown above the fields. */
  message?: string;
  fields: Field[];
  /** Label of the confirming button. */
  confirm: string;
  /** Confirming destroys or replaces something: the button is styled as a warning. */
  danger?: boolean;
}

export type Field =
  | {
      kind: "text";
      key: string;
      label: string;
      value?: string;
      placeholder?: string;
      /** Confirming is disabled while this is blank. */
      required?: boolean;
      /** Mask the input (a password or token). */
      secret?: boolean;
      multiline?: boolean;
    }
  | { kind: "checkbox"; key: string; label: string; value?: boolean };

/** What was entered, by field key: strings for text fields, booleans for checkboxes. */
export type DialogValues = Record<string, string | boolean>;

interface Pending {
  spec: DialogSpec;
  resolve: (values: DialogValues | null) => void;
}

/**
 * The app's modal dialogs, one at a time. `ask` queues a dialog and resolves when the user
 * confirms (with the values) or cancels (with `null`). A credential prompt from git can arrive
 * while another dialog is open; it waits its turn.
 */
export class Dialogs {
  /** The dialog on screen, if any. */
  current = $state.raw<Pending | null>(null);
  #queue: Pending[] = [];

  ask(spec: DialogSpec): Promise<DialogValues | null> {
    return new Promise((resolve) => {
      this.#queue.push({ spec, resolve });
      if (!this.current) this.#next();
    });
  }

  /** Closes the current dialog, confirming with `values` or cancelling with `null`. */
  close(values: DialogValues | null): void {
    const current = this.current;
    if (!current) return;
    this.current = null;
    current.resolve(values);
    this.#next();
  }

  #next(): void {
    this.current = this.#queue.shift() ?? null;
  }
}

export const dialogs = new Dialogs();

/** Initial values of `spec`'s fields. */
export function initialValues(spec: DialogSpec): DialogValues {
  const values: DialogValues = {};
  for (const field of spec.fields) {
    values[field.key] = field.kind === "text" ? (field.value ?? "") : (field.value ?? false);
  }
  return values;
}

/** Whether `values` may be confirmed: every required text field has non-blank text. */
export function canConfirm(spec: DialogSpec, values: DialogValues): boolean {
  return spec.fields.every(
    (field) => field.kind !== "text" || !field.required || String(values[field.key] ?? "").trim() !== "",
  );
}
