// What you do to a card from the Board or from its sheet: forget what Claude learned about it,
// unblock it, delete it.
import { Show, createSignal } from "solid-js";
import { api, type Card } from "./api";
import type { UnblockAction } from "./cards";
import { Icon } from "./icons";
import { Sheet, Spinner, toast } from "./ui";
import { plural, t } from "./i18n";
import { msg } from "./format";

/** The card carries something of Claude's: the analysis mark, its notes or the judge's, or open questions. */
export const hasMemory = (c: Card) =>
  !!c.analyzed || (c.notes ?? []).some((n) => n.by !== "you") || (c.questions ?? []).some((q) => q.answer == null) || (c.after?.length ?? 0) > 0;

/**
 * Forget what Claude learned, to analyse again from scratch: its notes and the judge's, the open
 * questions, the analysis mark and the order. Says exactly what goes and what stays, and needs a
 * deliberate confirm (no Enter-to-confirm, the safe button has the focus).
 */
export function ForgetSheet(props: { repo: string; cards: Card[]; onDone: () => void; onClose: () => void }) {
  const [busy, setBusy] = createSignal(false);
  const notes = () => props.cards.reduce((n, c) => n + (c.notes ?? []).filter((x) => x.by !== "you").length, 0);
  const open = () => props.cards.reduce((n, c) => n + (c.questions ?? []).filter((q) => q.answer == null).length, 0);
  const one = () => props.cards.length === 1;
  const go = async () => {
    setBusy(true);
    try {
      const r = await api.cardsForget(props.repo, props.cards.map((c) => c.id));
      props.onDone();
      toast({ tone: "done", title: one() ? t("#{id} is back to not analysed", { id: props.cards[0]!.id }) : t("{n} cards are back to not analysed", { n: props.cards.length }), body: t("Forgot {notes} and {questions}.", { notes: plural(r.notes, "{n} note", "{n} notes"), questions: plural(r.questions, "{n} open question", "{n} open questions") }) });
      props.onClose();
    } catch (e) {
      toast({ tone: "fail", title: t("Nothing was forgotten"), body: e instanceof Error ? e.message : String(e) });
      setBusy(false);
    }
  };
  return (
    <Sheet
      title={one() ? t("Forget the analysis of #{id}", { id: props.cards[0]!.id }) : t("Forget the analysis of {n} cards", { n: props.cards.length })}
      onClose={props.onClose}
      foot={
        <>
          <button type="button" class="btn btn-ghost" autofocus onClick={props.onClose} disabled={busy()}>
            {t("Keep it")}
          </button>
          <button type="button" class="btn btn-danger" aria-busy={busy()} disabled={busy()} onClick={() => void go()}>
            <Show when={busy()} fallback={<Icon name="undo" small />}>
              <Spinner />
            </Show>
            {one() ? t("Forget it") : t("Forget all")}
          </button>
        </>
      }
    >
      <p>
        {one() ? t("Claude starts over on this card: the next Analyse reads it from scratch.") : t("Claude starts over on these cards: the next Analyse reads them from scratch.")}
      </p>
      <ul class="forget-list">
        <li>
          <b>{t("Goes:")}</b>{" "}
          {t("{notes}, {questions}, the analysis mark and the order it worked out.", {
            notes: plural(notes(), "{n} note by Claude and the judge", "{n} notes by Claude and the judge"),
            questions: plural(open(), "{n} unanswered question", "{n} unanswered questions"),
          })}
        </li>
        <li>
          <b>{t("Stays:")}</b> {t("the spec, your own notes, your answers to its questions, and any lane or run.")}
        </li>
      </ul>
      <p class="field-help">{t("This cannot be undone.")}</p>
    </Sheet>
  );
}

/** Carries out what you chose for a blocked card, and says what happened. */
export async function unblockCard(repo: string, card: Card, action: UnblockAction, onChanged: () => void): Promise<void> {
  try {
    const run = await api.unblockCard(repo, card.id, action);
    const title =
      action === "backlog" ? t("#{id} back in Backlog", { id: card.id })
      : action === "review" ? t("#{id} is ready for your review", { id: card.id })
      : run ? t("#{id} runs again", { id: card.id })
      : t("Reading #{id} again", { id: card.id });
    toast({ tone: "done", title });
    onChanged();
  } catch (e) {
    toast({ tone: "fail", title: t("Could not move #{id}", { id: card.id }), body: msg(e) });
  }
}

/** Deletes a card, with an undo in the toast (the file waits in the board's deleted folder). */
export async function deleteCard(repo: string, card: Card, onChanged: () => void): Promise<boolean> {
  try {
    await api.cardDelete(repo, card.id);
    onChanged();
    toast(
      {
        tone: "done",
        title: t("Deleted #{id} {title}", { id: card.id, title: card.title }),
        body: card.lane ? t("Its lane {lane} stays; clean it from Lanes if you do not need it.", { lane: card.lane }) : undefined,
        action: {
          label: t("Undo"),
          run: () =>
            void api.cardRestore(repo, card.id).then(onChanged, (e) => toast({ tone: "fail", title: t("Could not restore #{id}", { id: card.id }), body: e instanceof Error ? e.message : String(e) })),
        },
      },
      12000,
    );
    return true;
  } catch (e) {
    toast({ tone: "fail", title: t("Could not delete #{id}", { id: card.id }), body: e instanceof Error ? e.message : String(e) });
    return false;
  }
}
