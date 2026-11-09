# Character Migration

A Character Migration copies Accounts and Characters from a vmangos or cmangos server, the
Migration Source, into a single-Shard Realm. [ADR 0008](./adr/0008-migrate-characters-from-mangos-servers.md)
records why the Module owns the translation. `module/src/character_migration.rs` holds the reducers,
and `migrator/` holds the tool that reads the source.

## Running a migration

`lyracore-migrator` reads the Migration Source and calls the reducers. It runs in two steps, so
you can read the report before anything changes.

1. Import the world into the Realm first. A plan only keeps content that the Shard's catalogues
   hold.
2. Stop the source server. A Character that is online at the source is refused.
3. Give the tool a MySQL user that can only `SELECT` from realmd and characters. It reads each
   database in one read-only snapshot and never executes source SQL. Pass the URLs with
   `--realmd` and `--characters`, or in `LYRACORE_MIGRATION_REALMD_URL` and
   `LYRACORE_MIGRATION_CHARACTERS_URL` to keep the password off the command line.
4. Write a plan:

   ```bash
   cargo run --release -p lyracore-migrator -- plan \
     --source-kind vmangos --source vmangos-main \
     --server local --database lyracore --out plan.json
   ```

   `--source` is the Migration Source label. Use the same label on every run against this source.
   `--account NAME` limits the plan to named Accounts and can repeat.
5. Read the findings. The tool prints them and stores them in the plan. `refused` means the Account
   or Character is not in the plan. `dropped` names source state the Shard does not hold, such as
   spells, quests, areas or taxi nodes that are not imported. `changed` names a value the
   migration changes, such as a Character that arrives at its home bind or a temporary ban that
   becomes permanent. `skipped` means the Character is already migrated.
6. Apply the plan:

   ```bash
   cargo run --release -p lyracore-migrator -- apply --server local --database lyracore plan.json
   ```

   Each Character is one reducer call. A refusal is printed and the next Character continues.
   Fix the cause, write a new plan and apply it again. Migrated Characters are skipped.

The plan file holds each Account's SRP salt and verifier. The tool creates it readable only by its
owner; keep it as private as the realmd database. `apply` passes each reducer argument on the
`spacetime` command line, where other local users can read it while the call runs, so run it on a
host only Operators use. For a remote source, add `?require_ssl=true` to the MySQL URL.

The tool checks every column that it reads against the Source Profile of `--source-kind` before it
reads rows. A missing column or a column of another type stops the run with its name. The profiles
are in `migrator/src/read.rs`.

## Reducers

The reducers are Operator-only and refuse on a Shard with a Shard role.

- `migrate_account(account)` creates an Account from a username, a 32-byte salt and a 32-byte
  verifier, both little-endian as the logon proof uses them. The username is normalized to upper
  case. An existing Account with the same name is reused only when salt and verifier match; a
  source ban then also bans it. Any other existing Account refuses the migration.
- `migrate_character(migration)` resolves the Account the same way, then writes one Character. Each
  call is one transaction. A refusal names the first bad value and leaves no Account, Character or
  record.

`game_character_migration` is private and keyed by `<source>/<source guid>`. The source label is 1
to 64 ASCII letters, digits, hyphens or underscores. A second call with a recorded key returns
success and changes nothing. The record is not Character-owned: it outlives the Character, so a
rerun never brings back a Character that its player deleted.

## What arrives

| Source state | LyraCore result |
| --- | --- |
| Name, race, class, appearance | Refused on a name in use (case-insensitive), an invalid race and class pair or a full Account. |
| Level, XP, money, played time | Level 1 to 60. XP must stay below the next level; at the cap it is zero. |
| Position | Kept on a continent. A Character in an Instance map, or with no position, arrives at its home bind. |
| Home bind, rest | Kept. The last logout time and inn flag feed the normal offline rest at login. |
| Items | Vanilla slots 0 to 62 and keyring slots 81 to 112 keep their address. Bag contents use the flat bag-content slot of their bag. |
| Bank bags, large bags | This model has no bank bag slots and addresses 18 slots in each bag. A bank bag, its contents and Items past slot 18 of a bag move to the first free bank, backpack or bag slot, or the Character is refused. |
| Item state | Entry, stack, durability (capped at the template), permanent enchantment, random property and soulbound. A Letter Copy gets its own Item Text row. |
| Spells | Kept. A spell that is a talent rank becomes a talent row at its highest rank instead; world entry relearns the rank spell. Spent points cannot exceed the level's points. |
| Skills, action buttons | Kept. Buttons are 0 to 119. |
| Reputation | Earned standing is added to the faction's base standing. The faction must have a reputation index. |
| Quests | Rewarded quests and up to 20 quest log entries. A rewarded quest that cannot be repeated stays out of the log. Only a timed quest keeps its deadline. Kill and GameObject counters restart at zero, because the catalogue does not keep each objective's source slot; item objectives count the inventory as usual. |
| Hunter pet | The active Hunter pet keeps its creature, name, rename right, level, XP, loyalty and happiness (capped at 1,000,000). Its care clock starts at migration. Only a Hunter may bring one, and the creature must be an imported beast. LyraCore has no stable, so stabled pets are reported and stay behind. Pet spells and summoned pets such as Warlock demons do not migrate. |
| Explored zones, taxi nodes | Explored bits map to areas by area bit. Taxi nodes map by `TaxiNodes.dbc` id. |

Every referenced item, spell, skill, faction, quest, area and taxi node must exist in the Shard's
catalogues, or the reducer refuses the Character. The tool drops unknown spells, skills, factions,
quests, explored areas and taxi nodes with a finding, and refuses a Character with an unknown Item.

## Letters

`migrate_letter(letter)` puts one source Mail in the inbox of a migrated Character, as one
transaction. `apply` sends letters after every Character, so a letter from one migrated
Character to another names the migrated sender. A letter whose sender is not migrated arrives
with sender guid 0, and its expiry deletes it instead of returning it. Auction house, creature and
GameObject senders keep their entry; other source message types are refused.

The letter keeps its subject, body, copper, cash on delivery price, mail template and check flags.
Its creation time is derived from the source expiry and its current cash on delivery price, so
it expires when it would have expired at the source. A letter whose cash on delivery was already
paid has no price left and gets the full 30 days. A cash on delivery letter needs a migrated sender
to receive the copper; the tool drops the price when the sender does not migrate. A pending delivery keeps its delivery time. Stationery follows the sender
kind. An attached Letter Copy gets its own Item Text row.

1.12 Mail carries one Item. A source Mail with more Items becomes one letter for each Item; the
first keeps the copper and the cash on delivery price. `game_mail_migration` records
`<source>/<mail id>/<item guid>`, with item guid 0 for the first letter, so a rerun changes
nothing. A refused letter leaves nothing behind.

## Friends and ignores

`migrate_contacts(source, source_guid, contacts)` gives one migrated Character its friends and
ignores, once. `apply` sends contact lists after every letter. A target that is not a migrated
Character, for example one the reducer refused or one its player deleted, is skipped. The normal
contact rules apply: no self, no friend of the other faction, at most 50
friends and 25 ignores. `game_contact_migration` records the owner's `<source>/<source guid>`, so a
rerun does not add back a contact that its player removed.

The tool keeps a contact only when both Characters are in the migration, already migrated or
planned. It reports the other contacts, friends of the other faction and contacts over a cap.

## Guilds

`migrate_guild(guild)` founds one Guild of migrated Characters through the normal founding rules:
a valid and unused name, and a leader in no Guild. It then sets the message of the day, the
information text, the emblem and the source ranks, five to ten ids from 0 without gaps. Members
join through the normal membership path with their rank and notes; only the leader holds rank 0,
and every member must be on the leader's team. A member that is not a migrated Character is
skipped; the leader must be one. `apply` sends Guilds last.
`game_guild_migration` records `<source>/<source guild id>`, so a rerun changes nothing. A refusal
leaves no Guild.

The tool refuses a Guild whose leader does not migrate. It drops members that do not migrate and
shortens text to the 1.12 limits, with a finding for each. The guild event log does not migrate.

Health starts full and power starts at its class default at the first world entry. The Character
does not see the opening cinematic. GM levels, Auras, cooldowns, Instance bindings, temporary
enchantments and item charges do not migrate.
