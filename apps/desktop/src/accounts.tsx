// Accounts: which login each run uses, whether it is signed in, how much of it is used, and the
// route the runs follow. Moving this repository between your subscription accounts is your hand,
// here, never Pitlane's.
import { For, Show, createResource, createSignal, onCleanup } from "solid-js";
import { api, type AccountKind, type AccountView } from "./api";
import { Icon } from "./icons";
import { locale, t } from "./i18n";
import { ErrorLine, Sheet, Spinner, toast, ok } from "./ui";
import { msg, timeOfDay } from "./format";


const ADD = "pit account add <name> && pit account login <name>";

const kindLabel: Record<AccountKind, string> = { subscription: "Subscription", team: "Team seat", "api-key": "API key" };

/** `opus@repo` → `opus on this repository's account`. */
function routeLabel(entry: string, here: string | undefined): string {
  const [model, account] = entry.split("@");
  const who = account === "repo" ? (here ? t("{here}, this repository's account", { here: here! }) : t("this repository's account")) : account;
  return t("{model} on {who}", { model: model || t("The default model"), who: who ?? "" });
}

/** A plan as people say it: `max_20x` → Max 20x. */
function planLabel(p: string | null | undefined): string | null {
  if (!p) return null;
  const m = /^([a-z]+)(?:_(\d+)x)?$/i.exec(p);
  if (!m) return p;
  const name = m[1]!.charAt(0).toUpperCase() + m[1]!.slice(1).toLowerCase();
  return m[2] ? `${name} ${m[2]}x` : name;
}

/** A usage window as Claude Code's /usage names it: what it covers, then its span. */
function windowLabel(w: string): { name: string; span: string } {
  if (w === "five_hour") return { name: t("Current session"), span: t("5-hour window") };
  const week = /^seven_day(?:_(.+))?$/.exec(w);
  if (week) {
    const rest = week[1];
    if (!rest) return { name: t("This week"), span: t("all models") };
    if (rest === "overage_included") return { name: t("This week"), span: t("with extra usage") };
    return { name: t("This week"), span: rest.charAt(0).toUpperCase() + rest.slice(1).replace(/_/g, " ") };
  }
  return { name: w.replace(/_/g, " "), span: "" };
}

/** When a window resets: the hour today or tomorrow with how long until it, else the day. */
function resetLabel(at: number, nowS: number): string {
  if (!at) return t("Reset time not reported yet");
  const d = new Date(at * 1000);
  const left = at - nowS;
  if (left <= 0) return t("Reset, fills again from zero");
  const time = timeOfDay(at);
  if (left < 86400) {
    const h = Math.floor(left / 3600);
    const m = Math.floor((left % 3600) / 60);
    return t("Resets at {time} (in {left})", { time, left: h > 0 ? `${h} h ${m} min` : `${m} min` });
  }
  const day = d.toLocaleDateString(locale(), { weekday: "short", day: "numeric", month: "short" });
  return t("Resets {day}, {time}", { day, time });
}

function UsageRow(props: { window: string; used: number; resets_at: number; at: number; now: number }) {
  const label = () => windowLabel(props.window);
  const level = () => (props.used >= 95 ? "out" : props.used >= props.at ? "high" : undefined);
  return (
    <div class="usage-row" data-level={level()}>
      <div class="usage-head">
        <span class="usage-name">
          {label().name}
          <Show when={label().span}>
            <span class="usage-span"> · {label().span}</span>
          </Show>
        </span>
        <b class="usage-figure">{t("{n}% used", { n: Math.round(props.used) })}</b>
      </div>
      <div class="usage-track">
        <div class="usage-fill" style={{ "--v": String(Math.min(props.used, 100) / 100) }} />
      </div>
      <span class="usage-reset">{resetLabel(props.resets_at, props.now)}</span>
    </div>
  );
}

const OVERAGE_WHY: Record<string, string> = {
  org_level_disabled: "turned off for your organization",
  out_of_credits: "no credits left",
  member_level_disabled: "turned off for your seat",
};

/** Extra usage past the plan, in a word and why. */
function overageLabel(o: NonNullable<AccountView["overage"]>): { text: string; tone: "done" | "needs" | "queued" } {
  if (o.using) return { text: t("In use now: runs spend extra usage"), tone: "needs" };
  if (o.status === "allowed" || o.status === "allowed_warning") return { text: t("Available when the plan runs out"), tone: "done" };
  const why = o.reason ? t(OVERAGE_WHY[o.reason] ?? o.reason.replace(/_/g, " ")) : null;
  return { text: why ? t("Off: {why}", { why }) : t("Off"), tone: "queued" };
}

export function AccountsSheet(props: { repo: string; repoName: string; onChanged: () => void; onClose: () => void }) {
  const [data, { refetch }] = createResource(() => api.accounts(props.repo));
  const [busy, setBusy] = createSignal<string | null>(null);
  const here = () => ok(data)?.accounts.find((a) => a.here)?.id;
  // Resets count down while the sheet is open.
  const [nowS, setNowS] = createSignal(Math.floor(Date.now() / 1000));
  const tick = setInterval(() => setNowS(Math.floor(Date.now() / 1000)), 30_000);
  onCleanup(() => clearInterval(tick));

  async function use(a: AccountView) {
    setBusy(a.id);
    try {
      await api.accountUse(props.repo, a.id);
      toast({ tone: "done", title: t("{repo} uses {id}", { repo: props.repoName, id: a.id }), body: t("From its next run. Runs already going keep their account.") });
      await refetch();
      props.onChanged();
    } catch (e) {
      toast({ tone: "fail", title: t("Not changed"), body: msg(e) });
    } finally {
      setBusy(null);
    }
  }

  const copy = (text: string) => {
    void navigator.clipboard.writeText(text);
    toast({ tone: "done", title: t("Copied"), body: t("Run it in a terminal.") });
  };

  return (
    <Sheet
      title={t("Accounts")}
      wide
      onClose={props.onClose}
      foot={
        <>
          <span class="sheet-note">{ok(data)?.file}</span>
          <button type="button" class="btn btn-primary" onClick={props.onClose}>
            {t("Done")}
          </button>
        </>
      }
    >
      <Show when={!data.loading} fallback={<p class="loading-line"><Spinner /> {t("Asking Claude Code about each account")}</p>}>
        <Show when={!data.error} fallback={<ErrorLine>{msg(data.error)}</ErrorLine>}>
          <ul class="accounts" role="radiogroup" aria-label={t("Account for {repo}", { repo: props.repoName })}>
            <For each={ok(data)?.accounts}>
              {(a) => (
                <li class="account-card" data-here={a.here ? "" : undefined}>
                  <div class="account-top">
                    <label class="account-pick">
                      <Show when={a.kind !== "api-key"} fallback={<span class="account-radio-gap" aria-hidden="true" />}>
                        <input
                          type="radio"
                          class="check"
                          name="account"
                          aria-label={t("Use {id} for {repo}", { id: a.id, repo: props.repoName })}
                          checked={a.here}
                          disabled={busy() !== null}
                          onChange={() => void use(a)}
                        />
                      </Show>
                      <span class="account-name">
                        <span class="account-title">
                          <b>{a.id}</b>
                          <span class="account-kind">{t(kindLabel[a.kind])}</span>
                          <Show when={planLabel(a.status.plan)}>{(p) => <span class="account-plan">{p()}</span>}</Show>
                          <Show when={a.here}>
                            <span class="account-here">{t("This repository's account")}</span>
                          </Show>
                        </span>
                        <span class="account-who">
                          <Show when={a.status.email} fallback={a.id === "default" ? t("Your normal Claude Code login") : a.kind === "api-key" ? t("Paid per token, always asks first") : ""}>
                            <span class="selectable">{a.status.email}</span>
                          </Show>
                          <Show when={a.status.org && !a.status.org.startsWith(`${a.status.email ?? "\u0000"}'s`)}>
                            <span> · {a.status.org}</span>
                          </Show>
                        </span>
                      </span>
                    </label>
                    <span class="account-state">
                      <Show
                        when={a.status.logged_in}
                        fallback={
                          <Show when={!a.status.problem} fallback={<span class="chip" data-state="queued" title={a.status.problem ?? undefined}>{t("Unknown")}</span>}>
                            <button type="button" class="btn account-signin" onClick={() => copy(`pit account login ${a.id}`)}>
                              <Icon name="copy" small />
                              pit account login {a.id}
                            </button>
                          </Show>
                        }
                      >
                        <span class="chip" data-state="done">
                          <Icon name="done" small />
                          {a.kind === "api-key" ? t("Key set") : t("Signed in")}
                        </span>
                      </Show>
                    </span>
                  </div>

                  <Show
                    when={(a.windows ?? []).length > 0}
                    fallback={
                      <Show when={a.kind !== "api-key"}>
                        <p class="account-hint usage-empty">{t("Usage shows here after this account's first run: Claude Code reports it as it works.")}</p>
                      </Show>
                    }
                  >
                    <div class="usage">
                      <For each={a.windows}>
                        {(w) => <UsageRow window={w.window} used={w.used} resets_at={w.resets_at} at={w.window === "five_hour" ? ok(data)!.five_hour_at : ok(data)!.seven_day_at} now={nowS()} />}
                      </For>
                    </div>
                  </Show>

                  <Show when={a.kind !== "api-key" && (a.overage || a.status.login_expires_at)}>
                    <dl class="account-facts">
                      <Show when={a.overage}>
                        {(o) => (
                          <div>
                            <dt>{t("Extra usage")}</dt>
                            <dd data-tone={overageLabel(o()).tone}>{overageLabel(o()).text}</dd>
                          </div>
                        )}
                      </Show>
                      <Show when={a.status.login_expires_at}>
                        {(at) => (
                          <div>
                            <dt>{t("Login")}</dt>
                            <dd>{t("Renews by {date}", { date: new Date(at()).toLocaleDateString(locale(), { day: "numeric", month: "long" }) })}</dd>
                          </div>
                        )}
                      </Show>
                    </dl>
                  </Show>
                </li>
              )}
            </For>
          </ul>
          <p class="sheet-note usage-note">{t("Figures are what Claude Code reported during this account's last run, so they can trail what /usage shows by a few minutes.")}</p>

          <p class="accounts-rule">
            {t("Switching between your subscription accounts is always yours: here or")} <code>pit account use</code>.
          </p>
          <div class="sheet-sub-row">
            <h3 class="sheet-sub">{t("The route")}</h3>
            <span class="sheet-sub-note">
              {t("under {a}% of 5h and {b}% of 7d", { a: ok(data)?.five_hour_at ?? "", b: ok(data)?.seven_day_at ?? "" })}
            </span>
          </div>
          <p class="side-hint">{t("An API key always asks first.")}</p>
          <ol class="route">
            <For each={ok(data)?.route}>{(r) => <li>{routeLabel(r, here())}</li>}</For>
          </ol>

          <h3 class="sheet-sub">{t("Add an account")}</h3>
          <div class="install">
            <code class="selectable">{ADD}</code>
            <button type="button" class="btn" onClick={() => copy(ADD)}>
              <Icon name="copy" small />
              {t("Copy")}
            </button>
          </div>
        </Show>
      </Show>
    </Sheet>
  );
}
