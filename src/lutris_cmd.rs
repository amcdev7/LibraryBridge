//! The `librarybridge lutris` subcommands.
//!
//! `detect` and `scan` read only. `plan` writes definitions into LibraryBridge's
//! own state directory and changes nothing in Lutris. `import` is the only
//! command that hands anything to Lutris, and it re-checks every path first.

use std::fs;
use std::path::PathBuf;

use crate::commands::{json_string, Options};
use crate::covers;
use crate::discover::{self, Candidate, Confidence};
use crate::json::{self, Json};
use crate::lutris::{self, Installation};

pub fn dispatch(options: &Options, arguments: &[String]) -> Result<i32, String> {
    match arguments.first().map(String::as_str) {
        Some("detect") => detect(options),
        Some("scan") => scan(options),
        Some("plan") => plan(options),
        Some("import") => import(options),
        Some("covers") => covers(options),
        Some("forget") => forget(options),
        Some(other) => Err(format!(
            "unknown lutris command '{other}'. Try detect, scan, plan, import, covers or forget."
        )),
        None => {
            Err("lutris needs a command: detect, scan, plan, import, covers or forget.".to_string())
        }
    }
}

/// How long to keep waiting for a Lutris install dialog to write its config.
/// `lutris --install` returns as soon as the window opens, so without a wait
/// a game that the user is still confirming would be reported as skipped.
/// The wait ends the moment the entry appears; this is only the upper bound.
const ADMIN_POLL_SECS: u64 = 120;

fn state_dir() -> PathBuf {
    crate::state::app_data_dir().join("lutris")
}

fn require_lutris() -> Result<Installation, String> {
    lutris::find_installations()
        .into_iter()
        .next()
        .ok_or_else(|| {
            "Lutris was not found. Install it from your distribution's packages or from \
         Flathub as net.lutris.Lutris, then run this again."
                .to_string()
        })
}

// -------------------------------------------------------------------- detect

fn detect(options: &Options) -> Result<i32, String> {
    let installations = lutris::find_installations();

    if options.json {
        let rows: Vec<String> = installations
            .iter()
            .map(|i| {
                format!(
                    "    {{\"packaging\": \"{}\", \"config\": \"{}\", \"games\": {}}}",
                    i.packaging.label(),
                    i.config_dir.display(),
                    lutris::existing_entries(i).len()
                )
            })
            .collect();
        println!(
            "{{\n  \"schema\": 1,\n  \"lutris\": [\n{}\n  ]\n}}",
            rows.join(",\n")
        );
        return Ok(0);
    }

    if installations.is_empty() {
        println!("Lutris was not found.");
        println!();
        println!("Install it from your distribution's packages, or from Flathub:");
        println!("    flatpak install flathub net.lutris.Lutris");
        return Ok(2);
    }

    for installation in &installations {
        let entries = lutris::existing_entries(installation);
        println!("Lutris      {} install", installation.packaging.label());
        println!("Config      {}", installation.config_dir.display());
        println!("Knows about {} games", entries.len());
        let runners = {
            let mut list: Vec<String> = entries.iter().map(|e| e.runner.clone()).collect();
            list.sort();
            list.dedup();
            list
        };
        if !runners.is_empty() {
            println!("Runners     {}", runners.join(", "));
        }
        println!();
    }
    Ok(0)
}

// ---------------------------------------------------------------------- scan

fn scan(options: &Options) -> Result<i32, String> {
    let installations = lutris::find_installations();
    let existing: Vec<lutris::Entry> = installations
        .iter()
        .flat_map(lutris::existing_entries)
        .collect();

    if installations.is_empty() {
        eprintln!(
            "librarybridge: Lutris was not found, so nothing can be marked as already added."
        );
    }

    // Machine output reports progress on stdout as JSON lines, exactly like
    // `fix` and `undo`, so a caller (the window) can show how much is left.
    // The final document is still the only thing that ever follows, so a
    // script that ignores these lines is unaffected.
    let scanning = !options.roots.is_empty();
    let progress = move |done: usize, total: usize| {
        if options.json && scanning {
            emit_scan(
                options,
                "scanning",
                &[("done", done.to_string()), ("total", total.to_string())],
            );
        }
    };
    let candidates = discover::scan(
        &options.roots,
        options.steam_root.as_deref(),
        &existing,
        progress,
    );

    let shown: Vec<&Candidate> = if options.all {
        candidates.iter().collect()
    } else {
        candidates.iter().filter(|c| !c.in_lutris).collect()
    };

    if options.json {
        print!("{}", scan_json(&shown));
        return Ok(0);
    }

    if options.roots.is_empty() {
        println!("No folders were given, so only Steam libraries were checked.");
        println!("Point at a games folder to find everything else:");
        println!("    librarybridge lutris scan --root /run/media/you/Games");
        println!();
    }

    if shown.is_empty() {
        println!("Nothing found that Lutris does not already have.");
        return Ok(0);
    }

    println!("Found {} games not in Lutris", shown.len());
    for candidate in &shown {
        println!();
        println!("  [{}] {}", candidate.id, candidate.name);
        println!(
            "      runner      {} ({} source)",
            candidate.runner, candidate.source
        );
        if let Some(appid) = &candidate.appid {
            println!("      steam app   {appid}");
        }
        if let Some(exe) = &candidate.exe {
            println!("      executable  {}", exe.display());
        }
        if let Some(prefix) = &candidate.prefix {
            println!("      prefix      {}", prefix.display());
        }
        println!("      confidence  {}", candidate.confidence.label());
        for reason in &candidate.reasons {
            println!("      because     {reason}");
        }
        if !candidate.alternatives.is_empty() {
            println!("      or maybe    {}", candidate.alternatives[0].display());
            for other in candidate.alternatives.iter().skip(1) {
                println!("                  {}", other.display());
            }
        }
        if let Some(warning) = &candidate.filesystem_warning {
            println!("      warning     {warning}");
            println!("                  adding it to Lutris will not fix that on its own");
        }
        for warning in &candidate.launch_warnings {
            println!("      launch      {warning}");
            println!("                  this shows up the first time you run it");
        }
        if candidate.in_lutris {
            println!("      note        Lutris already has this one");
        }
    }

    let steam_count = shown.iter().filter(|c| c.source == "steam").count();
    println!();
    if steam_count > 0 {
        println!(
            "{steam_count} of these are Steam games. Lutris shows installed Steam games through"
        );
        println!("its own Steam source, so importing them usually makes a duplicate entry.");
        println!();
    }
    println!("To add some of them:");
    println!(
        "    librarybridge lutris plan --candidate {} --output plan.json",
        shown[0].id
    );
    println!("    librarybridge lutris import --plan plan.json");
    Ok(0)
}

/// A progress line for machine output, in the same shape `fix` uses. Emitted
/// to stdout; the final JSON document follows and is the only thing a caller
/// that ignores these lines will ever see.
fn emit_scan(options: &Options, event: &str, fields: &[(&str, String)]) {
    if !options.json {
        return;
    }
    let mut line = format!("{{\"event\": {}", json_string(event));
    for (key, value) in fields {
        line.push_str(&format!(", {}: {value}", json_string(key)));
    }
    line.push('}');
    println!("{line}");
}

/// Emit a machine-readable import phase while keeping the normal human log
/// on stdout for callers that want to show both.
fn emit(options: &Options, event: &str, fields: &[(&str, String)]) {
    if !options.json {
        return;
    }
    let mut line = format!("{{\"event\": {}", json_string(event));
    for (key, value) in fields {
        line.push_str(&format!(", {}: {value}", json_string(key)));
    }
    line.push('}');
    println!("{line}");
}

fn scan_json(candidates: &[&Candidate]) -> String {
    let mut out = String::from("{\n  \"schema\": 1,\n  \"candidates\": [\n");
    let rows: Vec<String> = candidates
        .iter()
        .map(|c| candidate_json(c, "    "))
        .collect();
    out.push_str(&rows.join(",\n"));
    out.push_str("\n  ]\n}\n");
    out
}

fn candidate_json(candidate: &Candidate, indent: &str) -> String {
    let optional = |value: &Option<PathBuf>| match value {
        Some(path) => quote(&path.to_string_lossy()),
        None => "null".to_string(),
    };
    format!(
        "{indent}{{\n\
         {indent}  \"id\": {},\n\
         {indent}  \"name\": {},\n\
         {indent}  \"runner\": {},\n\
         {indent}  \"source\": {},\n\
         {indent}  \"exe\": {},\n\
         {indent}  \"appid\": {},\n\
         {indent}  \"prefix\": {},\n\
         {indent}  \"working_dir\": {},\n\
         {indent}  \"confidence\": {},\n\
         {indent}  \"in_lutris\": {},\n\
         {indent}  \"eligible\": {},\n\
         {indent}  \"blocking_reason\": {},\n\
         {indent}  \"filesystem_warning\": {},\n\
         {indent}  \"warnings\": [{}],\n\
         {indent}  \"alternatives\": [{}],\n\
         {indent}  \"reasons\": [{}]\n\
         {indent}}}",
        quote(&candidate.id),
        quote(&candidate.name),
        quote(&candidate.runner),
        quote(candidate.source),
        optional(&candidate.exe),
        match &candidate.appid {
            Some(id) => quote(id),
            None => "null".to_string(),
        },
        optional(&candidate.prefix),
        optional(&candidate.working_dir),
        quote(candidate.confidence.label()),
        candidate.in_lutris,
        candidate.eligible,
        match &candidate.blocking_reason {
            Some(reason) => quote(reason),
            None => "null".to_string(),
        },
        match &candidate.filesystem_warning {
            Some(warning) => quote(warning),
            None => "null".to_string(),
        },
        candidate
            .launch_warnings
            .iter()
            .map(|w| quote(w))
            .collect::<Vec<_>>()
            .join(", "),
        candidate
            .alternatives
            .iter()
            .map(|path| quote(&path.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(", "),
        candidate
            .reasons
            .iter()
            .map(|r| quote(r))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------- plan

fn plan(options: &Options) -> Result<i32, String> {
    if options.candidates.is_empty() {
        return Err(
            "plan needs at least one --candidate ID from `librarybridge lutris scan`.".into(),
        );
    }
    let output = options
        .output
        .clone()
        .ok_or("plan needs --output PATH for the file to write")?;

    let installations = lutris::find_installations();
    let existing: Vec<lutris::Entry> = installations
        .iter()
        .flat_map(lutris::existing_entries)
        .collect();
    let candidates = discover::scan(
        &options.roots,
        options.steam_root.as_deref(),
        &existing,
        move |_, _| (),
    );

    let mut chosen = Vec::new();
    for wanted in &options.candidates {
        let matches: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| c.id.starts_with(wanted.as_str()))
            .collect();
        match matches.len() {
            1 => chosen.push(matches[0].clone()),
            0 => {
                return Err(format!(
                    "no candidate matches '{wanted}'. Run the same scan again with the same \
                     --root arguments to see current ids."
                ))
            }
            n => {
                return Err(format!(
                    "'{wanted}' matches {n} candidates. Use the full id."
                ))
            }
        }
    }

    // Refuse here rather than writing a plan whose rows the import would drop.
    if let Some(blocked) = chosen.iter().find(|c| !c.eligible) {
        return Err(format!(
            "{} cannot be imported: {}. Leave it out of the selection.",
            blocked.name,
            blocked
                .blocking_reason
                .clone()
                .unwrap_or_else(|| "it is not eligible".to_string())
        ));
    }

    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let rows: Vec<String> = chosen.iter().map(|c| candidate_json(c, "    ")).collect();
    let document = format!(
        "{{\n  \"schema\": 1,\n  \"created\": {created},\n  \"games\": [\n{}\n  ]\n}}\n",
        rows.join(",\n")
    );

    // A dry run is read-only in every subcommand, and writing the plan file is
    // still a write.
    if options.dry_run {
        println!("Dry run. Would write {} with:", output.display());
        for candidate in &chosen {
            println!(
                "  {} ({} runner, {} confidence)",
                candidate.name,
                candidate.runner,
                candidate.confidence.label()
            );
        }
        println!();
        println!("Nothing was written.");
        return Ok(0);
    }

    fs::write(&output, document).map_err(|e| format!("{}: {e}", output.display()))?;

    println!("Wrote {} with {} games:", output.display(), chosen.len());
    for candidate in &chosen {
        println!(
            "  {} ({} runner, {} confidence)",
            candidate.name,
            candidate.runner,
            candidate.confidence.label()
        );
        if candidate.confidence == Confidence::Low {
            println!(
                "    check the executable before importing: {}",
                candidate
                    .exe
                    .as_ref()
                    .map(|e| e.display().to_string())
                    .unwrap_or_default()
            );
        }
    }
    println!();
    println!("Review it, edit any executable that looks wrong, then:");
    println!(
        "    librarybridge lutris import --plan {}",
        output.display()
    );
    Ok(0)
}

// -------------------------------------------------------------------- import

fn import(options: &Options) -> Result<i32, String> {
    let plan_path = options
        .plan
        .clone()
        .ok_or("import needs --plan PATH, produced by `librarybridge lutris plan`")?;
    let installation = require_lutris()?;

    let text =
        fs::read_to_string(&plan_path).map_err(|e| format!("{}: {e}", plan_path.display()))?;
    let parsed = json::parse(&text).map_err(|e| format!("{}: {e}", plan_path.display()))?;
    if parsed.get("schema") != Some(&Json::Number(1.0)) {
        return Err(format!("{}: unrecognised plan schema", plan_path.display()));
    }
    let games = parsed.get("games").map(Json::as_array).unwrap_or_default();
    if games.is_empty() {
        return Err(format!(
            "{}: the plan has no games in it",
            plan_path.display()
        ));
    }

    let before = lutris::existing_entries(&installation);

    // A dry run validates the whole proposal and stops. It writes no
    // definition, records nothing, and never starts Lutris.
    if options.dry_run {
        println!("Lutris      {} install", installation.packaging.label());
        println!("Dry run     {} games in the plan", games.len());
        println!();
        for game in games {
            let name = game.string("name").unwrap_or_else(|| "unnamed".into());
            match import_blocker(game, &before) {
                Some(reason) => println!("  skip  {name}: {reason}"),
                None => println!("  add   {name}"),
            }
        }
        println!();
        println!("Nothing was changed and Lutris was not started.");
        return Ok(0);
    }

    emit(options, "lutris_preparing", &[]);

    let definitions_dir = state_dir().join("definitions");
    fs::create_dir_all(&definitions_dir)
        .map_err(|e| format!("{}: {e}", definitions_dir.display()))?;

    println!("Lutris      {} install", installation.packaging.label());
    println!("Importing   {} games", games.len());
    println!();
    println!("Lutris shows its own dialog for each game. Confirm each one there.");
    println!();

    let mut imported = 0;
    let mut skipped = 0;
    let total = games.len() as u64;
    let mut processed = 0_u64;
    for game in games {
        let name = game.string("name").unwrap_or_else(|| "unnamed".into());
        let runner = game.string("runner").unwrap_or_else(|| "wine".into());
        let exe = game.string("exe").map(PathBuf::from);
        let appid = game.string("appid");

        // The plan is a proposal, not an instruction. Everything is rechecked.
        if let Some(reason) = import_blocker(game, &before) {
            println!("  skipped {name}: {reason}");
            skipped += 1;
            continue;
        }
        let slug = lutris::slugify(&name);

        let definition = lutris::Definition {
            name: name.clone(),
            slug: slug.clone(),
            runner,
            exe: exe.clone(),
            appid: appid.clone(),
            prefix: game.string("prefix").map(PathBuf::from),
            working_dir: game.string("working_dir").map(PathBuf::from),
        };
        let yaml_path = definitions_dir.join(format!("{slug}.yml"));
        fs::write(&yaml_path, definition.to_yaml())
            .map_err(|e| format!("{}: {e}", yaml_path.display()))?;

        println!("  {name}");
        let already_running = processed == 0 && lutris::is_running(&installation);
        if already_running {
            emit(
                options,
                "lutris_adding",
                &[
                    ("done", processed.to_string()),
                    ("total", total.to_string()),
                ],
            );
        } else if processed == 0 {
            emit(options, "lutris_opening", &[]);
        } else {
            emit(
                options,
                "lutris_adding",
                &[
                    ("done", processed.to_string()),
                    ("total", total.to_string()),
                ],
            );
        }
        let exit = lutris::install(&installation, &yaml_path);
        if !already_running {
            emit(
                options,
                "lutris_adding",
                &[
                    ("done", processed.to_string()),
                    ("total", total.to_string()),
                ],
            );
        }
        match exit {
            Ok(code) if code != 0 => {
                println!(
                    "    Lutris exited with status {code} (its install dialog may have been cancelled)"
                );
                // Continue: a non-zero exit is not proof nothing was added.
                // The config check below is the real test.
            }
            Ok(_) => {}
            Err(message) => {
                println!("    could not run Lutris: {message}");
                skipped += 1;
                processed += 1;
                continue;
            }
        }

        let after = lutris::existing_entries(&installation);
        let landed = after.iter().find(|e| {
            !before.iter().any(|b| b.config == e.config)
                && (e.slug.starts_with(&slug) || e.exe == exe)
        });
        match landed {
            Some(entry) => {
                record_provenance(&definition, entry)?;
                println!("    added, config at {}", entry.config.display());
                imported += 1;
            }
            None => {
                // `lutris --install` returns as soon as the install window is
                // shown, not when it closes. The config is written when the
                // user finishes the dialog, so wait for it before concluding
                // the import failed. This is what makes the window say
                // "stopped without finishing" when the game actually got
                // added.
                let deadline = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
                    + ADMIN_POLL_SECS;
                let mut landed = None;
                println!("    Waiting for you to finish the Lutris dialog…");
                while std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
                    < deadline
                {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let latest = lutris::existing_entries(&installation);
                    if let Some(entry) = latest.iter().find(|e| {
                        !before.iter().any(|b| b.config == e.config)
                            && (e.slug.starts_with(&slug) || e.exe == exe)
                    }) {
                        landed = Some(entry.clone());
                        break;
                    }
                }
                match landed {
                    Some(entry) => {
                        record_provenance(&definition, &entry)?;
                        println!("    added, config at {}", entry.config.display());
                        imported += 1;
                    }
                    None => {
                        println!(
                            "    Lutris did not report a new entry within {}s. Check Lutris \
                             manually.",
                            ADMIN_POLL_SECS
                        );
                        skipped += 1;
                    }
                }
            }
        }
        processed += 1;
    }

    println!();
    println!("{imported} added, {skipped} skipped.");
    println!("No game files or prefixes were changed.");
    // Part of the job succeeded: that is a success for the caller even if some
    // games had to be skipped. Only report failure when nothing was added at
    // all.
    Ok(if imported == 0 && skipped > 0 { 2 } else { 0 })
}

/// Why a planned game cannot be imported, or `None` if it can. Used by both
/// the dry run and the real run so the two always agree.
fn import_blocker(game: &Json, before: &[lutris::Entry]) -> Option<String> {
    let name = game.string("name").unwrap_or_default();
    if name.trim().is_empty() {
        return Some("the plan gives no name".to_string());
    }
    let exe = game.string("exe").map(PathBuf::from);
    let appid = game.string("appid");

    if let Some(exe) = &exe {
        if !exe.is_file() {
            return Some(format!("{} is no longer there", exe.display()));
        }
    } else if appid.is_none() {
        return Some("the plan has neither an executable nor a Steam app id".to_string());
    }

    if game.string("runner").as_deref() == Some("steam") {
        return Some(
            "Lutris lists installed Steam games itself, so this would create a duplicate"
                .to_string(),
        );
    }

    let slug = lutris::slugify(&name);
    let known = before.iter().any(|entry| {
        entry.slug == slug
            || (exe.is_some() && entry.exe == exe)
            || (appid.is_some() && entry.appid == appid)
    });
    if known {
        return Some("Lutris already has it".to_string());
    }
    None
}

fn record_provenance(definition: &lutris::Definition, entry: &lutris::Entry) -> Result<(), String> {
    let dir = state_dir().join("imports");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let document = format!(
        "{{\n  \"schema\": 1,\n  \"created\": {created},\n  \"slug\": {},\n  \
         \"name\": {},\n  \"runner\": {},\n  \"exe\": {},\n  \"lutris_config\": {}\n}}\n",
        quote(&definition.slug),
        quote(&definition.name),
        quote(&definition.runner),
        quote(
            &definition
                .exe
                .as_ref()
                .map(|e| e.to_string_lossy().to_string())
                .unwrap_or_default()
        ),
        quote(&entry.config.to_string_lossy())
    );
    let path = dir.join(format!("{}.json", definition.slug));
    fs::write(&path, document).map_err(|e| format!("{}: {e}", path.display()))
}

// -------------------------------------------------------------------- covers

/// Fetch missing cover art from SteamGridDB.
///
/// Lutris resolves a game's cover as a file in its own `coverart` directory,
/// named after the game's slug, so putting one there is all it takes. Nothing
/// here reads or writes Lutris's database, and unlike the script this is
/// adapted from, nothing is ever deleted: a cover left over for a game that is
/// no longer installed is simply ignored.
fn covers(options: &Options) -> Result<i32, String> {
    let installation = require_lutris()?;
    // The games Lutris itself lists, so leftovers in the config directory and
    // two files for one game cannot show up as separate entries.
    let all = covers::games(&installation);

    // Listing is offline and needs no key: it only reports what is on disk.
    if options.list_covers {
        let missing: Vec<&covers::Game> = all.iter().filter(|game| game.cover.is_none()).collect();
        if options.json {
            print!("{}", covers_list_json(&missing));
            return Ok(0);
        }
        if missing.is_empty() {
            println!("Every game in Lutris already has cover art.");
            return Ok(0);
        }
        println!(
            "{} of {} games have no cover art:",
            missing.len(),
            all.len()
        );
        for game in &missing {
            println!("  {}  ({})", game.name, game.slug);
        }
        println!();
        println!("Fetch them with:");
        println!("    librarybridge lutris covers");
        return Ok(0);
    }

    let key = covers::read_key(options.apikey.as_deref()).ok_or_else(|| {
        format!(
            "no SteamGridDB API key. Get one free at \
             https://www.steamgriddb.com/profile/preferences/api, then set SGDB_API_KEY, pass \
             --apikey PATH, or save it to {}.",
            covers::key_path().display()
        )
    })?;

    // A pinned id is checked once, not per game, so a typo fails immediately.
    let wanted_id: Option<u64> =
        match &options.cover_match {
            Some(value) => Some(value.parse().map_err(|_| {
                format!("--match takes a SteamGridDB id (a number), not '{value}'")
            })?),
            None => None,
        };
    // Picking a cover by id is an explicit choice, so it replaces whatever is
    // there without also needing --overwrite.
    let replace = options.overwrite || wanted_id.is_some();

    let targets: Vec<covers::Game> = match &options.game {
        Some(wanted) => {
            let matches: Vec<&covers::Game> = all
                .iter()
                .filter(|game| game.slug == *wanted || game.slug.starts_with(wanted.as_str()))
                .collect();
            match matches.len() {
                1 => vec![matches[0].clone()],
                0 => {
                    return Err(format!(
                        "no Lutris game matches '{wanted}'. Run `librarybridge lutris covers \
                         --list` to see the slugs."
                    ))
                }
                count => {
                    return Err(format!(
                        "'{wanted}' matches {count} games. Use the full slug."
                    ))
                }
            }
        }
        None => all
            .iter()
            .filter(|game| replace || game.cover.is_none())
            .cloned()
            .collect(),
    };

    if targets.is_empty() {
        println!("Every game in Lutris already has cover art.");
        return Ok(0);
    }

    // `--matches` looks candidates up and downloads nothing.
    if options.matches {
        let entry = &targets[0];
        let query = options.query.clone().unwrap_or_else(|| entry.name.clone());
        let mut found = covers::search(&key, &query)?;
        found.truncate(covers::MAX_MATCHES);
        covers::with_thumbnails(&key, &mut found);
        if options.json {
            print!("{}", covers_matches_json(entry, &query, &found));
            return Ok(0);
        }
        if found.is_empty() {
            println!("SteamGridDB has no match for '{query}'.");
            return Ok(0);
        }
        println!("Matches for {} (searching \"{query}\"):", entry.name);
        for candidate in &found {
            println!("  [{}] {}", candidate.id, candidate.name);
        }
        println!();
        println!("Use one with:");
        println!(
            "    librarybridge lutris covers --game {} --match {}",
            entry.slug, found[0].id
        );
        return Ok(0);
    }

    let total = targets.len();
    let mut saved = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut finished = 0usize;

    if !options.json {
        println!("Cover art for {total} game(s)");
    }

    for entry in &targets {
        finished += 1;
        if !replace && covers::existing(&installation, &entry.slug).is_some() {
            if !options.json {
                println!("  {:<44} already has one", entry.name);
            }
            skipped += 1;
            emit_cover(options, entry, "exists", finished, total);
            continue;
        }

        let query = options.query.clone().unwrap_or_else(|| entry.name.clone());
        let found = match covers::search(&key, &query) {
            Ok(found) => found,
            Err(error) => {
                failed += 1;
                report_cover_failure(options, entry, &error);
                emit_cover(options, entry, "failed", finished, total);
                continue;
            }
        };
        pause();

        let chosen = match wanted_id {
            Some(id) => found.iter().find(|candidate| candidate.id == id).cloned(),
            None => covers::best_match(&found, &entry.name).cloned(),
        };
        let Some(chosen) = chosen else {
            failed += 1;
            report_cover_failure(
                options,
                entry,
                &format!("no SteamGridDB match for '{query}'"),
            );
            emit_cover(options, entry, "no_match", finished, total);
            continue;
        };

        let art = match covers::art(&key, chosen.id) {
            Ok(Some(art)) => art,
            Ok(None) => {
                failed += 1;
                report_cover_failure(
                    options,
                    entry,
                    &format!("\"{}\" has no vertical cover", chosen.name),
                );
                emit_cover(options, entry, "no_cover", finished, total);
                continue;
            }
            Err(error) => {
                failed += 1;
                report_cover_failure(options, entry, &error);
                emit_cover(options, entry, "failed", finished, total);
                continue;
            }
        };
        pause();

        if options.dry_run {
            if !options.json {
                println!("  {:<44} would use \"{}\"", entry.name, chosen.name);
            }
            skipped += 1;
            emit_cover(options, entry, "dry_run", finished, total);
            continue;
        }

        match covers::place(&installation, &entry.slug, &art) {
            Ok(_) => {
                saved += 1;
                if !options.json {
                    println!("  {:<44} saved from \"{}\"", entry.name, chosen.name);
                }
                emit_cover(options, entry, "saved", finished, total);
            }
            Err(error) => {
                failed += 1;
                report_cover_failure(options, entry, &error);
                emit_cover(options, entry, "failed", finished, total);
            }
        }
    }

    if options.json {
        emit(
            options,
            "covers_done",
            &[
                ("saved", saved.to_string()),
                ("skipped", skipped.to_string()),
                ("failed", failed.to_string()),
            ],
        );
    } else {
        println!();
        println!("{saved} saved, {skipped} skipped, {failed} with no cover found.");
        println!("Restart Lutris to see them.");
    }
    Ok(if saved == 0 && failed > 0 { 2 } else { 0 })
}

/// The pause between calls, to stay inside SteamGridDB's rate limit.
fn pause() {
    std::thread::sleep(std::time::Duration::from_millis(200));
}

fn report_cover_failure(options: &Options, game: &covers::Game, reason: &str) {
    if !options.json {
        println!("  {:<44} {reason}", game.name);
    }
}

fn emit_cover(options: &Options, game: &covers::Game, status: &str, done: usize, total: usize) {
    emit(
        options,
        "cover",
        &[
            ("slug", quote(&game.slug)),
            ("name", quote(&game.name)),
            ("status", quote(status)),
            ("done", done.to_string()),
            ("total", total.to_string()),
        ],
    );
}

fn covers_list_json(missing: &[&covers::Game]) -> String {
    let rows: Vec<String> = missing
        .iter()
        .map(|game| {
            format!(
                "    {{\"slug\": {}, \"name\": {}}}",
                quote(&game.slug),
                quote(&game.name)
            )
        })
        .collect();
    format!(
        "{{\n  \"schema\": 1,\n  \"missing\": [\n{}\n  ]\n}}\n",
        rows.join(",\n")
    )
}

fn covers_matches_json(game: &covers::Game, query: &str, found: &[covers::Match]) -> String {
    let rows: Vec<String> = found
        .iter()
        .map(|candidate| {
            format!(
                "    {{\"id\": {}, \"name\": {}, \"thumb\": {}}}",
                candidate.id,
                quote(&candidate.name),
                match &candidate.thumb {
                    Some(thumb) => quote(thumb),
                    None => "null".to_string(),
                }
            )
        })
        .collect();
    format!(
        "{{\n  \"schema\": 1,\n  \"slug\": {},\n  \"name\": {},\n  \"query\": {},\n  \
         \"matches\": [\n{}\n  ]\n}}\n",
        quote(&game.slug),
        quote(&game.name),
        quote(query),
        rows.join(",\n")
    )
}

// -------------------------------------------------------------------- forget

/// A slug names one import record. It is not a path, and anything that could
/// leave the records directory is refused rather than sanitised, because a
/// guess about what the user meant ends in a deleted file.
fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 80
        && slug != "."
        && slug != ".."
        && !slug.starts_with('.')
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn forget(options: &Options) -> Result<i32, String> {
    let wanted = options
        .entry
        .clone()
        .ok_or("forget needs --entry SLUG, as shown by `librarybridge lutris import`")?;
    if !valid_slug(&wanted) {
        return Err(format!(
            "'{wanted}' is not an import record name. Use the slug shown by \
             `librarybridge lutris import`: letters, digits, dashes and underscores only, \
             and no path."
        ));
    }

    let directory = state_dir().join("imports");
    let path = directory.join(format!("{wanted}.json"));
    // Belt and braces: the name is already validated, so this can only fail
    // if the records directory itself has been replaced underneath us.
    if path.parent() != Some(directory.as_path()) {
        return Err(format!(
            "{}: refusing to act outside the records directory",
            path.display()
        ));
    }
    // A record is a regular file this tool wrote. A symlink at that name is
    // somebody redirecting a delete, so it is refused rather than followed.
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(format!(
                "{}: the record is a symlink, which LibraryBridge will not follow to delete \
                 something else.",
                path.display()
            ))
        }
        _ => {}
    }
    if !path.is_file() {
        return Err(format!(
            "LibraryBridge has no record of importing '{wanted}'. It will not touch entries it \
             did not create."
        ));
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let record = json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    // It must look like one of ours, not merely sit in the right place with
    // the right extension.
    if record.get("schema").is_none() || record.string("slug").as_deref() != Some(wanted.as_str()) {
        return Err(format!(
            "{}: this file is not a LibraryBridge import record, so it will not be removed.",
            path.display()
        ));
    }
    let config = record.string("lutris_config").unwrap_or_default();

    if options.dry_run {
        println!("Dry run. Would remove the import record for '{wanted}'.");
        println!("{}", path.display());
        return Ok(0);
    }

    fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("Removed LibraryBridge's record of '{wanted}'.");
    println!();
    println!("The Lutris entry itself is still there. LibraryBridge does not write to Lutris's");
    println!("database, so removing the game is done in Lutris: right-click it and remove.");
    if !config.is_empty() {
        println!("Its config file is {config}");
    }
    println!("Nothing was deleted from your game files, prefix or saves.");
    Ok(0)
}
