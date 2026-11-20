import { render } from "solid-js/web";
// Bundled fonts: the app works offline.
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
// The design system is the single source: design/ at the repo root.
import "../../../design/tokens.css";
import "../../../design/base.css";
import "../../../design/components.css";
import "./app.css";
import App from "./App";

render(() => <App />, document.getElementById("root")!);
