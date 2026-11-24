// The sessions' timelines, one copy for every screen that shows a session (a card's run, the
// Terminal's Agents, the dock): read once from the daemon, then grown by its events.
import { createStore, produce, reconcile } from "solid-js/store";
import { api, onRun, type RunEvent } from "../api";

const [timelines, setTimelines] = createStore<Record<string, RunEvent[]>>({});
const loading = new Set<string>();
/** Screens holding a timeline open (a run's page), by session: never dropped while held. */
const held = new Map<string, number>();

/** A session's events in order, or undefined until they were read (`ensureTimeline`). */
export const timelineOf = (id: string): RunEvent[] | undefined => timelines[id];

/** Reads a session's timeline once (again with `force`, when events were missed). */
export async function ensureTimeline(id: string, force = false): Promise<void> {
  if ((!force && timelines[id]) || loading.has(id)) return;
  loading.add(id);
  try {
    const t = await api.runTimeline(id);
    const evs = t.map(([, e]) => e);
    // Live events may have arrived meanwhile: keep the longer one.
    if (!timelines[id] || evs.length >= timelines[id]!.length) setTimelines(id, reconcile(evs));
  } catch {
    if (!timelines[id]) setTimelines(id, []);
  } finally {
    loading.delete(id);
  }
}

/** One event from the daemon: appended when it is the next one, re-read when some were missed,
 *  ignored when it is already there or nobody reads that session. */
function take(run: string, i: number, e: RunEvent) {
  const t = timelines[run];
  if (!t) return;
  if (i === t.length) setTimelines(run, t.length, e);
  else if (i > t.length) void ensureTimeline(run, true);
}

/** Keeps a session's timeline while a screen shows it; call the result when the screen leaves. */
export function holdTimeline(id: string): () => void {
  held.set(id, (held.get(id) ?? 0) + 1);
  void ensureTimeline(id);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    const n = (held.get(id) ?? 1) - 1;
    if (n > 0) held.set(id, n);
    else held.delete(id);
  };
}

/** Frees the timelines of sessions nobody shows any more: a long session's log is thousands of
 *  events, and without this every one ever opened stays in memory for the app's whole life. */
export function dropTimelines(shown: (id: string) => boolean) {
  const gone = Object.keys(timelines).filter((id) => !held.has(id) && !shown(id));
  if (gone.length) setTimelines(produce((all) => gone.forEach((id) => delete all[id])));
}

/** Another repository: nothing of the last one stays, except what a screen still holds. */
export function resetTimelines() {
  dropTimelines(() => false);
}

let listening = false;
/** Once, at launch: grows the timelines with the daemon's events. `onEvent` and `onEnd` hear
 *  them too (the Agents list refreshes on some). */
export async function startTimelines(onEvent: (run: string, e: RunEvent) => void, onEnd: (run: string) => void) {
  if (listening) return;
  listening = true;
  await onRun(
    (run, i, e) => {
      take(run, i, e);
      onEvent(run, e);
    },
    (run) => onEnd(run),
  );
}
