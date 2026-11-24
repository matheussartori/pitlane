// The repository's data, read from the daemon and kept fresh: the yard (branches and lanes), the
// cards, the runs and the quota, the merge queue and Run all's reports. One repository at a time.
import { createSignal } from "solid-js";
import { api, isLive, type Quota, type Card, type Lane, type Report, type RunMeta, type ShipJob, type Yard, isOpenJob } from "../api";
import { keepUnchanged } from "../keep";
import { toast } from "../ui";
import { plural, t } from "../i18n";
import { msg } from "../format";

export const [repo, setRepo] = createSignal<string | null>(null);
export const [cards, setCards] = createSignal<Card[] | null>(null);
export const [runs, setRuns] = createSignal<RunMeta[] | null>(null);
export const [quota, setQuota] = createSignal<Quota | null>(null);
export const [problems, setProblems] = createSignal<string[]>([]);
export const [yard, setYard] = createSignal<Yard | null>(null);
export const [fetching, setFetching] = createSignal(false);
export const [error, setError] = createSignal<string | null>(null);
export const [fetchedAt, setFetchedAt] = createSignal<number | null>(null);

// Loads are tagged so a slow answer for an old repo never overwrites a newer one.
// The fetching flag has its own tag: a plain reload that starts while a fetch is in flight
// drops the fetch's answer, but must not leave the spinner on.
let generation = 0;
let fetchGen = 0;
export async function load(fetch: boolean) {
  const path = repo();
  if (!path) return;
  const gen = ++generation;
  const fgen = fetch ? ++fetchGen : 0;
  if (fetch) setFetching(true);
  try {
    const y = await api.yard(path, fetch);
    if (gen !== generation) return;
    // The lanes that read the same keep their objects, so their rows keep their DOM (hover, an
    // open menu, selected text) across a refresh.
    setYard((old) => ({ ...y, lanes: keepUnchanged(old?.lanes ?? null, y.lanes, (l) => l.branch) }));
    setError(null);
    if (fetch) setFetchedAt(Date.now());
  } catch (e) {
    if (gen === generation) setError(msg(e));
  } finally {
    if (fetch && fgen === fetchGen) setFetching(false);
  }
}
/** Drops the answer of every load already on its way: the repository changed. */
export function dropLoads() {
  generation++;
}

// Cards are small files: re-read them often, on their own, without touching git.
let cardsGen = 0;
export async function loadCards() {
  const path = repo();
  if (!path) return;
  const gen = ++cardsGen;
  try {
    const r = await api.cards(path);
    if (gen !== cardsGen || path !== repo()) return;
    setCards((old) => keepUnchanged(old, r.cards));
    setProblems(r.problems);
  } catch (e) {
    if (gen === cardsGen && path === repo()) setProblems([msg(e)]);
  }
}
/** Drops the answer of a card poll already on its way. */
export function dropCardPoll() {
  cardsGen++;
}

// Runs come from the daemon (or its records on disk). Live ones are followed, so their end
// refreshes the screens even when nobody has the run open.
let runsGen = 0;
export async function loadRuns() {
  const path = repo();
  if (!path) return;
  const gen = ++runsGen;
  try {
    const r = await api.runs(path);
    if (gen !== runsGen || path !== repo()) return;
    setRuns((old) => keepUnchanged(old, r));
    for (const m of r) if (isLive(m.state)) void api.runFollow(m.id, 0).catch(() => {});
  } catch {
    // Keep what was last read (the daemon may be restarting): an empty list would tell Run all
    // that nothing runs, and it would act on cards that are still working. The next poll retries.
    return;
  }
  // The quota is its own call: when it fails, the runs just read stay on screen.
  try {
    const q = await api.quota(path);
    if (gen === runsGen && path === repo()) setQuota(q);
  } catch {
    // Keep the last quota; the next poll tries again.
  }
}

// The merge queue: polled while something is in line; a job that finishes refreshes the lanes.
export const [jobs, setJobs] = createSignal<ShipJob[]>([]);
let jobsGen = 0;
// Run all's reports, read when the screen opens and when a batch ends.
export const [reports, setReports] = createSignal<Report[] | null>(null);
let reportsGen = 0;
export async function loadReports() {
  const path = repo();
  if (!path) return;
  const gen = ++reportsGen;
  try {
    const r = await api.reports(path);
    if (gen === reportsGen && path === repo()) setReports(r);
  } catch {
    if (gen === reportsGen && path === repo()) setReports([]);
  }
}

export async function loadQueue() {
  const path = repo();
  if (!path) return;
  const gen = ++jobsGen;
  try {
    const next = await api.queue(path);
    if (gen !== jobsGen || path !== repo()) return;
    const wasOpen = new Set(jobs().filter(isOpenJob).map((j) => j.id));
    const landed = next.filter((j) => wasOpen.has(j.id) && !isOpenJob(j));
    setJobs(next);
    for (const j of landed) {
      // A card's lane in a related repository: its own lane's landing speaks for the card.
      if (j.state === "done" && j.card_repo) continue;
      if (j.state === "done") toast({ tone: "done", title: t("{branch} is in {target}", { branch: j.branch, target: j.target }) });
      else
        toast({
          tone: "fail",
          title: t("{branch} did not ship to {target}", { branch: j.branch, target: j.target }),
          body: j.state === "conflict" ? t("It conflicts in {files}.", { files: j.conflicts.join(", ") }) : (j.error ?? undefined),
        });
    }
    if (landed.length) {
      void load(false);
      void loadCards();
    }
  } catch {
    // Kept, as the runs are: a failed read is not an empty queue.
  }
}
export async function queueAll(lanes: Lane[], to: string) {
  const path = repo();
  if (!path) return;
  let n = 0;
  for (const l of lanes) {
    try {
      await api.enqueue(path, l.branch, to);
      n++;
    } catch (e) {
      toast({ tone: "fail", title: t("{branch} stayed out of line", { branch: l.branch }), body: msg(e) });
    }
  }
  if (n) toast({ tone: "done", title: plural(n, "{n} lane is in line for {to}", "{n} lanes are in line for {to}", { to }), body: t("The queue ships them one at a time.") });
  void loadQueue();
}
export async function dequeue(id: string) {
  try {
    await api.dequeue(id);
  } catch (e) {
    toast({ tone: "fail", title: t("Not cleared"), body: msg(e) });
  }
  void loadQueue();
}
