// Review: what the agent did and what the checks found, beside the diff. The verdict leads; the
// primary action follows the evidence (changes when something fails, approve when all passes).
import { For, Show, createEffect, createMemo, createResource, createSignal, on, onMount } from "solid-js";
import { api, type Card, type Finding, type LaneDiff, type RunMeta, type Verification } from "./api";
import { reviewSummary } from "./cards";
import { Icon } from "./icons";
import { plural, t } from "./i18n";
import { ErrorLine, Spinner, toast, ok, okLatest } from "./ui";
import { Grip, REVIEW_SIDE_W } from "./layout";
import { clock, elapsed, money, msg } from "./format";


// ---------------------------------------------------------------- the diff

export interface DiffLine {
  kind: "add" | "del" | "ctx" | "meta";
  text: string;
  old?: number;
  new?: number;
}
export interface DiffHunk {
  head: string;
  lines: DiffLine[];
}
export interface DiffFile {
  path: string;
  hunks: DiffHunk[];
  binary: boolean;
  /** Added, deleted, renamed (from `from`), or modified. */
  status: "added" | "deleted" | "renamed" | "modified";
  from?: string;
}

/** Unified diff text into files and hunks, with line numbers on both sides. */
export function parsePatch(patch: string): DiffFile[] {
  const files: DiffFile[] = [];
  let file: DiffFile | null = null;
  let hunk: DiffHunk | null = null;
  let o = 0;
  let n = 0;
  for (const raw of patch.split("\n")) {
    const header = /^diff --git a\/(.+) b\/(.+)$/.exec(raw);
    if (header) {
      file = { path: header[2]!, hunks: [], binary: false, status: header[1] !== header[2] ? "renamed" : "modified", from: header[1] !== header[2] ? header[1] : undefined };
      files.push(file);
      hunk = null;
      continue;
    }
    if (!file) continue;
    if (!hunk && raw.startsWith("new file mode")) {
      file.status = "added";
      continue;
    }
    if (!hunk && raw.startsWith("deleted file mode")) {
      file.status = "deleted";
      continue;
    }
    if (raw.startsWith("Binary files")) {
      file.binary = true;
      continue;
    }
    const h = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/.exec(raw);
    if (h) {
      o = Number(h[1]);
      n = Number(h[2]);
      hunk = { head: raw, lines: [] };
      file.hunks.push(hunk);
      continue;
    }
    if (!hunk) continue;
    if (raw.startsWith("+")) hunk.lines.push({ kind: "add", text: raw.slice(1), new: n++ });
    else if (raw.startsWith("-")) hunk.lines.push({ kind: "del", text: raw.slice(1), old: o++ });
    else if (raw.startsWith(" ")) hunk.lines.push({ kind: "ctx", text: raw.slice(1), old: o++, new: n++ });
    else if (raw.startsWith("\\")) hunk.lines.push({ kind: "meta", text: raw });
  }
  return files;
}

/** Where each finding sits in the diff: the files and new-side lines it shows. */
function anchors(files: DiffFile[], findings: Finding[]): Set<number> {
  const at = new Set<number>();
  findings.forEach((f, i) => {
    const file = files.find((d) => d.path === f.file);
    if (file && f.line != null && file.hunks.some((h) => h.lines.some((l) => l.new === f.line))) at.add(i);
  });
  return at;
}

const VIEWED_KEY = "pitlane.viewed";
function loadViewed(key: string): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(`${VIEWED_KEY}.${key}`) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}
function saveViewed(key: string, v: Set<string>) {
  try {
    localStorage.setItem(`${VIEWED_KEY}.${key}`, JSON.stringify([...v]));
  } catch {
    /* private window: viewed marks last for this visit only */
  }
}

function FindingLine(props: { f: Finding; i: number; picked: boolean; onPick: (i: number, on: boolean) => void }) {
  return (
    <label class="diff-finding" id={`finding-${props.i}`} data-severity={props.f.severity}>
      <input type="checkbox" class="check" checked={props.picked} onChange={(e) => props.onPick(props.i, e.currentTarget.checked)} />
      <span class="finding-sev">{props.f.severity}</span>
      <span>{props.f.text}</span>
    </label>
  );
}

/** Your comment on one line of the diff; it goes to the agent with the next round. */
export interface LineNote {
  file: string;
  side: "new" | "old";
  line: number;
  text: string;
}
const noteKey = (file: string, side: string, line: number) => `${file}\0${side}\0${line}`;

/** The composer under a line: ⌘↩ saves, Esc lets go. */
function NoteEditor(props: { text: string; onSave: (text: string) => void; onCancel: () => void }) {
  let box!: HTMLTextAreaElement;
  onMount(() => box.focus());
  return (
    <div class="diff-note diff-note-edit">
      <textarea
        ref={box}
        class="input textarea"
        rows="2"
        value={props.text}
        placeholder={t("What should change here?")}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter" && e.metaKey) (e.preventDefault(), props.onSave(box.value));
          if (e.key === "Escape") (e.preventDefault(), props.onCancel());
        }}
      />
      <div class="row-actions">
        <button type="button" class="btn btn-ghost" onClick={props.onCancel}>
          {t("Cancel")}
        </button>
        <button type="button" class="btn" onClick={() => props.onSave(box.value)}>
          {t("Comment")} <span class="kbd">⌘↵</span>
        </button>
      </div>
    </div>
  );
}

function DiffView(props: {
  diff: LaneDiff;
  /** Comments on lines, and how to change them (only while the card waits on your review). */
  notes: LineNote[];
  onNote?: (file: string, side: "new" | "old", line: number, text: string) => void;
  findings: Finding[];
  picked: Set<number>;
  onPick: (i: number, on: boolean) => void;
  viewedKey: string;
  onOpen: (path: string) => void;
  /** Per-hunk decisions, only while the card waits on your review. */
  onDrop?: (file: string, header: string) => Promise<void>;
}) {
  const [kept, setKept] = createSignal(loadViewed(`${props.viewedKey}.kept`));
  const [dropping, setDropping] = createSignal<string | null>(null);
  const hunkKey = (file: string, head: string) => `${file}\0${head}`;
  // Files that did not change keep their object, so their section (open, scroll) stays put.
  const files = createMemo<ReturnType<typeof parsePatch>>((prev) => {
    const before = new Map(prev.map((f) => [f.path, f]));
    return parsePatch(props.diff.patch).map((f) => {
      const o = before.get(f.path);
      return o && JSON.stringify(o) === JSON.stringify(f) ? o : f;
    });
  }, []);
  const stat = (path: string) => props.diff.files.find((f) => f.path === path);
  const at = (path: string, line: number | undefined) =>
    line == null ? [] : props.findings.map((f, i) => ({ f, i })).filter(({ f }) => f.file === path && f.line === line);
  const [viewed, setViewed] = createSignal(loadViewed(props.viewedKey));
  const [draft, setDraft] = createSignal<string | null>(null);
  const noteAt = (file: string, side: string, line: number) => props.notes.find((n) => n.file === file && n.side === side && n.line === line);
  return (
    <div class="review-diff">
      <For each={files()}>
        {(f) => {
          const big = f.hunks.reduce((n, h) => n + h.lines.length, 0) > 400;
          const [open, setOpen] = createSignal(!big && !viewed().has(f.path));
          const count = () => props.findings.filter((x) => x.file === f.path).length;
          return (
            <section class="diff" aria-label={f.path}>
              <div class="diff-file">
                <button type="button" class="diff-toggle" aria-expanded={open()} onClick={() => setOpen(!open())}>
                  <Icon name="chev" small class={open() ? "" : "chev-closed"} />
                  <span class="path selectable">{f.path}</span>
                  <Show when={stat(f.path)}>
                    {(s) => (
                      <>
                        <span class="stat-add">+{s().added}</span>
                        <span class="stat-del">−{s().removed}</span>
                      </>
                    )}
                  </Show>
                </button>
                <Show when={count() > 0}>
                  <span class="diff-noted">
                    {plural(count(), "{n} finding", "{n} findings")}
                  </span>
                </Show>
                <button type="button" class="link-btn diff-open" title={t("Open this file in Code")} onClick={() => props.onOpen(f.path)}>
                  {t("Open")}
                </button>
                <label class="diff-viewed">
                  <input
                    type="checkbox"
                    class="check"
                    checked={viewed().has(f.path)}
                    onChange={(e) => {
                      const v = new Set(viewed());
                      if (e.currentTarget.checked) {
                        v.add(f.path);
                        setOpen(false);
                      } else v.delete(f.path);
                      setViewed(v);
                      saveViewed(props.viewedKey, v);
                    }}
                  />
                  {t("Viewed")}
                </label>
              </div>
              <Show when={open()}>
                <Show when={!f.binary} fallback={<p class="diff-empty">{t("A binary file changed.")}</p>}>
                  <For each={f.hunks}>
                    {(h) => (
                      <>
                        <div class="hunk-bar" data-kept={kept().has(hunkKey(f.path, h.head)) ? "" : undefined}>
                          <span class="selectable">{h.head}</span>
                          <Show when={props.onDrop}>
                            <span class="actions">
                              <Show
                                when={!kept().has(hunkKey(f.path, h.head))}
                                fallback={
                                  <button
                                    type="button"
                                    class="btn btn-ghost hunk-kept"
                                    title={t("Undo: mark this change as not decided")}
                                    onClick={() => {
                                      const k = new Set(kept());
                                      k.delete(hunkKey(f.path, h.head));
                                      setKept(k);
                                      saveViewed(`${props.viewedKey}.kept`, k);
                                    }}
                                  >
                                    <Icon name="done" small />
                                    {t("Kept")}
                                  </button>
                                }
                              >
                                <button
                                  type="button"
                                  class="btn btn-ghost"
                                  title={t("Keep this change")}
                                  onClick={() => {
                                    const k = new Set(kept());
                                    k.add(hunkKey(f.path, h.head));
                                    setKept(k);
                                    saveViewed(`${props.viewedKey}.kept`, k);
                                  }}
                                >
                                  {t("Keep")}
                                </button>
                                <button
                                  type="button"
                                  class="btn btn-ghost btn-danger"
                                  title={t("Take this change out of the lane, as a commit")}
                                  disabled={dropping() != null}
                                  aria-busy={dropping() === hunkKey(f.path, h.head)}
                                  onClick={async () => {
                                    setDropping(hunkKey(f.path, h.head));
                                    try {
                                      await props.onDrop!(f.path, h.head);
                                    } finally {
                                      setDropping(null);
                                    }
                                  }}
                                >
                                  <Show when={dropping() === hunkKey(f.path, h.head)}>
                                    <Spinner />
                                  </Show>
                                  {t("Drop")}
                                </button>
                              </Show>
                            </span>
                          </Show>
                        </div>
                        <div class="lines selectable">
                          <For each={h.lines}>
                            {(l) => {
                              const side = l.new != null ? "new" : "old";
                              const n = l.new ?? l.old;
                              const key = n == null ? "" : noteKey(f.path, side, n);
                              const note = () => (n == null ? undefined : noteAt(f.path, side, n));
                              return (
                              <>
                                <div class={`line ${l.kind === "add" ? "add" : l.kind === "del" ? "del" : ""}`} data-noted={note() ? "" : undefined}>
                                  <Show when={props.onNote && n != null} fallback={<span class="ln">{n ?? ""}</span>}>
                                    <button type="button" class="ln ln-btn" title={t("Comment on line {n}", { n: n! })} aria-label={t("Comment on line {n}", { n: n! })} onClick={() => setDraft(key)}>
                                      {n}
                                    </button>
                                  </Show>
                                  <span class="sign">{l.kind === "add" ? "+" : l.kind === "del" ? "−" : ""}</span>
                                  <span>{l.text}</span>
                                </div>
                                <For each={at(f.path, l.new)}>
                                  {({ f: fi, i }) => <FindingLine f={fi} i={i} picked={props.picked.has(i)} onPick={props.onPick} />}
                                </For>
                                <Show
                                  when={draft() === key && key}
                                  fallback={
                                    <Show when={note()}>
                                      {(nt) => (
                                        <div class="diff-note">
                                          <Icon name="note" small />
                                          <span class="diff-note-text">{nt().text}</span>
                                          <Show when={props.onNote}>
                                            <span class="actions">
                                              <button type="button" class="btn btn-ghost" onClick={() => setDraft(key)}>
                                                {t("Edit")}
                                              </button>
                                              <button type="button" class="btn btn-ghost" onClick={() => props.onNote!(f.path, side, n!, "")}>
                                                {t("Remove")}
                                              </button>
                                            </span>
                                          </Show>
                                        </div>
                                      )}
                                    </Show>
                                  }
                                >
                                  <NoteEditor
                                    text={note()?.text ?? ""}
                                    onCancel={() => setDraft(null)}
                                    onSave={(text) => {
                                      props.onNote!(f.path, side, n!, text.trim());
                                      setDraft(null);
                                    }}
                                  />
                                </Show>
                              </>
                              );
                            }}
                          </For>
                        </div>
                      </>
                    )}
                  </For>
                </Show>
              </Show>
            </section>
          );
        }}
      </For>
      <Show when={props.diff.truncated}>
        <p class="diff-empty">{t("The patch is cut here for size; the counts above are complete.")}</p>
      </Show>
      <Show when={files().length === 0}>
        <p class="diff-empty">{t("Nothing changed in this range.")}</p>
      </Show>
    </div>
  );
}

// ---------------------------------------------------------------- the screen

export function ReviewScreen(props: {
  repo: string;
  card: Card | undefined;
  run: RunMeta | undefined;
  now: number;
  integration: string | null;
  onBack: () => void;
  onChanged: () => void;
  onOpenRun: (id: string) => void;
  onOpenFile: (branch: string | null | undefined, path: string) => void;
}) {
  const [verification] = createResource(() => props.run?.id, (id) => api.runVerification(id));
  const round = () => props.run?.round ?? 0;
  const [scope, setScope] = createSignal<"card" | "round">(round() > 0 ? "round" : "card");
  // Keyed by value: a card or run re-read that changed nothing does not read the diff again.
  const diffSource = createMemo(
    () => (props.card?.lane ? ([props.repo, props.card.lane, scope() === "round" ? (props.run?.base_sha ?? undefined) : undefined] as const) : null),
    undefined,
    { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) },
  );
  const [diff, { refetch: rereadDiff }] = createResource(diffSource, ([repo, lane, from]) => api.laneDiff(repo, lane, from));
  const judgement = () => ok(verification)?.judge?.judgement ?? null;
  const findings = () => judgement()?.findings ?? [];
  const placed = createMemo(() => anchors(parsePatch(okLatest(diff)?.patch ?? ""), findings()));
  const [picked, setPicked] = createSignal<Set<number>>(new Set());
  const [note, setNote] = createSignal("");
  // Comments on lines, kept per card until they are sent (leaving the screen does not lose them).
  const notesKey = () => `pitlane.review.${props.card?.id}.notes`;
  const readNotes = (): LineNote[] => {
    try {
      return JSON.parse(localStorage.getItem(notesKey()) ?? "[]") as LineNote[];
    } catch {
      return [];
    }
  };
  const [lineNotes, setLineNotesSig] = createSignal<LineNote[]>(readNotes());
  createEffect(on(() => props.card?.id, () => setLineNotesSig(readNotes()), { defer: true }));
  const setLineNotes = (v: LineNote[]) => {
    setLineNotesSig(v);
    try {
      if (v.length) localStorage.setItem(notesKey(), JSON.stringify(v));
      else localStorage.removeItem(notesKey());
    } catch {
      /* private window: kept for this visit only */
    }
  };
  const comment = (file: string, side: "new" | "old", line: number, text: string) => {
    const rest = lineNotes().filter((n) => !(n.file === file && n.side === side && n.line === line));
    setLineNotes(text ? [...rest, { file, side, line, text }] : rest);
    if (text) setAsking(true);
  };
  const [asking, setAsking] = createSignal(false);
  const [busy, setBusy] = createSignal<string | null>(null);
  const summary = () => reviewSummary(props.run);
  const needsChanges = () => props.run?.proofs_ok === false || props.run?.verdict === "changes";
  const maxRounds = () => ok(verification)?.max_rounds ?? 3;
  const roundsLeft = () => maxRounds() - round();

  const pick = (i: number, on: boolean) => {
    const s = new Set(picked());
    if (on) s.add(i);
    else s.delete(i);
    setPicked(s);
    if (s.size > 0) setAsking(true);
  };

  const message = () => {
    const fs = findings().filter((_, i) => picked().has(i));
    const lines = fs.map((f) => `- ${f.file ? `${f.file}${f.line ? `:${f.line}` : ""}: ` : ""}${f.text}`);
    const mine = [...lineNotes()]
      .sort((a, b) => a.file.localeCompare(b.file) || a.line - b.line)
      .map((n) => `- ${n.file}:${n.line}${n.side === "old" ? " (a removed line)" : ""}: ${n.text}`);
    return [note().trim(), mine.length ? `On the diff:\n${mine.join("\n")}` : "", lines.length ? `From the review:\n${lines.join("\n")}` : ""].filter(Boolean).join("\n\n");
  };

  async function act(action: "approve" | "changes" | "discard") {
    const c = props.card;
    if (!c || busy()) return;
    setBusy(action);
    try {
      const r = await api.cardReview(props.repo, c.id, action, action === "changes" ? message() : undefined);
      if (action === "approve")
        toast({ tone: "done", title: t("#{id} is in line for {to}", { id: c.id, to: r.shipped_to ?? t("its next stage") }), body: t("The merge queue ships it next; the card moves when it lands.") });
      if (action === "discard") toast({ tone: "done", title: t("#{id} is ready again", { id: c.id }), body: t("Its lane stays as it is.") });
      props.onChanged();
      if (action === "changes") setLineNotes([]);
      if (action === "changes" && r.run) props.onOpenRun(r.run.id);
      else props.onBack();
    } catch (e) {
      toast({ tone: "fail", title: action === "approve" ? t("Not shipped") : t("Not sent"), body: msg(e) });
    } finally {
      setBusy(null);
    }
  }

  const h = () => props.run?.handoff;
  const v = (): Verification | null | undefined => ok(verification);
  const passing = () => (v()?.proofs ?? []).filter((p) => p.ok).length;

  return (
    <Show
      when={props.card}
      fallback={
        <div class="empty">
          <div class="empty-title">{t("Card not found")}</div>
          <button type="button" class="btn" onClick={props.onBack}>
            {t("Back to the board")}
          </button>
        </div>
      }
    >
      {(c) => (
        <div class="review-screen">
          <header class="screen-head">
            <button type="button" class="btn btn-ghost btn-icon back" aria-label={t("Back to the board")} onClick={props.onBack}>
              <Icon name="chev" small />
            </button>
            <h1 class="title">
              #{c().id} {c().title}
            </h1>
            <span class="spacer" />
            <Show when={c().state === "review"}>
              <button type="button" class="btn btn-ghost" disabled={!!busy()} onClick={() => void act("discard")}>
                {t("Discard")}
              </button>
              <Show when={!asking()}>
                <button
                  type="button"
                  class={needsChanges() ? "btn btn-primary" : "btn"}
                  disabled={!!busy() || roundsLeft() <= 0}
                  title={roundsLeft() <= 0 ? t("All {n} rounds of changes are used", { n: maxRounds() }) : undefined}
                  onClick={() => setAsking(true)}
                >
                  {t("Ask for changes")}
                </button>
                <button
                  type="button"
                  class={needsChanges() ? "btn" : "btn btn-primary"}
                  disabled={!!busy()}
                  aria-busy={busy() === "approve"}
                  onClick={() => void act("approve")}
                >
                  <Show when={busy() === "approve"} fallback={<Icon name="ship" small />}>
                    <Spinner />
                  </Show>
                  {t("Approve, ship to {to}", { to: props.integration ?? "main" })}
                </button>
              </Show>
            </Show>
          </header>
          <p class="review-verdict" data-tone={summary().chip}>
            <Icon name={summary().icon} small />
            {summary().text}
          </p>
          <p class="run-where">
            <span class="branch selectable">{c().lane}</span>
            <Show when={props.run}>
              {(r) => (
                <button type="button" class="link-btn" onClick={() => props.onOpenRun(r().id)}>
                  {t("Run {time}, {cost}, {model}", { time: clock(elapsed(r(), props.now)), cost: money(r().cost_usd ?? 0), model: r().model ?? t("default model") })}
                </button>
              )}
            </Show>
            <Show when={round() > 0}>
              <span>
                {t("Changes round {n} of {max}", { n: round(), max: maxRounds() })}
              </span>
            </Show>
          </p>

          <div class="review-body" style={{ "--side-w": `${REVIEW_SIDE_W.get()}px` }}>
            <Grip size={REVIEW_SIDE_W} edge="left" label={t("Resize the side column")} style={{ left: "auto", right: "calc(var(--side-w) + var(--space-lg) / 2 - 5px)" }} />
            <div class="review-main">
              <section class="walkthrough" aria-label={t("What the agent says it did")}>
                <h2 class="side-title">{t("What changed")}</h2>
                <p class="selectable">{h()?.summary ?? t("The agent left no summary.")}</p>
              </section>
              <Show when={round() > 0 && props.run?.base_sha}>
                <div class="seg" role="group" aria-label={t("Diff range")}>
                  <button type="button" aria-pressed={scope() === "round"} onClick={() => setScope("round")}>
                    {t("This round")}
                  </button>
                  <button type="button" aria-pressed={scope() === "card"} onClick={() => setScope("card")}>
                    {t("Whole card")}
                  </button>
                </div>
              </Show>
              {/* A re-read keeps the diff on screen (and your scroll); only the first read shows the spinner. */}
              <Show when={okLatest(diff) !== undefined || !diff.loading} fallback={<p class="loading-line"><Spinner /> {t("Reading the diff")}</p>}>
                <Show when={okLatest(diff)} fallback={<ErrorLine>{diff.error ? msg(diff.error) : t("No lane to compare.")}</ErrorLine>}>
                  {(d) => (
                    <DiffView
                      diff={d()}
                      findings={findings()}
                      notes={lineNotes()}
                      onNote={c().state === "review" ? comment : undefined}
                      picked={picked()}
                      onPick={pick}
                      viewedKey={`${c().id}.${d().head.slice(0, 12)}`}
                      onOpen={(p) => props.onOpenFile(c().lane, p)}
                      onDrop={
                        c().state === "review"
                          ? async (file, header) => {
                              try {
                                await api.dropHunk(props.repo, c().lane!, d().base, d().head, file, header);
                                toast({ tone: "done", title: t("Dropped a change in {file}", { file }), body: t("Committed on the lane as review: drop a change.") });
                                await rereadDiff();
                                props.onChanged();
                              } catch (e) {
                                toast({ tone: "fail", title: t("Not dropped"), body: msg(e) });
                              }
                            }
                          : undefined
                      }
                    />
                  )}
                </Show>
              </Show>
            </div>

            <aside class="review-side" aria-label={t("What the checks found")}>
              <div class="sheet-sub-row">
                <h2 class="side-title">{t("Checks")}</h2>
                <Show when={(v()?.proofs.length ?? 0) > 0}>
                  <span class="sheet-sub-note">
                    {t("{n} of {total} pass", { n: passing(), total: v()!.proofs.length })}
                  </span>
                </Show>
              </div>
              <Show when={!verification.loading} fallback={<p class="loading-line"><Spinner /> {t("Reading the checks")}</p>}>
                <Show when={v()} fallback={<p class="side-hint">{t("No checks ran for this run.")}</p>}>
                  {(vv) => (
                    <>
                      <Show when={vv().no_commands}>
                        <p class="side-hint">{t("No commands configured. Add [verify] commands to .pitlane/flow.toml.")}</p>
                      </Show>
                      <ul class="proofs">
                        <For each={vv().proofs}>
                          {(p) => (
                            <li class="proof-row" data-ok={p.ok ? "" : undefined}>
                              <Icon name={p.ok ? "done" : "fail"} small />
                              <code class="selectable">{p.command}</code>
                              <span class="figure">{(p.duration_ms / 1000).toFixed(1)}s</span>
                              <Show when={!p.ok && p.tail}>
                                <pre class="proof-tail selectable">{p.tail}</pre>
                              </Show>
                            </li>
                          )}
                        </For>
                      </ul>

                      <h2 class="side-title">{t("Judge")}</h2>
                      <Show when={judgement()} fallback={<p class="side-hint">{vv().judge?.error ?? t("The judge is off for this repository.")}</p>}>
                        {(j) => (
                          <>
                            <span class="chip" data-state={j().verdict === "approve" ? "done" : "failed"}>
                              <Icon name={j().verdict === "approve" ? "done" : "fail"} small />
                              {j().verdict === "approve" ? t("Approves") : t("Asks for changes")}
                            </span>
                            <p class="selectable">{j().summary}</p>
                            <ul class="findings">
                              <For each={j().findings}>
                                {(f, i) => (
                                  <li class="finding">
                                    <input
                                      type="checkbox"
                                      class="check"
                                      aria-label={t("Send this finding with your changes: {text}", { text: f.text })}
                                      checked={picked().has(i())}
                                      onChange={(e) => pick(i(), e.currentTarget.checked)}
                                    />
                                    <span class="finding-sev">{f.severity}</span>
                                    <Show
                                      when={placed().has(i())}
                                      fallback={<span class="finding-text">{f.file ? `${f.file}${f.line ? `:${f.line}` : ""}: ` : ""}{f.text}</span>}
                                    >
                                      <button
                                        type="button"
                                        class="link-btn finding-jump"
                                        onClick={() => document.getElementById(`finding-${i()}`)?.scrollIntoView({ block: "center" })}
                                      >
                                        {f.file}:{f.line}
                                      </button>
                                    </Show>
                                  </li>
                                )}
                              </For>
                            </ul>
                            <p class="side-hint">
                              {t("A separate {model} session that saw only the spec, the decisions and the diff", { model: vv().judge?.model ?? "" })}
                              {vv().judge?.cost_usd != null ? `, ${money(vv().judge!.cost_usd)}` : ""}.
                            </p>
                          </>
                        )}
                      </Show>
                    </>
                  )}
                </Show>
              </Show>

              <Show when={asking()}>
                <section class="approval" aria-label={t("Ask for changes")}>
                  <p class="approval-ask">{t("What should the agent change?")}</p>
                  <textarea
                    class="input textarea"
                    rows="4"
                    placeholder={t("Your comments. Findings you tick and comments on lines go along.")}
                    value={note()}
                    onInput={(e) => setNote(e.currentTarget.value)}
                  />
                  <Show when={picked().size > 0}>
                    <p class="side-hint">
                      {plural(picked().size, "{n} finding goes along.", "{n} findings go along.")}
                    </p>
                  </Show>
                  <Show when={lineNotes().length > 0}>
                    <p class="side-hint">
                      {plural(lineNotes().length, "{n} comment on the diff goes along.", "{n} comments on the diff go along.")}
                    </p>
                  </Show>
                  <div class="row-actions">
                    <button type="button" class="btn btn-ghost" onClick={() => setAsking(false)}>
                      {t("Cancel")}
                    </button>
                    <button type="button" class="btn btn-primary" disabled={!message() || !!busy() || roundsLeft() <= 0} onClick={() => void act("changes")}>
                      {t("Send round {n} of {max}", { n: round() + 1, max: maxRounds() })}
                    </button>
                  </div>
                </section>
              </Show>
            </aside>
          </div>
        </div>
      )}
    </Show>
  );
}
