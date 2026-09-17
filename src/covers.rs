//! Cover art from SteamGridDB, placed where Lutris looks for it.
//!
//! Lutris resolves a game's cover as `<data dir>/coverart/<slug>.jpg` or
//! `<slug>.png` -- see its `settings.py` (`COVERART_PATH`) and
//! `services/service_media.py` (the `["%s.jpg", "%s.png"]` file patterns and
//! `resolve_media_path`, which prefers `.jpg`). So a cover only has to be
//! copied in under the right name; nothing in Lutris's database is touched.
//!
//! The network call is `curl`, run as a subprocess, so the core crate keeps no
//! HTTP client and no TLS stack. That is the same trade already made for `ps`,
//! `df` and Lutris itself.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::json::{self, Json};
use crate::lutris::{self, Installation};

/// SteamGridDB's documented API root.
const API: &str = "https://www.steamgriddb.com/api/v2";

/// Vertical cover art: the shape a Lutris grid shows.
pub const COVER_DIMENSIONS: &str = "600x900";

/// How many search hits are kept. Enough to choose from; few enough that asking
/// for a preview of each does not spend the whole rate limit.
pub const MAX_MATCHES: usize = 8;

/// A game Lutris knows about, and whether it already has a cover.
#[derive(Debug, Clone)]
pub struct Game {
    pub slug: String,
    pub name: String,
    pub cover: Option<PathBuf>,
}

/// One SteamGridDB search result.
#[derive(Debug, Clone)]
pub struct Match {
    pub id: u64,
    pub name: String,
    /// Preview URL, filled in only when the caller asks for it.
    pub thumb: Option<String>,
}

/// One piece of art to download.
#[derive(Debug, Clone)]
pub struct Art {
    pub url: String,
    pub thumb: Option<String>,
}

// ------------------------------------------------------------------- the key

/// Where a key saved by the window lives. A person using the command line can
/// write the same file themselves, or use `--apikey`, or `SGDB_API_KEY`.
pub fn key_path() -> PathBuf {
    crate::state::app_data_dir().join("sgdb-api-key")
}

/// The API key, in order of specificity: an explicit file, then the
/// environment, then the saved file. `None` when there is none anywhere.
pub fn read_key(explicit: Option<&Path>) -> Option<String> {
    if let Some(path) = explicit {
        return read_key_file(path);
    }
    if let Ok(key) = std::env::var("SGDB_API_KEY") {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Some(key);
        }
    }
    read_key_file(&key_path())
}

fn read_key_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().and_then(|text| {
        let key = text.trim().to_string();
        (!key.is_empty()).then_some(key)
    })
}

// ---------------------------------------------------------------- the covers

/// Where Lutris keeps the covers for this installation. Lutris 0.5+ puts its
/// data directory under the data home; for a Flatpak install that is inside the
/// app's own data directory, which is what `config_dir` already tracks.
pub fn cover_dir(installation: &Installation) -> PathBuf {
    installation.config_dir.join("coverart")
}

/// The cover Lutris would already use for a slug, if there is one. `.jpg` is
/// preferred because that is the order Lutris itself resolves them in.
pub fn existing(installation: &Installation, slug: &str) -> Option<PathBuf> {
    let dir = cover_dir(installation);
    ["jpg", "png"]
        .iter()
        .map(|extension| dir.join(format!("{slug}.{extension}")))
        .find(|path| path.is_file())
}

/// Every game Lutris knows about, with its current cover.
///
/// The list comes from Lutris itself, so it is exactly what Lutris shows: no
/// duplicate entries and no config leftovers. The per-game files are the
/// fallback for when the command cannot be run.
pub fn games(installation: &Installation) -> Vec<Game> {
    let listed: Vec<(String, String)> = match installation.list_games() {
        Some(games) => games,
        None => lutris::existing_entries(installation)
            .into_iter()
            .map(|entry| (entry.slug, entry.name))
            .collect(),
    };
    let mut games: Vec<Game> = listed
        .into_iter()
        .map(|(slug, name)| Game {
            cover: existing(installation, &slug),
            slug,
            name,
        })
        .collect();
    games.sort_by(|a, b| a.slug.cmp(&b.slug));
    games.dedup_by(|a, b| a.slug == b.slug);
    games
}

/// Copy a cover into place under the slug Lutris knows the game by.
///
/// Always written as `.jpg`: Lutris reads the file by content, so a PNG served
/// for a `.jpg` name still displays, and one fixed name means a download can
/// never leave an older copy under the other extension shadowing the new one.
/// The bytes land under a temporary name first, so a download that fails part
/// way leaves nothing where Lutris would show a broken image.
pub fn place(installation: &Installation, slug: &str, art: &Art) -> Result<PathBuf, String> {
    let dir = cover_dir(installation);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let destination = dir.join(format!("{slug}.jpg"));
    let temporary = dir.join(format!(".{slug}.jpg.{}.part", std::process::id()));
    if let Err(error) = download(&art.url, &temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    fs::rename(&temporary, &destination).map_err(|e| {
        let _ = fs::remove_file(&temporary);
        format!("{}: {e}", destination.display())
    })?;
    Ok(destination)
}

// -------------------------------------------------------------- the matching

/// The candidate that best matches a game's name: an exact match, then one that
/// starts with it, then the first hit. The script this is adapted from always
/// took the first hit, which is how "AC Black Flag Resynced" became an AC/DC
/// record. The window shows the candidates so a wrong guess can be corrected;
/// this only decides what is tried first.
pub fn best_match<'a>(found: &'a [Match], name: &str) -> Option<&'a Match> {
    let wanted = name.trim().to_lowercase();
    found
        .iter()
        .find(|candidate| candidate.name.to_lowercase() == wanted)
        .or_else(|| {
            found
                .iter()
                .find(|candidate| candidate.name.to_lowercase().starts_with(&wanted))
        })
        .or_else(|| found.first())
}

// ------------------------------------------------------------- the API calls

/// Search by name. The endpoint is the one the script used; only the caller's
/// search text differs (the display name rather than the slug).
pub fn search(key: &str, query: &str) -> Result<Vec<Match>, String> {
    let url = format!("{API}/search/autocomplete/{}", urlencode(query));
    let parsed = get_json(key, &url)?;
    let mut found = Vec::new();
    for item in parsed.get("data").map(Json::as_array).unwrap_or_default() {
        if let (Some(id), Some(name)) = (item.get("id").and_then(Json::as_u64), item.string("name"))
        {
            found.push(Match {
                id,
                name,
                thumb: None,
            });
        }
    }
    Ok(found)
}

/// The first vertical cover SteamGridDB has for a game, if any.
pub fn art(key: &str, game_id: u64) -> Result<Option<Art>, String> {
    let url = format!("{API}/grids/game/{game_id}?dimensions={COVER_DIMENSIONS}");
    let parsed = get_json(key, &url)?;
    let data = parsed.get("data").map(Json::as_array).unwrap_or_default();
    let Some(first) = data.first() else {
        return Ok(None);
    };
    let Some(url) = first.string("url") else {
        return Ok(None);
    };
    Ok(Some(Art {
        url,
        thumb: first.string("thumb"),
    }))
}

/// Fill in a preview URL for the first few matches. Bounded and best-effort: a
/// missing thumbnail is not worth failing the whole search over, and the pause
/// keeps the calls inside SteamGridDB's rate limit.
pub fn with_thumbnails(key: &str, found: &mut [Match]) {
    for candidate in found.iter_mut().take(MAX_MATCHES) {
        if let Ok(Some(art)) = art(key, candidate.id) {
            candidate.thumb = art.thumb;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn get_json(key: &str, url: &str) -> Result<Json, String> {
    let body = run_curl(&[
        "-H".to_string(),
        format!("Authorization: Bearer {key}"),
        url.to_string(),
    ])?;
    let text = String::from_utf8_lossy(&body);
    let parsed = json::parse(text.trim())
        .map_err(|e| format!("SteamGridDB returned a response this tool could not read: {e}"))?;
    if parsed.get("success").and_then(Json::as_bool) == Some(false) {
        let errors = parsed.get("errors").map(Json::as_array).unwrap_or_default();
        let detail: Vec<&str> = errors.iter().filter_map(Json::as_str).collect();
        return Err(if detail.is_empty() {
            "SteamGridDB refused the request (a wrong or expired key does this)".to_string()
        } else {
            format!("SteamGridDB refused the request: {}", detail.join("; "))
        });
    }
    Ok(parsed)
}

/// Download straight to a file, following redirects. Written to disk by curl
/// rather than held in memory, because cover files are megabytes.
pub fn download(url: &str, destination: &Path) -> Result<(), String> {
    run_curl(&[
        "-L".to_string(),
        "-o".to_string(),
        destination.to_string_lossy().to_string(),
        url.to_string(),
    ])?;
    Ok(())
}

fn run_curl(arguments: &[String]) -> Result<Vec<u8>, String> {
    let output = Command::new("curl")
        .args(["-sS", "--fail-with-body"])
        .args(arguments)
        .output()
        .map_err(|e| {
            format!(
                "could not run curl ({e}). Covers are fetched with curl; install it and try again."
            )
        })?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!(
                "curl exited with status {}",
                output.status.code().unwrap_or(-1)
            )
        } else {
            stderr
        })
    }
}

/// Percent-encode a value for a URL path segment. Enough for a search term:
/// letters, digits and the RFC 3986 unreserved marks pass through.
fn urlencode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: u64, name: &str) -> Match {
        Match {
            id,
            name: name.to_string(),
            thumb: None,
        }
    }

    #[test]
    fn search_terms_are_url_encoded() {
        assert_eq!(
            urlencode("Assassin's Creed IV"),
            "Assassin%27s%20Creed%20IV"
        );
        assert_eq!(urlencode("AC/DC"), "AC%2FDC");
        assert_eq!(urlencode("Portal 2"), "Portal%202");
        assert_eq!(urlencode("Half-Life_2.0~"), "Half-Life_2.0~");
    }

    #[test]
    fn the_best_match_prefers_an_exact_name_then_a_prefix() {
        let found = vec![
            candidate(1, "AC/DC Live: Rock Band"),
            candidate(2, "Assassin's Creed IV: Black Flag"),
        ];
        // An exact name wins even though it is not first.
        assert_eq!(
            best_match(&found, "Assassin's Creed IV: Black Flag")
                .unwrap()
                .id,
            2
        );
        // Case does not matter.
        assert_eq!(
            best_match(&found, "assassin's creed iv: black flag")
                .unwrap()
                .id,
            2
        );
        // A prefix is better than nothing.
        assert_eq!(best_match(&found, "Assassin's").unwrap().id, 2);
        // A name that matches nothing falls back to the first hit, as before.
        assert_eq!(best_match(&found, "Black Flag").unwrap().id, 1);
        assert!(best_match(&[], "Anything").is_none());
    }
}
