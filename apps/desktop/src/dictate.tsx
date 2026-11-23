// Dictation into a text field: click, speak, click again; the words land where the cursor was.
// Whisper runs on this Mac from the model the app bundles, so nothing leaves the machine.
import { Show, createSignal, onCleanup } from "solid-js";
import { dictation } from "./api";
import { Icon } from "./icons";
import { Spinner, toast } from "./ui";
import { t } from "./i18n";
import { msg, clock } from "./format";


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
  // Set when the button goes away: a start still in flight then closes what it opened.
  let gone = false;
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
      if (gone) return release();
      await dictation.start();
      if (gone) {
        void dictation.cancel();
        return release();
      }
      setSecs(0);
      const t0 = Date.now();
      tick = window.setInterval(() => setSecs(Math.floor((Date.now() - t0) / 1000)), 250);
      setPhase("listening");
    } catch (e) {
      release();
      toast({ tone: "fail", title: t("The microphone did not open"), body: msg(e) });
    }
  }

  async function stop() {
    release();
    setPhase("writing");
    try {
      const text = (await dictation.stop()).trim();
      if (!text) {
        toast({ tone: "info", title: t("Nothing heard"), body: t("If you spoke, allow Pitlane in System Settings › Privacy & Security › Microphone.") });
        return;
      }
      insert(text);
    } catch (e) {
      toast({ tone: "fail", title: t("Dictation failed"), body: msg(e) }, 0);
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
    gone = true;
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
        title={phase() === "listening" ? t("Stop and write it down (Esc cancels)") : t("Dictate: speak instead of typing, transcribed on this Mac")}
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
        <Show when={phase() === "listening"} fallback={phase() === "writing" ? t("Writing") : t("Dictate")}>
          <span class="dictate-meter" aria-hidden="true">
            <span style={{ transform: `scaleX(${Math.max(0.06, level())})` }} />
          </span>
          <span class="figure">{clock(secs())}</span>
        </Show>
      </button>
    </Show>
  );
}
