// The three actions that change a repository. Each shows exactly what will happen first.
import { For, Match, Show, Switch, createResource, createSignal } from "solid-js";
import { api, landedLabel, short, type CleanPlan, type Lane, type ShipPlan } from "./api";
import { Icon } from "./icons";
import { ErrorLine, Sheet, Spinner, toast } from "./ui";
import { Dictate } from "./dictate";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));


// ---------------------------------------------------------------- Ship

export function ShipSheet(props: { repo: string; lane: Lane; to: string | null; onDone: () => void; onClose: () => void }) {
  const [plan] = createResource(() => api.shipPlan(props.repo, props.lane.branch, props.to));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const ready = () => {
    const p = plan();
    return p && !p.already && p.conflicts.length === 0 && p.tree ? p : null;
  };

  async function ship(p: ShipPlan) {
    setBusy(true);
    setError(null);
    try {
      // Shipping goes through the merge queue: one lane at a time, each against a fresh target.
      const job = await api.enqueue(props.repo, p.branch, p.target);
      toast({ tone: "done", title: `${p.branch} is in line for ${job.target}`, body: "The queue ships one lane at a time; Lanes shows its place." });
      props.onDone();
      props.onClose();
    } catch (e) {
      setError(msg(e));
    } finally {
      setBusy(false);
    }
  }

  const title = () => `Ship ${props.lane.branch}` + (plan()?.target ? ` to ${plan()!.target}` : "");

  return (
    <Sheet
      title={title()}
      onClose={props.onClose}
      foot={
        <>
          <button type="button" class="btn btn-ghost" onClick={props.onClose} disabled={busy()}>
            {ready() ? "Cancel" : "Close"}
          </button>
          <Show when={ready()}>
            {(p) => (
              <button type="button" class="btn btn-primary" aria-busy={busy()} disabled={busy()} onClick={() => ship(p())}>
                <Show when={busy()} fallback={<Icon name="ship" small />}>
                  <Spinner />
                </Show>
                {busy() ? "Adding" : `Ship to ${p().target}`}
              </button>
            )}
          </Show>
        </>
      }
    >
      <Switch>
        <Match when={plan.loading}>
          <div class="loading-line">
            <Spinner /> Fetching origin and simulating the merge
          </div>
        </Match>
        <Match when={plan.error}>
          <ErrorLine>{msg(plan.error)}</ErrorLine>
        </Match>
        <Match when={plan()?.already}>
          {(how) => (
            <div class="note">
              <Icon name="done" small />
              <span>
                Already in {plan()!.target} ({landedLabel[how()]}). Nothing to ship.
              </span>
            </div>
          )}
        </Match>
        <Match when={plan() && plan()!.conflicts.length > 0}>
          <ErrorLine>
            Conflicts with origin/{plan()!.target} in {plan()!.conflicts.length} file
            {plan()!.conflicts.length === 1 ? "" : "s"}. Nothing changed.
          </ErrorLine>
          <ul class="files">
            <For each={plan()!.conflicts}>{(f) => <li>{f}</li>}</For>
          </ul>
          <p>Resolve it by hand where {plan()!.target} is checked out:</p>
          <code class="files">
            git switch {plan()!.target} && git pull && git merge {plan()!.branch} && git push
          </code>
          <p>rerere records the resolution, so the same conflict resolves itself on the next stage.</p>
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
                It joins the merge queue. When its turn comes, Pitlane fetches {p().target} again, builds{" "}
                <span class="code">Merge {p().branch} into {p().target}</span> without touching your checkouts, and pushes
                it. A conflict at that point stops only this lane.
              </p>
              <Show when={p().check}>
                {(c) => (
                  <div class="note">
                    <Icon name="verify" small />
                    <span>
                      Runs <span class="code">{c()[0]}</span> first; a failure ships nothing.
                    </span>
                  </div>
                )}
              </Show>
              <Show when={p().remote_behind > 0}>
                <div class="note">
                  <Icon name="needs" small />
                  <span>
                    origin/{p().branch} has {p().remote_behind} commit{p().remote_behind === 1 ? "" : "s"} you don't have
                    locally. They will not ship; pull first if they belong in it.
                  </span>
                </div>
              </Show>
              <Show when={p().remote_ahead > 0}>
                <div class="note">
                  <Icon name="ship" small />
                  <span>
                    {p().remote_ahead} local commit{p().remote_ahead === 1 ? "" : "s"} not pushed to origin/{p().branch}{" "}
                    yet; they ship inside the merge.
                  </span>
                </div>
              </Show>
              <Show when={p().dirty > 0}>
                <div class="note">
                  <Icon name="needs" small />
                  <span>
                    {p().dirty} uncommitted change{p().dirty === 1 ? "" : "s"} in the worktree will not ship.
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
      const out = await api.clean(props.repo, branches);
      const ok = out.filter((o) => !o.error).map((o) => o.branch);
      const failed = out.filter((o) => o.error);
      if (ok.length) {
        toast(
          {
            tone: "done",
            title: `Deleted ${ok.length} lane${ok.length === 1 ? "" : "s"}`,
            body: `${ok.join(", ")}. Undo stays available for 30 days.`,
            action: {
              label: "Undo",
              run: async () => {
                for (const b of ok) {
                  try {
                    await api.undoClean(props.repo, b);
                  } catch (e) {
                    toast({ tone: "fail", title: `Could not restore ${b}`, body: msg(e) });
                  }
                }
                toast({ tone: "done", title: "Restored", body: "Branches are back where they were; worktrees are not recreated." });
                props.onDone();
              },
            },
          },
          15000,
        );
      }
      for (const f of failed) toast({ tone: "fail", title: `${f.branch} not deleted`, body: f.error ?? "" }, 12000);
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
      title="Clean up shipped lanes"
      onClose={props.onClose}
      foot={
        <>
          <button type="button" class="btn btn-ghost" onClick={props.onClose} disabled={busy()}>
            Cancel
          </button>
          <Show when={(plan()?.items.length ?? 0) > 0}>
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
              Delete {chosen().size}
            </button>
          </Show>
        </>
      }
    >
      <Switch>
        <Match when={plan.loading}>
          <div class="loading-line">
            <Spinner /> Fetching origin and checking every lane against main
          </div>
        </Match>
        <Match when={plan.error}>
          <ErrorLine>{msg(plan.error)}</ErrorLine>
        </Match>
        <Match when={plan()}>
          {(p) => (
            <>
              <Show
                when={p().items.length > 0}
                fallback={<p>Nothing to clean: no lane is in production with a clean worktree.</p>}
              >
                <p>
                  These are in production and their worktrees are clean. Deleting removes the worktree, the local branch
                  and the remote branch. Each can be undone for 30 days.
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
                            {[i.local ? "branch" : null, i.worktree ? "worktree" : null, i.delete_remote ? "remote branch" : null]
                              .filter(Boolean)
                              .join(" + ")}
                          </span>
                        </span>
                        <span class="tag">{i.how ? landedLabel[i.how] : ""}</span>
                      </label>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={p().skipped.length > 0}>
                <p class="caption">Kept:</p>
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
        r.copied.length ? `copied ${r.copied.join(", ")}` : null,
        r.cloned.length ? `cloned ${r.cloned.join(", ")}` : null,
        r.missing.length ? `${r.missing.join(", ")} not found` : null,
        r.setup_ok === false ? "setup command failed" : null,
      ].filter(Boolean);
      toast(
        {
          tone: r.setup_ok === false ? "fail" : "done",
          title: `${r.branch} from ${r.base} ${short(r.base_sha)}`,
          body: extra.length ? `${r.path}. ${extra.join(", ")}.` : r.path,
          action: { label: "Copy path", run: () => void navigator.clipboard.writeText(r.path) },
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
      title="New lane"
      onClose={props.onClose}
      foot={
        <>
          <button type="button" class="btn btn-ghost" onClick={props.onClose} disabled={busy()}>
            Cancel
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
            {busy() ? "Creating" : "Create lane"}
          </button>
        </>
      }
    >
      <form id="new-lane" class="field" classList={{ "is-error": !!error() }} onSubmit={create}>
        <label class="field-label" for="lane-name">
          Branch
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
            ? "Fetching, creating the worktree, copying files, running setup"
            : `Starts from a freshly fetched origin/${props.base}, in its own worktree.`}
        </span>
      </form>
      <Show when={error()}>
        <ErrorLine>{error()}</ErrorLine>
      </Show>
    </Sheet>
  );
}

// ---------------------------------------------------------------- Run all

export interface RunAllChoice {
  /** null: the repository's flow (a lane per card from the base, shipped through each stage). */
  into: string | null;
  direct: boolean;
  /** What happens around and after the work, in the person's words; every agent reads it, and a
   * closing session does the "when it is all done" part. */
  instructions: string;
}

/**
 * Before Run all starts: where the work lands. A lane per card, branched from the target when it
 * is a lane and merged back into it once approved; or every card directly in the target's
 * worktree, one at a time.
 */
export function RunAllSheet(props: {
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
  let notesRef: HTMLTextAreaElement | undefined;
  const trunk = () => into() === props.production || into() === props.integration;
  const target = () => into() || null;
  const flowWords = () => `origin/${props.base}, then ${[props.integration, props.production].filter(Boolean).join(" and ")} through the merge queue`;
  const help = () => {
    const t = target();
    if (!t) return `Each card gets a lane of its own from a freshly fetched ${flowWords()}.`;
    if (direct()) return `Every card is worked in ${t}'s own worktree, one at a time, committing there. Nothing is pushed.`;
    if (trunk()) return `Each card gets a lane from a freshly fetched origin/${props.base}; approved, it ships straight into ${t}.`;
    return `Each card gets a lane branched from ${t}; approved, it is merged into ${t} on this machine (nothing pushed). Cards that build on others start once those landed.`;
  };

  return (
    <Sheet
      title={`Run all ${props.count} card${props.count === 1 ? "" : "s"}`}
      onClose={props.onClose}
      foot={
        <>
          <button type="button" class="btn btn-ghost" onClick={props.onClose}>
            Cancel
          </button>
          <button type="button" class="btn btn-primary" onClick={() => props.onStart({ into: target(), direct: direct() && !!target(), instructions: instructions().trim() })}>
            <Icon name="play" small /> Start
          </button>
        </>
      }
    >
      <div class="field">
        <label class="field-label" for="run-all-into">
          Into
        </label>
        <select id="run-all-into" class="select input-mono" value={into()} onChange={(e) => setInto(e.currentTarget.value)}>
          <option value="">The flow ({[props.integration, props.production].filter(Boolean).join(" → ")})</option>
          <For each={lanes()}>{(l) => <option value={l.branch}>{l.branch}</option>}</For>
          <Show when={props.integration}>{(i) => <option value={i()}>{i()} only</option>}</Show>
          <option value={props.production}>{props.production} directly</option>
        </select>
      </div>
      <Show when={target()}>
        <div class="field">
          <span class="field-label">How</span>
          <div class="seg" role="group" aria-label="How the cards are worked">
            <button type="button" aria-pressed={!direct()} onClick={() => setDirect(false)}>
              A lane per card
            </button>
            <button type="button" aria-pressed={direct()} onClick={() => setDirect(true)}>
              Directly in {target()}
            </button>
          </div>
        </div>
      </Show>
      <p class="field-help">{help()}</p>
      <Show when={direct() && trunk()}>
        <ErrorLine>
          Agents will commit straight into {target()} in its checkout. Make sure nothing of yours is uncommitted there.
        </ErrorLine>
      </Show>
      <div class="field">
        <span class="field-label-row">
          <label class="field-label" for="run-all-after">
            When it is all done
          </label>
          <Dictate field={() => notesRef} value={instructions} onChange={setInstructions} />
        </span>
        <textarea
          id="run-all-after"
          ref={notesRef}
          class="input run-all-after"
          rows="4"
          placeholder="Optional. E.g. never run the e2e tests; when everything landed, merge into dev and build a local dev version."
          value={instructions()}
          onInput={(e) => setInstructions(e.currentTarget.value)}
        />
        <span class="field-help">Every agent reads this. What is for the end is done by one closing session once the cards landed, then you get the report.</span>
      </div>
      <p class="field-help">
        Cards start in board order. One that builds on another waits for it to land; two that change the same engine or schema never run together.
      </p>
    </Sheet>
  );
}
