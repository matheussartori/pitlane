// Monaco, loaded on first use (it is large), with its language workers bundled for offline use and
// themes drawn from the design tokens.
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import JsonWorker from "monaco-editor/language/json/json.worker?worker";
import CssWorker from "monaco-editor/language/css/css.worker?worker";
import HtmlWorker from "monaco-editor/language/html/html.worker?worker";
import TsWorker from "monaco-editor/language/typescript/ts.worker?worker";

export type Monaco = typeof import("monaco-editor");

(self as unknown as { MonacoEnvironment: unknown }).MonacoEnvironment = {
  getWorker(_: string, label: string) {
    if (label === "json") return new JsonWorker();
    if (label === "css" || label === "scss" || label === "less") return new CssWorker();
    if (label === "html" || label === "handlebars" || label === "razor") return new HtmlWorker();
    if (label === "typescript" || label === "javascript") return new TsWorker();
    return new EditorWorker();
  },
};

/** A CSS colour (oklch included) as #rrggbb. */
export function hex(css: string): string {
  const c = document.createElement("canvas");
  c.width = c.height = 1;
  const ctx = c.getContext("2d");
  if (!ctx) return "#000000";
  ctx.fillStyle = "#000";
  ctx.fillStyle = css;
  ctx.fillRect(0, 0, 1, 1);
  const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
  return `#${[r, g, b].map((v) => v!.toString(16).padStart(2, "0")).join("")}`;
}

let loaded: Promise<Monaco> | null = null;

export function loadMonaco(): Promise<Monaco> {
  loaded ??= import("monaco-editor").then((m) => {
    // The editor sees one file at a time, not the project: type errors from missing imports would
    // be noise. Syntax errors stay.
    for (const d of [m.typescript.typescriptDefaults, m.typescript.javascriptDefaults]) {
      d.setDiagnosticsOptions({ noSemanticValidation: true, noSyntaxValidation: false });
      d.setCompilerOptions({ ...d.getCompilerOptions(), jsx: m.typescript.JsxEmit.Preserve, allowNonTsExtensions: true, target: m.typescript.ScriptTarget.ESNext });
    }
    defineThemes(m);
    window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
      defineThemes(m);
      m.editor.setTheme(themeName());
    });
    return m;
  });
  return loaded;
}

export const themeName = () => (getComputedStyle(document.documentElement).getPropertyValue("color-scheme").includes("dark") ? "pitlane-dark" : "pitlane-light");

function defineThemes(m: Monaco) {
  const cs = getComputedStyle(document.documentElement);
  const v = (n: string) => hex(cs.getPropertyValue(n).trim());
  const dark = themeName() === "pitlane-dark";
  const strip = (h: string) => h.slice(1);
  m.editor.defineTheme(themeName(), {
    base: dark ? "vs-dark" : "vs",
    inherit: true,
    rules: [
      { token: "comment", foreground: strip(v("--code-comment")), fontStyle: "italic" },
      { token: "keyword", foreground: strip(v("--code-keyword")) },
      { token: "string", foreground: strip(v("--code-string")) },
      { token: "number", foreground: strip(v("--code-number")) },
      { token: "type", foreground: strip(v("--code-fn")) },
    ],
    colors: {
      "editor.background": v("--color-paper-sunk"),
      "editor.foreground": v("--color-ink"),
      "editorLineNumber.foreground": v("--color-muted"),
      "editorLineNumber.activeForeground": v("--color-ink-2"),
      "editor.lineHighlightBackground": v("--color-paper"),
      "editor.lineHighlightBorder": v("--color-paper"),
      "editorCursor.foreground": v("--color-accent"),
      "editor.selectionBackground": dark ? "#2c3340" : "#d6dbe4",
      "editorIndentGuide.background1": v("--color-rule"),
      "editorWidget.background": v("--color-paper-2"),
      "editorWidget.border": v("--color-rule-strong"),
      "diffEditor.insertedLineBackground": v("--color-add"),
      "diffEditor.removedLineBackground": v("--color-del"),
      "diffEditor.insertedTextBackground": v("--color-add-strong"),
      "diffEditor.removedTextBackground": v("--color-del-strong"),
      "scrollbarSlider.background": dark ? "#ffffff18" : "#00000018",
    },
  });
}
