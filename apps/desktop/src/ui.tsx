// Small building blocks: modal sheet, toasts, spinner.
import { For, Show, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { Icon } from "./icons";

/** A menu or popover that closes like any other: a click outside it, Escape, or the window
 *  losing focus. Call during a component's setup; `box` is the element holding the trigger and the menu. */
export function dismissable(box: () => HTMLElement | undefined, open: () => boolean, close: () => void) {
  const onDown = (e: PointerEvent) => {
    const b = box();
    if (open() && b && !b.contains(e.target as Node)) close();
  };
  const onKey = (e: KeyboardEvent) => {
    if (open() && e.key === "Escape") close();
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
  return <span class="spinner" role="status" aria-label="Working" />;
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
      toast({ tone: "info", title: "You have unsaved changes", body: "Save them, or press Escape (or click outside) again to discard." }, 4000);
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

export function toast(t: Omit<Toast, "id">, ms = 7000) {
  const id = nextId++;
  setToasts((all) => [...all.slice(-3), { ...t, id }]);
  // 0 keeps it until it is answered or dismissed: for anything that waits on a decision.
  if (ms > 0) setTimeout(() => dismiss(id), ms);
}

function dismiss(id: number) {
  setToasts((all) => all.filter((t) => t.id !== id));
}

export function Toasts() {
  return (
    <div class="toasts" aria-live="polite">
      <For each={toasts()}>
        {(t) => (
          <div class="toast" data-tone={t.tone}>
            <Icon name={t.tone === "fail" ? "fail" : t.tone === "done" ? "done" : "needs"} />
            <div>
              <div class="toast-title">{t.title}</div>
              <Show when={t.body}>
                <div class="toast-body">{t.body}</div>
              </Show>
            </div>
            <div class="row-actions">
              <Show when={t.action}>
                {(a) => (
                  <button
                    type="button"
                    class="btn"
                    onClick={() => {
                      dismiss(t.id);
                      a().run();
                    }}
                  >
                    {a().label}
                  </button>
                )}
              </Show>
              <button type="button" class="btn btn-ghost btn-icon" aria-label="Dismiss" onClick={() => dismiss(t.id)}>
                <Icon name="close" small />
              </button>
            </div>
          </div>
        )}
      </For>
    </div>
  );
}
