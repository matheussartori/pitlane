// Small building blocks: modal sheet, toasts, spinner.
import { ErrorBoundary, For, Show, createEffect, createSignal, on, onCleanup, onMount, type JSX, type Resource } from "solid-js";
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

/** Tooltips for any element with `data-tip` (beside the nav rail, below or above elsewhere): one floating label, placed in the window (never
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
    const side = el.classList.contains("navrail-item");
    if (side) tip.dataset.side = "";
    else delete tip.dataset.side;
    const w = tip.offsetWidth;
    const h = tip.offsetHeight;
    if (side) {
      tip.style.left = `${r.right + 8}px`;
      tip.style.top = `${r.top + r.height / 2 - h / 2}px`;
      return;
    }
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
    const el = (e.target as Element | null)?.closest?.<HTMLElement>("[data-tip]") ?? null;
    if (el === on) return;
    hide();
    if (!el) return;
    on = el;
    timer = window.setTimeout(() => on === el && show(el), 350);
  });
  document.addEventListener("focusin", (e) => {
    const el = (e.target as Element | null)?.closest?.<HTMLElement>("[data-tip]") ?? null;
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

/** A log that follows its newest line while you are at its end, and stays where you put it once
 *  you scroll up to read. Intent counts before position: a wheel or key moving up lets go at
 *  once, so a scroll to the end already scheduled for the line that just came cannot pull you
 *  back, and the end is checked when that scroll runs, not when it was asked for. */
export function tailFollower(slack = 32) {
  let el: HTMLElement | undefined;
  let stick = true;
  const atEnd = (e: HTMLElement) => e.scrollHeight - e.scrollTop - e.clientHeight < slack;
  return {
    ref(node: HTMLElement) {
      el = node;
      node.addEventListener("scroll", () => (stick = atEnd(node)), { passive: true });
      node.addEventListener("wheel", (e) => e.deltaY < 0 && (stick = false), { passive: true });
      node.addEventListener("keydown", (e) => ["ArrowUp", "PageUp", "Home"].includes(e.key) && (stick = false));
    },
    /** New lines came: back to the end, if you are still following when the frame is drawn. */
    follow() {
      requestAnimationFrame(() => {
        if (el && stick) el.scrollTop = el.scrollHeight;
      });
    },
    /** Another log in the same box (another session): follow it from its end. */
    restart() {
      stick = true;
      this.follow();
    },
  };
}

/** Whether a key press belongs to what has focus rather than to a screen's shortcuts: any text
 *  field, a select, editable content or a terminal, and Enter or Space on a button or link. */
export function ownsKey(e: KeyboardEvent): boolean {
  const el = e.target;
  if (!(el instanceof HTMLElement)) return false;
  if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement) return true;
  if (el.isContentEditable || el.closest(".xterm")) return true;
  return (e.key === "Enter" || e.key === " ") && !!el.closest("button, a[href], [role=button]");
}

/** A resource's value, or undefined while it is failed. Solid throws when a failed resource is
 *  read, and nothing up the tree catches it, so read resources that can fail through this. */
export function ok<T>(r: Resource<T>): T | undefined {
  return r.error ? undefined : r();
}

/** Like `ok`, for `latest`: the last value, kept on screen while a re-read is in flight. */
export function okLatest<T>(r: Resource<T>): T | undefined {
  return r.error ? undefined : r.latest;
}

/** A screen that fails to draw shows why, in its own place, instead of taking the whole app
 *  down with it. It tries again when `resetOn` changes (another tab, another repository) or on
 *  Try again. */
export function ScreenBoundary(props: { resetOn: unknown; children: JSX.Element }) {
  return (
    <ErrorBoundary
      fallback={(err, reset) => {
        createEffect(on(() => props.resetOn, () => reset(), { defer: true }));
        console.error(err);
        return (
          <div class="screen-error">
            <ErrorLine>{t("This screen hit an error: {error}", { error: err instanceof Error ? err.message : String(err) })}</ErrorLine>
            <button type="button" class="btn" onClick={reset}>
              {t("Try again")}
            </button>
          </div>
        );
      }}
    >
      {props.children}
    </ErrorBoundary>
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
