import { render } from "solid-js/web";
// Bundled fonts: the app works offline.
import "@fontsource/archivo/400.css";
import "@fontsource/archivo/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/600.css";
// The design system is the single source: design/ at the repo root.
import "../../../design/tokens.css";
import "../../../design/base.css";
import "../../../design/components.css";
import "./app.css";
import App from "./App";

render(() => <App />, document.getElementById("root")!);
