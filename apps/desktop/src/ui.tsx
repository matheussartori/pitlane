// Small building blocks: modal sheet, toasts, spinner.
import { For, Show, createSignal, onMount, type JSX } from "solid-js";
import { Icon } from "./icons";

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
