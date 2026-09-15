/** Dev-only startup hooks, read from the environment Vite runs in (never set in release builds). */
interface ImportMetaEnv {
  /** Path of a repository to open on startup. */
  readonly VITE_FERGIT_OPEN?: string;
  /** Row index to select once that repository is open. */
  readonly VITE_FERGIT_SELECT?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
