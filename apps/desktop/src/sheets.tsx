// The three actions that change a repository. Each shows exactly what will happen first.
import { For, Match, Show, Switch, createResource, createSignal } from "solid-js";
import { api, landedLabel, short, type CleanPlan, type Lane, type ShipPlan } from "./api";
import { Icon } from "./icons";
import { ErrorLine, Sheet, Spinner, toast, ok } from "./ui";
import { Dictate } from "./dictate";
import { plural, t, tx } from "./i18n";
import { msg } from "./format";
import type { RunAllChoice } from "./runall";



// ---------------------------------------------------------------- Ship

export function ShipSheet(props: { repo: string; lane: Lane; to: string | null; onDone: () => void; onClose: () => void }) {
  const [plan] = createResource(() => api.shipPlan(props.repo, props.lane.branch, props.to));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const ready = () => {
    const p = ok(plan);
    return p && !p.already && p.conflicts.length === 0 && p.tree ? p : null;
  };

  async function ship(p: ShipPlan) {
    setBusy(true);
    setError(null);
    try {
      // Shipping goes through the merge queue: one lane at a time, each against a fresh target.
      const job = await api.enqueue(props.repo, p.branch, p.target);
      toast({ tone: "done", title: t("{branch} is in line for {target}", { branch: p.branch, target: job.target }), body: t("The queue ships one lane at a time; Lanes shows its place.") });
      props.onDone();
      props.onClose();
    } catch (e) {
      setError(msg(e));
    } finally {
      setBusy(false);
    }
  }

  const title = () => (ok(plan)?.target ? t("Ship {branch} to {target}", { branch: props.lane.branch, target: ok(plan)!.target }) : t("Ship {branch}", { branch: props.lane.branch }));

  return (
    <Sheet
      title={title()}
      onClose={props.onClose}
      foot={
        <>
          <button type="button" class="btn btn-ghost" onClick={props.onClose} disabled={busy()}>
            {ready() ? t("Cancel") : t("Close")}
          </button>
          <Show when={ready()}>
            {(p) => (
              <button type="button" class="btn btn-primary" aria-busy={busy()} disabled={busy()} onClick={() => ship(p())}>
                <Show when={busy()} fallback={<Icon name="ship" small />}>
                  <Spinner />
                </Show>
                {busy() ? t("Adding") : t("Ship to {target}", { target: p().target })}
              </button>
            )}
          </Show>
        </>
      }
    >
      <Switch>
        <Match when={plan.loading}>
          <div class="loading-line">
            <Spinner /> {t("Fetching origin and simulating the merge")}
          </div>
        </Match>
        <Match when={plan.error}>
          <ErrorLine>{msg(plan.error)}</ErrorLine>
        </Match>
        <Match when={ok(plan)?.already}>
          {(how) => (
            <div class="note">
              <Icon name="done" small />
              <span>
                {t("Already in {target} ({how}). Nothing to ship.", { target: ok(plan)!.target, how: t(landedLabel[how()]) })}
              </span>
            </div>
          )}
        </Match>
        <Match when={ok(plan) && ok(plan)!.conflicts.length > 0}>
          <ErrorLine>
            {plural(ok(plan)!.conflicts.length, "Conflicts with origin/{target} in {n} file. Nothing changed.", "Conflicts with origin/{target} in {n} files. Nothing changed.", { target: ok(plan)!.target })}
          </ErrorLine>
          <ul class="files">
            <For each={ok(plan)!.conflicts}>{(f) => <li>{f}</li>}</For>
          </ul>
          <p>{t("Resolve it by hand where {target} is checked out:", { target: ok(plan)!.target })}</p>
          <code class="files">
            git switch {ok(plan)!.target} && git pull && git merge {ok(plan)!.branch} && git push
          </code>
          <p>{t("rerere records the resolution, so the same conflict resolves itself on the next stage.")}</p>
        </Match>
        <Match when={ready()}>
          {(p) => (
            <>
              <div class="flow">
                <div class="ref">
                  <b>{p().branch}</b>
                  <span>{short(p().sha)}</span>
                </div>
                <Icon name="ship" />
                <div class="ref">
                  <b>origin/{p().target}</b>
                  <span>{short(p().target_sha)}</span>
                </div>
              </div>
              <p>
                {t("It joins the merge queue. When its turn comes, Pitlane fetches {target} again, builds", { target: p().target })}{" "}
                <span class="code">Merge {p().branch} into {p().target}</span>{" "}
                {t("without touching your checkouts, and pushes it. A conflict at that point stops only this lane.")}
              </p>
              <Show when={p().check}>
                {(c) => (
                  <div class="note">
                    <Icon name="verify" small />
                    <span>
                      {tx("Runs", "verb")} <span class="code">{c()[0]}</span> {t("first; a failure ships nothing.")}
                    </span>
                  </div>
                )}
              </Show>
              <Show when={p().remote_behind > 0}>
                <div class="note">
                  <Icon name="needs" small />
                  <span>
                    {plural(p().remote_behind, "origin/{branch} has {n} commit you don’t have locally. It will not ship; pull first if it belongs in it.", "origin/{branch} has {n} commits you don’t have locally. They will not ship; pull first if they belong in it.", { branch: p().branch })}
                  </span>
                </div>
              </Show>
              <Show when={p().remote_ahead > 0}>
                <div class="note">
                  <Icon name="ship" small />
                  <span>
                    {plural(p().remote_ahead, "{n} local commit not pushed to origin/{branch} yet; it ships inside the merge.", "{n} local commits not pushed to origin/{branch} yet; they ship inside the merge.", { branch: p().branch })}
                  </span>
                </div>
              </Show>
              <Show when={p().dirty > 0}>
                <div class="note">
                  <Icon name="needs" small />
                  <span>
                    {plural(p().dirty, "{n} uncommitted change in the worktree will not ship.", "{n} uncommitted changes in the worktree will not ship.")}
                  </span>
                </div>
              </Show>
            </>
          )}
        </Match>
      </Switch>
      <Show when={error()}>
        <ErrorLine>{error()}</ErrorLine>
      </Show>
    </Sheet>
  );
}

// ---------------------------------------------------------------- Clean

export function CleanSheet(props: { repo: string; onDone: () => void; onClose: () => void }) {
  const [chosen, setChosen] = createSignal<Set<string>>(new Set());
  const [plan] = createResource(async (): Promise<CleanPlan> => {
    const p = await api.cleanPlan(props.repo);
    setChosen(new Set(p.items.map((i) => i.branch)));
    return p;
  });
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  function toggle(branch: string, on: boolean) {
    const s = new Set(chosen());
    if (on) s.add(branch);
    else s.delete(branch);
    setChosen(s);
  }

  async function run() {
    const branches = [...chosen()];
    setBusy(true);
    setError(null);
    try {
      const repo = props.repo; // Undo, clicked later, restores here even if another repository is open by then
      const out = await api.clean(repo, branches);
      const ok = out.filter((o) => !o.error).map((o) => o.branch);
      const failed = out.filter((o) => o.error);
      if (ok.length) {
        toast(
          {
            tone: "done",
            title: plural(ok.length, "Deleted {n} lane", "Deleted {n} lanes"),
            body: t("{branches}. Undo stays available for 30 days.", { branches: ok.join(", ") }),
            action: {
              label: t("Undo"),
              run: async () => {
                for (const b of ok) {
                  try {
                    await api.undoClean(repo, b);
                  } catch (e) {
                    toast({ tone: "fail", title: t("Could not restore {branch}", { branch: b }), body: msg(e) });
                  }
                }
                toast({ tone: "done", title: t("Restored"), body: t("Branches are back where they were; worktrees are not recreated.") });
                props.onDone();
              },
            },
          },
          15000,
        );
      }
      for (const f of failed) toast({ tone: "fail", title: t("{branch} not deleted", { branch: f.branch }), body: f.error ?? "" }, 12000);
      props.onDone();
      props.onClose();
    } catch (e) {
      setError(msg(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet
      title={t("Clean up shipped lanes")}
      onClose={props.onClose}
      foot={
        <>
          <button type="button" class="btn btn-ghost" onClick={props.onClose} disabled={busy()}>
            {t("Cancel")}
          </button>
          <Show when={(ok(plan)?.items.length ?? 0) > 0}>
            <button
              type="button"
              class="btn btn-danger"
              aria-busy={busy()}
              disabled={busy() || chosen().size === 0}
              onClick={run}
            >
              <Show when={busy()} fallback={<Icon name="trash" small />}>
                <Spinner />
              </Show>
              {t("Delete {n}", { n: chosen().size })}
            </button>
          </Show>
        </>
      }
    >
      <Switch>
        <Match when={plan.loading}>
          <div class="loading-line">
            <Spinner /> {t("Fetching origin and checking every lane against main")}
          </div>
        </Match>
        <Match when={plan.error}>
          <ErrorLine>{msg(plan.error)}</ErrorLine>
        </Match>
        <Match when={ok(plan)}>
          {(p) => (
            <>
              <Show
                when={p().items.length > 0}
                fallback={<p>{t("Nothing to clean: no lane is in production with a clean worktree.")}</p>}
              >
                <p>
                  {t("These are in production and their worktrees are clean. Deleting removes the worktree, the local branch and the remote branch. Each can be undone for 30 days.")}
                </p>
                <div>
                  <For each={p().items}>
                    {(i) => (
                      <label class="pick">
                        <input
                          type="checkbox"
                          class="check"
                          checked={chosen().has(i.branch)}
                          onChange={(e) => toggle(i.branch, e.currentTarget.checked)}
                        />
                        <span class="lane-name">
                          <span class="branch">{i.branch}</span>
                          <span class="what">
                            {[i.local ? t("branch") : null, i.worktree ? t("worktree") : null, i.delete_remote ? t("remote branch") : null]
                              .filter(Boolean)
                              .join(" + ")}
                          </span>
                        </span>
                        <span class="tag">{i.how ? t(landedLabel[i.how]) : ""}</span>
                      </label>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={p().skipped.length > 0}>
                <p class="caption">{t("Kept:")}</p>
                <For each={p().skipped}>
                  {(s) => (
                    <div class="note">
                      <Icon name="blocked" small />
                      <span>
                        <span class="code">{s.branch}</span>: {s.reason}
                      </span>
                    </div>
                  )}
                </For>
              </Show>
            </>
          )}
        </Match>
      </Switch>
      <Show when={error()}>
        <ErrorLine>{error()}</ErrorLine>
      </Show>
    </Sheet>
  );
}

// ---------------------------------------------------------------- New lane

export function NewLaneSheet(props: { repo: string; base: string; onDone: () => void; onClose: () => void }) {
  const [name, setName] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function create(e: Event) {
    e.preventDefault();
    const branch = name().trim();
    if (!branch) return;
    setBusy(true);
    setError(null);
    try {
      const r = await api.newLane(props.repo, branch);
      const extra = [
        r.copied.length ? t("copied {files}", { files: r.copied.join(", ") }) : null,
        r.cloned.length ? t("cloned {files}", { files: r.cloned.join(", ") }) : null,
        r.missing.length ? t("{files} not found", { files: r.missing.join(", ") }) : null,
        r.setup_ok === false ? t("setup command failed") : null,
      ].filter(Boolean);
      toast(
        {
          tone: r.setup_ok === false ? "fail" : "done",
          title: t("{branch} from {base} {sha}", { branch: r.branch, base: r.base, sha: short(r.base_sha) }),
          body: extra.length ? `${r.path}. ${extra.join(", ")}.` : r.path,
          action: { label: t("Copy path"), run: () => void navigator.clipboard.writeText(r.path) },
        },
        12000,
      );
      props.onDone();
      props.onClose();
    } catch (err) {
      setError(msg(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet
      title={t("New lane")}
      onClose={props.onClose}
      dirty={() => !!name().trim()}
      foot={
        <>
          <button type="button" class="btn btn-ghost" onClick={props.onClose} disabled={busy()}>
            {t("Cancel")}
          </button>
          <button
            type="submit"
            form="new-lane"
            class="btn btn-primary"
            aria-busy={busy()}
            disabled={busy() || !name().trim()}
          >
            <Show when={busy()} fallback={<Icon name="plus" small />}>
              <Spinner />
            </Show>
            {busy() ? t("Creating") : t("Create lane")}
          </button>
        </>
      }
    >
      <form id="new-lane" class="field" classList={{ "is-error": !!error() }} onSubmit={create}>
        <label class="field-label" for="lane-name">
          {t("Branch")}
        </label>
        <input
          id="lane-name"
          class="input input-mono"
          placeholder="feat/thing"
          autocomplete="off"
          spellcheck={false}
          autofocus
          value={name()}
          onInput={(e) => setName(e.currentTarget.value)}
          disabled={busy()}
        />
        <span class="field-help">
          {busy()
            ? t("Fetching, creating the worktree, copying files, running setup")
            : t("Starts from a freshly fetched origin/{base}, in its own worktree.", { base: props.base })}
        </span>
      </form>
      <Show when={error()}>
        <ErrorLine>{error()}</ErrorLine>
      </Show>
    </Sheet>
  );
}

// ---------------------------------------------------------------- Run all

/**
 * Before Run all starts: where the work lands. A lane per card, branched from the target when it
 * is a lane and merged back into it once approved; or every card directly in the target's
 * worktree, one at a time.
 */
export function RunAllSheet(props: {
  repo: string;
  count: number;
  yard: Lane[];
  base: string;
  production: string;
  integration: string | null;
  /** The lane selected in the Tower, offered first. */
  selected: string | null;
  onStart: (c: RunAllChoice) => void;
  onClose: () => void;
}) {
  const lanes = () => props.yard.filter((l) => l.local && l.branch !== props.production && l.branch !== props.integration);
  const first = () => (props.selected && lanes().some((l) => l.branch === props.selected) ? props.selected : "");
  const [into, setInto] = createSignal<string>(first());
  const [direct, setDirect] = createSignal(false);
  const [instructions, setInstructions] = createSignal("");
  // Sibling repositories the analysis found the code in: kept unless taken off here.
  const [repos, { mutate: setRepos }] = createResource(() => props.repo, (p) => api.projectRepos(p).catch(() => [] as string[]));
  async function dropRepo(rel: string) {
    try {
      setRepos(await api.projectRepoRemove(props.repo, rel));
    } catch (e) {
      toast({ tone: "fail", title: t("{name} is still on the list", { name: rel }), body: String(e) }, 0);
    }
  }
  let notesRef: HTMLTextAreaElement | undefined;
  const trunk = () => into() === props.production || into() === props.integration;
  const target = () => into() || null;
  const help = () => {
    const to = target();
    if (!to)
      return t("Each card gets a lane of its own from a freshly fetched origin/{base}, then {stages} through the merge queue.", {
        base: props.base,
        stages: [props.integration, props.production].filter(Boolean).join(t(" and ")),
      });
    if (direct()) return t("Every card is worked in {to}’s own worktree, one at a time, committing there. Nothing is pushed.", { to });
    if (trunk()) return t("Each card gets a lane from a freshly fetched origin/{base}; once its checks pass (after fix rounds if the judge asks), it ships straight into {to} on its own.", { base: props.base, to });
    return t("Each card gets a lane branched from {to}; once its checks pass (after fix rounds if the judge asks), it is merged into {to} on this machine by itself (nothing pushed). Cards that build on others start once those landed.", { to });
  };

  return (
    <Sheet
      title={plural(props.count, "Run all {n} card", "Run all {n} cards", { n: props.count })}
      onClose={props.onClose}
      dirty={() => !!instructions().trim()}
      foot={
        <>
          <button type="button" class="btn btn-ghost" onClick={props.onClose}>
            {t("Cancel")}
          </button>
          <button type="button" class="btn btn-primary" onClick={() => props.onStart({ into: target(), direct: direct() && !!target(), instructions: instructions().trim() })}>
            <Icon name="play" small /> {t("Start")}
          </button>
        </>
      }
    >
      <div class="field">
        <label class="field-label" for="run-all-into">
          {t("Into")}
        </label>
        <select id="run-all-into" class="select input-mono" value={into()} onChange={(e) => setInto(e.currentTarget.value)}>
          <option value="">{t("The flow")} ({[props.integration, props.production].filter(Boolean).join(" → ")})</option>
          <For each={lanes()}>{(l) => <option value={l.branch}>{l.branch}</option>}</For>
          <Show when={props.integration}>{(i) => <option value={i()}>{t("{name} only", { name: i() })}</option>}</Show>
          <option value={props.production}>{t("{name} directly", { name: props.production })}</option>
        </select>
      </div>
      <Show when={target()}>
        <div class="field">
          <span class="field-label">{t("How")}</span>
          <div class="seg" role="group" aria-label={t("How the cards are worked")}>
            <button type="button" aria-pressed={!direct()} onClick={() => setDirect(false)}>
              {t("A lane per card")}
            </button>
            <button type="button" aria-pressed={direct()} onClick={() => setDirect(true)}>
              {t("Directly in {name}", { name: target() ?? "" })}
            </button>
          </div>
        </div>
      </Show>
      <p class="field-help">{help()}</p>
      <Show when={(repos() ?? []).length > 0}>
        <div class="field">
          <span class="field-label">{t("Also works in")}</span>
          <ul class="run-all-repos">
            <For each={repos()}>
              {(rel) => (
                <li>
                  <span class="input-mono">{rel}</span>
                  <button type="button" class="btn btn-ghost btn-icon" aria-label={t("Take {name} off the list", { name: rel })} title={t("Take {name} off the list", { name: rel })} onClick={() => void dropRepo(rel)}>
                    <Icon name="close" small />
                  </button>
                </li>
              )}
            </For>
          </ul>
          <span class="field-help">{t("Each card gets the same branch in these repositories too, and lands there the same way. Found by the analysis; kept in .pitlane/flow.toml.")}</span>
        </div>
      </Show>
      <Show when={direct() && trunk()}>
        <ErrorLine>
          {t("Agents will commit straight into {name} in its checkout. Make sure nothing of yours is uncommitted there.", { name: target() ?? "" })}
        </ErrorLine>
      </Show>
      <div class="field">
        <span class="field-label-row">
          <label class="field-label" for="run-all-after">
            {t("When it is all done")}
          </label>
          <Dictate field={() => notesRef} value={instructions} onChange={setInstructions} />
        </span>
        <textarea
          id="run-all-after"
          ref={notesRef}
          class="input run-all-after"
          rows="4"
          placeholder={t("Optional. E.g. never run the e2e tests; when everything landed, merge into dev and build a local dev version.")}
          value={instructions()}
          onInput={(e) => setInstructions(e.currentTarget.value)}
        />
        <span class="field-help">{t("Every agent reads this. What is for the end is done by one closing session once the cards landed, then you get the report.")}</span>
      </div>
      <p class="field-help">
        {t("Cards start in board order. One that builds on another waits for it to land; two that change the same engine or schema never run together.")}
      </p>
      <p class="field-help">{t("Nothing waits on you once it starts: agents run without asking for permissions, fix what the checks find and land on their own.")}</p>
    </Sheet>
  );
}

// ---------------------------------------------------------------- Project

/**
 * What every session of this project should know and the board cannot say: the repositories it
 * spans, how their branches land together, what may be pushed or deployed. Kept beside the board,
 * never in the repository; runs, analyses and the closing session all read it.
 */
export function ProjectSheet(props: { repo: string; repoName: string; onClose: () => void }) {
  // A read that failed is not an empty context: saving over it would wipe the real one.
  const [unread, setUnread] = createSignal<string | null>(null);
  const [saved, { mutate: setSaved }] = createResource(
    () => props.repo,
    (p) =>
      api.projectContext(p).catch((e) => {
        setUnread(msg(e));
        return "";
      }),
  );
  const [text, setText] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const value = () => text() ?? saved() ?? "";
  const dirty = () => text() !== null && text()!.trim() !== (saved() ?? "").trim();
  const [repos, { mutate: setRepos }] = createResource(() => props.repo, (p) => api.projectRepos(p).catch(() => [] as string[]));
  let ref: HTMLTextAreaElement | undefined;
  async function dropRepo(rel: string) {
    try {
      setRepos(await api.projectRepoRemove(props.repo, rel));
    } catch (e) {
      toast({ tone: "fail", title: t("{name} is still on the list", { name: rel }), body: msg(e) }, 0);
    }
  }
  async function save() {
    setBusy(true);
    try {
      setSaved(await api.projectContextSet(props.repo, value()));
      setText(null);
      props.onClose();
    } catch (e) {
      toast({ tone: "fail", title: t("The project context was not saved"), body: msg(e) }, 0);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Sheet
      title={t("{name}: project context", { name: props.repoName })}
      wide
      onClose={props.onClose}
      dirty={dirty}
      foot={
        <>
          <button type="button" class="btn btn-ghost" onClick={props.onClose}>
            {t("Cancel")}
          </button>
          <button type="button" class="btn btn-primary" disabled={busy() || !dirty() || unread() !== null} title={unread() ? t("The project context could not be read, so it cannot be saved over: {why}", { why: unread()! }) : undefined} onClick={() => void save()}>
            {busy() ? <Spinner /> : null} {t("Save")}
          </button>
        </>
      }
    >
      <div class="field">
        <span class="field-label-row">
          <label class="field-label" for="project-context">
            {t("Context for every session")}
          </label>
          <Dictate field={() => ref} value={value} onChange={setText} />
        </span>
        <textarea
          id="project-context"
          ref={ref}
          class="input input-mono project-context"
          rows="16"
          placeholder={t("E.g. the games live in ../amity-games: push the lane there before publishing it to the dev channel. Never merge into main.")}
          value={value()}
          onInput={(e) => setText(e.currentTarget.value)}
        />
        <span class="field-help">
          {t("Every run, analysis and closing session of this project reads it. It can allow what the general rules forbid, like pushing a lane or deploying to dev. Kept with the board on this Mac, not in the repository.")}
        </span>
      </div>
      <Show when={(repos() ?? []).length > 0}>
        <div class="field">
          <span class="field-label">{t("Also works in")}</span>
          <ul class="run-all-repos">
            <For each={repos()}>
              {(rel) => (
                <li>
                  <span class="input-mono">{rel}</span>
                  <button type="button" class="btn btn-ghost btn-icon" aria-label={t("Take {name} off the list", { name: rel })} title={t("Take {name} off the list", { name: rel })} onClick={() => void dropRepo(rel)}>
                    <Icon name="close" small />
                  </button>
                </li>
              )}
            </For>
          </ul>
          <span class="field-help">{t("Each lane gets the same branch in these repositories, lands there with it, and ships to dev and main together with it.")}</span>
        </div>
      </Show>
    </Sheet>
  );
}
