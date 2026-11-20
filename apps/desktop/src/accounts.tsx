// Accounts: which login each run uses, whether it is signed in, how much of it is used, and the
// route the runs follow. Moving this repository between your subscription accounts is your hand,
// here, never Pitlane's.
import { For, Show, createResource, createSignal } from "solid-js";
import { api, type AccountKind, type AccountView } from "./api";
import { Icon } from "./icons";
import { ErrorLine, Sheet, Spinner, toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

const ADD = "pit account add <name> && pit account login <name>";

const kindLabel: Record<AccountKind, string> = { subscription: "Subscription", team: "Team seat", "api-key": "API key" };

/** `opus@repo` → `opus on this repository's account`. */
function routeLabel(entry: string, here: string | undefined): string {
  const [model, account] = entry.split("@");
  const who = account === "repo" ? (here ? `${here}, this repository's account` : "this repository's account") : account;
  return `${model || "The default model"} on ${who}`;
}

function Meter(props: { label: string; used: number | null | undefined; at: number }) {
  return (
    <Show when={props.used != null}>
      <div class="meter" data-level={props.used! >= 95 ? "out" : props.used! >= props.at ? "high" : undefined}>
        <div class="meter-head">
          {props.label}
          <b>{Math.round(props.used!)}%</b>
        </div>
        <div class="meter-track">
          <div class="meter-fill" style={{ "--v": String(props.used! / 100) }} />
        </div>
      </div>
    </Show>
  );
}

export function AccountsSheet(props: { repo: string; repoName: string; onChanged: () => void; onClose: () => void }) {
  const [data, { refetch }] = createResource(() => api.accounts(props.repo));
  const [busy, setBusy] = createSignal<string | null>(null);
  const here = () => data()?.accounts.find((a) => a.here)?.id;

  async function use(a: AccountView) {
    setBusy(a.id);
    try {
      await api.accountUse(props.repo, a.id);
      toast({ tone: "done", title: `${props.repoName} uses ${a.id}`, body: "From its next run. Runs already going keep their account." });
      await refetch();
      props.onChanged();
    } catch (e) {
      toast({ tone: "fail", title: "Not changed", body: msg(e) });
    } finally {
      setBusy(null);
    }
  }

  const copy = (text: string) => {
    void navigator.clipboard.writeText(text);
    toast({ tone: "done", title: "Copied", body: "Run it in a terminal." });
  };

  return (
    <Sheet
      title="Accounts"
      wide
      onClose={props.onClose}
      foot={
        <>
          <span class="sheet-note">{data()?.file}</span>
          <button type="button" class="btn btn-primary" onClick={props.onClose}>
            Done
          </button>
        </>
      }
    >
      <Show when={!data.loading} fallback={<p class="loading-line"><Spinner /> Asking Claude Code about each account</p>}>
        <Show when={!data.error} fallback={<ErrorLine>{msg(data.error)}</ErrorLine>}>
          <p class="accounts-rule">
            Switching between your subscription accounts is always yours: here or <code>pit account use</code>.
          </p>
          <ul class="accounts" role="radiogroup" aria-label={`Account for ${props.repoName}`}>
            <For each={data()?.accounts}>
              {(a) => (
                <li class="account" data-here={a.here ? "" : undefined}>
                  <label class="account-pick">
                    <Show when={a.kind !== "api-key"} fallback={<span class="account-radio-gap" aria-hidden="true" />}>
                      <input
                        type="radio"
                        class="check"
                        name="account"
                        aria-label={`Use ${a.id} for ${props.repoName}`}
                        checked={a.here}
                        disabled={busy() !== null}
                        onChange={() => void use(a)}
                      />
                    </Show>
                    <span class="account-name">
                      <b>{a.id}</b>
                      <span class="account-kind">
                        {kindLabel[a.kind]}
                        {a.id === "default" ? ", your normal Claude Code login" : ""}
                      </span>
                    </span>
                  </label>
                  <span class="account-state">
                    <Show
                      when={a.status.logged_in}
                      fallback={
                        <Show
                          when={!a.status.problem}
                          fallback={<span class="chip" data-state="queued">Unknown</span>}
                        >
                          <button type="button" class="btn account-signin" onClick={() => copy(`pit account login ${a.id}`)}>
                            <Icon name="copy" small />
                            pit account login {a.id}
                          </button>
                        </Show>
                      }
                    >
                      <span class="chip" data-state="done">
                        <Icon name="done" small />
                        {a.kind === "api-key" ? "Key set" : "Signed in"}
                      </span>
                    </Show>
                  </span>
                  <span class="account-usage">
                    <Show when={a.usage} fallback={<span class="account-hint">{a.kind === "api-key" ? "Only after you agree" : "No runs yet"}</span>}>
                      {(u) => (
                        <>
                          <Meter label="5h" used={u().five_hour} at={data()!.five_hour_at} />
                          <Meter label="7d" used={u().seven_day} at={data()!.seven_day_at} />
                        </>
                      )}
                    </Show>
                  </span>
                </li>
              )}
            </For>
          </ul>

          <div class="sheet-sub-row">
            <h3 class="sheet-sub">The route</h3>
            <span class="sheet-sub-note">
              under {data()?.five_hour_at}% of 5h and {data()?.seven_day_at}% of 7d
            </span>
          </div>
          <p class="side-hint">An API key always asks first.</p>
          <ol class="route">
            <For each={data()?.route}>{(r) => <li>{routeLabel(r, here())}</li>}</For>
          </ol>

          <h3 class="sheet-sub">Add an account</h3>
          <div class="install">
            <code class="selectable">{ADD}</code>
            <button type="button" class="btn" onClick={() => copy(ADD)}>
              <Icon name="copy" small />
              Copy
            </button>
          </div>
        </Show>
      </Show>
    </Sheet>
  );
}
