//! Architecture Tests: source scans for structural invariants that behaviour tests cannot observe.
//! Each failure message opens with the invariant it protects, and each scanner has a negative test
//! that feeds it a small violating source.
//!
//! - [`character_owned_tripwire`]: every table with a Character-capable guid has a
//!   `character_owned` sweep marker or a reasoned exclusion.
//! - [`build_scan_strip_tripwire`]: a commented-out marker invocation never registers.
//! - [`partition_discipline_tripwire`]: no whole-table scan of a spatial table outside the budget,
//!   and no Module game logic reads a shard id.
//! - [`character_fence_tripwire`]: no raw `game_character` lookup outside the transfer fence.
//! - [`gc_reap_tripwire`]: every TTL-shaped event table is reaped in `gc.rs`.
//! - [`grid_cell_tripwire`]: a grid-coordinate write also writes the packed cell.
//! - [`package_name_tripwire`]: Core source names no official Package.
//!
//! The spatial and fence scanners share one engine, [`crate::test_scan::raw_table_reads`].

#[cfg(test)]
pub(crate) mod character_owned_tripwire {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Guid fields whose name fixes a non-Character object kind. Every other `*_guid: u64` field is
    /// treated as Character-capable. A new object-specific name fails closed until it is classified.
    const NON_UNIT_GUID_FIELDS: &[&str] = &[
        "corpse_guid",
        "creature_guid",
        "flag_guid",
        "go_guid",
        "item_guid",
        "live_pet_guid",
        "npc_guid",
        "pet_guid",
    ];

    /// Tables with a Character-capable guid that deliberately do not belong to one Character.
    /// Each reason states the row's actual owner or lifetime.
    const NOT_CHARACTER_OWNED: &[(&[&str], &str)] = &[
        (
            &[
                "game_account_claim",
                "game_account_fence",
                "game_account_character_owner",
            ],
            "Realm Account ownership retained across Character deletion and Transfer",
        ),
        (
            &[
                "game_auction",
                "game_auction_bid_decision",
                "game_auction_operation_receipt",
            ],
            "realm-owned Auction value and protocol state",
        ),
        (
            &["game_bot_invite_intent"],
            "short-lived Group Intent consumed by the Gateway or event GC",
        ),
        (
            &["game_bot_transfer_intent"],
            "durable source-side Transfer Intent consumed by exact Gateway completion",
        ),
        (
            &["game_party_command_intent"],
            "source Module command relay retained through issuer Transfer until Gateway finalization or event GC",
        ),
        (
            &[
                "game_auction_notice",
                "game_chat_channel_notice_event",
                "game_chat_event",
                "game_combat_event",
                "game_emote_event",
                "game_group_event",
                "game_guild_event",
                "game_mail_arrival",
                "game_realm_chat_event",
                "game_roll_event",
                "game_spell_cast_event",
                "game_spell_impact_event",
                "game_system_message_event",
                "game_teleport_event",
                "game_trade_event",
                "game_xp_event",
            ],
            "short-lived Relay event reaped by event GC",
        ),
        (
            &[
                "game_creature_ai_movement_intent",
                "game_creature_ai_relay_arrival",
                "game_creature_ai_relay_run",
                "game_creature_ai_summon_origin",
                "game_creature_move_event",
                "game_creature_quest_tap",
                "game_creature_quest_tap_member",
            ],
            "creature-owned state cleared with its creature, engagement, or delivery",
        ),
        (
            &[
                "game_combo_point",
                "game_dr_state",
                "game_melee_attack",
                "game_taunt_lock",
                "game_threat",
            ],
            "transient combat state cleared when its engagement ends",
        ),
        (
            &[
                "game_dynamic_object",
                "game_ground_area",
                "game_pending_cast",
                "game_pending_spell_impact",
                "game_ranged_impact_schedule",
                "game_resurrect_request",
                "game_school_lockout",
                "game_self_resurrect_option",
                "game_spell_cd",
                "game_spell_cooldown",
            ],
            "transient spell state removed, consumed, or made inert by its deadline",
        ),
        (
            &["game_corpse_loot", "game_corpse_loot_eligible"],
            "corpse-owned loot state removed when the corpse decays",
        ),
        (
            &["game_group"],
            "Realm-core Group authority shared by all members",
        ),
        (
            &[
                "game_guild",
                "game_guild_member",
                "game_guild_invite",
                "game_guild_petition",
                "game_guild_petition_signature",
            ],
            "realm-owned guild state on Realm-core, which holds no Character rows; the Gateway's character-gone reconciliation forgets a deleted Character",
        ),
        (
            &["game_guild_fee_decision"],
            "Realm-core guild fee decision kept as the replay answer for its operation id",
        ),
        (
            &["game_loot_roll_vote"],
            "Loot Roll-owned vote snapshot resolved or removed with the roll",
        ),
        (
            &["game_channel_event", "game_channel_member"],
            "retired shard-local channel tables that nothing writes",
        ),
        (
            &["game_whisper_event"],
            "retired whisper table that nothing writes; whispers are Realm Chat Lines",
        ),
        (
            &[
                "game_chat_channel",
                "game_chat_channel_ban",
                "game_chat_channel_member",
            ],
            "Realm-core Chat Channel state; membership ends with the Account Claim that admitted it",
        ),
        (
            &["game_meeting_stone_seeker"],
            "Realm-core Meeting Stone Queue; a solo Seeker ends with its Account Claim, a party Seeker with its party membership",
        ),
        (&["game_gateway_session"], "live Session routing state"),
        (
            &["game_entity_motion_pending"],
            "short-lived motion staging row drained by the next publish tick",
        ),
        (
            &["game_movement_violation"],
            "short-lived movement diagnostic reaped by event GC",
        ),
        (
            &["game_character"],
            "the root Character row deleted directly after its dependent state",
        ),
        (
            &["game_corpse"],
            "the Character's corpse is deleted directly through its deterministic corpse guid",
        ),
        (
            &["game_entity_motion", "game_world_entity"],
            "live unit state removed directly when the unit leaves the world",
        ),
        (
            &[
                "game_creature_spawn",
                "game_creature_spline",
                "game_encounter_spawn",
            ],
            "creature-owned spawn or movement state keyed by the creature guid",
        ),
        (
            &["game_gameobject"],
            "World GameObject state keyed by the GameObject's own guid",
        ),
    ];

    /// Every `.rs` file that compiles into this crate: core `src/` plus each drop-in
    /// `packages/*/src/` (build.rs compiles those in, so every source-scanning tripwire must see
    /// them too). Shared with `partition_discipline_tripwire` below and `gc_reap_tripwire`.
    ///
    /// `pub(crate)`, not `pub(super)`: `creatures::tick`'s
    /// `nothing_writes_the_unsubscribed_move_event_table` walks the whole compiled tree the
    /// same way, from outside this module entirely.
    pub(crate) fn scanned_files() -> Vec<PathBuf> {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mut files = Vec::new();
        collect_rs_files(&Path::new(manifest_dir).join("src"), &mut files);
        let packages_dir = Path::new(manifest_dir)
            .parent()
            .expect("module/ has a parent (the repo root)")
            .join("packages");
        if packages_dir.is_dir() {
            for entry in fs::read_dir(&packages_dir).expect("readable packages/") {
                let pkg_src = entry.expect("readable dir entry").path().join("src");
                if pkg_src.is_dir() {
                    collect_rs_files(&pkg_src, &mut files);
                }
            }
        }
        files
    }

    pub(crate) fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in
            fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read dir {}: {e}", dir.display()))
        {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                out.push(path);
            }
        }
    }

    /// Replace comments, literals, and `#[cfg(test)]` items with spaces while preserving byte offsets
    /// and newlines. The scanner can then use simple balanced-delimiter walks without accepting a
    /// declaration that production never compiles.
    pub(super) fn production_code(content: &str) -> String {
        fn blank(bytes: &mut [u8], start: usize, end: usize) {
            for byte in &mut bytes[start..end] {
                if *byte != b'\n' {
                    *byte = b' ';
                }
            }
        }

        let source = content.as_bytes();
        let mut code = source.to_vec();
        let mut i = 0;
        while i < source.len() {
            if source[i..].starts_with(b"//") {
                let end = source[i..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map(|offset| i + offset)
                    .unwrap_or(source.len());
                blank(&mut code, i, end);
                i = end;
                continue;
            }
            if source[i..].starts_with(b"/*") {
                let start = i;
                i += 2;
                let mut depth = 1usize;
                while i < source.len() && depth > 0 {
                    if source[i..].starts_with(b"/*") {
                        depth += 1;
                        i += 2;
                    } else if source[i..].starts_with(b"*/") {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                blank(&mut code, start, i);
                continue;
            }
            if source[i] == b'r' {
                let mut quote = i + 1;
                while source.get(quote) == Some(&b'#') {
                    quote += 1;
                }
                if source.get(quote) == Some(&b'"') {
                    let hashes = quote - i - 1;
                    let start = i;
                    i = quote + 1;
                    while i < source.len() {
                        if source[i] == b'"'
                            && source.get(i + 1..i + 1 + hashes)
                                == Some(&source[quote - hashes..quote])
                        {
                            i += 1 + hashes;
                            break;
                        }
                        i += 1;
                    }
                    blank(&mut code, start, i);
                    continue;
                }
            }
            if source[i] == b'"' {
                let start = i;
                i += 1;
                while i < source.len() {
                    match source[i] {
                        b'\\' => i = (i + 2).min(source.len()),
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                blank(&mut code, start, i);
                continue;
            }
            if source[i] == b'\'' {
                // A lifetime starts with the same quote. Only mask the narrow Rust char-literal
                // shapes: one byte or an escape followed by a closing quote.
                let escaped = source.get(i + 1) == Some(&b'\\');
                let close = if escaped { i + 3 } else { i + 2 };
                if source.get(close) == Some(&b'\'') {
                    blank(&mut code, i, close + 1);
                    i = close + 1;
                    continue;
                }
            }
            i += 1;
        }

        let test_attr = b"#[cfg(test)]";
        let mut search_from = 0usize;
        while let Some(offset) = code[search_from..]
            .windows(test_attr.len())
            .position(|window| window == test_attr)
        {
            let test_start = search_from + offset;
            let mut item_start = test_start;
            loop {
                let Some(end) = code[..item_start]
                    .iter()
                    .rposition(|byte| !byte.is_ascii_whitespace())
                    .map(|position| position + 1)
                else {
                    break;
                };
                if code.get(end - 1) != Some(&b']') {
                    break;
                }

                let mut depth = 0usize;
                let mut attribute_start = None;
                for position in (0..end).rev() {
                    match code[position] {
                        b']' => depth += 1,
                        b'[' => {
                            depth -= 1;
                            if depth == 0 {
                                if position > 0 && code[position - 1] == b'#' {
                                    attribute_start = Some(position - 1);
                                }
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let Some(start) = attribute_start else {
                    break;
                };
                item_start = start;
            }

            let mut cursor = test_start + test_attr.len();
            while code.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            while code.get(cursor..cursor + 2) == Some(b"#[") {
                let Some(close) = code[cursor..].iter().position(|byte| *byte == b']') else {
                    break;
                };
                cursor += close + 1;
                while code.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                    cursor += 1;
                }
            }
            let semicolon = code[cursor..]
                .iter()
                .position(|byte| *byte == b';')
                .map(|offset| cursor + offset);
            let open = code[cursor..]
                .iter()
                .position(|byte| *byte == b'{')
                .map(|offset| cursor + offset);
            let item_end = match (semicolon, open) {
                (Some(end), None) => end + 1,
                (Some(end), Some(open)) if end < open => end + 1,
                (_, Some(open)) => {
                    let mut depth = 0i32;
                    let mut end = code.len();
                    for (offset, byte) in code[open..].iter().enumerate() {
                        match byte {
                            b'{' => depth += 1,
                            b'}' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = open + offset + 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    end
                }
                _ => code.len(),
            };
            blank(&mut code, item_start, item_end);
            search_from = item_end;
        }

        String::from_utf8(code).expect("masking Rust source preserves UTF-8")
    }

    /// Extract every production `#[table(accessor = NAME` struct's field block as
    /// `(accessor, fields_text)`.
    ///
    /// `pub(super)`: [`gc_reap_tripwire`] (a sibling in this same file) reuses this rather than
    /// re-walking every `#[table]` struct a third time.
    pub(super) fn extract_tables(content: &str) -> Vec<(String, String)> {
        let code = production_code(content);
        let mut out = Vec::new();
        let mut search_from = 0usize;
        while let Some(rel) = code[search_from..].find("#[table(") {
            let attr_start = search_from + rel;
            let attr_rest = &code[attr_start..];
            let accessor = attr_rest
                .find("accessor")
                .and_then(|i| attr_rest[i..].find('=').map(|eq| i + eq + 1))
                .map(|name_start| {
                    let name_rest = attr_rest[name_start..].trim_start();
                    let end = name_rest
                        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .unwrap_or(name_rest.len());
                    name_rest[..end].to_string()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "malformed #[table(...)] (no accessor=NAME found) near byte {attr_start}"
                    )
                });
            let struct_kw = attr_rest.find("struct ").unwrap_or_else(|| {
                panic!("#[table(accessor = {accessor}, ...)] not followed by a struct")
            });
            let after_struct = &attr_rest[struct_kw..];
            let body_start = after_struct
                .find('{')
                .unwrap_or_else(|| panic!("struct for accessor {accessor} has no body"));
            let mut depth = 0i32;
            let mut body_end = None;
            for (i, c) in after_struct[body_start..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            body_end = Some(body_start + i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let body_end = body_end
                .unwrap_or_else(|| panic!("unterminated struct body for accessor {accessor}"));
            let fields = after_struct[body_start..=body_end].to_string();
            out.push((accessor, fields));
            search_from = attr_start + "#[table(".len();
        }
        out
    }

    fn character_owned_markers(content: &str) -> Vec<String> {
        let code = production_code(content);
        let needle = format!("{}{}(", "character_owned", "!");
        let mut markers = Vec::new();
        let mut search_from = 0usize;
        while let Some(offset) = code[search_from..].find(&needle) {
            let start = search_from + offset;
            let open = start + needle.len() - 1;
            let mut depth = 0i32;
            let mut end = None;
            for (offset, byte) in code.as_bytes()[open..].iter().enumerate() {
                match byte {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(open + offset + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let end = end.unwrap_or_else(|| panic!("unterminated character_owned marker"));
            let marker = &code[start..end];
            let kind = code[open + 1..].trim_start();
            if kind.starts_with("delete") || kind.starts_with("restamp") {
                markers.push(marker.to_string());
            }
            search_from = end;
        }
        markers
    }

    fn marker_covers_accessor(marker: &str, accessor: &str) -> bool {
        fn contains_identifier(text: &str, identifier: &str) -> bool {
            text.match_indices(identifier).any(|(start, _)| {
                let before = text[..start]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
                let after = text[start + identifier.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
                before && after
            })
        }

        contains_identifier(marker, &format!("sweep_delete_{accessor}"))
            || contains_identifier(marker, &format!("sweep_restamp_{accessor}"))
            || marker.contains(&format!(".{accessor}()"))
    }

    fn character_capable_guid_fields(fields: &str) -> Vec<&str> {
        let tokens = fields
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>();
        let mut guids = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            if !(token.ends_with("_guid") || *token == "guid")
                || NON_UNIT_GUID_FIELDS.contains(token)
            {
                continue;
            }
            let is_u64 = tokens.get(index + 1) == Some(&"u64")
                || (tokens.get(index + 1) == Some(&"Option")
                    && tokens.get(index + 2) == Some(&"u64"));
            if is_u64 && !guids.contains(token) {
                guids.push(*token);
            }
        }
        guids
    }

    fn unclassified_tables(content: &str) -> Vec<(String, Vec<String>)> {
        let markers = character_owned_markers(content);
        extract_tables(content)
            .into_iter()
            .filter_map(|(accessor, fields)| {
                let guids = character_capable_guid_fields(&fields);
                if guids.is_empty()
                    || NOT_CHARACTER_OWNED
                        .iter()
                        .any(|(excluded, _)| excluded.contains(&accessor.as_str()))
                    || markers
                        .iter()
                        .any(|marker| marker_covers_accessor(marker, &accessor))
                {
                    None
                } else {
                    Some((accessor, guids.into_iter().map(str::to_string).collect()))
                }
            })
            .collect()
    }

    #[test]
    fn every_character_capable_guid_table_is_classified() {
        let files = scanned_files();

        let mut missing = Vec::new();
        for file in &files {
            // `tripwires.rs` (this file) defines no `#[table]` struct of its own — it only ever
            // MENTIONS the attribute in prose/string literals (including this very scanner's own
            // source, e.g. `extract_tables`'s `.find("#[table(")`), which would otherwise misparse
            // as malformed attributes. Every real table lives in a submodule.
            if file.file_name().and_then(|n| n.to_str()) == Some("tripwires.rs") {
                continue;
            }
            let content = fs::read_to_string(file)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
            for (accessor, guids) in unclassified_tables(&content) {
                missing.push(format!("{accessor} ({guids:?}, in {})", file.display()));
            }
        }

        assert!(
            missing.is_empty(),
            "Invariant: deleting or re-owning a Character sweeps every table keyed by its guid.\n\
             table(s) with a Character-capable guid have no matching `character_owned` sweep marker \
             or `NOT_CHARACTER_OWNED` classification: {missing:?}\n\
             Add a `crate::character_owned` `delete` marker invocation (and usually the `restamp` \
             form too, see the macro doc at the top of lib.rs), or add the accessor to \
             `NOT_CHARACTER_OWNED` with its actual owner or lifetime."
        );
    }

    #[test]
    fn an_unclassified_target_guid_table_fails_the_scanner() {
        let fixture = r#"
            #[table(accessor = game_unclassified_target, public)]
            pub struct UnclassifiedTarget {
                #[primary_key]
                pub id: u64,
                pub target_guid: u64,
            }
        "#;

        assert_eq!(
            unclassified_tables(fixture),
            vec![(
                "game_unclassified_target".to_string(),
                vec!["target_guid".to_string()]
            )]
        );
    }

    #[test]
    fn an_unclassified_bare_guid_table_fails_the_scanner() {
        let source = r#"
            #[table(accessor = game_unclassified)]
            pub struct Unclassified {
                pub guid: u64,
            }
        "#;

        assert_eq!(
            unclassified_tables(source),
            vec![("game_unclassified".to_owned(), vec!["guid".to_owned()])]
        );
    }

    #[test]
    fn a_marker_for_another_table_does_not_classify_the_target_table() {
        let fixture = r#"
            #[table(accessor = game_owned)]
            pub struct Owned { pub character_guid: u64 }
            crate::character_owned!(delete, fn sweep_delete_game_owned(ctx, character_guid) {
                ctx.db.game_owned().character_guid().delete(character_guid);
            });

            #[table(accessor = game_unclassified_target)]
            pub struct UnclassifiedTarget { pub target_guid: u64 }
        "#;

        assert_eq!(
            unclassified_tables(fixture),
            vec![(
                "game_unclassified_target".to_string(),
                vec!["target_guid".to_string()]
            )]
        );
    }

    #[test]
    fn a_prefix_sharing_marker_does_not_classify_the_shorter_accessor() {
        let fixture = r#"
            #[table(accessor = game_owned)]
            pub struct Owned { pub target_guid: u64 }

            #[table(accessor = game_owned_extra)]
            pub struct OwnedExtra { pub character_guid: u64 }
            crate::character_owned!(delete, fn sweep_delete_game_owned_extra(ctx, character_guid) {
                ctx.db.game_owned_extra().character_guid().delete(character_guid);
            });
        "#;

        assert_eq!(
            unclassified_tables(fixture),
            vec![("game_owned".to_string(), vec!["target_guid".to_string()])]
        );
    }

    #[test]
    fn comments_and_literals_do_not_declare_tables() {
        let fixture = r##"
            // #[table(accessor = game_line_comment)]
            // struct LineComment { target_guid: u64 }
            /*
            #[table(accessor = game_block_comment)]
            struct BlockComment { target_guid: u64 }
            */
            const EXAMPLE: &str = "#[table(accessor = game_string)] struct StringTable { target_guid: u64 }";
            const RAW_EXAMPLE: &str = r#"#[table(accessor = game_raw_string)]
                struct RawStringTable { target_guid: u64 }"#;
        "##;

        assert!(unclassified_tables(fixture).is_empty());
    }

    #[test]
    fn test_only_declarations_do_not_enter_the_census() {
        let fixture = r#"
            #[cfg(test)]
            #[table(accessor = game_test_fixture)]
            pub struct TestFixture {
                #[primary_key]
                pub target_guid: u64,
            }
        "#;

        assert!(unclassified_tables(fixture).is_empty());
    }

    #[test]
    fn a_table_attribute_before_cfg_test_does_not_attach_to_the_next_struct() {
        let fixture = r#"
            #[table(accessor = game_test_fixture)]
            #[cfg(test)]
            pub struct TestFixture { pub target_guid: u64 }

            #[table(accessor = game_production)]
            pub struct Production { pub source_guid: u64 }
        "#;

        assert_eq!(
            unclassified_tables(fixture),
            vec![("game_production".to_owned(), vec!["source_guid".to_owned()])]
        );
    }

    #[test]
    fn lifetimes_do_not_hide_following_table_declarations() {
        let fixture = r#"
            fn borrow<'a>(value: &'a str) -> &'a str { value }
            #[table(accessor = game_after_lifetime)]
            pub struct AfterLifetime { pub target_guid: u64 }
        "#;

        assert_eq!(
            unclassified_tables(fixture),
            vec![(
                "game_after_lifetime".to_string(),
                vec!["target_guid".to_string()]
            )]
        );
    }

    #[test]
    fn exclusions_match_the_current_table_census_and_give_reasons() {
        let mut candidates = Vec::new();
        for file in scanned_files() {
            if file.file_name().and_then(|name| name.to_str()) == Some("tripwires.rs") {
                continue;
            }
            let content = fs::read_to_string(&file)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
            for (accessor, fields) in extract_tables(&content) {
                if !character_capable_guid_fields(&fields).is_empty() {
                    candidates.push(accessor);
                }
            }
        }

        let mut stale_or_unreasoned = NOT_CHARACTER_OWNED
            .iter()
            .flat_map(|(accessors, reason)| {
                accessors.iter().filter_map(|accessor| {
                    (reason.trim().is_empty()
                        || !candidates.iter().any(|candidate| candidate == accessor))
                    .then_some(*accessor)
                })
            })
            .collect::<Vec<_>>();
        stale_or_unreasoned.sort_unstable();

        assert!(
            stale_or_unreasoned.is_empty(),
            "stale or unreasoned `NOT_CHARACTER_OWNED` entries: {stale_or_unreasoned:?}"
        );
    }
}

/// ENFORCEMENT tripwire for the marker scan itself: a commented-out marker invocation must
/// neither register nor break the build. The comment right inside this mod IS the fixture — it is
/// real, well-formed marker syntax that would have panicked a naive substring scan (no `fn`
/// body brace games, the genuine article). If comment-stripping ever regresses, one of two things
/// happens loudly: build.rs panics on the "malformed" comment text, or the ghost pass registers
/// and this test fails.
#[cfg(test)]
mod build_scan_strip_tripwire {
    // game_tick_pass!(fn ghost_commented_out_pass(ctx) { unreachable!() })
    // character_owned!(delete, fn ghost_commented_out_sweep(ctx, character_guid) { unreachable!() })

    #[test]
    fn commented_out_markers_do_not_register() {
        // Assert on the GENERATED TEXT, not the registry consts: the ghost fn never compiles
        // into anything, so there is no fn-pointer value to look for — and referencing the const
        // arrays would materialize every REAL registered fn's pointer, dragging SpacetimeDB host
        // imports into this native test binary (which cannot link them).
        let registries = include_str!(concat!(env!("OUT_DIR"), "/package_registries.rs"));
        assert!(
            !registries.contains("ghost_commented_out_pass"),
            "a commented-out game_tick_pass registered — build.rs comment-stripping regressed"
        );
        let sweeps = include_str!(concat!(env!("OUT_DIR"), "/character_sweeps.rs"));
        assert!(
            !sweeps.contains("ghost_commented_out_sweep"),
            "a commented-out character_owned marker registered — build.rs comment-stripping regressed"
        );
    }
}

/// ENFORCEMENT tripwire (part of the elastic-world-sharding spec): a full
/// `.iter()` over a SPATIAL table reads the whole world, so it silently breaks shardability —
/// once the world is cut into region/continent/instance shards, a whole-table scan can only ever
/// see the rows on the caller's own shard, and the code that relied on seeing everything goes
/// quietly wrong. Every spatial read must instead go through the partition-scoped helper family
/// (`helpers::entities_near` + `helpers::in_same_partition`), which is `(map_id, instance_id)`-
/// scoped and grid-indexed and therefore keeps meaning the same thing after a shard split.
///
/// This test source-scans `src/**/*.rs` + `packages/*/src/**/*.rs` (same file set as
/// `character_owned_tripwire`) for raw scans and fails on any file over its whitelisted budget.
/// The whitelist FREEZES today's legitimate scans; it is a ratchet, so it should only ever shrink
/// as perf-catalog Tier 1 work replaces scans with indexed helpers.
///
// Deliberate simplification: whitelist granularity is (file, count), not (file, line) — line
// numbers churn on every unrelated edit above them and would make this test a merge-conflict
// generator. The ceiling: swapping one whitelisted scan for a different one inside an
// already-whitelisted file slips through. Upgrade path if that ever bites: key on the enclosing
// `fn` name instead.
//
// Deliberate simplification: "raw" is a whole-table read (`.iter()` or `.count()`) reached either
// straight off the accessor OR off a local handle bound from it one line earlier — `let entities =
// ctx.db.game_world_entity();` is this codebase's DOMINANT idiom (~120 bindings vs ~18 inline
// calls), so a scanner that only matched the inline form would wave through most of the real
// scans and would fail exactly nobody who copied the style of the code next to them. Index reads
// (`.by_grid().filter(..)`, `.guid().find(..)`) are already partition-safe or point reads.
// Ceiling: still a text scan — a handle passed to another fn, or an accessor name pasted together
// by a macro, defeats it. Upgrade path: none planned — the tripwire's job is to make the lazy
// paths fail, not to be a sound analysis.
#[cfg(test)]
mod partition_discipline_tripwire {
    use crate::test_scan::{line_of, on_comment_line, repo_root};

    /// Tables whose rows have a world position, i.e. the ones a shard split partitions.
    /// Non-spatial tables (accounts, items, quests, templates) are realm-wide by construction and
    /// are none of this test's business.
    const SPATIAL_ACCESSORS: &[&str] = &[
        "game_world_entity",
        "game_creature_spawn",
        "game_gameobject",
        "game_dynamic_object",
    ];

    /// `(repo-relative path, allowed raw-scan count, why)`. One line of justification each.
    const WHITELIST: &[(&str, usize, &str)] = &[
        // Diagnostics and harness code — never on a gameplay path. A split of the former single
        // `debug.rs` (budget 9, down from 12) into a directory; the 9 raw scans
        // landed in two of the seven files — same total, just split along the new file boundary.
        ("module/src/debug/mod.rs", 9, "Debug fixture and operator migration reducers inspect the whole Shard, including the packed-cell backfill."),
        ("module/src/debug/instance.rs", 3, "Debug fixture setup and floor validation inspect template populations and guid allocation."),
        // Importers — realm-wide by definition: they wipe and rebuild every partition at once.
        ("module/src/creatures/spawn.rs", 4, "World imports replace all authored spawns. Timer normalization and region retirement inspect the complete spawn catalogue."),
        ("module/src/go_collider.rs", 1, "Operator reconciliation rebuilds derived colliders for every hosted partition"),
        ("module/src/gameobject.rs", 2, "`import_gameobjects` drops every gameobject row before reloading the world; plus one by-`template_entry` lookup (an entry-keyed find, not a spatial query — no index on that column)"),
        ("module/src/package_import/creatures.rs", 1, "a Package Delta apply clears the whole Package creature range before it writes, so a Package that left the enabled set takes its spawns with it — realm-wide reconciliation, and the band has no index"),
        ("module/src/package_import/gameobjects.rs", 1, "the same whole-band clear for the Package gameobject range"),
        // Deliberately cross-partition: these ARE the code that manages partitions.
        ("module/src/instance.rs", 8, "instance population spawn/teardown + the reaper's occupied-instance census — classifying every partition at once is the job"),
        ("module/src/encounter.rs", 2, "guid high-water marks over spawn/gameobject TEMPLATE rows when allocating a wave guid, not live-world reads"),
        // Unindexed lookups whose target is co-located with the caller anyway.
        ("module/src/creatures/pet.rs", 1, "`nearest_hostile_near` — KNOWN DEBT, a textbook `entities_near` radius search kept only because pets are rare (one per online warlock)"),
        ("packages/deadmines/src/choreography.rs", 1, "single instance-scoped boss-liveness check inside one encounter"),
        // Split tick.rs into tick/{mod,movement,lifecycle,sense}.rs; this budget-5 entry
        // splits with it, same total (3 + 2 = 5), same reasoning as the pre-split note below.
        ("module/src/creatures/tick/mod.rs", 3, "Active-cell classification snapshots all players. Diagnostic counters measure total rows and narrowed pass candidates."),
        ("module/src/creatures/cycle/ctx.rs", 2, "Aggro snapshots all players and regeneration selects entities with health or power. Neither predicate has its own index."),
        ("module/src/spell/cast/targeting.rs", 1, "AoE/chain target resolution by full-iter + squared distance; a textbook `helpers::entities_near` call (perf-catalog Tier 1)"),
        // Both nearest-trainer scans now route through `helpers::nearest_entity` (an
        // indexed `by_map` scan, partition-scoped) — this file's raw-scan count dropped to 0, so
        // it no longer needs a whitelist entry.
    ];

    /// The text at `byte_idx` opens a whole-table read: `.iter()`, or `.count()` (the same read
    /// with the rows thrown away — `Table::count` is no more partition-scoped than `iter`).
    /// Whitespace is collapsed so a rustfmt-wrapped method chain still matches.
    fn opens_whole_table_read(content: &str, byte_idx: usize) -> bool {
        let tail: String = content[byte_idx..]
            .chars()
            .take(64)
            .filter(|c| !c.is_whitespace())
            .collect();
        tail.starts_with(".iter()") || tail.starts_with(".count()")
    }

    fn raw_scans(content: &str) -> Vec<(usize, &'static str)> {
        crate::test_scan::raw_table_reads(content, SPATIAL_ACCESSORS, opens_whole_table_read)
    }

    #[test]
    fn the_scan_counts_inline_and_bound_handle_scans_but_not_comments() {
        let src =
            "fn f(ctx: &ReducerContext) {\n    for e in ctx.db.game_world_entity().iter() {}\n    \
                   let spawns = ctx.db.game_creature_spawn();\n    let n = spawns.count();\n    \
                   // ctx.db.game_world_entity().iter()\n}\n";
        assert_eq!(
            raw_scans(src),
            vec![(2, "game_world_entity"), (4, "game_creature_spawn")]
        );
    }

    #[test]
    fn no_unwhitelisted_raw_spatial_scans() {
        let root = repo_root();
        let mut violations = Vec::new();
        for file in super::character_owned_tripwire::scanned_files() {
            // tripwires.rs (this file) holds the whitelist and this scanner's own prose; it defines
            // no reducers, so skipping it costs nothing — same carve-out the sibling tripwire makes.
            if file.file_name().and_then(|n| n.to_str()) == Some("tripwires.rs") {
                continue;
            }
            let rel = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let content =
                std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
            let found = raw_scans(&content);
            let allowed = WHITELIST
                .iter()
                .find(|(p, _, _)| *p == rel)
                .map(|(_, n, _)| *n)
                .unwrap_or(0);
            if found.len() > allowed {
                for (line, accessor) in found {
                    violations.push(format!("{rel}:{line} scans {accessor} (budget {allowed})"));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "Invariant: no whole-table scan of a spatial table outside the partition-scoped \
             helpers.\nraw spatial-table scan(s) over budget:\n  {}\n\n\
             A whole-table `.iter()` over a spatial table is not shardable: after a region or continent split it silently sees only the caller's own shard. Use \
             `crate::helpers::entities_near(ctx, map_id, instance_id, x, y, radius)` for a radius \
             search (grid-indexed, `(map_id, instance_id)`-scoped) and \
             `crate::helpers::in_same_partition(&e, map_id, instance_id)` to test partition \
             membership. If the scan is genuinely realm-wide or partition-managing, raise that \
             file's budget in `WHITELIST` (module/src/tripwires.rs) WITH a one-line justification. \
             Whitelisted today: {} scan(s) across {} file(s).",
            violations.join("\n  "),
            WHITELIST.iter().map(|(_, n, _)| n).sum::<usize>(),
            WHITELIST.len(),
        );
    }

    /// The whitelist is a ratchet: an entry whose file no longer needs its full budget (or no
    /// longer exists) must be trimmed, or the budget quietly re-opens the door it was closed for.
    ///
    /// An entry naming an OPTIONAL drop-in package that isn't installed is skipped, not counted as
    /// zero — `crate::test_scan::is_installed` draws that line at the package DIRECTORY, so a file
    /// missing from an INSTALLED package still reports `actual 0` and fails here.
    #[test]
    fn whitelist_has_no_stale_entries() {
        let root = repo_root();
        let mut stale = Vec::new();
        for (rel, allowed, _) in WHITELIST {
            if !crate::test_scan::is_installed(rel) {
                continue;
            }
            let actual = std::fs::read_to_string(root.join(rel))
                .map(|c| raw_scans(&c).len())
                .unwrap_or(0);
            if actual < *allowed {
                stale.push(format!("{rel}: budget {allowed}, actual {actual}"));
            }
        }
        assert!(
            stale.is_empty(),
            "WHITELIST budget(s) larger than the scans that actually exist — lower them:\n  {}",
            stale.join("\n  ")
        );
    }

    // ==========================================================================================
    //  EXTENSION: no module game logic may read a SHARD ID.
    // ==========================================================================================

    /// The only files allowed to touch a shard column: the one that DEFINES the assignment table
    /// and the operator reducer that writes it (`region.rs`), and the one that DEFINES the
    /// per-shard load-sample table and ITS operator reducer (`load.rs`) — `game_shard_load
    /// equality, exactly like `RegionAssignment.shard`. Both write or compare; neither branches on
    /// the value to decide anything gameplay-visible.
    const SHARD_ID_OWNERS: &[&str] = &["module/src/region.rs", "module/src/load.rs"];

    /// The ways module code could reach a shard id: the assignment table's accessor, the `.shard`
    /// column itself (`row.shard`), and the ROW TYPE, because `let RegionAssignment { shard,.. }
    /// = row;` and `match row { RegionAssignment { shard: db,.. } => … }` bind the column without
    /// ever writing a dot (both forms were confirmed to slip past the first two). You cannot
    /// destructure a type you may not name. `game_character_shard` / `CharacterShard` deliberately
    /// do NOT count — that table stores a `(map_id, instance_id)` LOCATION precisely so nothing has
    /// to name a database. `game_shard_load` / `ShardLoad` get the same destructuring
    /// protection as `RegionAssignment` — they carry a shard-name LABEL an ops sample is filed
    /// under, not a location, but the same "cannot destructure a type you may not name" argument
    /// applies to it too.
    const SHARD_ID_FORMS: &[&str] = &[
        "game_region_assignment",
        ".shard",
        "RegionAssignment",
        "game_shard_load",
        "ShardLoad",
    ];

    /// Every `(line, form)` where a file names a shard id. `.shard` must be a whole field access,
    /// so `.shard_name`/`.shard_index` (gateway vocabulary, if it ever appears in prose) and
    /// `game_character_shard` (no dot before `shard`) are not hits.
    fn shard_id_reads(content: &str) -> Vec<(usize, &'static str)> {
        let mut out = Vec::new();
        for form in SHARD_ID_FORMS {
            for (idx, _) in content.match_indices(form) {
                if on_comment_line(content, idx) {
                    continue;
                }
                let after_ok = content[idx + form.len()..]
                    .chars()
                    .next()
                    .map(|c| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(true);
                if after_ok {
                    out.push((line_of(content, idx), *form));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn the_scan_finds_a_shard_field_read_but_not_prose_or_longer_names() {
        let src = "fn f(ctx: &ReducerContext, row: Row) {\n    if row.shard == DB {}\n    \
                   // row.shard is prose\n    let name = row.shard_name;\n    \
                   let loc = ctx.db.game_character_shard();\n}\n";
        assert_eq!(shard_id_reads(src), vec![(2, ".shard")]);
    }

    /// of **region definitions are data; shard ids are the gateway's business.** The
    /// module stores `region_assignment { map_id, region_id, shard, epoch }` (it has to — realm-core
    /// is this same wasm under another database name), but the instant a reducer *branches* on
    /// `shard` the module stops being relocatable: the same code on two databases would take two
    /// different paths, and a region migration would change gameplay instead of changing routing.
    /// Regions and cells are fair game anywhere; the database name is not.
    /// The module's own `src` + `packages/*` (what the sibling tripwires scan) **plus
    /// `crates/lyracore-shared/src`**, which is compiled INTO this module and therefore just as capable
    /// of naming a database.
    ///
    /// Moved `DUNGEON_MAPS` out of `module/src/instance.rs` into `lyracore_shared::instance` so
    /// the gateway and the module could not disagree about which maps are dungeons. A map-id set is a
    /// world fact and entirely fine here — but the move quietly put a piece of module logic in a crate
    /// this tripwire did not look at, so "no module game logic reads a shard id" would have stopped
    /// being enforceable for anything that followed it there. It is scanned now. (Only THIS tripwire
    /// is extended: the spatial-scan and character-owned tripwires are about `#[table]` definitions
    /// and spatial iteration, neither of which exists in a `no_std`-shaped shared crate.)
    fn shard_id_scanned_files() -> Vec<std::path::PathBuf> {
        let mut files = super::character_owned_tripwire::scanned_files();
        let shared = repo_root().join("crates/lyracore-shared/src");
        if shared.is_dir() {
            super::character_owned_tripwire::collect_rs_files(&shared, &mut files);
        }
        files
    }

    #[test]
    fn no_module_game_logic_reads_a_shard_id() {
        let root = repo_root();
        let mut violations = Vec::new();
        for file in shard_id_scanned_files() {
            if file.file_name().and_then(|n| n.to_str()) == Some("tripwires.rs") {
                continue; // this file holds the scanner's own vocabulary (see the sibling tests)
            }
            let rel = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            if SHARD_ID_OWNERS.contains(&rel.as_str()) {
                continue;
            }
            let content =
                std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
            for (line, form) in shard_id_reads(&content) {
                violations.push(format!("{rel}:{line} names a shard id (`{form}`)"));
            }
        }
        assert!(
            violations.is_empty(),
            "Invariant: no Module game logic reads a shard id.\n\
             module code outside {SHARD_ID_OWNERS:?} reads a SHARD ID:\n  {}\n\n\
             A shard is a database name, a GATEWAY routing fact. Module \
             game logic may read cells (`lyracore_shared::spatial`) and regions \
             (`lyracore_shared::region::RegionMap`), and may partition by `(map_id, instance_id)` via \
             `crate::helpers`, but must never branch on which database owns a region: that is what \
             lets the same wasm run on every shard and lets a region move between shards without a \
             code change. Route in `gateway/src/config.rs` instead.",
            violations.join("\n  ")
        );
    }

    /// The owner list is a ratchet too: a file that no longer touches a shard id must come off it,
    /// or it keeps a hole open for the next person who copies the code next to them.
    #[test]
    fn shard_id_owner_list_has_no_stale_entries() {
        let root = repo_root();
        let stale: Vec<&str> = SHARD_ID_OWNERS
            .iter()
            .copied()
            .filter(|rel| {
                std::fs::read_to_string(root.join(rel)).is_ok_and(|c| shard_id_reads(&c).is_empty())
            })
            .collect();
        assert!(
            stale.is_empty(),
            "SHARD_ID_OWNERS names file(s) that no longer touch a shard id — remove them: {stale:?}"
        );
    }
}

/// ENFORCEMENT tripwire (part of the elastic-world-sharding spec): a reducer that
/// reaches a character by GUID or by NAME straight into `game_character` touches none of the
/// escrowed-transfer chokepoints — not `helpers::entity_by_owner` (the actor side), not
/// `world::player_login` (re-materialisation), not `begin_transfer`'s delete of the live entity
/// (the target side). Same-database that is harmless: the write lands on the SAME row the
/// destination reads. Cross-database each one is a LOST WRITE, because the export blob was
/// serialized at `begin_transfer` and the mutation dies with the source copy.
///
/// The fence is `helpers::character_by_guid` / `character_by_name`, which read an in-transit
/// character as ABSENT so each caller's existing "no such character" arm fires. This test
/// source-scans the same file set as its two sibling tripwires for RAW character lookups and fails
/// on any file over its whitelisted budget. `WHITELIST` FREEZES today's audited exceptions in
/// Core's own source, each with a verdict (see the verdict table in `transfer/mod.rs`'s module doc);
/// it is a ratchet, so it should only ever shrink. `PACKAGE_FILE_BUDGET` carries the same budget
/// for an installed Package's file, keyed by file shape instead of by Package, so Core names no
/// Package.
#[cfg(test)]
mod character_fence_tripwire {
    use crate::test_scan::repo_root;

    /// `(repo-relative path, allowed raw-lookup count, verdict + why)`. One line each; every entry
    /// is an audited exception from the by-guid verdict table in `module/src/transfer/mod.rs`.
    /// Core's own files only — an installed Package's file is never named here; see
    /// `PACKAGE_FILE_BUDGET` below.
    const WHITELIST: &[(&str, usize, &str)] = &[
        ("module/src/account_ownership.rs", 2, "Account fencing reads ownership even during Transfer. These two reads only check the Account name before fencing or removing a live entity; Character rows and Transfer records remain intact."),
        // THE GATE ITSELF.
        ("module/src/helpers.rs", 2, "`character_by_guid` + `character_by_name` — the fence; these two ARE the raw lookups everything else routes through"),
        // TRANSFER MACHINERY — must read the row the fence hides, or it could not move it.
        ("module/src/transfer/mod.rs", 4, "Transfer reads the fenced Character for export, destination materialization, durability checks, and the in-transit Instance census."),
        ("module/src/world.rs", 8, "player_login carries its own `is_in_transit` fence (the re-materialisation chokepoint, 2 reads); teleport_player, set_home, persist_entity and is_gm_character take a guid already resolved through a fenced entity (persist_entity is called BY begin_transfer); debug_delete_character keeps a raw existence probe so a missing character stays a no-op while an in-transit one is REFUSED, with the fence on the same expression; cascade_delete_character's raw row DELETE is the sweep that probe guards. `recall_to_home` is now FENCED (character_by_guid) — it was the one teleport_player caller needing no live entity"),
        // OPEN, not decided — spec puts group MEMBERSHIP state on realm-core, settled.
        ("module/src/group.rs", 3, "Group accept, uninvite, and leave read member identity while changing membership, which is authoritative on Realm-core."),
        // REGENERATE at the destination — connection-derived state, never carried in the blob.
        ("module/src/auth.rs", 4, "Character creation checks name uniqueness and initializes the guid allocator. Character deletion distinguishes absence, ownership, and the Transfer fence."),
        // READS, not writes: name/class/race/identity lookups that mutate nothing on the character.
        ("module/src/items/ops.rs", 2, "race/class reads for the starter loadout and the mana-class gate — no write to the character"),
        ("module/src/spell/cast/targeting.rs", 1, "caster NAME for the resurrect prompt — no write"),
        ("module/src/reputation.rs", 1, "owner_identity fallback for RLS visibility of a new rep row — no write to the character"),
        ("module/src/stats.rs", 2, "set_character_level reads (race, class) and writes level/xp for a guid the caller already resolved through a fenced entity (gm_command's entity_by_owner) or through the FENCED `debug::debug_set_level`; the core itself also serves guids with no character row, so the gate belongs at its callers"),
        ("module/src/xp.rs", 1, "rested-pool drain keyed on the ATTACKER's guid, which came from a live entity (kill credit)"),
        ("module/src/rest.rs", 2, "the rest-state flip takes a live `mover` entity; the 30s accrual pass is BACKGROUND (no entity anywhere in it) and so carries its OWN `is_in_transit` fence — begin_transfer persists with set_offline:false, which is the branch that would have stopped the rest clock, so an escrowed character keeps resting==true and stays in the scan"),
        ("module/src/talent.rs", 1, "the guid comes from the caller's own entity_by_owner-resolved entity"),
        ("module/src/gm.rs", 2, "set_gm_level is FENCED (character_by_name); the two raw reads are gm_command's own gm_level probe and its .money write, both on the caller's entity_by_owner-resolved guid"),
        ("module/src/loot/mod.rs", 1, "DEFER: credit_purse writes the durable row AFTER folding the delta into the escrowed blob (`transfer::defer_money_delta`) — refusing would drop a third party's copper"),
        ("module/src/operations.rs", 1, "`debug_repair_after_publish`'s gm-tester backfill (guid 1); every debug WRITER that touches character state is fenced"),
    ];

    /// `(path relative to a Package's own root, allowed raw-lookup count, verdict + why)` — keyed
    /// this way so Core names no Package. No staleness ratchet: it names a file shape, not one
    /// installed Package, so there is no single file to measure it against.
    const PACKAGE_FILE_BUDGET: &[(&str, usize, &str)] = &[(
        "src/mod.rs",
        2,
        "roster bookkeeping over rows this same reducer just created: the free-name probe and the post-create fetch",
    )];

    /// If `rel` sits under `packages/<pkg>/`, its path relative to that Package's own root —
    /// `packages/example/src/mod.rs` → `Some("src/mod.rs")`. `None` for a Core file.
    fn package_relative_path(rel: &str) -> Option<&str> {
        let after = rel.strip_prefix("packages/")?;
        let (_pkg, tail) = after.split_once('/')?;
        Some(tail)
    }

    /// The raw-lookup budget for `rel`: a Package file is checked against `PACKAGE_FILE_BUDGET` by
    /// its path relative to the Package root; a Core file is checked against `WHITELIST` by its
    /// full repo-relative path. Either way, an unlisted file gets zero.
    fn budget_for(rel: &str) -> usize {
        if let Some(pkg_rel) = package_relative_path(rel) {
            return PACKAGE_FILE_BUDGET
                .iter()
                .find(|(p, _, _)| *p == pkg_rel)
                .map(|(_, n, _)| *n)
                .unwrap_or(0);
        }
        WHITELIST
            .iter()
            .find(|(p, _, _)| *p == rel)
            .map(|(_, n, _)| *n)
            .unwrap_or(0)
    }

    /// The lookup forms that reach a character row raw. `.guid().find(` and `.name().find(` are the
    /// indexed point reads; `.iter()` is the case-folding name scan (`character_by_name`'s
    /// convention) and the account-scoped counts. `.guid().update(` is deliberately NOT here: the
    /// row it writes was already resolved by one of these, so the READ is the chokepoint.
    const LOOKUP_FORMS: &[&str] = &[
        ".guid().find(",
        ".name().find(",
        ".guid().delete(",
        ".iter()",
    ];

    /// The one accessor this tripwire watches.
    const ACCESSOR: &[&str] = &["game_character"];

    fn opens_raw_lookup(content: &str, byte_idx: usize) -> bool {
        let tail: String = content[byte_idx..]
            .chars()
            .take(64)
            .filter(|c| !c.is_whitespace())
            .collect();
        LOOKUP_FORMS.iter().any(|f| tail.starts_with(f))
    }

    fn raw_lookups(content: &str) -> Vec<usize> {
        let mut out: Vec<usize> =
            crate::test_scan::raw_table_reads(content, ACCESSOR, opens_raw_lookup)
                .into_iter()
                .map(|(line, _)| line)
                .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    #[test]
    fn the_scan_counts_raw_lookups_and_deletes_but_not_the_fence() {
        let src = "fn f(ctx: &ReducerContext, guid: u64) {\n    \
                   let c = ctx.db.game_character().guid().find(guid);\n    \
                   let chars = ctx.db.game_character();\n    chars.guid().delete(guid);\n    \
                   let fenced = crate::helpers::character_by_guid(ctx, guid);\n}\n";
        assert_eq!(raw_lookups(src), vec![2, 4]);
    }

    #[test]
    fn no_unwhitelisted_raw_character_lookups() {
        let root = repo_root();
        let mut violations = Vec::new();
        for file in super::character_owned_tripwire::scanned_files() {
            // tripwires.rs holds this scanner's own prose and defines no reducers — same carve-out
            // both sibling tripwires make.
            if file.file_name().and_then(|n| n.to_str()) == Some("tripwires.rs") {
                continue;
            }
            let rel = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let content =
                std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
            let found = raw_lookups(&content);
            let allowed = budget_for(&rel);
            if found.len() > allowed {
                violations.push(format!(
                    "{rel}: {} raw game_character lookup(s) (budget {allowed}) at line(s) {found:?}",
                    found.len()
                ));
            }
        }

        assert!(
            violations.is_empty(),
            "Invariant: every Character lookup goes through the transfer fence.\n\
             raw `game_character` lookup(s) outside the by-guid chokepoint:\n  {}\n\n\
             A reducer that reaches a character by guid or by name passes NONE of the escrowed-\
             transfer chokepoints, so cross-database it can write to a source \
             copy the destination already serialized past — a lost write. Route the READ through \
             `crate::helpers::character_by_guid(ctx, guid)` or \
             `crate::helpers::character_by_name(ctx, name)`; both read an in-transit character as \
             absent, so your existing not-found arm fires and no new error string reaches the \
             gateway. If refusal is the WRONG answer for your path (it would drop a third party's \
             value, or the field is connection-derived and regenerated at the destination), pick the \
             DEFER or REGENERATE verdict instead and add the site WITH its verdict — to `WHITELIST` \
             (module/src/tripwires.rs) for a Core file, or to `PACKAGE_FILE_BUDGET` for a Package \
             file — see the table in `module/src/transfer/mod.rs`'s module doc. Whitelisted today: \
             {} Core lookup(s) across {} Core file(s), plus {} Package lookup(s) across {} Package \
             file shape(s).",
            violations.join("\n  "),
            WHITELIST.iter().map(|(_, n, _)| n).sum::<usize>(),
            WHITELIST.len(),
            PACKAGE_FILE_BUDGET.iter().map(|(_, n, _)| n).sum::<usize>(),
            PACKAGE_FILE_BUDGET.len(),
        );
    }

    /// Ratchet: a `WHITELIST` entry whose file no longer needs its full budget must be trimmed, or
    /// the budget quietly re-opens the door it was closed for. `WHITELIST` holds Core files only
    /// (never optional), so every entry is checked unconditionally; `PACKAGE_FILE_BUDGET` carries no
    /// matching ratchet — see its own doc comment for why.
    #[test]
    fn character_whitelist_has_no_stale_entries() {
        let root = repo_root();
        let mut stale = Vec::new();
        for (rel, allowed, _) in WHITELIST {
            let actual = std::fs::read_to_string(root.join(rel))
                .map(|c| raw_lookups(&c).len())
                .unwrap_or(0);
            if actual < *allowed {
                stale.push(format!("{rel}: budget {allowed}, actual {actual}"));
            }
        }
        assert!(
            stale.is_empty(),
            "WHITELIST budget(s) larger than the lookups that actually exist — lower them:\n  {}",
            stale.join("\n  ")
        );
    }
}

/// ENFORCEMENT tripwire: `gc.rs` has a hand-maintained `reap!` list. It covers the
/// fifteen event tables reaped on their 1s `EVENT_TTL_MICROS` and the handful of tables with their
/// own ad-hoc TTL block below it. A NEW short-lived event table
/// that forgets to add its line leaks forever, silently: nothing else ever deletes its rows.
///
/// This scans every `#[table]` struct across the compiled tree (same file set and same
/// `extract_tables` as `character_owned_tripwire`) for the `id: u64` PK + `created_at: Timestamp`
/// shape every existing TTL-reaped table uses, and fails if `gc.rs`'s own source never actually
/// reaps that accessor — either via `reap!(<accessor>)` or a direct `ctx.db.<accessor>()` call (the
/// shape the ad-hoc blocks and the sweep fns `gc.rs` calls out to use). A bare substring match on
/// the accessor NAME would be satisfied by a comment mentioning it (this file's own `gc.rs` carries
/// exactly that kind of comment about `game_creature_move_event`, explaining why it is no longer
/// reaped), so the check is deliberately narrower than "the name appears somewhere".
///
/// Caught on landing: `game_rest_state_event` (`rest.rs`) carries this exact shape and its own doc
/// comment calls it "a one-shot relay row with a GC TTL" (`transfer/transport.rs`'s
/// `NOT_TRANSPORTED` table), but no line in `gc.rs` ever reaped it — every rest-area threshold
/// crossing for the lifetime of a character left one more row behind. Fixed in the same change by
/// adding it to the `reap!` list.
#[cfg(test)]
mod gc_reap_tripwire {
    /// Accessors carrying the `id: u64` + `created_at: Timestamp` TTL shape that are deliberately
    /// NOT reaped by `gc.rs`:
    /// - `game_creature_move_event`: dead — nothing writes it any more (perf catalog 2.1 moved
    ///   creature legs onto the in-place `game_creature_spline` row, updated rather than
    ///   inserted/reaped); the table stays in the schema, empty, rather than as a separate
    ///   destructive migration to drop it. See the comment atop `reap_movement_events` in `gc.rs`.
    /// - `game_mail`: DURABLE state whose `created_at` starts its life. Each Mail's own Mail Timer
    ///   ends that life (`mail_timer.rs`). A TTL reap would delete Mail that Mail Expiry returns.
    const EXEMPT_ACCESSORS: &[&str] = &[
        "game_creature_move_event",
        // Retired with the shard-local channel path: nothing writes it.
        "game_channel_event",
        "game_mail",
        // Durable source-side work. The exact Gateway completion deletes it; a transfer source
        // deletion retains it so process restart can finish the destination release.
        "game_bot_transfer_intent",
    ];

    /// `gc.rs` reaps `accessor` through `reap!(accessor)` or a direct `ctx.db.accessor()` call.
    /// Comments and string literals do not count.
    fn gc_reaps(gc_code: &str, accessor: &str) -> bool {
        gc_code.contains(&format!("reap!({accessor})"))
            || gc_code.contains(&format!("ctx.db.{accessor}()"))
    }

    /// The `id: u64` primary key plus `created_at: Timestamp` shape every TTL-reaped table uses.
    /// Full field names, because `id: u64` alone is a substring of every `*_guid: u64` field.
    fn is_ttl_shaped(fields: &str) -> bool {
        fields.contains("pub id: u64,")
            && (fields.contains("pub created_at: Timestamp,")
                || fields.contains("pub created_at: spacetimedb::Timestamp,"))
    }

    /// Every TTL-shaped, non-exempt table declared in `content` that `gc_src` never reaps.
    fn unreaped_tables(content: &str, gc_src: &str) -> Vec<String> {
        let gc_code = super::character_owned_tripwire::production_code(gc_src);
        super::character_owned_tripwire::extract_tables(content)
            .into_iter()
            .filter(|(accessor, fields)| {
                is_ttl_shaped(fields)
                    && !EXEMPT_ACCESSORS.contains(&accessor.as_str())
                    && !gc_reaps(&gc_code, accessor)
            })
            .map(|(accessor, _)| accessor)
            .collect()
    }

    #[test]
    fn every_ttl_shaped_table_is_reaped_by_gc_rs() {
        let gc_src = crate::test_scan::read_scanned("module/src/gc.rs")
            .expect("module/src/gc.rs ships in every checkout — module/ is never optional");

        let mut missing = Vec::new();
        for file in super::character_owned_tripwire::scanned_files() {
            // gc.rs defines no table of its own; nothing to check it against itself.
            if file.file_name().and_then(|n| n.to_str()) == Some("gc.rs") {
                continue;
            }
            let content = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
            for accessor in unreaped_tables(&content, &gc_src) {
                missing.push(format!("{accessor} (in {})", file.display()));
            }
        }

        assert!(
            missing.is_empty(),
            "Invariant: every TTL-shaped event table is reaped in gc.rs.\n\
             table(s) shaped `id: u64` PK + `created_at: Timestamp` — the TTL-reaped convention \
             every existing short-lived event table uses — with no reap in gc.rs: {missing:?}\n\n\
             A short-lived event table only stops growing without bound if something deletes its \
             rows. Add `reap!(<accessor>);` to `reap_movement_events` in `gc.rs` (the 1s \
             `EVENT_TTL_MICROS` default), or — if the table's lifecycle needs its own policy, like \
             `game_group_invite`'s 2-minute dialog window or `game_corpse`'s decay-to-bones — a \
             dedicated block or sweep fn called from there (see `corpse::sweep_corpse_decay` for the \
             pattern: gc.rs calls it, it owns the policy). OR add the accessor to `EXEMPT_ACCESSORS` \
             above with a comment justifying why it is intentionally unreaped."
        );
    }

    #[test]
    fn the_scan_flags_a_ttl_table_that_gc_only_mentions_in_a_comment() {
        let table = "#[table(accessor = game_fixture_event)]\n\
                     pub struct FixtureEvent {\n    #[primary_key]\n    pub id: u64,\n    \
                     pub created_at: Timestamp,\n}\n";
        let mentioned = "fn reap(ctx: &ReducerContext) {\n    // reap!(game_fixture_event);\n}\n";
        assert_eq!(
            unreaped_tables(table, mentioned),
            vec!["game_fixture_event"]
        );

        let reaped = "fn reap(ctx: &ReducerContext) {\n    reap!(game_fixture_event);\n}\n";
        assert!(unreaped_tables(table, reaped).is_empty());
    }
}

// =================================================================================================
//  Every `grid_x` write is accompanied by the packed `cell` write
// =================================================================================================

/// The four AOI-scoped tables carry a `cell` column that packs `(grid_x, grid_y)` into one indexed
/// value, because that is the only shape SpacetimeDB 2.7.1's subscription planner can serve from an
/// index. It is therefore a DENORMALIZATION, and the usual denormalization hazard applies with an
/// unusually nasty failure mode: a row whose `cell` disagrees with its `grid_x`/`grid_y` is not
/// slow, it is in the WRONG PLACE. The AOI subscription probes `cell`, so such a row is invisible to
/// every player standing on it and visible to whoever happens to occupy the cell it wrongly claims.
///
/// Nothing in the type system couples them: `grid_x`, `grid_y` and `cell` are three independent
/// columns, and the census behind this tripwire found **24 independent write sites** across 8 files, with no
/// shared constructor for `game_world_entity` re-stamps (7 hand-rolled `e.grid_x =..` runs) or for
/// `game_gameobject` (9 independent struct literals). Adding a 25th and forgetting the third line
/// compiles clean, passes every existing test, and shows up live as entities that vanish.
///
/// So: this scan requires every `grid_x` WRITE, a `grid_x:..` struct-literal field or a
/// `<recv>.grid_x =..` assignment, to have a `cell` write within the same short window of source
/// text. It is deliberately textual and deliberately local: the point is to fail the build next to
/// the line someone just wrote, not to prove a semantic property.
#[cfg(test)]
pub(crate) mod grid_cell_tripwire {
    use crate::test_scan::{line_of, on_comment_line};

    /// How far after a `grid_x` write the matching `cell` write may sit. Every real site writes the
    /// three within a few adjacent lines; 400 characters is roomy enough for a doc comment between
    /// them and far too tight to reach an unrelated statement.
    const WINDOW: usize = 400;

    /// Files whose `grid_x` mentions are reads, table DEFINITIONS, or fixtures for a table with no
    /// `cell` column at all — assembled at run time so this list cannot match itself.
    fn is_exempt(path: &str) -> bool {
        // `helpers.rs` reads the grid off rows (`grid_of`, `entity_addr`) and its ONE fixture writer
        // is covered; the exemptions below are files where `grid_x` appears only in a signature,
        // a tuple destructure or a comment.
        ["module/src/tripwires.rs", "module/src/region.rs"]
            .iter()
            .any(|p| path.ends_with(p) || path.replace('\\', "/").ends_with(p))
    }

    /// The AOI-scoped tables' own `#[table]` definitions declare `pub grid_x: i32` next to a `cell`
    /// column further down the struct — further than WINDOW, and not a "write" in any case. Skip a
    /// match that is a struct FIELD DECLARATION rather than an initializer or assignment.
    fn is_field_declaration(code: &str, idx: usize) -> bool {
        let line_start = code[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = code[idx..]
            .find('\n')
            .map(|i| idx + i)
            .unwrap_or(code.len());
        let line = code[line_start..line_end].trim();
        line.starts_with("pub grid_x") || line.starts_with("grid_x: i32")
    }

    /// `true` if the nearest non-whitespace character before `idx` is `{` or `,` — i.e. `grid_x,` at
    /// `idx` sits in a struct-literal FIELD position (first field right after the opening brace, or
    /// any field after a preceding one), not buried inside some other expression. This is what
    /// actually distinguishes the struct-literal shorthand from an ordinary function-call argument
    /// (`grid_cell_id(grid_x, grid_y)`: the character before that `grid_x` is `(`, not `{`/`,`) —
    /// deliberately independent of whether the field shares its line with other fields, so both
    /// this repo's one-field-per-line style AND a compact `E { grid_x, grid_y }` one-liner match.
    fn is_field_position(code: &str, idx: usize) -> bool {
        let bytes = code.as_bytes();
        let mut i = idx;
        while i > 0 {
            let c = bytes[i - 1];
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                i -= 1;
                continue;
            }
            return c == b'{' || c == b',';
        }
        false
    }

    /// `impl` block a `Self {.. }` struct literal at `idx` resolves to. Walks backward for the
    /// nearest `impl` keyword, then takes `impl<..> TypeName` or, for a trait impl, the identifier
    /// after ` for ` (`impl Trait for TypeName`) — the same rule `Self` follows in the language.
    /// A near-clone of the `impl<..>` generics-skip is unavoidable here (no shared engine, unlike
    /// `raw_table_reads`) because this walks forward from a keyword instead of backward from a brace.
    fn resolve_self_type(code: &str, idx: usize) -> Option<&str> {
        let impl_pos = code[..idx].rfind("impl")?;
        let after = code[impl_pos + "impl".len()..].trim_start();
        let bytes = after.as_bytes();
        let mut i = 0usize;
        if bytes.first() == Some(&b'<') {
            let mut depth = 0i32;
            while i < bytes.len() {
                match bytes[i] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        i += 1;
                        if depth == 0 {
                            break;
                        }
                        continue;
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        let rest = after[i..].trim_start();
        fn ident_at(s: &str) -> Option<&str> {
            let end = s
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(s.len());
            if end == 0 {
                None
            } else {
                Some(&s[..end])
            }
        }
        if let Some(for_pos) = rest.find(" for ") {
            return ident_at(rest[for_pos + " for ".len()..].trim_start());
        }
        ident_at(rest)
    }

    /// Walks backward from `idx` tracking `{`/`}` depth to find the nearest enclosing brace, then
    /// takes the identifier immediately before it (skipping whitespace). Returns that identifier
    /// only if it looks like a type name (starts uppercase, matching this codebase's convention) —
    /// `TypeName {.. }` or `TypeName {..prev }`, so `None` correctly falls out for a shorthand
    /// `grid_x,` that is really an ordinary function-CALL argument (`queue_motion(.., grid_x,
    /// grid_y,..)`: its nearest enclosing `{` is a `fn`/`if`/`match` body brace, not preceded by a
    /// type name) or any other bare code block. A literal `Self {.. }` constructor resolves through
    /// [`resolve_self_type`] to the `impl` block's real type name instead of returning `"Self"`
    /// verbatim — `struct_has_cell_field` searches for `struct Self`, which never exists.
    fn enclosing_struct_literal_type(code: &str, idx: usize) -> Option<&str> {
        let bytes = code.as_bytes();
        let mut depth = 0i32;
        let mut i = idx;
        while i > 0 {
            i -= 1;
            match bytes[i] {
                b'}' => depth += 1,
                b'{' => {
                    if depth > 0 {
                        depth -= 1;
                        continue;
                    }
                    let mut end = i;
                    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
                        end -= 1;
                    }
                    let mut start = end;
                    while start > 0 {
                        let c = bytes[start - 1];
                        if c.is_ascii_alphanumeric() || c == b'_' {
                            start -= 1;
                        } else {
                            break;
                        }
                    }
                    if start == end {
                        return None; // no identifier right before `{` — a bare code block
                    }
                    let ident = &code[start..end];
                    if ident == "Self" {
                        return resolve_self_type(code, i);
                    }
                    return ident
                        .chars()
                        .next()
                        .filter(|c| c.is_ascii_uppercase())
                        .map(|_| ident);
                }
                _ => {}
            }
        }
        None
    }

    /// `true` if `code` (a single file's text) declares `struct TYPE_NAME {.. pub cell: i64.. }`
    /// somewhere — i.e. `TYPE_NAME` is one of the cell-bearing rows this tripwire polices. Every
    /// cell-bearing type's struct literals in this codebase live in the same file as its own `#[table]`
    /// definition, so a same-file search is enough; a type this can't find (or that genuinely has no
    /// `cell` column — several event-log tables carry `grid_x`/`grid_y` as a plain `by_grid` btree
    /// index with no packed `cell` at all, by design) is treated as "not this tripwire's business",
    /// not as a violation.
    fn struct_has_cell_field(code: &str, type_name: &str) -> bool {
        let needle = format!("struct {type_name}");
        let mut search_from = 0usize;
        while let Some(rel) = code[search_from..].find(&needle) {
            let start = search_from + rel;
            let after = start + needle.len();
            let boundary_ok = code[after..]
                .chars()
                .next()
                .map(|c| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(true);
            if boundary_ok {
                if let Some(rel_brace) = code[after..].find('{') {
                    let body_start = after + rel_brace;
                    let mut depth = 0i32;
                    let mut body_end = None;
                    for (i, c) in code[body_start..].char_indices() {
                        match c {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    body_end = Some(body_start + i);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some(end) = body_end {
                        if code[body_start..=end].contains("pub cell: i64") {
                            return true;
                        }
                    }
                }
            }
            search_from = start + needle.len();
        }
        false
    }

    /// Every line where `code` writes `grid_x` — via `grid_x: value` (an explicit initializer or
    /// assignment target), `.grid_x = value` (a field assignment), or bare field-init SHORTHAND
    /// `grid_x,` — with no `cell` write within `WINDOW` characters after it. Shared by the
    /// real-file scan below and its own fixture test, so both exercise the identical matching logic.
    ///
    /// The shorthand form gets two extra gates the explicit forms don't need, because unlike
    /// `grid_x: ` / `.grid_x = ` its literal text also shows up somewhere that ISN'T a struct-literal
    /// write: `grid_x,` must sit in a struct-literal FIELD position (`is_field_position` — the
    /// nearest non-whitespace character before it is `{` or `,`; excludes a read like
    /// `grid_cell_id(grid_x, grid_y)`, whose preceding character is `(`, regardless of whether the
    /// field shares a line with others), AND its enclosing struct-literal type must actually declare
    /// a `cell` column (`enclosing_struct_literal_type` + `struct_has_cell_field` — excludes both
    /// ordinary function-call arguments, whose enclosing brace is a fn/if/match body not a type name,
    /// and the several event-log tables that carry `grid_x`/`grid_y` with no `cell` column at all,
    /// by design). `enclosing_struct_literal_type` resolves a literal `Self {.. }` constructor
    /// through `resolve_self_type` to the `impl` block's real type name, so a constructor written as
    /// `Self {.. }` inside `impl E {.. }` is policed exactly like `E {.. }` would be.
    fn grid_x_writes_missing_cell(code: &str) -> Vec<usize> {
        // Assembled at run time — a contiguous literal would match this file's own text.
        let field_init = format!("{}{}", "grid_x", ": ");
        let assign = format!("{}{}", ".grid_x", " = ");
        let shorthand = format!("{}{}", "grid_x", ",");
        let cell_token = "cell";

        let mut violations = Vec::new();
        for needle in [field_init.as_str(), assign.as_str(), shorthand.as_str()] {
            for (idx, _) in code.match_indices(needle) {
                if on_comment_line(code, idx) || is_field_declaration(code, idx) {
                    continue;
                }
                if needle == shorthand.as_str() {
                    if !is_field_position(code, idx) {
                        continue;
                    }
                    let Some(type_name) = enclosing_struct_literal_type(code, idx) else {
                        continue;
                    };
                    if !struct_has_cell_field(code, type_name) {
                        continue;
                    }
                }
                let mut end = (idx + WINDOW).min(code.len());
                // The window is a byte count; step past a multi-byte char it would otherwise split.
                while !code.is_char_boundary(end) {
                    end += 1;
                }
                if !code[idx..end].contains(cell_token) {
                    violations.push(line_of(code, idx));
                }
            }
        }
        violations
    }

    #[test]
    fn every_grid_x_write_also_writes_the_packed_cell() {
        let mut violations = Vec::new();
        for file in super::character_owned_tripwire::scanned_files() {
            let display = file.display().to_string();
            if is_exempt(&display) {
                continue;
            }
            let code = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("cannot read {display}: {e}"));
            for line in grid_x_writes_missing_cell(&code) {
                violations.push(format!("{display}:{line}"));
            }
        }

        assert!(
            violations.is_empty(),
            "Invariant: a grid-coordinate write also writes the packed `cell`.\n\
             a `grid_x` write with no `cell` write beside it:\n  {}\n\n\
             `cell` packs `(grid_x, grid_y)` into the one indexed value the AOI subscription probes \
             by equality — it is what makes the box query index-served instead of a full partition \
             scan. It is a plain column, so nothing updates it for you: write it in the SAME \
             statement run that writes `grid_x`/`grid_y`, via \
             `lyracore_shared::spatial::grid_cell_id(grid_x, grid_y)` (or `cell_id_at(x, y)` when \
             you have the world position rather than the cell). A row whose `cell` disagrees with \
             its grid columns is not merely slow — it is addressed to the WRONG CELL, so it is \
             invisible to the players standing on it.",
            violations.join("\n  ")
        );
    }

    /// The tripwire is only worth anything if it would actually fire. Feed it the shape it hunts.
    #[test]
    fn the_scan_rejects_a_grid_write_with_no_cell_beside_it() {
        let bad = "fn f(e: &mut E) {\n    e.grid_x = gx;\n    e.grid_y = gy;\n}\n";
        let needle = format!("{}{}", ".grid_x", " = ");
        let idx = bad.find(&needle).expect("fixture must contain the shape");
        let end = (idx + WINDOW).min(bad.len());
        assert!(
            !bad[idx..end].contains("cell"),
            "the fixture is supposed to be a VIOLATION"
        );
        let good = "fn f(e: &mut E) {\n    e.grid_x = gx;\n    e.grid_y = gy;\n    \
                    e.cell = spatial::grid_cell_id(gx, gy);\n}\n";
        let idx = good.find(&needle).unwrap();
        let end = (idx + WINDOW).min(good.len());
        assert!(
            good[idx..end].contains("cell"),
            "the accompanied form must PASS"
        );
    }

    /// The tripwire's original `grid_x: ` needle never saw field-init SHORTHAND
    /// (`grid_x,`, no `: value`) — what an initializer built from same-named locals uses, the
    /// idiom `motion.rs` uses throughout. This feeds `grid_x_writes_missing_cell` a struct literal
    /// written that way and asserts a broken one (no `cell` field) is CAUGHT, a correct one PASSES,
    /// and a `grid_x,` that is merely a function-call ARGUMENT (a read, not a struct-literal field)
    /// is not mistaken for a write.
    #[test]
    fn the_scan_catches_a_broken_shorthand_initializer() {
        // `struct E` declares a `cell` column, so this tripwire is E's business — mirrors a real
        // cell-bearing table's own file, where the `#[table]` struct and every construction site
        // live together.
        let cell_bearing_struct =
            "struct E {\n    pub grid_x: i32,\n    pub grid_y: i32,\n    pub cell: i64,\n}\n";

        let bad = format!(
            "{cell_bearing_struct}fn f() -> E {{\n    E {{\n        grid_x,\n        grid_y,\n    }}\n}}\n"
        );
        assert_eq!(
            grid_x_writes_missing_cell(&bad).len(),
            1,
            "a shorthand `grid_x,` field with no `cell` field beside it must be a VIOLATION"
        );

        let good = format!(
            "{cell_bearing_struct}fn f() -> E {{\n    E {{\n        grid_x,\n        grid_y,\n        cell,\n    }}\n}}\n"
        );
        assert!(
            grid_x_writes_missing_cell(&good).is_empty(),
            "a shorthand initializer that also sets `cell` must PASS"
        );

        let read = "fn f() {\n    let cell = grid_cell_id(grid_x, grid_y);\n}\n";
        assert!(
            grid_x_writes_missing_cell(read).is_empty(),
            "`grid_x,` as a function-call ARGUMENT (a read) must not be mistaken for a write"
        );

        // A struct with grid_x/grid_y but genuinely NO `cell` column (the event-log-table shape) —
        // shorthand there is legitimately not this tripwire's business.
        let no_cell_struct = "struct NoCell {\n    pub grid_x: i32,\n    pub grid_y: i32,\n}\n";
        let no_cell_use = format!(
            "{no_cell_struct}fn f() -> NoCell {{\n    NoCell {{\n        grid_x,\n        grid_y,\n    }}\n}}\n"
        );
        assert!(
            grid_x_writes_missing_cell(&no_cell_use).is_empty(),
            "a type with no `cell` column must not be flagged for lacking one"
        );
    }

    #[test]
    fn the_scan_catches_a_broken_self_shorthand_initializer() {
        let bad = "struct E {\n    pub grid_x: i32,\n    pub grid_y: i32,\n    pub cell: i64,\n}\n\
                   impl E {\n    fn f() -> Self {\n        Self {\n            grid_x,\n            grid_y,\n        }\n    }\n}\n";
        assert_eq!(
            grid_x_writes_missing_cell(bad).len(),
            1,
            "a broken `Self {{ .. }}` shorthand initializer must be a VIOLATION"
        );

        let good = "struct E {\n    pub grid_x: i32,\n    pub grid_y: i32,\n    pub cell: i64,\n}\n\
                    impl E {\n    fn f() -> Self {\n        Self {\n            grid_x,\n            grid_y,\n            cell,\n        }\n    }\n}\n";
        assert!(
            grid_x_writes_missing_cell(good).is_empty(),
            "a `Self {{ .. }}` shorthand initializer that also sets `cell` must PASS"
        );
    }

    #[test]
    fn the_scan_catches_a_broken_compact_one_line_shorthand() {
        let cell_bearing_struct =
            "struct E {\n    pub grid_x: i32,\n    pub grid_y: i32,\n    pub cell: i64,\n}\n";

        let bad = format!("{cell_bearing_struct}fn f() -> E {{ E {{ grid_x, grid_y }} }}\n");
        assert_eq!(
            grid_x_writes_missing_cell(&bad).len(),
            1,
            "a broken compact one-line shorthand initializer must be a VIOLATION"
        );

        let good = format!("{cell_bearing_struct}fn f() -> E {{ E {{ grid_x, grid_y, cell }} }}\n");
        assert!(
            grid_x_writes_missing_cell(&good).is_empty(),
            "a compact one-line shorthand initializer that also sets `cell` must PASS"
        );
    }
}

/// ENFORCEMENT tripwire: Core source names no official Package, in code, comments or the
/// generated Gateway bindings. Core must build and read the same with no Package installed.
#[cfg(test)]
pub(crate) mod package_name_tripwire {
    use std::path::{Path, PathBuf};

    /// The official Packages (LyraCoreProject/packages) that Core must not name.
    const OFFICIAL_PACKAGES: &[&str] = &["playerbots"];

    const CORE_TREES: &[&str] = &["module", "gateway", "crates", "importer"];

    /// Test-only files may name a Package, and this file holds the list.
    fn is_exempt(rel: &str) -> bool {
        rel == "module/src/tripwires.rs"
            || rel.ends_with("_tests.rs")
            || rel.ends_with("/tests.rs")
            || rel.split('/').any(|part| part == "tests")
    }

    fn named_packages(content: &str) -> Vec<(usize, &'static str)> {
        let content = content.to_ascii_lowercase();
        let mut found = Vec::new();
        for (index, line) in content.lines().enumerate() {
            for name in OFFICIAL_PACKAGES {
                if line.contains(name) {
                    found.push((index + 1, *name));
                }
            }
        }
        found
    }

    fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                collect_files(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    #[test]
    fn core_source_names_no_official_package() {
        let root = crate::test_scan::repo_root();
        let mut files = Vec::new();
        for tree in CORE_TREES {
            collect_files(&root.join(tree), &mut files);
        }
        let mut found = Vec::new();
        for file in files {
            let rel = file
                .strip_prefix(&root)
                .expect("walked from the repo root")
                .to_string_lossy()
                .replace('\\', "/");
            if is_exempt(&rel) {
                continue;
            }
            let bytes = std::fs::read(&file).expect("readable Core file");
            for (line, name) in named_packages(&String::from_utf8_lossy(&bytes)) {
                found.push(format!("{rel}:{line} names {name}"));
            }
        }
        assert!(
            found.is_empty(),
            "Invariant: Core source names no official Package.\n\
             Core source names an official Package:\n  {}\n\nName the capability instead.",
            found.join("\n  ")
        );
    }

    #[test]
    fn the_scan_reads_comments_and_bindings_but_not_tests() {
        let source = "let x = 1;\n/// a Playerbots brain\nlet t = \"pkg_playerbots_bot\";\n";
        assert_eq!(
            named_packages(source),
            vec![(2, "playerbots"), (3, "playerbots")]
        );
        assert!(!is_exempt("gateway/src/stdb/bindings/mod.rs"));
        assert!(is_exempt("gateway/src/world/party_tests.rs"));
        assert!(is_exempt("module/tests/package_account.rs"));
    }
}
