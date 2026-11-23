//! Cards: the board's cards, their questions, notes and images, and the project's context.

use pit_core::cards::{self, CardOut, CardPatch, CardState, NewCard};
use pit_core::default_homes;
use serde::Serialize;

use crate::daemon::daemon;
use crate::{CmdResult, blocking, open};

#[derive(Debug, Serialize)]
pub struct CardList {
    pub cards: Vec<CardOut>,
    /// Card files that did not parse, with why.
    pub problems: Vec<String>,
}

#[tauri::command]
pub(crate) async fn cards_list(path: String) -> CmdResult<CardList> {
    blocking(move || {
        let (all, problems) = cards::list(&open(&path)?)?;
        Ok(CardList { cards: all.into_iter().map(CardOut::from).collect(), problems })
    })
    .await
}

#[tauri::command]
pub(crate) async fn card_add(path: String, title: String, body: String) -> CmdResult<CardOut> {
    blocking(move || {
        let repo = open(&path)?;
        let card = cards::add(&repo, NewCard { title, body, ..Default::default() })?;
        describe_later(&repo.root, card.id, true);
        Ok(card.into())
    })
    .await
}

/// Asks the daemon for Claude's title (when `title`) and summary of a card, in the background:
/// the card is there at once, and the board shows them when they arrive.
fn describe_later(root: &std::path::Path, id: u32, title: bool) {
    let root = root.to_owned();
    std::thread::spawn(move || {
        if let Err(e) = daemon().and_then(|mut c| c.describe(&root, id, title)) {
            eprintln!("pitlane: no summary for card #{id}: {e:#}");
        }
    });
}

/// `lane`: a branch to link, an empty string to unlink, absent to leave as is.
#[tauri::command]
pub(crate) async fn card_update(
    path: String,
    id: u32,
    title: Option<String>,
    body: Option<String>,
    state: Option<String>,
    lane: Option<String>,
) -> CmdResult<CardOut> {
    blocking(move || {
        let patch = CardPatch {
            title,
            body,
            state: state.map(|s| CardState::parse(&s)).transpose()?,
            lane: lane.map(|l| Some(l).filter(|l| !l.trim().is_empty())),
            tags: None,
            ..Default::default()
        };
        let repo = open(&path)?;
        let before = cards::get(&repo, id)?.body;
        let card = cards::update(&repo, id, patch)?;
        // A new spec gets a new summary (the title stays yours).
        if card.body.trim() != before.trim() {
            describe_later(&repo.root, id, false);
        }
        Ok(card.into())
    })
    .await
}

/// Puts the cards in this order on the board.
#[tauri::command]
pub(crate) async fn card_reorder(path: String, ids: Vec<u32>) -> CmdResult<()> {
    blocking(move || cards::reorder(&open(&path)?, &ids)).await
}

/// Forgets what Claude learned on these cards (its notes, open questions, the analysis).
#[tauri::command]
pub(crate) async fn cards_forget(path: String, ids: Vec<u32>) -> CmdResult<cards::Forgot> {
    blocking(move || cards::forget(&open(&path)?, &ids)).await
}

/// Deletes a card (undo with `card_restore`).
#[tauri::command]
pub(crate) async fn card_delete(path: String, id: u32) -> CmdResult<()> {
    blocking(move || cards::delete(&open(&path)?, id).map(drop)).await
}

#[tauri::command]
pub(crate) async fn card_restore(path: String, id: u32) -> CmdResult<()> {
    blocking(move || cards::restore(&open(&path)?, id).map(drop)).await
}

/// A note you add to a card, for whoever works it next.
#[tauri::command]
pub(crate) async fn card_note(path: String, id: u32, text: String) -> CmdResult<CardOut> {
    blocking(move || Ok(cards::note(&open(&path)?, id, "you", &text)?.into())).await
}

/// What you chose for a blocked card (retry, review or backlog); the run it started, if any.
#[tauri::command]
pub(crate) async fn card_unblock(path: String, id: u32, action: String) -> CmdResult<Option<pit_core::runs::RunMeta>> {
    blocking(move || {
        let action = cards::Unblock::parse(&action)?;
        let root = open(&path)?.root;
        daemon()?.unblock(&root, id, action)
    })
    .await
}

/// Run all's analysis: questions and notes on each card not analysed yet.
#[tauri::command]
pub(crate) async fn cards_analyze(path: String, ids: Vec<u32>) -> CmdResult<serde_json::Value> {
    blocking(move || {
        let root = open(&path)?.root;
        daemon()?.analyze(&root, ids)
    })
    .await
}

#[derive(Debug, Serialize)]
pub struct AnswerOut {
    pub decision: String,
    pub card: CardOut,
}

#[tauri::command]
pub(crate) async fn card_answer(path: String, id: u32, question: u32, answer: String) -> CmdResult<AnswerOut> {
    blocking(move || {
        let repo = open(&path)?;
        let a = cards::answer(&repo, id, question, &answer)?;
        let card = pit_core::runs::recheck_after_answer(&repo, a.card);
        Ok(AnswerOut { decision: a.decision, card: card.into() })
    })
    .await
}

#[tauri::command]
pub(crate) async fn card_reanswer(path: String, id: u32, question: u32, answer: String) -> CmdResult<AnswerOut> {
    blocking(move || {
        let a = cards::reanswer(&open(&path)?, id, question, &answer)?;
        Ok(AnswerOut { decision: a.decision, card: a.card.into() })
    })
    .await
}

/// Talks a question over with Claude: your message goes on it, Claude's answer comes back with
/// the options as they stand now. Without a message, the last one is answered again.
#[tauri::command]
pub(crate) async fn card_talk(path: String, id: u32, question: u32, message: Option<String>) -> CmdResult<serde_json::Value> {
    blocking(move || {
        let root = open(&path)?.root;
        daemon()?.talk(&root, id, question, message)
    })
    .await
}

/// Stops the conversation going on a card's questions.
#[tauri::command]
pub(crate) async fn card_talk_stop(path: String, id: u32) -> CmdResult<usize> {
    blocking(move || {
        let root = open(&path)?.root;
        daemon()?.stop_talk(&root, id)
    })
    .await
}

/// The repositories this project also works in (`[project] repos`), as flow.toml lists them.
#[tauri::command]
pub(crate) async fn project_repos(path: String) -> CmdResult<Vec<String>> {
    blocking(move || Ok(open(&path)?.flow.project.repos.clone())).await
}

/// What the person wrote about the project for every session (`context.md` beside the board).
#[tauri::command]
pub(crate) async fn project_context(path: String) -> CmdResult<String> {
    blocking(move || Ok(cards::project_context(&open(&path)?))).await
}

#[tauri::command]
pub(crate) async fn project_context_set(path: String, text: String) -> CmdResult<String> {
    blocking(move || {
        let repo = open(&path)?;
        cards::set_project_context(&repo, &text)?;
        Ok(cards::project_context(&repo))
    })
    .await
}

#[tauri::command]
pub(crate) async fn project_repo_remove(path: String, rel: String) -> CmdResult<Vec<String>> {
    blocking(move || {
        pit_core::related::remove(&open(&path)?, &rel)?;
        Ok(open(&path)?.flow.project.repos.clone())
    })
    .await
}

/// Attaches an image (base64, as the webview read it) to a card.
#[tauri::command]
pub(crate) async fn card_attach(path: String, id: u32, name: String, data: String) -> CmdResult<CardOut> {
    blocking(move || {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).map_err(|e| anyhow::anyhow!("the image did not arrive whole: {e}"))?;
        Ok(cards::attach(&open(&path)?, id, &name, &bytes)?.into())
    })
    .await
}

#[tauri::command]
pub(crate) async fn card_detach(path: String, id: u32, file: String) -> CmdResult<CardOut> {
    blocking(move || Ok(cards::detach(&open(&path)?, id, &file)?.into())).await
}

/// One of a card's images as a data URL, for the sheet's thumbnails.
#[tauri::command]
pub(crate) async fn card_image(path: String, id: u32, file: String) -> CmdResult<String> {
    blocking(move || {
        if !pit_core::is_plain_name(&file) {
            anyhow::bail!("bad image name {file}");
        }
        let ty = cards::image_type(&file).ok_or_else(|| anyhow::anyhow!("{file} is not an image"))?;
        let bytes = std::fs::read(cards::images_dir(&open(&path)?, id).join(&file))?;
        Ok(data_url(ty, &bytes))
    })
    .await
}

/// A screenshot a run's tool gave back (`runs/<run>/shots/<file>`), as a data URL.
#[tauri::command]
pub(crate) async fn run_shot(run: String, file: String) -> CmdResult<String> {
    blocking(move || {
        if !pit_core::is_plain_name(&run) || !pit_core::is_plain_name(&file) {
            anyhow::bail!("bad screenshot name {run}/{file}");
        }
        let ty = cards::image_type(&file).ok_or_else(|| anyhow::anyhow!("{file} is not an image"))?;
        let bytes = std::fs::read(pit_core::runs::runs_dir(&default_homes().1).join(&run).join("shots").join(&file))?;
        Ok(data_url(ty, &bytes))
    })
    .await
}

/// Image bytes as a `data:` URL of type `ty`, for an <img> in the webview.
fn data_url(ty: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!("data:{ty};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}
