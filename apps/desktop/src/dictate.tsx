// Dictation into a text field: click, speak, click again; the words land where the cursor was.
// Whisper runs on this Mac from the model the app bundles, so nothing leaves the machine.
import { Show, createSignal, onCleanup } from "solid-js";
import { dictation } from "./api";
import { Icon } from "./icons";
import { Spinner, toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));
const clock = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;

export function Dictate(props: {
  /** The field the words go into. */
  field: () => HTMLTextAreaElement | HTMLInputElement | undefined;
  value: () => string;
  onChange: (v: string) => void;
}) {
  const [phase, setPhase] = createSignal<"idle" | "listening" | "writing">("idle");
  const [level, setLevel] = createSignal(0);
  const [secs, setSecs] = createSignal(0);
  let off: (() => void) | undefined;
  let tick: number | undefined;
  // Where the cursor was when listening began: the words go there, not wherever focus wanders.
  let at: [number, number] | null = null;

  function release() {
    off?.();
    off = undefined;
    clearInterval(tick);
    setLevel(0);
  }

  async function start() {
    const f = props.field();
    at = f ? [f.selectionStart ?? props.value().length, f.selectionEnd ?? props.value().length] : null;
    try {
      off = await dictation.onLevel((l) => setLevel(Math.min(1, l * 6)));
      await dictation.start();
      setSecs(0);
      const t0 = Date.now();
      tick = window.setInterval(() => setSecs(Math.floor((Date.now() - t0) / 1000)), 250);
      setPhase("listening");
    } catch (e) {
      release();
      toast({ tone: "fail", title: "The microphone did not open", body: msg(e) });
    }
  }

  async function stop() {
    release();
    setPhase("writing");
    try {
      const text = (await dictation.stop()).trim();
      if (!text) {
        toast({ tone: "info", title: "Nothing heard", body: "If you spoke, allow Pitlane in System Settings › Privacy & Security › Microphone." });
        return;
      }
      insert(text);
    } catch (e) {
      toast({ tone: "fail", title: "Dictation failed", body: msg(e) }, 0);
    } finally {
      setPhase("idle");
    }
  }

  function insert(text: string) {
    const v = props.value();
    const [a, b] = at ?? [v.length, v.length];
    const before = v.slice(0, a);
    const after = v.slice(b);
    const lead = before && !/\s$/.test(before) ? " " : "";
    const trail = after && !/^\s/.test(after) ? " " : "";
    const next = before + lead + text + trail + after;
    props.onChange(next);
    const f = props.field();
    if (f) {
      const caret = (before + lead + text).length;
      queueMicrotask(() => {
        f.focus();
        f.setSelectionRange(caret, caret);
      });
    }
  }

  onCleanup(() => {
    if (phase() === "listening") void dictation.cancel();
    release();
  });

  return (
    <Show when={dictation.available}>
      <button
        type="button"
        class="btn btn-ghost dictate"
        data-phase={phase()}
        disabled={phase() === "writing"}
        aria-busy={phase() === "writing"}
        aria-pressed={phase() === "listening"}
        title={phase() === "listening" ? "Stop and write it down (Esc cancels)" : "Dictate: speak instead of typing, transcribed on this Mac"}
        onClick={() => void (phase() === "listening" ? stop() : start())}
        onKeyDown={(e) => {
          if (e.key === "Escape" && phase() === "listening") {
            e.preventDefault();
            e.stopPropagation();
            release();
            void dictation.cancel();
            setPhase("idle");
          }
        }}
      >
        <Show when={phase() !== "writing"} fallback={<Spinner />}>
          <Icon name={phase() === "listening" ? "stop" : "mic"} small />
        </Show>
        <Show when={phase() === "listening"} fallback={phase() === "writing" ? "Writing" : "Dictate"}>
          <span class="dictate-meter" aria-hidden="true">
            <span style={{ transform: `scaleX(${Math.max(0.06, level())})` }} />
          </span>
          <span class="figure">{clock(secs())}</span>
        </Show>
      </button>
    </Show>
  );
}
