// A file's changes as VS Code's diff editor shows them: the old text on the left, the new on the
// right, removed lines in red and added ones in green, line numbers on both sides. Opened from a
// run's What changed; the file list beside it moves between the files of the same diff.
import { For, Show, createMemo, createSignal } from "solid-js";
import type { DiffFile, DiffHunk } from "./review";
import { Icon } from "./icons";
import { plural, t } from "./i18n";
import { DIFF_FILES_W, Grip } from "./layout";

export type FileStatus = DiffFile["status"];

/** One changed file as the side column lists it. */
export interface ChangedFile {
  path: string;
  added: number | null;
  removed: number | null;
  status: FileStatus | null;
}

export const statusLetter: Record<FileStatus, string> = { added: "A", deleted: "D", renamed: "R", modified: "M" };
export const statusWord: Record<FileStatus, string> = { added: "Added", deleted: "Deleted", renamed: "Renamed", modified: "Modified" };

/** +12 −3, each in its colour; nothing for a count that is not known. */
export function Counts(props: { added: number | null; removed: number | null }) {
  return (
    <span class="dv-counts">
      <Show when={props.added != null && props.added > 0}>
        <span class="dv-add">+{props.added}</span>
      </Show>
      <Show when={props.removed != null && props.removed > 0}>
        <span class="dv-del">−{props.removed}</span>
      </Show>
    </span>
  );
}

type Side = { no?: number; text: string; kind: "add" | "del" | "ctx" | "none" };
type Row = { kind: "hunk"; head: string } | { kind: "pair"; left: Side; right: Side };

/** A hunk as side-by-side rows: a run of removed lines faces the run of added lines after it. */
function rows(hunks: DiffHunk[]): Row[] {
  const out: Row[] = [];
  for (const h of hunks) {
    out.push({ kind: "hunk", head: h.head });
    let dels: Side[] = [];
    let adds: Side[] = [];
    const flush = () => {
      for (let i = 0; i < Math.max(dels.length, adds.length); i++)
        out.push({ kind: "pair", left: dels[i] ?? { text: "", kind: "none" }, right: adds[i] ?? { text: "", kind: "none" } });
      dels = [];
      adds = [];
    };
    for (const l of h.lines) {
      if (l.kind === "del") {
        if (adds.length) flush();
        dels.push({ no: l.old, text: l.text, kind: "del" });
      } else if (l.kind === "add") adds.push({ no: l.new, text: l.text, kind: "add" });
      else if (l.kind === "ctx") {
        flush();
        out.push({ kind: "pair", left: { no: l.old, text: l.text, kind: "ctx" }, right: { no: l.new, text: l.text, kind: "ctx" } });
      }
    }
    flush();
  }
  return out;
}

const hunkWhere = (head: string) => /@@[^@]*@@\s?(.*)$/.exec(head)?.[1]?.trim() ?? "";

/** The diff of one file, side by side or inline. */
export function FileDiffView(props: { file: DiffFile; inline: boolean }) {
  const table = createMemo(() => rows(props.file.hunks));
  return (
    <Show when={!props.file.binary} fallback={<p class="dv-empty">{t("A binary file changed: there is no text to compare.")}</p>}>
      <Show when={props.file.hunks.length > 0} fallback={<p class="dv-empty">{t("Only the file's mode or name changed.")}</p>}>
        <Show
          when={!props.inline}
          fallback={
            <div class="dv-grid dv-inline" role="table" aria-label={props.file.path}>
              <For each={props.file.hunks}>
                {(h) => (
                  <>
                    <div class="dv-hunk">{hunkWhere(h.head) || h.head}</div>
                    <For each={h.lines.filter((l) => l.kind !== "meta")}>
                      {(l) => (
                        <div class="dv-row" data-kind={l.kind}>
                          <span class="dv-no">{l.old ?? ""}</span>
                          <span class="dv-no">{l.new ?? ""}</span>
                          <span class="dv-sign">{l.kind === "add" ? "+" : l.kind === "del" ? "−" : ""}</span>
                          <code class="dv-text">{l.text || " "}</code>
                        </div>
                      )}
                    </For>
                  </>
                )}
              </For>
            </div>
          }
        >
          <div class="dv-grid dv-split" role="table" aria-label={props.file.path}>
            <For each={table()}>
              {(r) =>
                r.kind === "hunk" ? (
                  <div class="dv-hunk">{hunkWhere(r.head) || r.head}</div>
                ) : (
                  <div class="dv-row">
                    <span class="dv-no" data-kind={r.left.kind}>{r.left.no ?? ""}</span>
                    <code class="dv-text" data-kind={r.left.kind}>{r.left.text || " "}</code>
                    <span class="dv-no dv-gutter" data-kind={r.right.kind}>{r.right.no ?? ""}</span>
                    <code class="dv-text" data-kind={r.right.kind}>{r.right.text || " "}</code>
                  </div>
                )
              }
            </For>
          </div>
        </Show>
      </Show>
    </Show>
  );
}

const INLINE_KEY = "pitlane.diff.inline";
const readInline = () => {
  try {
    return localStorage.getItem(INLINE_KEY) === "1";
  } catch {
    return false;
  }
};

/** A full-size view of a diff: the files on the left, the chosen one side by side. */
export function DiffViewer(props: {
  title: string;
  files: ChangedFile[];
  diffs: DiffFile[];
  truncated: boolean;
  start: string;
  onOpenInCode: (path: string) => void;
  onClose: () => void;
}) {
  let ref!: HTMLDialogElement;
  const [path, setPath] = createSignal(props.start);
  const [inline, setInline] = createSignal(readInline());
  const current = () => props.diffs.find((d) => d.path === path());
  const stat = () => props.files.find((f) => f.path === path());
  const index = () => props.files.findIndex((f) => f.path === path());
  const go = (step: number) => {
    const i = index() + step;
    if (i >= 0 && i < props.files.length) setPath(props.files[i]!.path);
  };
  const toggle = () => {
    const v = !inline();
    setInline(v);
    try {
      localStorage.setItem(INLINE_KEY, v ? "1" : "0");
    } catch {
      /* this visit only */
    }
  };
  const cut = (p: string) => p.lastIndexOf("/");
  return (
    <dialog
      ref={(el) => {
        ref = el;
        queueMicrotask(() => ref.showModal());
      }}
      class="sheet dv-sheet"
      aria-label={props.title}
      onCancel={(e) => {
        e.preventDefault();
        props.onClose();
      }}
      onKeyDown={(e) => {
        if (e.target instanceof HTMLInputElement) return;
        if (e.key === "ArrowDown" || e.key === "j") (e.preventDefault(), go(1));
        else if (e.key === "ArrowUp" || e.key === "k") (e.preventDefault(), go(-1));
      }}
      onClick={(e) => e.target === ref && props.onClose()}
    >
      <div class="dv-layout" style={{ "--side-w": `${DIFF_FILES_W.get()}px` }}>
        <Grip size={DIFF_FILES_W} edge="right" label={t("Resize the file list")} style={{ right: "auto", left: "calc(var(--side-w) - 5px)" }} />
        <nav class="dv-files" aria-label={t("Changed files")}>
          <p class="dv-files-head">
            {props.title}
            <span class="dv-files-count">{props.files.length}</span>
          </p>
          <ul>
            <For each={props.files}>
              {(f) => (
                <li>
                  <button type="button" class="dv-file" aria-current={f.path === path()} onClick={() => setPath(f.path)} title={f.path}>
                    <span class="dv-status" data-status={f.status ?? "modified"}>{statusLetter[f.status ?? "modified"]}</span>
                    <span class="dv-file-name" data-status={f.status ?? undefined}>{f.path.slice(cut(f.path) + 1)}</span>
                    <Counts added={f.added} removed={f.removed} />
                    <span class="dv-file-dir">{cut(f.path) > 0 ? f.path.slice(0, cut(f.path)) : ""}</span>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </nav>
        <section class="dv-main">
          <header class="dv-head">
            <span class="dv-status" data-status={stat()?.status ?? "modified"}>{statusLetter[stat()?.status ?? "modified"]}</span>
            <div class="dv-title">
              <b>{path().slice(cut(path()) + 1)}</b>
              <span class="dv-title-dir">
                <Show when={current()?.from} fallback={cut(path()) > 0 ? path().slice(0, cut(path())) : ""}>
                  {t("from {path}", { path: current()!.from! })}
                </Show>
              </span>
            </div>
            <Show when={stat()}>{(s) => <Counts added={s().added} removed={s().removed} />}</Show>
            <span class="spacer" />
            <div class="dv-seg" role="group" aria-label={t("Layout")}>
              <button type="button" aria-pressed={!inline()} onClick={() => inline() && toggle()}>
                {t("Side by side")}
              </button>
              <button type="button" aria-pressed={inline()} onClick={() => !inline() && toggle()}>
                {t("Inline")}
              </button>
            </div>
            <button type="button" class="btn" disabled={stat()?.status === "deleted"} onClick={() => props.onOpenInCode(path())}>
              <Icon name="code" small />
              {t("Open in Code")}
            </button>
            <button type="button" class="btn btn-ghost btn-icon" aria-label={t("Close")} onClick={props.onClose}>
              <Icon name="close" small />
            </button>
          </header>
          <Show when={!inline()}>
            <div class="dv-sides" aria-hidden="true">
              <span>{stat()?.status === "added" ? t("Did not exist") : t("Before")}</span>
              <span>{stat()?.status === "deleted" ? t("Deleted") : t("After")}</span>
            </div>
          </Show>
          <div class="dv-body">
            <Show
              when={current()}
              fallback={
                <p class="dv-empty">
                  {props.truncated
                    ? t("The patch was cut for size before this file; open it in Code to see it.")
                    : t("This file is outside this repository's diff (a related repository, or not committed yet).")}
                </p>
              }
            >
              {(d) => <FileDiffView file={d()} inline={inline()} />}
            </Show>
          </div>
          <footer class="dv-foot">
            <span class="kbd">↑</span> <span class="kbd">↓</span> {t("next file")} · <span class="kbd">esc</span> {t("closes")}
            <span class="spacer" />
            {plural(props.files.length, "{n} file", "{n} files")}
          </footer>
        </section>
      </div>
    </dialog>
  );
}
