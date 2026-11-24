// An image opened over the whole window, centered: zoom with the wheel or a pinch (around the
// pointer), drag with the hand to look around, double-click to go between fitting and full size,
// copy it to the clipboard or save it. A modal <dialog>, so it sits above any sheet as well.
import { Show, createEffect, createSignal, on, onCleanup } from "solid-js";
import { api } from "./api";
import { t } from "./i18n";
import { Icon } from "./icons";
import { msg } from "./format";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const PAD = 32;
const MAX = 16;

/** The image as a PNG data URL (what a clipboard takes), drawn at its own size. */
async function asPng(src: string): Promise<string> {
  if (src.startsWith("data:image/png;")) return src;
  const img = new Image();
  img.src = src;
  await img.decode();
  const c = document.createElement("canvas");
  c.width = img.naturalWidth;
  c.height = img.naturalHeight;
  c.getContext("2d")!.drawImage(img, 0, 0);
  return c.toDataURL("image/png");
}

const blobOf = async (url: string) => (await fetch(url)).blob();

/** A name to save under: the one given, else "image" with the type's extension. */
function fileName(src: string, name?: string): string {
  if (name && /\.[a-z0-9]{2,5}$/i.test(name)) return name.split("/").pop()!;
  const ext = /^data:image\/([a-z0-9+]+)/i.exec(src)?.[1]?.replace("jpeg", "jpg").replace("svg+xml", "svg") ?? "png";
  return `${name || "image"}.${ext}`;
}

export function ImageViewer(props: { open: boolean; src: string; alt: string; label: string; name?: string; onClose: () => void }) {
  let dialog: HTMLDialogElement | undefined;
  let stage: HTMLDivElement | undefined;
  const [natural, setNatural] = createSignal({ w: 0, h: 0 });
  const [scale, setScale] = createSignal(1);
  const [at, setAt] = createSignal({ x: 0, y: 0 });
  // Fitted: the view follows the window's size until you zoom or move the image.
  const [fitted, setFitted] = createSignal(true);
  const [dragging, setDragging] = createSignal(false);
  const [note, setNote] = createSignal<{ text: string; fail?: boolean } | null>(null);
  let noteTimer = 0;
  const say = (text: string, fail = false) => {
    window.clearTimeout(noteTimer);
    setNote({ text, fail });
    noteTimer = window.setTimeout(() => setNote(null), fail ? 4000 : 1600);
  };
  onCleanup(() => window.clearTimeout(noteTimer));

  const box = () => ({ w: stage?.clientWidth ?? window.innerWidth, h: stage?.clientHeight ?? window.innerHeight });
  const fit = () => {
    const n = natural();
    if (!n.w || !n.h) return 1;
    const b = box();
    return Math.min(1, (b.w - PAD * 2) / n.w, (b.h - PAD * 2) / n.h);
  };
  const minScale = () => Math.min(fit(), 1) / 4;
  /** Keeps the image from leaving the stage: it moves only as far as it overflows. */
  const clampAt = (p: { x: number; y: number }, s = scale()) => {
    const n = natural();
    const b = box();
    const mx = Math.max(0, (n.w * s - b.w) / 2 + PAD);
    const my = Math.max(0, (n.h * s - b.h) / 2 + PAD);
    return { x: Math.max(-mx, Math.min(mx, p.x)), y: Math.max(-my, Math.min(my, p.y)) };
  };
  const pannable = () => {
    const n = natural();
    const b = box();
    return n.w * scale() > b.w - PAD * 2 + 1 || n.h * scale() > b.h - PAD * 2 + 1;
  };
  const toFit = () => {
    setScale(fit());
    setAt({ x: 0, y: 0 });
    setFitted(true);
  };
  /** Zoom to `next`, keeping the point under (cx, cy) (from the stage's center) where it is. */
  const zoomTo = (next: number, cx = 0, cy = 0) => {
    const s = scale();
    const n = Math.max(minScale(), Math.min(MAX, next));
    const p = at();
    setScale(n);
    setAt(clampAt({ x: cx - (cx - p.x) * (n / s), y: cy - (cy - p.y) * (n / s) }, n));
    setFitted(false);
  };
  const fromCenter = (e: { clientX: number; clientY: number }) => {
    const r = stage!.getBoundingClientRect();
    return { x: e.clientX - r.left - r.width / 2, y: e.clientY - r.top - r.height / 2 };
  };

  // Open and close with the caller; the dialog's own close (Escape) tells the caller.
  createEffect(
    on(
      () => props.open,
      (open) => {
        if (!dialog) return;
        if (open && !dialog.open) {
          dialog.showModal();
          dialog.focus();
        } else if (!open && dialog.open) dialog.close();
      },
    ),
  );
  createEffect(on(() => props.src, () => (setNatural({ w: 0, h: 0 }), setNote(null))));
  const onResize = () => (fitted() ? toFit() : setAt(clampAt(at())));
  window.addEventListener("resize", onResize);
  onCleanup(() => window.removeEventListener("resize", onResize));

  // The wheel zooms: a mouse notch is about 15%, a trackpad pinch (ctrlKey) follows the fingers.
  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    const c = fromCenter(e);
    const unit = e.deltaMode === 1 ? 16 : 1;
    zoomTo(scale() * Math.exp(-e.deltaY * unit * (e.ctrlKey ? 0.01 : 0.0015)), c.x, c.y);
  };

  // Dragging moves the image; a click that did not drag, outside the image, closes.
  let start: { x: number; y: number; ax: number; ay: number; moved: boolean; onImage: boolean } | null = null;
  const onDown = (e: PointerEvent) => {
    if (e.button !== 0) return;
    start = { x: e.clientX, y: e.clientY, ax: at().x, ay: at().y, moved: false, onImage: e.target instanceof HTMLImageElement };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  };
  const onMove = (e: PointerEvent) => {
    if (!start) return;
    const dx = e.clientX - start.x;
    const dy = e.clientY - start.y;
    if (!start.moved && Math.hypot(dx, dy) < 4) return;
    start.moved = true;
    if (!pannable()) return;
    setDragging(true);
    setFitted(false);
    setAt(clampAt({ x: start.ax + dx, y: start.ay + dy }));
  };
  const onUp = () => {
    const s = start;
    start = null;
    setDragging(false);
    if (s && !s.moved && !s.onImage) props.onClose();
  };
  const onDbl = (e: MouseEvent) => {
    const c = fromCenter(e);
    const f = fit();
    if (Math.abs(scale() - f) < 0.001) zoomTo(f < 1 ? 1 : Math.min(MAX, f * 2), c.x, c.y);
    else toFit();
  };

  const copy = async () => {
    try {
      const png = await asPng(props.src);
      try {
        await navigator.clipboard.write([new ClipboardItem({ "image/png": blobOf(png) })]);
      } catch (e) {
        if (!inTauri) throw e;
        await api.copyImage(png);
      }
      say(t("Copied to the clipboard"));
    } catch (e) {
      say(t("Could not copy it: {why}", { why: msg(e) }), true);
    }
  };
  const save = async () => {
    const name = fileName(props.src, props.name);
    try {
      if (inTauri) {
        if (await api.saveImage(props.src, name)) say(t("Saved"));
        return;
      }
      const a = document.createElement("a");
      a.href = props.src;
      a.download = name;
      a.click();
    } catch (e) {
      say(t("Could not save it: {why}", { why: msg(e) }), true);
    }
  };

  // Its keys are its own while it is open; nothing behind it hears them.
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") return; // the dialog's cancel, which closes it
    const k = e.key.toLowerCase();
    const take = () => e.preventDefault();
    if ((e.metaKey || e.ctrlKey) && k === "c") (take(), void copy());
    else if ((e.metaKey || e.ctrlKey) && k === "s") (take(), void save());
    else if (k === "+" || k === "=") (take(), zoomTo(scale() * 1.25));
    else if (k === "-" || k === "_") (take(), zoomTo(scale() / 1.25));
    else if (k === "0") (take(), toFit());
    else if (k === "1") (take(), zoomTo(1));
    else if (e.key.startsWith("Arrow") && pannable()) {
      take();
      const d = e.shiftKey ? 200 : 60;
      const p = at();
      const step = { ArrowLeft: [d, 0], ArrowRight: [-d, 0], ArrowUp: [0, d], ArrowDown: [0, -d] }[e.key] ?? [0, 0];
      setFitted(false);
      setAt(clampAt({ x: p.x + step[0]!, y: p.y + step[1]! }));
    }
    e.stopPropagation();
  };

  const pct = () => `${Math.round(scale() * 100)}%`;
  return (
    <dialog
      ref={dialog}
      class="viewer"
      aria-label={props.label}
      onClose={() => props.open && props.onClose()}
      onKeyDown={onKey}
    >
      <Show when={props.open}>
        <div class="viewer-bar" onPointerDown={(e) => e.stopPropagation()}>
          <span class="viewer-name" title={props.name || undefined}>
            {props.name ? fileName(props.src, props.name) : props.label}
            <Show when={natural().w > 0}>
              <span class="viewer-dims">
                {natural().w} × {natural().h}
              </span>
            </Show>
          </span>
          <Show when={note()}>
            {(n) => (
              <span class="viewer-note" data-fail={n().fail ? "" : undefined} role="status">
                <Icon name={n().fail ? "fail" : "done"} small />
                {n().text}
              </span>
            )}
          </Show>
          <div class="viewer-tools" role="toolbar" aria-label={t("Image")}>
            <button type="button" class="pane-btn" aria-label={t("Zoom out")} data-tip={`${t("Zoom out")}  −`} onClick={() => zoomTo(scale() / 1.25)}>
              <Icon name="zoom-out" small />
            </button>
            <button type="button" class="viewer-pct" aria-label={t("Fit to the window")} data-tip={`${t("Fit to the window")}  0`} onClick={toFit}>
              {pct()}
            </button>
            <button type="button" class="pane-btn" aria-label={t("Zoom in")} data-tip={`${t("Zoom in")}  +`} onClick={() => zoomTo(scale() * 1.25)}>
              <Icon name="zoom-in" small />
            </button>
            <button type="button" class="viewer-pct" aria-label={t("Actual size")} data-tip={`${t("Actual size")}  1`} aria-pressed={Math.abs(scale() - 1) < 0.001} onClick={() => zoomTo(1)}>
              1:1
            </button>
            <span class="viewer-sep" />
            <button type="button" class="pane-btn" aria-label={t("Copy the image")} data-tip={`${t("Copy the image")}  ⌘C`} onClick={() => void copy()}>
              <Icon name="copy" small />
            </button>
            <button type="button" class="pane-btn" aria-label={t("Download")} data-tip={`${t("Download")}  ⌘S`} onClick={() => void save()}>
              <Icon name="download" small />
            </button>
            <span class="viewer-sep" />
            <button type="button" class="pane-btn" aria-label={t("Close")} data-tip={`${t("Close")}  Esc`} onClick={() => props.onClose()}>
              <Icon name="close" small />
            </button>
          </div>
        </div>
        <div
          ref={(el) => {
            stage = el;
            // Not passive, so the page itself never scrolls or zooms under it.
            el.addEventListener("wheel", onWheel, { passive: false });
          }}
          class="viewer-stage"
          data-pan={pannable() ? "" : undefined}
          data-dragging={dragging() ? "" : undefined}
          onPointerDown={onDown}
          onPointerMove={onMove}
          onPointerUp={onUp}
          onPointerCancel={() => ((start = null), setDragging(false))}
          onDblClick={onDbl}
        >
          <img
            src={props.src}
            alt={props.alt}
            draggable={false}
            style={{
              width: natural().w ? `${natural().w}px` : undefined,
              transform: `translate(-50%, -50%) translate(${at().x}px, ${at().y}px) scale(${scale()})`,
              visibility: natural().w ? "visible" : "hidden",
              // Past 200% each pixel shows sharp, to read a screenshot closely.
              "image-rendering": scale() >= 2 ? "pixelated" : "auto",
            }}
            onLoad={(e) => {
              setNatural({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight });
              toFit();
            }}
          />
          <p class="viewer-hint">{t("Scroll to zoom · drag to move · double-click to fit")}</p>
        </div>
      </Show>
    </dialog>
  );
}
