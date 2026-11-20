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

/** Native <dialog>: Escape and focus trapping come free. Unmount it to close. */
export function Sheet(props: { title: string; wide?: boolean; onClose: () => void; children: JSX.Element; foot: JSX.Element }) {
  let ref!: HTMLDialogElement;
  onMount(() => ref.showModal());
  return (
    <dialog
      ref={ref}
      class={props.wide ? "sheet sheet-wide" : "sheet"}
      aria-label={props.title}
      onCancel={(e) => {
        e.preventDefault();
        props.onClose();
      }}
    >
      <div class="sheet-head">
        <h2>{props.title}</h2>
      </div>
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
