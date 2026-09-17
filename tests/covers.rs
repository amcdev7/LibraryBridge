//! Tests for `lutris covers`. Nothing here talks to SteamGridDB: `curl` on the
//! fixture's PATH is a script that answers with canned JSON and writes a marker
//! when it is asked to download.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_librarybridge");
const KEY: &str = "test-key";

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    lutris: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "librarybridge-covers-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let bin = root.join("bin");
        let lutris = home.join(".local/share/lutris");
        fs::create_dir_all(lutris.join("games")).unwrap();
        fs::create_dir_all(lutris.join("coverart")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let root = root.canonicalize().unwrap();
        let home = root.join("home");
        let bin = root.join("bin");
        let lutris = home.join(".local/share/lutris");
        // Keep the machine's real Lutris out of these tests. The fake exits
        // non-zero, so the tool falls back to the fixture's own config files
        // instead of listing the real library.
        let stub = bin.join("lutris");
        fs::write(&stub, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        Fixture {
            root,
            home,
            bin,
            lutris,
        }
    }

    /// A Lutris entry, as Lutris writes one: the display name and the slug the
    /// cover is looked up by.
    fn game(&self, slug: &str, name: &str) {
        let path = self
            .lutris
            .join("games")
            .join(format!("{slug}-1700000000.yml"));
        fs::write(
            &path,
            format!("name: '{name}'\ngame_slug: {slug}\nversion: test\nrunner: wine\n"),
        )
        .unwrap();
    }

    fn cover(&self, slug: &str) {
        fs::write(
            self.lutris.join("coverart").join(format!("{slug}.jpg")),
            b"already here",
        )
        .unwrap();
    }

    /// The fake `curl`. It reads its canned answers from the fixture root, which
    /// it finds relative to itself, so no path has to be interpolated in.
    fn fake_curl(&self) {
        let script = r#"#!/bin/sh
base=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
url=""
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
  url="$arg"
done
case "$url" in
  *search/autocomplete*) cat "$base/search.json" ;;
  *grids/game/*)
    id=$(printf '%s' "$url" | sed -n 's#.*grids/game/\([0-9]*\).*#\1#p')
    if [ -f "$base/grids-$id.json" ]; then cat "$base/grids-$id.json"; else echo '{"success":true,"data":[]}'; fi
    ;;
  *)
    if [ -n "$out" ]; then
      printf '%s' "$url" > "$base/downloaded-url.txt"
      printf 'JPEG-BYTES' > "$out"
      exit 0
    fi
    echo '{"success":true,"data":[]}'
    ;;
esac
"#;
        let path = self.bin.join("curl");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn write(&self, relative: &str, contents: &str) {
        fs::write(self.root.join(relative), contents).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args, false)
    }

    fn run_keyed(&self, args: &[&str]) -> Output {
        self.command(args, true)
    }

    fn command(&self, args: &[&str], with_key: bool) -> Output {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            // Keep the standard tools, and put the fake curl first.
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env_remove("SGDB_API_KEY");
        if with_key {
            command.env("SGDB_API_KEY", KEY);
        }
        command.output().expect("failed to run librarybridge")
    }

    fn downloaded_url(&self) -> Option<String> {
        fs::read_to_string(self.root.join("downloaded-url.txt"))
            .ok()
            .map(|text| text.trim().to_string())
    }

    fn cover_exists(&self, slug: &str) -> bool {
        self.lutris
            .join("coverart")
            .join(format!("{slug}.jpg"))
            .is_file()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn listing_needs_no_key_and_reports_only_what_is_missing() {
    let fixture = Fixture::new("list");
    fixture.game("alpha", "Alpha Game");
    fixture.game("beta", "Beta Game");
    fixture.cover("beta");

    let output = fixture.run(&["lutris", "covers", "--list", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("\"slug\": \"alpha\""), "{text}");
    assert!(!text.contains("\"slug\": \"beta\""), "{text}");
}

#[test]
fn a_missing_cover_is_downloaded_under_the_game_slug() {
    let fixture = Fixture::new("download");
    fixture.fake_curl();
    fixture.game("alpha", "Alpha Game");
    fixture.write(
        "search.json",
        r#"{"success":true,"data":[{"id":10,"name":"Alpha Game"}]}"#,
    );
    fixture.write(
        "grids-10.json",
        r#"{"success":true,"data":[{"url":"https://cdn.example/alpha.jpg","thumb":"https://cdn.example/alpha-thumb.jpg","mime":"image/jpeg"}]}"#,
    );

    let output = fixture.run_keyed(&["lutris", "covers", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        fixture.downloaded_url().as_deref(),
        Some("https://cdn.example/alpha.jpg")
    );
    assert!(fixture.cover_exists("alpha"));
    let bytes = fs::read(fixture.lutris.join("coverart/alpha.jpg")).unwrap();
    assert_eq!(bytes, b"JPEG-BYTES");
}

#[test]
fn an_exact_name_match_wins_over_the_first_hit() {
    let fixture = Fixture::new("match");
    fixture.fake_curl();
    fixture.game("delta", "Delta");
    // The first hit is a different game; the exact name is second.
    fixture.write(
        "search.json",
        r#"{"success":true,"data":[{"id":20,"name":"Delta Force"},{"id":21,"name":"Delta"}]}"#,
    );
    fixture.write(
        "grids-20.json",
        r#"{"success":true,"data":[{"url":"https://cdn.example/wrong.jpg","mime":"image/jpeg"}]}"#,
    );
    fixture.write(
        "grids-21.json",
        r#"{"success":true,"data":[{"url":"https://cdn.example/right.jpg","mime":"image/jpeg"}]}"#,
    );

    let output = fixture.run_keyed(&["lutris", "covers", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        fixture.downloaded_url().as_deref(),
        Some("https://cdn.example/right.jpg")
    );
}

#[test]
fn a_pinned_match_is_used_instead() {
    let fixture = Fixture::new("pin");
    fixture.fake_curl();
    fixture.game("delta", "Delta");
    fixture.write(
        "search.json",
        r#"{"success":true,"data":[{"id":20,"name":"Delta Force"},{"id":21,"name":"Delta"}]}"#,
    );
    fixture.write(
        "grids-20.json",
        r#"{"success":true,"data":[{"url":"https://cdn.example/wrong.jpg","mime":"image/jpeg"}]}"#,
    );
    fixture.write(
        "grids-21.json",
        r#"{"success":true,"data":[{"url":"https://cdn.example/right.jpg","mime":"image/jpeg"}]}"#,
    );

    let output = fixture.run_keyed(&["lutris", "covers", "--game", "delta", "--match", "20"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        fixture.downloaded_url().as_deref(),
        Some("https://cdn.example/wrong.jpg")
    );
}

#[test]
fn matches_are_listed_with_previews_and_change_nothing() {
    let fixture = Fixture::new("matches");
    fixture.fake_curl();
    fixture.game("delta", "Delta");
    fixture.write(
        "search.json",
        r#"{"success":true,"data":[{"id":20,"name":"Delta Force"},{"id":21,"name":"Delta"}]}"#,
    );
    fixture.write(
        "grids-20.json",
        r#"{"success":true,"data":[{"url":"https://cdn.example/force.jpg","thumb":"https://cdn.example/force-thumb.jpg"}]}"#,
    );
    fixture.write(
        "grids-21.json",
        r#"{"success":true,"data":[{"url":"https://cdn.example/delta.jpg","thumb":"https://cdn.example/delta-thumb.jpg"}]}"#,
    );

    let output = fixture.run_keyed(&["lutris", "covers", "--game", "delta", "--matches", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("\"id\": 20"), "{text}");
    assert!(text.contains("\"id\": 21"), "{text}");
    assert!(
        text.contains("https://cdn.example/delta-thumb.jpg"),
        "{text}"
    );
    assert!(!fixture.cover_exists("delta"), "nothing should be written");
    assert_eq!(fixture.downloaded_url(), None);
}

#[test]
fn a_missing_key_is_explained_rather_than_guessed_at() {
    let fixture = Fixture::new("nokey");
    fixture.fake_curl();
    fixture.game("alpha", "Alpha Game");

    let output = fixture.run(&["lutris", "covers", "--json"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("SteamGridDB API key"),
        "{}",
        stderr(&output)
    );
    assert!(!fixture.cover_exists("alpha"));
}

#[test]
fn an_existing_cover_is_left_alone_without_overwrite() {
    let fixture = Fixture::new("existing");
    fixture.fake_curl();
    fixture.game("alpha", "Alpha Game");
    fixture.cover("alpha");
    fixture.write(
        "search.json",
        r#"{"success":true,"data":[{"id":10,"name":"Alpha Game"}]}"#,
    );
    fixture.write(
        "grids-10.json",
        r#"{"success":true,"data":[{"url":"https://cdn.example/new.jpg"}]}"#,
    );

    let output = fixture.run_keyed(&["lutris", "covers", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fixture.downloaded_url(), None);
    let bytes = fs::read(fixture.lutris.join("coverart/alpha.jpg")).unwrap();
    assert_eq!(bytes, b"already here");
}

#[test]
fn a_failed_download_leaves_nothing_where_a_cover_would_show() {
    let fixture = Fixture::new("partial");
    // A curl that writes a partial file and then fails, for a download.
    let script = r#"#!/bin/sh
base=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
url=""
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
  url="$arg"
done
if [ -n "$out" ]; then printf 'PARTIAL' > "$out"; exit 22; fi
case "$url" in
  *search/autocomplete*) cat "$base/search.json" ;;
  *grids/game/*)
    id=$(printf '%s' "$url" | sed -n 's#.*grids/game/\([0-9]*\).*#\1#p')
    if [ -f "$base/grids-$id.json" ]; then cat "$base/grids-$id.json"; else echo '{"success":true,"data":[]}'; fi
    ;;
  *) echo '{"success":true,"data":[]}' ;;
esac
"#;
    let path = fixture.bin.join("curl");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    fixture.game("alpha", "Alpha Game");
    fixture.write(
        "search.json",
        r#"{"success":true,"data":[{"id":10,"name":"Alpha Game"}]}"#,
    );
    fixture.write(
        "grids-10.json",
        r#"{"success":true,"data":[{"url":"https://cdn.example/alpha.jpg"}]}"#,
    );

    let output = fixture.run_keyed(&["lutris", "covers", "--json"]);
    assert!(!output.status.success());
    assert!(!fixture.cover_exists("alpha"));
    // No temporary file is left behind either.
    let leftovers: Vec<PathBuf> = fs::read_dir(fixture.lutris.join("coverart"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_game_that_cannot_be_matched_is_reported_and_nothing_is_written() {
    let fixture = Fixture::new("nomatch");
    fixture.fake_curl();
    fixture.game("alpha", "Alpha Game");
    fixture.write("search.json", r#"{"success":true,"data":[]}"#);

    let output = fixture.run_keyed(&["lutris", "covers", "--json"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(!fixture.cover_exists("alpha"));
}
