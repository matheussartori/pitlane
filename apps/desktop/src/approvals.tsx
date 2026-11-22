// What an agent is waiting on you to allow, and the four ways to answer.
import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { api, type Approval, type Decision } from "./api";
import { t } from "./i18n";
import { ErrorLine, Spinner, toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

/** `Bash(npm test:*)` → `npm test`; a tool rule is the tool's name. */
export function ruleLabel(rule: string): string {
  const m = /^Bash\((.*?)(?::\*)?\)$/.exec(rule);
  return m ? m[1]! : rule;
}

export function ApprovalBlock(props: {
  run: string;
  approval: Approval;
  /** Answer with 1 to 4 from the keyboard (one block per screen holds the keys). */
  keys?: boolean;
}) {
  const [busy, setBusy] = createSignal(false);
  const [denying, setDenying] = createSignal(false);
  const [reason, setReason] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  let reasonInput: HTMLInputElement | undefined;
  const label = () => ruleLabel(props.approval.rule);

  async function decide(d: Decision) {
    if (busy()) return;
    setBusy(true);
    setError(null);
    try {
      await api.runDecide(props.run, props.approval.id, d, d === "deny" ? reason().trim() || undefined : undefined);
      const said: Record<Decision, string> = {
        once: t("Allowed once"),
        run: t("Allowed {rule} for this run", { rule: label() }),
        always: t("Allowed {rule} in this repository", { rule: label() }),
        deny: t("Denied"),
      };
      toast({ tone: "done", title: said[d], body: d === "always" ? t("Saved to [run] allow in .pitlane/flow.toml.") : undefined });
    } catch (e) {
      setError(msg(e));
      setBusy(false);
    }
  }

  function startDeny() {
    setDenying(true);
    queueMicrotask(() => reasonInput?.focus());
  }

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!props.keys || e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      if (document.querySelector("dialog[open]")) return;
      const pick: Record<string, () => void> = {
        "1": () => void decide("once"),
        "2": () => void decide("run"),
        "3": () => void decide("always"),
        "4": startDeny,
      };
      const f = pick[e.key];
      if (f) {
        e.preventDefault();
        f();
      }
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <section class="approval" aria-label={t("Waiting for your answer")}>
      <p class="approval-ask">
        {props.approval.tool === "Bash" ? t("The agent wants to run") : t("The agent wants to use {tool}", { tool: props.approval.tool })}
      </p>
      <pre class="approval-cmd selectable">{props.approval.detail || props.approval.tool}</pre>
      <p class="approval-why">
        {t("No rule allows")} <code>{label()}</code> {t("yet, so the agent waits for you.")}
      </p>
      <div class="options" role="group" aria-label={t("Your answer")}>
        <button type="button" class="option" disabled={busy()} onClick={() => void decide("once")}>
          <Show when={props.keys}>
            <span class="kbd">1</span>
          </Show>
          <span>{t("Allow once")}</span>
        </button>
        <button type="button" class="option" disabled={busy()} onClick={() => void decide("run")}>
          <Show when={props.keys}>
            <span class="kbd">2</span>
          </Show>
          <span>
            {t("Allow")} <code>{label()}</code> {t("for this run")}
          </span>
        </button>
        <button type="button" class="option" disabled={busy()} onClick={() => void decide("always")}>
          <Show when={props.keys}>
            <span class="kbd">3</span>
          </Show>
          <span>
            {t("Always allow")} <code>{label()}</code> {t("in this repository")}
          </span>
          <small>{t("Saves to .pitlane/flow.toml")}</small>
        </button>
        <Show
          when={denying()}
          fallback={
            <button type="button" class="option option-deny" disabled={busy()} onClick={startDeny}>
              <Show when={props.keys}>
                <span class="kbd">4</span>
              </Show>
              <span>{t("Deny")}</span>
            </button>
          }
        >
          <form
            class="other"
            onSubmit={(e) => {
              e.preventDefault();
              void decide("deny");
            }}
          >
            <label class="visually-hidden" for={`deny-${props.approval.id}`}>
              {t("Why")}
            </label>
            <input
              ref={reasonInput}
              id={`deny-${props.approval.id}`}
              class="input"
              placeholder={t("Why, so the agent can find another way (optional)")}
              autocomplete="off"
              value={reason()}
              disabled={busy()}
              onInput={(e) => setReason(e.currentTarget.value)}
              onKeyDown={(e) => e.key === "Escape" && setDenying(false)}
            />
            <button type="submit" class="btn btn-danger" disabled={busy()}>
              {t("Deny")}
            </button>
          </form>
        </Show>
      </div>
      <Show when={busy()}>
        <p class="loading-line">
          <Spinner /> {t("Telling the agent")}
        </p>
      </Show>
      <Show when={error()}>
        <ErrorLine>{error()}</ErrorLine>
      </Show>
    </section>
  );
}
