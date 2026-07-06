import App from "./App.svelte";
import "./styles.css";

// CSS keys translucent (vibrancy-backed) surfaces off this attribute — only
// macOS gets native blur behind the transparent window; elsewhere the panel
// must stay opaque or the bare desktop would show through.
const ua = navigator.userAgent;
document.documentElement.dataset.platform = ua.includes("Mac")
  ? "macos"
  : ua.includes("Win")
    ? "windows"
    : "linux";

const app = new App({
  target: document.getElementById("app")!,
});

export default app;
