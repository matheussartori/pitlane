// Sizes you set by dragging a divider (a sidebar's width, the agents' share of a column), kept on
// this Mac. Each has a default; Settings puts them all back.
import { createSignal, onCleanup, type Accessor, type JSX } from "solid-js";
import { t } from "./i18n";

const PREFIX = "pitlane.size.";

export interface Size {
  get: Accessor<number>;
  set: (px: number) => void;
  reset: () => void;
  min: number;
  max: () => number;
  def: () => number;
}

const all: Size[] = [];
/** Keys other code keeps on its own (the agents dock), cleared with the rest. */
const extra: { key: string; reset: () => void }[] = [];

function read(key: string): number | null {
  try {
    const v = Number(localStorage.getItem(PREFIX + key));
    return Number.isFinite(v) && v > 0 ? v : null;
  } catch {
    return null;
  }
}

/** A size in pixels, remembered under `key`; `def` and `max` may depend on the window. */
export function size(key: string, opts: { def: number | (() => number); min: number; max: number | (() => number) }): Size {
  const def = typeof opts.def === "function" ? opts.def : () => opts.def as number;
  const max = typeof opts.max === "function" ? opts.max : () => opts.max as number;
  const [stored, setStored] = createSignal<number | null>(read(key));
  const clamp = (v: number) => Math.round(Math.max(opts.min, Math.min(Math.max(opts.min, max()), v)));
  const s: Size = {
    get: () => clamp(stored() ?? def()),
    set: (px) => {
      const v = clamp(px);
      setStored(v);
      try {
        localStorage.setItem(PREFIX + key, String(v));
      } catch {
        /* kept for this session only */
      }
    },
    reset: () => {
      setStored(null);
      try {
        localStorage.removeItem(PREFIX + key);
      } catch {
        /* fine */
      }
    },
    min: opts.min,
    max,
    def,
  };
  all.push(s);
  return s;
}

export function registerSize(key: string, reset: () => void) {
  extra.push({ key, reset });
}

/** Every divider back where it started. */
export function resetLayout() {
  for (const s of all) s.reset();
  for (const e of extra) {
    try {
      localStorage.removeItem(e.key);
    } catch {
      /* fine */
    }
    e.reset();
  }
}

/**
 * A divider on one edge of the panel whose size it sets: drag it, double click for the default,
 * arrows from the keyboard. Put it inside the panel (which is `position: relative`).
 *
 * `bare` is a grip drawn by a class of its own (`class`, `cursor`): no edge, hint or maximum on
 * it, and the page is not told a resize is going on. `onGrab` runs as a drag starts.
 */
export function Grip(props: {
  size: Size;
  edge: "left" | "right" | "top" | "bottom";
  label: string;
  style?: JSX.CSSProperties;
  class?: string;
  cursor?: string;
  bare?: boolean;
  onGrab?: () => void;
}) {
  const vertical = () => props.edge === "left" || props.edge === "right";
  const sign = () => (props.edge === "right" || props.edge === "bottom" ? 1 : -1);
  let from = 0;
  let start = 0;
  const move = (e: PointerEvent) => props.size.set(start + sign() * ((vertical() ? e.clientX : e.clientY) - from));
  const up = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    document.body.style.removeProperty("cursor");
    if (!props.bare) delete document.body.dataset.resizing;
  };
  onCleanup(up);
  return (
    <div
      class={props.class ?? "grip"}
      data-edge={props.bare ? undefined : props.edge}
      style={props.style}
      role="separator"
      aria-orientation={vertical() ? "vertical" : "horizontal"}
      aria-label={props.label}
      aria-valuenow={props.size.get()}
      aria-valuemin={props.size.min}
      aria-valuemax={props.bare ? undefined : props.size.max()}
      data-tip={props.bare ? undefined : t("Drag to resize · double click for the default")}
      tabindex="0"
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        props.onGrab?.();
        from = vertical() ? e.clientX : e.clientY;
        start = props.size.get();
        document.body.style.cursor = props.cursor ?? (vertical() ? "col-resize" : "row-resize");
        if (!props.bare) document.body.dataset.resizing = "";
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", up);
      }}
      onDblClick={() => props.size.reset()}
      onKeyDown={(e) => {
        const keys = vertical() ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"];
        const i = keys.indexOf(e.key);
        if (i < 0) return;
        e.preventDefault();
        // Right/Down move the divider right/down.
        props.size.set(props.size.get() + (i === 1 ? 1 : -1) * sign() * 24);
      }}
    />
  );
}

// ---------------------------------------------------------------- the app's dividers

const vw = (f: number) => () => Math.round(window.innerWidth * f);
/** Lanes' Tower, docked on the right. */
export const TOWER_W = size("tower", { def: 264, min: 200, max: vw(0.45) });
/** The run screen's side column (This run, Card). */
export const RUN_SIDE_W = size("run.side", { def: 300, min: 220, max: vw(0.5) });
/** The review screen's side column (what the checks found). */
export const REVIEW_SIDE_W = size("review.side", { def: 320, min: 220, max: vw(0.5) });
/** A card sheet's side column. */
export const CARD_SIDE_W = size("card.side", { def: 300, min: 220, max: vw(0.5) });
/** The diff viewer's file list. */
export const DIFF_FILES_W = size("diff.files", { def: 260, min: 160, max: vw(0.4) });
