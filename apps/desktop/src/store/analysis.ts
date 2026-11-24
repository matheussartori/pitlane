// Analysing cards on their own, outside Run all: one card from its tile or sheet, or every
// backlog card not analysed yet from the column's head. It works whatever Run all is doing in the
// repository, and nothing starts after it: the card only gets its notes and questions.
import { createSignal } from "solid-js";
import { api, isLive } from "../api";
import { plural, t } from "../i18n";
import { msg } from "../format";
import { toast } from "../ui";

const [reading, setReading] = createSignal<Record<string, number[]>>({});
/** Cards of this repository Claude reads now, outside Run all. */
export const analysing = (path: string | null | undefined) => (path ? (reading()[path] ?? []) : []);

type Ask = (path: string, ids: number[]) => void;
let asker: { auto: Ask; open: Ask } = { auto: () => {}, open: () => {} };
/** How the app shows the questions an analysis left (its questions sheet): `auto` when the
 *  analysis ends (it may wait, if you are busy elsewhere), `open` when you ask for them. */
export const setAnalysisAsker = (a: { auto: Ask; open: Ask }) => (asker = a);

/** Claude reads these cards (the ones not read yet nor being read); then their questions come up. */
export async function analyseCards(path: string, ids: number[], onChanged: () => void): Promise<void> {
  const busy = analysing(path);
  const fresh = ids.filter((id) => !busy.includes(id));
  if (fresh.length === 0) return;
  setReading((r) => ({ ...r, [path]: [...(r[path] ?? []), ...fresh] }));
  onChanged();
  try {
    const r = await api.cardsAnalyze(path, fresh);
    const stopped = r.analyzed.filter((a) => a.error && /\bstopped\b/i.test(a.error));
    const failed = r.analyzed.filter((a) => a.error && !stopped.includes(a));
    const done = r.analyzed.filter((a) => !a.error);
    const asked = done.filter((a) => (a.questions ?? 0) > 0);
    if (failed.length > 0)
      toast({ tone: "fail", title: t("Could not analyse {cards}", { cards: failed.map((f) => `#${f.card}`).join(", ") }), body: failed[0]!.error }, 0);
    if (done.length > 0) {
      const cards = done.map((a) => `#${a.card}`).join(", ");
      const questions = asked.reduce((n, a) => n + (a.questions ?? 0), 0);
      toast(
        asked.length > 0
          ? {
              tone: "info",
              title: t("{cards} analysed", { cards }),
              body: plural(questions, "Claude has {n} question before it starts.", "Claude has {n} questions before it starts."),
              action: { label: t("Answer"), run: () => asker.open(path, asked.map((a) => a.card)) },
            }
          : { tone: "done", title: t("{cards} analysed", { cards }), body: t("Its notes are on the card; nothing started.") },
        asked.length > 0 ? 0 : 7000,
      );
      if (asked.length > 0) asker.auto(path, asked.map((a) => a.card));
    }
  } catch (e) {
    toast({ tone: "fail", title: t("The analysis did not run"), body: msg(e) }, 0);
  } finally {
    setReading((r) => ({ ...r, [path]: (r[path] ?? []).filter((id) => !fresh.includes(id)) }));
    onChanged();
  }
}

/** Stops the readings of these cards (all of this repository's when none are named). A card still
 *  waiting for a slot has no session yet: it is stopped once it gets one, until none is left. */
export async function stopAnalysing(path: string, ids?: number[]): Promise<void> {
  const which = ids ?? analysing(path);
  const left = () => analysing(path).filter((id) => which.includes(id));
  try {
    for (let i = 0; i < 120 && left().length > 0; i++) {
      const ss = await api.sessions(path);
      await Promise.allSettled(ss.filter((s) => s.kind === "analysis" && isLive(s.state) && left().includes(s.card)).map((s) => api.runStop(s.id)));
      await new Promise((ok) => setTimeout(ok, 1000));
    }
  } catch (e) {
    toast({ tone: "fail", title: t("Could not stop the analysis"), body: msg(e) });
  }
}
