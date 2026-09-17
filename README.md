<p align="center">
  <img src="assets/branding/librarybridge-controller-bridge-top-lb-1024.png" alt="LibraryBridge icon" width="180">
</p>

# LibraryBridge

Keep Windows game files on their existing drive while putting Proton data on a
Linux filesystem. LibraryBridge also finds existing GOG and standalone games
and adds them to Lutris.

LibraryBridge runs on Linux. Download the [latest release](https://github.com/amcdev7/LibraryBridge/releases/latest),
currently **0.1.0**: an AppImage, a portable desktop tarball (no FUSE
required), or a command-line tarball. You can also build from source (below).

> LibraryBridge is independent and is not affiliated with or endorsed by Valve
> Corporation. Steam and Proton are trademarks of Valve Corporation.

## What it does

### Steam libraries

Proton stores a Wine prefix for each game in `steamapps/compatdata`. Those
prefixes need Linux filesystem features that are not reliable on NTFS. The
game files can stay on the NTFS drive while their Proton data moves to a Linux
filesystem.

LibraryBridge copies and verifies the data, keeps the original as
`compatdata.backup`, and leaves a link where Steam expects it. Your game files
do not move, and Steam does not need a configuration change.

### Lutris games

LibraryBridge can scan folders for GOG and standalone Windows games that
Lutris does not know about. It shows the likely executable for each game so
you can review the result before adding it to Lutris. Game files, prefixes,
and saves are not moved.

### Cover art

Lutris often has no cover art for games it did not download from its own site.
LibraryBridge can fetch it from [SteamGridDB](https://www.steamgriddb.com) and
put it where Lutris looks for it. Nothing in Lutris's database is changed, and
nothing is deleted.

## Requirements

- Linux
- Steam for the Steam repair feature
- Lutris for the optional game import feature
- `curl` and a free SteamGridDB API key for the optional cover art feature
- Rust and Cargo to build from source
- A Linux filesystem with enough free space for the moved Proton data

## Installation

### Linux downloads

Download the [latest release](https://github.com/amcdev7/LibraryBridge/releases/latest).

Available downloads:

- AppImage desktop build; requires FUSE
- Portable desktop tarball; no FUSE required
- Command-line tarball; no display server required
- Source archive

See the release page for checksums, recovery instructions, SBOM, and
third-party notices.

For the AppImage, make it executable and run it:

```bash
chmod +x LibraryBridge-0.1.0-x86_64.AppImage
./LibraryBridge-0.1.0-x86_64.AppImage
```

If your system cannot mount the AppImage, use
`--appimage-extract-and-run` or the desktop tarball instead.

To run the desktop tarball:

```bash
tar -xzf librarybridge-desktop-0.1.0-linux-x86_64.tar.gz
./LibraryBridge.AppDir/AppRun
```

To run the command-line tool:

```bash
tar -xzf librarybridge-cli-0.1.0-linux-x86_64.tar.gz
./librarybridge
```

### Arch-based distributions

An Arch `PKGBUILD` is included in the repository, but an AUR package is not
published yet.

Until the AUR package is available, use the Linux downloads above or build
from source.

### Build from source

```bash
git clone https://github.com/amcdev7/LibraryBridge.git
cd LibraryBridge
cargo build --locked --release --workspace
```

The binaries are created in `target/release`:

- `librarybridge`, the command-line tool
- `librarybridge-gui`, the desktop window

### Optional desktop integration

For a source build, this installs the app-menu entry, icons, and links to the
release binaries in `~/.local/bin`:

```bash
packaging/install-desktop.sh
```

This step is not needed for a packaged release. Packages should install the
desktop entry and icons themselves.

## Quick start

Close Steam before repairing a library.

List the libraries and find the id to use in the next commands:

```bash
./target/release/librarybridge scan
```

Preview a repair without changing anything:

```bash
./target/release/librarybridge fix <id> --dry-run
```

If the preview looks right, apply it:

```bash
./target/release/librarybridge fix <id>
```

The command asks for confirmation unless you pass `--yes`. The original
Proton data stays beside the library as `compatdata.backup`.

To move the current data back to the game drive:

```bash
./target/release/librarybridge undo <id>
```

Do not delete the backup by hand. After you have confirmed that the game
works, `backup` can remove it and reclaim the space. This is the only command
that deletes data, and it requires recorded evidence that the game launched
and saved successfully.

```bash
./target/release/librarybridge backup <id>
```

## Choosing where Proton data goes

By default, moved data lives under:

```text
~/.local/share/librarybridge/<library>-<id>/compatdata
```

To use another Linux filesystem with more space:

```bash
./target/release/librarybridge \
  --data-dir /mnt/games/librarybridge \
  fix <id>
```

The destination can be ext4, btrfs, xfs, or another Linux filesystem that
supports the required features. It cannot be exFAT or the same filesystem as
the Steam library.

To see how much space the moved data uses:

```bash
./target/release/librarybridge storage
```

## Desktop window

```bash
./target/release/librarybridge-gui
```

The window shows library status, explains what needs attention, previews each
repair, and runs the same operations as the command-line tool. It also has
the Lutris scan and import flow. On the Lutris page, the **Cover art** panel
can automatically download covers for every game that is missing one, and lets
you choose the right match by its cover when the automatic match is wrong.

## Import games into Lutris

Use this when GOG or standalone Windows games already exist on a drive and you
want Lutris to manage them.

Check that Lutris is installed:

```bash
./target/release/librarybridge lutris detect
```

Scan a folder:

```bash
./target/release/librarybridge lutris scan \
  --root /run/media/you/Games
```

The scan reports candidate ids. Review the candidates, then create an import
plan and pass it to Lutris:

```bash
./target/release/librarybridge lutris plan \
  --root /run/media/you/Games \
  --candidate <id> \
  --output plan.json

./target/release/librarybridge lutris import --plan plan.json
```

Lutris opens its own installer dialog for each game. Steam games are not
imported because Lutris already lists them through its Steam integration.

## Cover art for Lutris games

Lutris reads a game's cover from its own `coverart` directory, named after the
game's slug. LibraryBridge fetches missing art from SteamGridDB and puts it
there. Nothing in Lutris's database is changed, and nothing is deleted.

Covers are fetched with `curl`, so it must be installed. A free SteamGridDB API
key is required; get one at
[steamgriddb.com/profile/preferences/api](https://www.steamgriddb.com/profile/preferences/api).

Save the key in the desktop window (it is kept for next time), or on the
command line:

```bash
echo 'YOUR_API_KEY' > ~/.local/share/librarybridge/sgdb-api-key
chmod 600 ~/.local/share/librarybridge/sgdb-api-key
```

`SGDB_API_KEY` and `--apikey PATH` also work. The key is never written anywhere
else.

In the desktop window, open **Lutris**, save the key from **Settings** if
needed, then click **Download missing** in the **Cover art** panel. LibraryBridge
uses each game's name to choose the best SteamGridDB match and downloads one
cover for every Lutris game that does not already have one. Use **Find art** on
an individual game to review preview images and choose a different match.

List what has no cover art. This is offline and needs no key:

```bash
./target/release/librarybridge lutris covers --list
```

Fetch art for every game that has none:

```bash
./target/release/librarybridge lutris covers
```

For one game, or to choose the match yourself:

```bash
# Search and list the candidates, with preview links, downloading nothing
./target/release/librarybridge lutris covers --game <slug> --matches

# Use a specific candidate by its id
./target/release/librarybridge lutris covers --game <slug> --match <id>

# Search with different text, or replace art that is already there
./target/release/librarybridge lutris covers --game <slug> --query "Exact Name" --overwrite
```

The search text defaults to the game's display name. When the automatic choice
is wrong, the desktop window's **Cover art** panel shows the candidates with
their covers so you can pick the right one. Restart Lutris to see new covers.

## Safety and recovery

- `scan`, `storage`, and `--dry-run` do not change files.
- A repair copies and verifies the data before changing the Steam library.
- A repair keeps the original. It does not delete it.
- `undo` copies the current data back, including saves made after the repair.
- `backup` is the only command that deletes anything. It checks the moved copy
  and requires evidence that the game launched and saved successfully.
- An interrupted repair can be reviewed again. Unfinished data is kept rather
  than deleted automatically.
- `lutris covers` writes one image file into Lutris's own `coverart` directory.
  It changes nothing else, and it never deletes a cover.

## Limitations

- Linux only
- exFAT libraries are detected but are not repaired automatically
- Timestamps, hard-link relationships, and sparse-file allocation are not
  preserved by the copy
- A successful repair fixes the location of Proton data. It does not guarantee
  that a particular game will run
- Cover art needs network access and a SteamGridDB API key, and the automatic
  match can choose the wrong game; pick the right one from the window, or with
  `--match`

## Troubleshooting

Run a fresh scan first. Include the LibraryBridge version, Linux distribution
and kernel, filesystem and mount driver, Steam installation type, and redacted
command output when [opening an issue](https://github.com/amcdev7/LibraryBridge/issues).
Use [GitHub Discussions](https://github.com/amcdev7/LibraryBridge/discussions)
for questions, ideas, and general feedback.

Do not attach saves, Proton prefixes, registry files, or Steam account data.

## For contributors

```bash
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked --release --workspace
```

The GUI has deterministic preview data for visual checks:

```bash
cargo run -p librarybridge-gui -- --preview --screen home
```

## License

LibraryBridge is available under the MIT License. See [LICENSE](LICENSE).
