// Language servers in Monaco: Monaco's own LSP client (monaco.lsp), spoken to through Tauri.
// The transport is where Pitlane fits that alpha client to real servers: it names the workspace
// in `initialize`, keeps each server to its own folder and language (the client offers every
// model to every server), restores the case of file URIs (the client lowercases them), gives TSX
// its language id, answers the few server requests the client does not, and creates models for
// files a definition or reference lands in, so peek can show them.
import { createSignal } from "solid-js";
import type * as M from "monaco-editor";
import { files, lspApi, onLsp, type LspStart } from "./api";

type Monaco = typeof import("monaco-editor");
type Msg = { jsonrpc: "2.0"; id?: number | string; method?: string; params?: any; result?: any; error?: any };

export type LspState =
  | { s: "starting" }
  | { s: "ready"; name: string }
  | { s: "missing"; hint: string; canInstall: boolean }
  | { s: "installing" }
  | { s: "failed"; why: string };

const [states, setStates] = createSignal<Record<string, LspState>>({});
export const lspStates = states;

const FAMILY: Record<string, string> = { ts: "ts", tsx: "ts", js: "ts", jsx: "ts", mjs: "ts", cjs: "ts", mts: "ts", cts: "ts", rs: "rust", py: "python", pyi: "python" };
const ext = (path: string) => path.slice(path.lastIndexOf(".") + 1).toLowerCase();
export const familyOf = (path: string) => FAMILY[ext(path)] ?? null;
export const familyName = (f: string) => ({ ts: "TypeScript", rust: "Rust", python: "Python" })[f] ?? f;
const stateKey = (root: string, family: string) => `${root}\0${family}`;
export const lspStateFor = (root: string, path: string) => {
  const f = familyOf(path);
  return f ? states()[stateKey(root, f)] : undefined;
};

/** LSP language ids: TSX and JSX are their own languages to a TypeScript server. */
function languageId(path: string, monacoId: string) {
  return ({ tsx: "typescriptreact", jsx: "javascriptreact", ts: "typescript", mts: "typescript", cts: "typescript", js: "javascript", mjs: "javascript", cjs: "javascript" } as Record<string, string>)[ext(path)] ?? monacoId;
}

const fileUri = (path: string) => `file://${path.split("/").map(encodeURIComponent).join("/")}`;
const pathOf = (uri: string) => (uri.startsWith("file://") ? decodeURIComponent(uri.slice(7)) : null);

/** Every `uri` / `targetUri` in a server message, decoded, so the client's lowercased lookup finds it. */
function decodeUris(v: any): any {
  if (Array.isArray(v)) return v.map(decodeUris);
  if (v && typeof v === "object") {
    for (const k of Object.keys(v)) {
      if ((k === "uri" || k === "targetUri") && typeof v[k] === "string" && v[k].startsWith("file://")) v[k] = `file://${decodeURIComponent(v[k].slice(7))}`;
      else v[k] = decodeUris(v[k]);
    }
  }
  return v;
}

const LOCATIONS = new Set(["textDocument/definition", "textDocument/typeDefinition", "textDocument/implementation", "textDocument/declaration", "textDocument/references"]);

function initOptions(family: string) {
  if (family === "rust") {
    // Light on a machine where agents also build: no priming, no workspace-wide check, its own target dir.
    return { cachePriming: { enable: false }, check: { workspace: false }, cargo: { targetDir: true } };
  }
  return {};
}

class TauriTransport {
  private listener: ((m: Msg) => void) | undefined;
  private queue: Msg[] = [];
  private asked = new Map<number | string, string>();
  private stateValue: { state: "open" } | { state: "closed"; error: Error | undefined } = { state: "open" };
  private stateListeners = new Set<(e: unknown) => void>();
  readonly state: { readonly value: TauriTransport["stateValue"]; onChange: (l: (e: unknown) => void) => { dispose(): void } };

  constructor(
    readonly id: number,
    readonly root: string,
    readonly family: string,
    private m: Monaco,
  ) {
    // eslint-disable-next-line @typescript-eslint/no-this-alias
    const t = this;
    this.state = {
      get value() {
        return t.stateValue;
      },
      onChange: (l) => {
        t.stateListeners.add(l);
        return { dispose: () => void t.stateListeners.delete(l) };
      },
    };
  }

  toString() {
    return `pitlane-lsp-${this.id}`;
  }

  setListener(l: ((m: Msg) => void) | undefined) {
    this.listener = l;
    if (l) for (const m of this.queue.splice(0)) l(m);
  }

  private deliver(m: Msg) {
    if (this.listener) this.listener(m);
    else this.queue.push(m);
  }

  closed() {
    this.stateValue = { state: "closed", error: undefined };
    for (const l of this.stateListeners) l(this.stateValue);
  }

  /** The model behind a (lowercased) URI, and whether it belongs to this server. */
  private real(uri: string): { uri: string; path: string; mine: boolean; lang: string } | null {
    const lower = uri.toLowerCase();
    const model = this.m.editor.getModels().find((x) => x.uri.toString(true).toLowerCase() === lower);
    const path = model ? model.uri.path : pathOf(uri);
    if (!path) return null;
    const mine = path.startsWith(this.root + "/") && familyOf(path) === this.family;
    return { uri: fileUri(path), path, mine, lang: languageId(path, model?.getLanguageId() ?? "") };
  }

  async send(msg: Msg): Promise<void> {
    if (msg.method === "initialize") {
      const uri = fileUri(this.root);
      msg.params = {
        ...msg.params,
        processId: null,
        rootUri: uri,
        rootPath: this.root,
        workspaceFolders: [{ uri, name: this.root.split("/").pop() }],
        initializationOptions: initOptions(this.family),
      };
      msg.params.capabilities = { ...msg.params.capabilities, workspace: { ...msg.params.capabilities?.workspace, workspaceFolders: true, configuration: true } };
    }
    const doc = msg.params?.textDocument;
    if (doc?.uri) {
      const r = this.real(doc.uri);
      if (!r?.mine) {
        // Another folder's or language's file: not this server's. A request gets an empty answer.
        if (msg.id != null) queueMicrotask(() => this.deliver({ jsonrpc: "2.0", id: msg.id, result: null }));
        return;
      }
      doc.uri = r.uri;
      if (msg.method === "textDocument/didOpen") doc.languageId = r.lang;
    }
    if (msg.id != null && msg.method) this.asked.set(msg.id, msg.method);
    await lspApi.send(this.id, JSON.stringify(msg));
  }

  async receive(text: string) {
    let msg: Msg;
    try {
      msg = decodeUris(JSON.parse(text));
    } catch {
      return;
    }
    // Server requests the client does not answer.
    if (msg.method && msg.id != null) {
      const reply = (result: unknown) => void lspApi.send(this.id, JSON.stringify({ jsonrpc: "2.0", id: msg.id, result }));
      if (msg.method === "workspace/configuration") return reply((msg.params?.items ?? []).map(() => null));
      if (msg.method === "window/workDoneProgress/create") return reply(null);
      if (msg.method === "workspace/workspaceFolders") return reply([{ uri: fileUri(this.root), name: this.root.split("/").pop() }]);
    }
    // A jump into a file nobody opened: give it a model first, so peek and go-to can show it.
    if (msg.id != null && !msg.method) {
      const method = this.asked.get(msg.id);
      this.asked.delete(msg.id);
      if (method && LOCATIONS.has(method) && msg.result) {
        const list = Array.isArray(msg.result) ? msg.result : [msg.result];
        const paths = [...new Set(list.map((l: any) => pathOf(l.targetUri ?? l.uri ?? "")).filter(Boolean) as string[])];
        await Promise.all(paths.slice(0, 50).map((p) => this.ensureModel(p)));
      }
    }
    this.deliver(msg);
  }

  private async ensureModel(path: string) {
    const uri = this.m.Uri.file(path);
    if (this.m.editor.getModel(uri) || !path.startsWith(this.root + "/")) return;
    try {
      const f = await files.read(this.root, path.slice(this.root.length + 1));
      if (!f.refused && !this.m.editor.getModel(uri)) this.m.editor.createModel(f.text, undefined, uri);
    } catch {
      /* outside the folder, or gone */
    }
  }
}

const transports = new Map<number, TauriTransport>();
const clients = new Map<string, unknown>();
let listening = false;

async function listenOnce() {
  if (listening) return;
  listening = true;
  await onLsp(
    (id, text) => void transports.get(id)?.receive(text),
    (id) => {
      const t = transports.get(id);
      if (!t) return;
      t.closed();
      transports.delete(id);
      clients.delete(stateKey(t.root, t.family));
      setStates({ ...states(), [stateKey(t.root, t.family)]: { s: "failed", why: "The language server stopped. Open the file again to restart it." } });
    },
  );
}

function setState(root: string, family: string, st: LspState) {
  setStates({ ...states(), [stateKey(root, family)]: st });
}

/** Monaco's TypeScript worker answers too: once a real server runs, it steps back. */
function quietBuiltIn(m: Monaco, family: string) {
  if (family !== "ts") return;
  const off = {
    completionItems: false,
    hovers: false,
    documentSymbols: false,
    definitions: false,
    references: false,
    documentHighlights: false,
    rename: false,
    diagnostics: false,
    documentRangeFormattingEdits: false,
    signatureHelp: false,
    onTypeFormattingEdits: false,
    codeActions: false,
    inlayHints: false,
  };
  m.typescript.typescriptDefaults.setModeConfiguration(off);
  m.typescript.javascriptDefaults.setModeConfiguration(off);
}

/** Starts (once) the server for `path`'s language in `root`. */
export async function ensureLsp(m: Monaco, root: string, path: string) {
  const family = familyOf(path);
  if (!family) return;
  const k = stateKey(root, family);
  const st = states()[k];
  if (clients.has(k) || st?.s === "starting" || st?.s === "installing" || st?.s === "missing") return;
  setState(root, family, { s: "starting" });
  await listenOnce();
  let r: LspStart;
  try {
    r = await lspApi.start(root, ext(path));
  } catch (e) {
    setState(root, family, { s: "failed", why: String(e) });
    return;
  }
  if (r.kind === "none") return setStates(Object.fromEntries(Object.entries(states()).filter(([x]) => x !== k)));
  if (r.kind === "missing") return setState(root, family, { s: "missing", hint: r.hint, canInstall: !!r.install });
  const t = new TauriTransport(r.id, root, family, m);
  transports.set(r.id, t);
  const lsp = (m as unknown as { lsp?: { MonacoLspClient: new (t: unknown) => unknown } }).lsp;
  if (!lsp) return setState(root, family, { s: "failed", why: "This Monaco has no LSP client." });
  clients.set(k, new lsp.MonacoLspClient(t));
  quietBuiltIn(m, family);
  setState(root, family, { s: "ready", name: r.name });
}

/** Installs the server for `path`'s language (the person clicked), then starts it. */
export async function installLsp(m: Monaco, root: string, path: string) {
  const family = familyOf(path);
  if (!family) return;
  setState(root, family, { s: "installing" });
  try {
    await lspApi.install(root, ext(path));
  } catch (e) {
    setState(root, family, { s: "failed", why: String(e) });
    return;
  }
  setStates(Object.fromEntries(Object.entries(states()).filter(([x]) => x !== stateKey(root, family))));
  await ensureLsp(m, root, path);
}

export type { M };
