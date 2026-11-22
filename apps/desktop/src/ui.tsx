// Small building blocks: modal sheet, toasts, spinner.
import { For, Show, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { Icon } from "./icons";
import { t } from "./i18n";

/** A menu or popover that closes like any other: a click outside it, Escape, or the window
 *  losing focus. Call during a component's setup; `box` is the element holding the trigger and the menu. */
export function dismissable(box: () => HTMLElement | undefined, open: () => boolean, close: () => void) {
  const onDown = (e: PointerEvent) => {
    const b = box();
    if (open() && b && !b.contains(e.target as Node)) close();
  };
  const onKey = (e: KeyboardEvent) => {
    // The Escape is the menu's: what is behind it (a screen that closes on Escape) never sees it.
    if (open() && e.key === "Escape") {
      e.preventDefault();
      close();
    }
  };
  const onBlur = () => open() && close();
  document.addEventListener("pointerdown", onDown, true);
  document.addEventListener("keydown", onKey, true);
  window.addEventListener("blur", onBlur);
  onCleanup(() => {
    document.removeEventListener("pointerdown", onDown, true);
    document.removeEventListener("keydown", onKey, true);
    window.removeEventListener("blur", onBlur);
  });
}

export function Spinner() {
  return <span class="spinner" role="status" aria-label={t("Working")} />;
}

/** Tooltips for any element with `data-tip` (the nav rail draws its own, to the right): one floating label, placed in the window (never
 *  clipped by a scrolling list), after a short hover or at once on keyboard focus. Install once. */
let tipsInstalled = false;
export function installTips() {
  if (tipsInstalled) return;
  tipsInstalled = true;
  const tip = document.createElement("div");
  tip.className = "tip";
  tip.setAttribute("role", "tooltip");
  document.body.append(tip);
  let timer = 0;
  let on: HTMLElement | null = null;
  const show = (el: HTMLElement) => {
    const text = el.dataset.tip;
    if (!text || !el.isConnected) return;
    tip.textContent = text;
    tip.dataset.on = "";
    const r = el.getBoundingClientRect();
    const w = tip.offsetWidth;
    const h = tip.offsetHeight;
    const below = r.bottom + 6 + h <= window.innerHeight;
    const x = Math.min(Math.max(8, r.left + r.width / 2 - w / 2), window.innerWidth - w - 8);
    tip.style.left = `${x}px`;
    tip.style.top = `${below ? r.bottom + 6 : r.top - 6 - h}px`;
  };
  const hide = () => {
    window.clearTimeout(timer);
    on = null;
    delete tip.dataset.on;
  };
  document.addEventListener("pointerover", (e) => {
    const el = (e.target as Element | null)?.closest?.<HTMLElement>("[data-tip]:not(.navrail-item)") ?? null;
    if (el === on) return;
    hide();
    if (!el) return;
    on = el;
    timer = window.setTimeout(() => on === el && show(el), 350);
  });
  document.addEventListener("focusin", (e) => {
    const el = (e.target as Element | null)?.closest?.<HTMLElement>("[data-tip]:not(.navrail-item)") ?? null;
    hide();
    if (el?.matches(":focus-visible")) (on = el), show(el);
  });
  for (const ev of ["pointerdown", "scroll", "blur", "focusout"] as const) window.addEventListener(ev, hide, true);
}

/** Native <dialog>: Escape and focus trapping come free; a click outside closes it too. Unmount
 * it to close. With `dirty`, the first Escape or outside click only warns (the edits stay); a
 * second one within a few seconds discards them. */
export function Sheet(props: {
  title: string;
  wide?: boolean;
  /** Extra class on the dialog, for a sheet with a layout of its own. */
  class?: string;
  /** Replaces the plain title in the head. */
  head?: JSX.Element;
  onClose: () => void;
  /** Unsaved edits: closing asks for a second Escape or click. */
  dirty?: () => boolean;
  children: JSX.Element;
  foot: JSX.Element;
}) {
  let ref!: HTMLDialogElement;
  let warned = 0;
  onMount(() => ref.showModal());
  const leave = () => {
    if (props.dirty?.() && Date.now() - warned > 4000) {
      warned = Date.now();
      toast({ tone: "info", title: t("You have unsaved changes"), body: t("Save them, or press Escape (or click outside) again to discard.") }, 4000);
      return;
    }
    props.onClose();
  };
  // Outside = both the press and the release fall outside the sheet's box (a text selection
  // dragged out of a field does not close it).
  const outside = (e: PointerEvent) => {
    const r = ref.getBoundingClientRect();
    return e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom;
  };
  let downOutside = false;
  return (
    <dialog
      ref={ref}
      class={`${props.wide ? "sheet sheet-wide" : "sheet"}${props.class ? ` ${props.class}` : ""}`}
      aria-label={props.title}
      onCancel={(e) => {
        e.preventDefault();
        leave();
      }}
      onPointerDown={(e) => (downOutside = e.target === ref && outside(e))}
      onPointerUp={(e) => {
        if (downOutside && e.target === ref && outside(e)) leave();
        downOutside = false;
      }}
    >
      <div class="sheet-head">{props.head ?? <h2>{props.title}</h2>}</div>
      <div class="sheet-body">{props.children}</div>
      <div class="sheet-foot">{props.foot}</div>
    </dialog>
  );
}

export function ErrorLine(props: { children: JSX.Element }) {
  return (
    <div class="error-line" role="alert">
      <Icon name="fail" />
      <div class="selectable">{props.children}</div>
    </div>
  );
}

export interface Toast {
  id: number;
  tone: "done" | "fail" | "info";
  title: string;
  body?: string;
  action?: { label: string; run: () => void };
}

const [toasts, setToasts] = createSignal<Toast[]>([]);
let nextId = 1;

export function toast(msg: Omit<Toast, "id">, ms = 7000) {
  const id = nextId++;
  setToasts((all) => [...all.slice(-3), { ...msg, id }]);
  // 0 keeps it until it is answered or dismissed: for anything that waits on a decision.
  if (ms > 0) setTimeout(() => dismiss(id), ms);
}

function dismiss(id: number) {
  setToasts((all) => all.filter((x) => x.id !== id));
}

export function Toasts() {
  return (
    <div class="toasts" aria-live="polite">
      <For each={toasts()}>
        {(m) => (
          <div class="toast" data-tone={m.tone}>
            <Icon name={m.tone === "fail" ? "fail" : m.tone === "done" ? "done" : "needs"} />
            <div>
              <div class="toast-title">{m.title}</div>
              <Show when={m.body}>
                <div class="toast-body">{m.body}</div>
              </Show>
            </div>
            <div class="row-actions">
              <Show when={m.action}>
                {(a) => (
                  <button
                    type="button"
                    class="btn"
                    onClick={() => {
                      dismiss(m.id);
                      a().run();
                    }}
                  >
                    {a().label}
                  </button>
                )}
              </Show>
              <button type="button" class="btn btn-ghost btn-icon" aria-label={t("Dismiss")} onClick={() => dismiss(m.id)}>
                <Icon name="close" small />
              </button>
            </div>
          </div>
        )}
      </For>
    </div>
  );
}
