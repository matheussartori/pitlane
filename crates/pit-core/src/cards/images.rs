//! Images attached to a card, in its own folder beside the board.

use super::*;

/// Where a card's images live, beside the cards (not in the repository).
pub fn images_dir(repo: &Repo, id: u32) -> PathBuf {
    repo.board.join("images").join(id.to_string())
}

/// The media type of an image file Pitlane accepts, from its name.
pub fn image_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// The largest image a card takes; the Claude API refuses bigger ones.
pub const IMAGE_MAX: usize = 5 * 1024 * 1024;

/// Attaches an image to the card under a fresh name built from `name`; returns the card.
pub fn attach(repo: &Repo, id: u32, name: &str, bytes: &[u8]) -> Result<Card> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if image_type(name).is_none() {
        bail!("{name} is not a PNG, JPEG, GIF or WebP image");
    }
    if bytes.len() > IMAGE_MAX {
        bail!("{name} is over 5 MB");
    }
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    let stem = slug(name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name));
    let stem = if stem.is_empty() { "image".to_owned() } else { stem };
    let n = (1..).find(|n| !card.images.iter().any(|i| i.starts_with(&format!("{n}-")))).unwrap_or(1);
    let file = format!("{n}-{stem}.{ext}");
    let dir = images_dir(repo, id);
    fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    fs::write(dir.join(&file), bytes).with_context(|| format!("could not write {file}"))?;
    card.images.push(file);
    card.updated = timestamp(now_secs());
    write(&card)?;
    Ok(card)
}

/// Takes an image off the card and deletes its file.
pub fn detach(repo: &Repo, id: u32, file: &str) -> Result<Card> {
    if !crate::is_plain_name(file) {
        bail!("bad image name {file}");
    }
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    let before = card.images.len();
    card.images.retain(|i| i != file);
    if card.images.len() == before {
        bail!("card #{id} has no image {file}");
    }
    let _ = fs::remove_file(images_dir(repo, id).join(file));
    card.updated = timestamp(now_secs());
    write(&card)?;
    Ok(card)
}

/// The card's images that are still on disk: (media type, bytes).
pub fn image_bytes(repo: &Repo, card: &Card) -> Vec<(&'static str, Vec<u8>)> {
    let dir = images_dir(repo, card.id);
    card.images
        .iter()
        .filter(|f| crate::is_plain_name(f))
        .filter_map(|f| Some((image_type(f)?, fs::read(dir.join(f)).ok()?)))
        .collect()
}
