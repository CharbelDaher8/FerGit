import { mount } from "svelte";
import App from "./App.svelte";
import "./app.css";
import { session } from "./lib/session.svelte";

// The one place errors are handled. Commands reject with `AppError`, and nothing on the way up
// catches them: every unhandled rejection or uncaught error ends up in the banner.
window.addEventListener("unhandledrejection", (event) => {
  event.preventDefault();
  session.reportError(event.reason);
});
window.addEventListener("error", (event) => {
  session.reportError(event.error ?? event.message);
});

const target = document.getElementById("app");
if (!target) throw new Error("FerGit: #app element missing from index.html");

export default mount(App, { target });
