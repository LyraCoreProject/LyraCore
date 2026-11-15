mod mysql_source;
mod plan;
mod read;
mod shard;
mod source;

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

use plan::{Outcome, Plan, PLAN_FORMAT_VERSION};
use read::Dialect;
use shard::Shard;
use source::SourceKind;

const USAGE: &str = "\
usage:
  lyracore-migrator plan --source-kind vmangos|cmangos --source LABEL
      [--realmd MYSQL_URL] [--characters MYSQL_URL] [--account NAME]...
      --server SERVER --database DATABASE --out PLAN.json
  lyracore-migrator apply --server SERVER --database DATABASE PLAN.json

MYSQL_URL defaults to LYRACORE_MIGRATION_REALMD_URL and LYRACORE_MIGRATION_CHARACTERS_URL.
See docs/character-migration.md.";

#[derive(Default)]
struct Args {
    command: String,
    source_kind: Option<String>,
    source: Option<String>,
    realmd: Option<String>,
    characters: Option<String>,
    accounts: Vec<String>,
    server: Option<String>,
    database: Option<String>,
    out: Option<String>,
    plan: Option<String>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args> {
    let mut parsed = Args {
        command: args.next().context(USAGE)?,
        ..Args::default()
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--source-kind" => parsed.source_kind = Some(value()?),
            "--source" => parsed.source = Some(value()?),
            "--realmd" => parsed.realmd = Some(value()?),
            "--characters" => parsed.characters = Some(value()?),
            "--account" => parsed.accounts.push(value()?.to_ascii_uppercase()),
            "--server" => parsed.server = Some(value()?),
            "--database" => parsed.database = Some(value()?),
            "--out" => parsed.out = Some(value()?),
            "-h" | "--help" => bail!("{USAGE}"),
            other if !other.starts_with('-') && parsed.plan.is_none() => {
                parsed.plan = Some(other.to_string())
            }
            other => bail!("unknown argument {other}\n{USAGE}"),
        }
    }
    Ok(parsed)
}

fn required(value: Option<String>, name: &str) -> Result<String> {
    value.with_context(|| format!("{name} is required\n{USAGE}"))
}

fn url(value: Option<String>, variable: &str, name: &str) -> Result<String> {
    value
        .or_else(|| std::env::var(variable).ok())
        .with_context(|| format!("{name} or {variable} is required\n{USAGE}"))
}

fn main() -> Result<()> {
    let args = parse_args(std::env::args().skip(1))?;
    let shard = Shard {
        server: required(args.server.clone(), "--server")?,
        database: required(args.database.clone(), "--database")?,
    };
    match args.command.as_str() {
        "plan" => write_plan(args, &shard),
        "apply" => apply(&required(args.plan, "PLAN.json")?, &shard),
        other => bail!("unknown command {other}\n{USAGE}"),
    }
}

fn write_plan(args: Args, shard: &Shard) -> Result<()> {
    let kind: SourceKind = required(args.source_kind, "--source-kind")?.parse()?;
    let source = required(args.source, "--source")?;
    let out = required(args.out, "--out")?;
    let dialect = Dialect::of(kind);
    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

    let mut realmd = mysql_source::MysqlDatabase::open(&url(
        args.realmd,
        "LYRACORE_MIGRATION_REALMD_URL",
        "--realmd",
    )?)?;
    let mut accounts = read::read_accounts(&mut realmd, dialect, now_secs)?;
    if !args.accounts.is_empty() {
        accounts.retain(|account| {
            args.accounts
                .contains(&account.username.to_ascii_uppercase())
        });
    }
    let mut characters_db = mysql_source::MysqlDatabase::open(&url(
        args.characters,
        "LYRACORE_MIGRATION_CHARACTERS_URL",
        "--characters",
    )?)?;
    read::check_characters_profile(&mut characters_db, dialect)?;
    let account_ids: Vec<u64> = accounts.iter().map(|account| account.id).collect();
    let characters = read::read_characters(&mut characters_db, dialect, &account_ids)?;
    let guilds = read::read_guilds(&mut characters_db, dialect)?;

    let catalogue = shard.catalogue()?;
    let plan = plan::build(&source, &catalogue, &accounts, &characters, &guilds);
    write_private(&out, &serde_json::to_string_pretty(&plan)?)
        .with_context(|| format!("write {out}"))?;
    for finding in &plan.findings {
        eprintln!(
            "{:?}: {}: {}",
            finding.outcome, finding.subject, finding.detail
        );
    }
    let refused = plan
        .findings
        .iter()
        .filter(|finding| finding.outcome == Outcome::Refused)
        .count();
    eprintln!(
        "plan {out}: {} Accounts, {} Characters and {} letters to migrate, {refused} refused",
        plan.accounts.len(),
        plan.characters.len(),
        plan.letters.len()
    );
    Ok(())
}

// The plan holds SRP salts and verifiers: a new owner-only file replaces any old one at once.
fn write_private(path: &str, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let staging = format!("{path}.{}.tmp", std::process::id());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&staging).and_then(|mut file| {
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    });
    match written.and_then(|()| std::fs::rename(&staging, path)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&staging);
            Err(error)
        }
    }
}

fn apply(path: &str, shard: &Shard) -> Result<()> {
    let plan: Plan = serde_json::from_str(
        &std::fs::read_to_string(path).with_context(|| format!("read {path}"))?,
    )
    .with_context(|| format!("parse {path}"))?;
    if plan.format_version != PLAN_FORMAT_VERSION {
        bail!(
            "{path} has plan format {}, this tool reads format {PLAN_FORMAT_VERSION}",
            plan.format_version
        );
    }
    let mut refusals = 0;
    let mut failed = HashSet::new();
    for account in &plan.accounts {
        if let Err(refusal) = shard.call("migrate_account", &serde_json::to_string(account)?)? {
            refusals += 1;
            eprintln!("refused: Account {}: {refusal}", account.username);
        }
    }
    for character in &plan.characters {
        match shard.call("migrate_character", &serde_json::to_string(character)?)? {
            Ok(()) => eprintln!("migrated: {} ({})", character.name, character.source_guid),
            Err(refusal) => {
                refusals += 1;
                failed.insert(character.source_guid);
                eprintln!(
                    "refused: Character {} ({}): {refusal}",
                    character.name, character.source_guid
                );
            }
        }
    }
    for letter in &plan.letters {
        let what = format!(
            "letter {}/{}",
            letter.source_mail_id, letter.source_item_guid
        );
        let linked = [letter.recipient_source_guid, letter.sender_source_guid];
        if deferred(&what, &linked, &failed) {
            refusals += 1;
            continue;
        }
        if let Err(refusal) = shard.call("migrate_letter", &serde_json::to_string(letter)?)? {
            refusals += 1;
            eprintln!(
                "refused: letter {}/{}: {refusal}",
                letter.source_mail_id, letter.source_item_guid
            );
        }
    }
    for list in &plan.contacts {
        let mut linked: Vec<u64> = list.contacts.iter().map(|c| c.target_source_guid).collect();
        linked.push(list.source_guid);
        if deferred(
            &format!("contacts of {}", list.source_guid),
            &linked,
            &failed,
        ) {
            refusals += 1;
            continue;
        }
        let arguments = [
            serde_json::to_string(&list.source)?,
            list.source_guid.to_string(),
            serde_json::to_string(&list.contacts)?,
        ];
        if let Err(refusal) = shard.call_with("migrate_contacts", &arguments)? {
            refusals += 1;
            eprintln!("refused: contacts of {}: {refusal}", list.source_guid);
        }
    }
    for guild in &plan.guilds {
        let mut linked: Vec<u64> = guild.members.iter().map(|m| m.source_guid).collect();
        linked.push(guild.leader_source_guid);
        if deferred(&format!("Guild {}", guild.name), &linked, &failed) {
            refusals += 1;
            continue;
        }
        if let Err(refusal) = shard.call("migrate_guild", &serde_json::to_string(guild)?)? {
            refusals += 1;
            eprintln!("refused: Guild {}: {refusal}", guild.name);
        }
    }
    if refusals > 0 {
        bail!("{refusals} migrations were refused; fix them and apply the plan again");
    }
    Ok(())
}

// A record written while a linked Character is missing would drop that link for good.
fn deferred(what: &str, linked: &[u64], failed: &HashSet<u64>) -> bool {
    let Some(missing) = linked.iter().find(|guid| failed.contains(guid)) else {
        return false;
    };
    eprintln!("deferred: {what}: source Character {missing} did not migrate in this run");
    true
}
