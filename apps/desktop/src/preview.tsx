// Preview: the dev server a lane runs, drawn inside Pitlane. Its address comes from what the
// terminals printed (Vite's "Local: http://localhost:5173/" and the like), or you type one. The page
// is a webview of its own over this screen's area (the app's CSP keeps frames out); it parks out of
// sight on another screen or under a sheet, and keeps its state while it does.
import { invoke } from "@tauri-apps/api/core";
import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { Icon } from "./icons";
import { t } from "./i18n";
import { localServers } from "./terminals";
import { openExternal } from "./api";
import { msg } from "./format";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const KEY = "pitlane.preview.url";

// What the page was last told: any DOM change asks to place it again (a sheet may have opened),
// and only a real change goes to the backend.
let placed = "";
const show = async (url: string, r: DOMRect) => {
  const key = `${url} ${r.left} ${r.top} ${r.width} ${r.height}`;
  if (key === placed) return;
  placed = key;
  try {
    await invoke<void>("preview_show", { url, x: r.left, y: r.top, w: r.width, h: r.height });
  } catch (e) {
    placed = "";
    throw e;
  }
};
const hide = () => {
  if (placed === "hidden") return Promise.resolve();
  placed = "hidden";
  return invoke<void>("preview_hide").catch(() => {
    placed = "";
  });
};
const reload = () => invoke<void>("preview_reload").catch(() => {});

/** http://localhost:5173 from "localhost:5173", ":5173" or "5173". */
function normalise(input: string): string {
  const s = input.trim();
  if (/^\d{2,5}$/.test(s)) return `http://localhost:${s}/`;
  if (/^:\d{2,5}/.test(s)) return `http://localhost${s}`;
  if (!/^https?:\/\//i.test(s)) return `http://${s}`;
  return s;
}

const saved = () => {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
};

export function PreviewScreen(props: { onTerminal: () => void }) {
  let host!: HTMLDivElement;
  const [servers, setServers] = createSignal(localServers());
  const [url, setUrl] = createSignal<string | null>(saved() ?? servers()[0]?.url ?? null);
  const [draft, setDraft] = createSignal(url() ?? "");
  const [error, setError] = createSignal<string | null>(null);

  const remember = (u: string | null) => {
    setUrl(u);
    setDraft(u ?? "");
    setError(null);
    try {
      if (u) localStorage.setItem(KEY, u);
      else localStorage.removeItem(KEY);
    } catch {
      /* private window */
    }
    place();
  };

  /** Over this screen's area, unless a sheet or menu covers it. */
  const place = () => {
    const u = url();
    if (!inTauri || !u || !host) return;
    if (document.querySelector("dialog[open]")) return void hide();
    const r = host.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return void hide();
    show(u, r).catch((e) => setError(msg(e)));
  };

  onMount(() => {
    place();
    const ro = new ResizeObserver(place);
    ro.observe(host);
    window.addEventListener("resize", place);
    // A sheet (⌘K, a card) is a dialog: the page steps aside while it is open.
    const mo = new MutationObserver(place);
    mo.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["open"] });
    // New servers show up while you are here.
    const look = () => {
      const found = localServers();
      setServers(found);
      // Nothing chosen yet: the newest server a terminal printed.
      if (!url() && found[0]) remember(found[0].url);
    };
    look();
    const poll = setInterval(look, 2000);
    onCleanup(() => {
      ro.disconnect();
      mo.disconnect();
      clearInterval(poll);
      window.removeEventListener("resize", place);
      void hide();
    });
  });

  return (
    <div class="preview-screen">
      <header class="preview-bar">
        <form
          class="preview-url search-field"
          onSubmit={(e) => {
            e.preventDefault();
            if (draft().trim()) remember(normalise(draft()));
          }}
        >
          <Icon name="preview" small />
          <input
            value={draft()}
            spellcheck={false}
            autocomplete="off"
            aria-label={t("Address")}
            placeholder={t("localhost:5173")}
            onInput={(e) => setDraft(e.currentTarget.value)}
          />
        </form>
        <button type="button" class="btn btn-ghost btn-icon" title={t("Reload")} aria-label={t("Reload")} disabled={!url()} onClick={() => void reload()}>
          <Icon name="refresh" small />
        </button>
        <button
          type="button"
          class="btn btn-ghost btn-icon"
          title={t("Open in the browser")}
          aria-label={t("Open in the browser")}
          disabled={!url()}
          onClick={() => void openExternal(url()!).catch(() => {})}
        >
          <Icon name="external" small />
        </button>
        <Show when={servers().length > 0}>
          <span class="preview-found">
            <For each={servers().slice(0, 4)}>
              {(s) => (
                <button
                  type="button"
                  class="preview-server"
                  aria-pressed={url() === s.url}
                  title={t("Printed in {place} ({cwd})", { place: s.label, cwd: s.cwd })}
                  onClick={() => remember(s.url)}
                >
                  {s.url.replace(/^https?:\/\//, "").replace(/\/$/, "")}
                  <span class="preview-server-place">{s.label}</span>
                </button>
              )}
            </For>
          </span>
        </Show>
      </header>
      <div class="preview-host" ref={host}>
        <Show when={!url() || !inTauri || error()}>
          <div class="empty">
            <Show
              when={url() && inTauri && error()}
              fallback={
                <>
                  <div class="empty-title">{inTauri ? t("No dev server yet") : t("The preview draws in the desktop app.")}</div>
                  <p class="empty-body">{t("Start one in a terminal (npm run dev) and its address shows up above, or type an address.")}</p>
                  <Show when={inTauri}>
                    <button type="button" class="btn" onClick={props.onTerminal}>
                      <Icon name="term" small />
                      {t("Terminal where you are")} <span class="kbd">⌘J</span>
                    </button>
                  </Show>
                </>
              }
            >
              <div class="empty-title">{t("The preview did not open")}</div>
              <p class="empty-body">{error()}</p>
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
}
