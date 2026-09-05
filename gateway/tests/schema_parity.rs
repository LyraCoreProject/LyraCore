//! Binding schema-parity test.
//!
//! `gateway/src/stdb/bindings/*_type.rs` is HAND-MAINTAINED (see `docs/danger-zones.md` §2): a
//! module column add/reorder/retype on a
//! gateway-SUBSCRIBED table that isn't mirrored in the binding breaks live BSATN row decode
//! SILENTLY — mock-store tests cannot catch it (a real `respec_count` binding drifted silently and
//! was only found by accident, during unrelated work). This test makes that drift a RED TEST instead.
//!
//!
//! Linux-only, same reason as the armor mirror tests: it needs the `lyracore-module` dev-dep,
//! whose native build only links where the ELF linker dead-strips the SpacetimeDB wasm-host
//! intrinsics (see gateway/Cargo.toml). Schema drift is platform-independent; one platform guards it.
#![cfg(target_os = "linux")]
//! # How the comparison works
//!
//! Every module `#[table]` struct implements `spacetimedb_sats::SpacetimeType` (the
//! `#[table(...)]` macro's `__TableHelper` derive expands to the same thing
//! `#[derive(SpacetimeType)]` would) — `make_type(&mut impl TypespaceBuilder) -> AlgebraicType`
//! gives its REAL, ordered field-name+type shape, natively, no live node. We call it through
//! `RawModuleDefV9Builder` — the same `TypespaceBuilder` the real `spacetime` CLI's schema
//! extraction uses — then `Typespace::inline_typerefs_in_type` to resolve any nested named-type
//! refs, producing a flat, `PartialEq`-comparable `AlgebraicType::Product`.
//!
//! # Premise correction — read before editing this file
//!
//! This test's original design assumed the gateway BINDING struct also implements `SpacetimeType`
//! symmetrically ("every gateway binding struct derives/implements `SpacetimeType`"). That is
//! **false** as of the SpacetimeDB 2.7.1 codegen actually vendored here:
//! `spacetimedb-bindings-macro`'s `Serialize`, `Deserialize`, and `SpacetimeType` derives are
//! three *separate* proc-macros (`derive_serialize` / `derive_deserialize` / `schema_type` in
//! `spacetimedb-bindings-macro-2.7.1/src/lib.rs`), and every generated `*_type.rs` file derives
//! only `Serialize, Deserialize, Clone, PartialEq, Debug` — never `SpacetimeType`. So
//! `check::<ModuleT, BindingT>("game_x")` with both sides bound by `SpacetimeType` does not
//! compile. Deriving `SpacetimeType` ourselves on the generated file is a non-starter (it would
//! collide with the file's own `Serialize`/`Deserialize` impls — E0119), and hand-editing a "DO
//! NOT EDIT, AUTOMATICALLY GENERATED" file is exactly the hazard `docs/danger-zones.md` §2 warns
//! about (the next `spacetime generate` silently drops the edit).
//!
//! The fallback actually implemented here is stronger than field-count-only (the originally
//! own escape hatch: "get SOMETHING structural working; do not water down to field-count-only").
//! For each binding struct we build ONE real instance (fields filled via the local `Sentinel`
//! trait below — real values of the REAL field types, not placeholders) and derive its schema
//! from that instance two independent ways, using only what the binding struct already derives:
//!
//!   1. **Order + names**: a struct's derived `Debug` impl visits fields in true declaration
//!      order (generated from the exact same parsed field list the `Serialize` derive would use).
//!      `top_level_debug_fields` parses the top-level `name: value` pairs out of
//!      `format!("{inst:?}")` — brace/paren/bracket-depth aware, so a nested `Timestamp`'s own
//!      derived `Debug` output (`Timestamp { __timestamp_micros_since_unix_epoch: .. }`) can't be
//!      mistaken for a top-level field — and asserts it equals this file's believed field-name
//!      order. Reordering/renaming/adding/removing a real field changes the true `Debug` order and
//!      fails this assertion first.
//!   2. **Types**: each field is read back off the real instance BY NAME (`&inst.<field>`, which
//!      only compiles against the struct's REAL current field), and its `AlgebraicType` is
//!      computed via that field's OWN `SpacetimeType::make_type` — every SATS primitive,
//!      `Identity`, `Timestamp`, `Option<T>`, and `Vec<T>` already implements `SpacetimeType`;
//!      only the OUTER row struct is missing it. (`Identity`/`Timestamp` need no special-case
//!      mapping: the gateway's `spacetimedb_sdk::Identity`/`Timestamp` and the module's
//!      `spacetimedb::Identity`/`Timestamp` are the literal same `spacetimedb_lib` type — both
//!      crates unify on `spacetimedb-lib`/`spacetimedb-sats` 2.7.1 in this workspace's one
//!      `Cargo.lock` — so their `AlgebraicType`s compare equal directly.)
//!
//! Both signals are compared against the module's real, auto-derived `AlgebraicType::Product` for
//! field COUNT, NAME-per-index, and TYPE-per-index. The literal `check::<ModuleT, BindingT>(name)`
//! signature originally sketched isn't achievable (no `SpacetimeType` on `BindingT`);
//! `check::<ModuleT>(name, binding_shape!(BindingT { field, field, .. }))` is the closest faithful
//! equivalent — one manifest line per table, naming both types, still driven by the REAL types.
//!
//! # Scope
//!
//! SUBSCRIBED tables only, per the coordinator's own subscription list in `stdb/connection.rs`
//! (parsed at test time below via `include_str!` — the completeness guard). Per-player
//! subscriptions set up elsewhere (`stdb/subscriptions.rs`) and reducer-arg parity are out of
//! scope.
//!
//! # No `[lib]` target workaround
//!
//! `lyracore-gateway` is a binary-only crate (no `[lib]` in `Cargo.toml`), so this
//! integration test cannot `use spacetime_core_gateway::...` (nothing to link against). The
//! generated `gateway/src/stdb/bindings/mod.rs` tree is fully self-contained (only depends on the
//! `spacetimedb_sdk` crate — verified: zero `crate::`/`super::super` references anywhere under
//! `stdb/bindings/`), so it's pulled in directly via `#[path]` below and recompiled as part of
//! this test binary instead. This changes zero gateway `src/` files.

#[path = "../src/stdb/bindings/mod.rs"]
#[allow(dead_code)]
// The SpacetimeDB codegen writes one function per table subscription; several run past any
// reasonable length ceiling and no edit there survives a regeneration.
#[allow(clippy::too_many_lines)]
mod bindings;

use spacetimedb_lib::db::raw_def::v9::RawModuleDefV9Builder;
use spacetimedb_lib::{AlgebraicType, ProductType, SpacetimeType};

// ---------------------------------------------------------------------------------------------
// Sentinel: builds ONE real, validly-typed instance of a binding struct so we can read its real
// field types back (via `&inst.field`) and its real field order back (via its derived `Debug`).
// ---------------------------------------------------------------------------------------------

trait Sentinel: Sized {
    fn sentinel() -> Self;
}

/// One concrete-type impl per primitive actually used across the subscribed tables' bindings.
/// (NOT a `impl<T: Default> Sentinel for T` blanket: rustc's coherence check treats a foreign
/// type's `Default`-ness as subject to change in a future upstream release, so a blanket-over-
/// `Default` plus a concrete override for `spacetimedb_lib::Timestamp` — which does NOT derive
/// `Default` today — is rejected as a potential future conflict (E0119) even though there's no
/// ACTUAL overlap right now. Enumerating concrete types sidesteps that.)
macro_rules! sentinel_via_default {
    ($($t:ty),+ $(,)?) => {
        $(impl Sentinel for $t {
            fn sentinel() -> Self {
                <$t as Default>::default()
            }
        })+
    };
}
sentinel_via_default!(
    bool,
    u8,
    u16,
    u32,
    u64,
    u128,
    i8,
    i16,
    i32,
    i64,
    i128,
    f32,
    f64,
    String,
    spacetimedb_lib::Identity,
);

/// `Timestamp` is the one field type in play that does NOT derive `Default` — special-cased.
impl Sentinel for spacetimedb_lib::Timestamp {
    fn sentinel() -> Self {
        spacetimedb_lib::Timestamp::UNIX_EPOCH
    }
}

impl Sentinel for spacetimedb_lib::ScheduleAt {
    fn sentinel() -> Self {
        spacetimedb_lib::ScheduleAt::Time(spacetimedb_lib::Timestamp::UNIX_EPOCH)
    }
}

/// Covers `Option<Identity>` and `Vec<u8>`/`Vec<u32>` (the only generic-container field types
/// among the subscribed tables) without needing a type-specific entry each.
impl<T: Sentinel> Sentinel for Option<T> {
    fn sentinel() -> Self {
        None
    }
}
impl<T: Sentinel> Sentinel for Vec<T> {
    fn sentinel() -> Self {
        Vec::new()
    }
}

/// `T::make_type` inferred from the REAL field's type via `&inst.field` — never hand-typed.
fn field_shape<T: SpacetimeType>(_value: &T, ts: &mut RawModuleDefV9Builder) -> AlgebraicType {
    T::make_type(ts)
}

/// Extract the top-level `field_name:` identifiers from a derived `Debug` output of the shape
/// `StructName { field1: v1, field2: v2, .. }`, ignoring any `word:`-shaped text that belongs to
/// a NESTED derived `Debug` one level down (e.g. `Timestamp`'s own single field prints the same
/// `name: value` way inside its own `{ .. }`). Brace/paren/bracket-depth aware; only records
/// identifiers seen at depth 1 (directly inside the outermost struct's braces).
///
/// NOT string-literal aware (found by review): a Sentinel value whose Debug output embeds
/// `word: ` inside a quoted string would inject a spurious identifier here. Every current sentinel
/// is empty/zero/None so this cannot fire today, and an injection would surface as a LOUD
/// order-mismatch failure, not a silent pass — but keep Sentinel impls free of colon/comma-bearing
/// string values (or teach this parser about `"` first).
fn top_level_debug_fields(debug: &str) -> Vec<String> {
    let open = debug
        .find('{')
        .expect("a derived struct Debug always has a top-level `{`");
    let chars: Vec<char> = debug[open..].chars().collect();
    let mut depth: i32 = 0;
    let mut names = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '{' | '(' | '[' => {
                depth += 1;
                i += 1;
            }
            '}' | ')' | ']' => {
                depth -= 1;
                i += 1;
            }
            c if depth == 1 && (c.is_alphabetic() || c == '_') => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();
                let mut j = i;
                while j < chars.len() && chars[j] == ' ' {
                    j += 1;
                }
                // A field is `ident:` but not `ident::` (no nested paths appear at this depth).
                if chars.get(j) == Some(&':') && chars.get(j + 1) != Some(&':') {
                    names.push(ident);
                }
            }
            _ => i += 1,
        }
    }
    names
}

/// The binding side's manifest-derived shape for one table, built by the `binding_shape!` macro.
struct BindingShape {
    field_names: Vec<&'static str>,
    field_shapes: Vec<AlgebraicType>,
    debug_order: Vec<String>,
}

/// Builds a `BindingShape` for `$Ty { field, field, .. }` — the field list is written ONCE (no
/// types, no values); `Sentinel::sentinel()` infers each field's real type from the real struct
/// literal, and `field_shape(&inst.field, ..)` reads that same real type back via `SpacetimeType`.
macro_rules! binding_shape {
    ($Ty:path, { $($f:ident),+ $(,)? }) => {{
        // A `path` fragment substituted directly as `$Ty { .. }` hits the classic macro_rules
        // "found `{`, expected an operator" parse error (an interpolated path is inserted as an
        // opaque already-parsed node, and Rust's grammar can't then extend it into a struct
        // literal). Binding it to a fresh local type alias first sidesteps that: `__BindingTy`
        // below is a plain identifier, not a substituted fragment, so `__BindingTy { .. }` parses
        // as an ordinary struct literal.
        type __BindingTy = $Ty;
        let inst = __BindingTy { $( $f: Sentinel::sentinel() ),+ };
        let debug_order = top_level_debug_fields(&format!("{inst:?}"));
        let mut ts = RawModuleDefV9Builder::new();
        let field_names: Vec<&'static str> = vec![$(stringify!($f)),+];
        let field_shapes: Vec<AlgebraicType> = vec![$(field_shape(&inst.$f, &mut ts)),+];
        BindingShape { field_names, field_shapes, debug_order }
    }};
}

/// Compares one table's module `AlgebraicType` (real, auto-derived) against the binding manifest
/// (real field types, believed order) for field COUNT, NAME-per-index, and TYPE-per-index.
///
/// `renames` is the escape hatch ("where the sdk renames, the manifest line takes
/// an explicit rename list"): `(module_field_name, binding_field_name)` pairs for fields where the
/// `spacetime` codegen normalizes the Rust IDENTIFIER (this is a cosmetic/tooling difference, not
/// a schema drift — BSATN row decode is positional, not name-keyed, so a renamed-but-same-
/// position-and-type field cannot break live decode). As of this test, real (not synthetic)
/// instances of this: module's `data0`/`data1` (`GameObjectTemplate`) and `p0`/`p1` (`SpellEffect`)
/// come out of the `spacetime generate` (CLI 2.6.1) codegen as `data_0`/`data_1`/`p_0`/`p_1` —
/// the generator inserts `_` before a bare trailing digit that doesn't already have one (compare
/// `spellid_1` on `ItemTemplate`, which module already spells with the underscore and which the
/// binding leaves untouched).
fn check<M: SpacetimeType>(table: &str, binding: BindingShape, renames: &[(&str, &str)]) {
    assert_eq!(
        binding.debug_order, binding.field_names,
        "{table}: the binding struct's TRUE field order (read off its own derived Debug impl) no \
         longer matches this file's manifest order for this table — a field was added, removed, \
         renamed, or reordered in the generated binding without updating \
         gateway/tests/schema_parity.rs. real (Debug) order: {:?}; manifest order: {:?}",
        binding.debug_order, binding.field_names,
    );

    let mut ts = RawModuleDefV9Builder::new();
    let mut module_ty = M::make_type(&mut ts);
    let mut module_def = ts.finish();
    module_def
        .typespace
        .inline_typerefs_in_type(&mut module_ty)
        .unwrap_or_else(|e| panic!("{table}: module type has an unresolvable type ref: {e:?}"));
    let module_fields = match module_ty {
        AlgebraicType::Product(p) => p,
        other => panic!(
            "{table}: module `make_type()` is not a Product ({other:?}) — table row structs are \
             always product types"
        ),
    };

    let binding_fields: ProductType = binding
        .field_names
        .iter()
        .copied()
        .zip(binding.field_shapes.iter().cloned())
        .map(|(name, ty)| (Some(name), ty))
        .collect();

    assert_eq!(
        module_fields.elements.len(),
        binding_fields.elements.len(),
        "{table}: field COUNT mismatch — module has {} fields, binding manifest has {} \
         (module fields: {:?}; binding fields: {:?})",
        module_fields.elements.len(),
        binding_fields.elements.len(),
        module_fields
            .elements
            .iter()
            .map(|e| e.name.as_deref().unwrap_or("?"))
            .collect::<Vec<_>>(),
        binding.field_names,
    );

    for (i, (m, b)) in module_fields
        .elements
        .iter()
        .zip(binding_fields.elements.iter())
        .enumerate()
    {
        let m_name = m.name.as_deref().unwrap_or("?");
        let b_name = b.name.as_deref().unwrap_or("?");
        let expected_b_name = renames
            .iter()
            .find(|(m, _)| *m == m_name)
            .map(|(_, b)| *b)
            .unwrap_or(m_name);
        if expected_b_name != m_name {
            // A rename entry may ONLY paper over the SDK's cosmetic trailing-digit underscore
            // normalization (data0 -> data_0). Without this guard, a PAIR of same-typed swapped
            // fields could hide behind two compensating bogus renames (found by review).
            assert_eq!(
                expected_b_name.replace('_', ""),
                m_name.replace('_', ""),
                "{table}: renames entry `{m_name} => {expected_b_name}` is not an underscore \
                 normalization — renames must never map one field NAME onto a different field"
            );
        }
        assert_eq!(
            expected_b_name, b_name,
            "{table}: field #{i} NAME mismatch — module has `{m_name}` (expected binding name \
             `{expected_b_name}` after applying this table's rename list), binding has `{b_name}` \
             (a field was reordered/renamed on one side only, or needs a new `renames:` entry)"
        );
        assert_eq!(
            m.algebraic_type, b.algebraic_type,
            "{table}: field `{m_name}` (#{i}) TYPE mismatch — module: {:?}, binding: {:?}",
            m.algebraic_type, b.algebraic_type,
        );
    }
}

/// `#[test] fn <name>() { check::<Module>(table, binding_shape!(Binding { .. })); }` — one
/// manifest entry per gateway-SUBSCRIBED table, each its own `#[test]` so one table's drift
/// doesn't mask another's in a single `cargo test` run.
macro_rules! parity_test {
    ($name:ident, $table:literal, $ModuleTy:ty, $BindingTy:path, { $($f:ident),+ $(,)? }) => {
        #[test]
        fn $name() {
            check::<$ModuleTy>($table, binding_shape!($BindingTy, { $($f),+ }), &[]);
        }
    };
    ($name:ident, $table:literal, $ModuleTy:ty, $BindingTy:path, { $($f:ident),+ $(,)? }, renames: { $($m:ident => $b:ident),+ $(,)? }) => {
        #[test]
        fn $name() {
            check::<$ModuleTy>(
                $table,
                binding_shape!($BindingTy, { $($f),+ }),
                &[$((stringify!($m), stringify!($b))),+],
            );
        }
    };
}

// ---------------------------------------------------------------------------------------------
// The manifest: one line per gateway-SUBSCRIBED table (`stdb/connection.rs`'s coordinator
// subscription list). `MANIFEST_TABLES` below MUST list the same table names — the completeness
// guard cross-checks the two.
// ---------------------------------------------------------------------------------------------

parity_test!(parity_game_realm, "game_realm", lyracore_module::Realm, bindings::realm_type::Realm, {
    id, name, address, realm_type, flags, population, timezone,
});
parity_test!(parity_game_teleport_event, "game_teleport_event", lyracore_module::TeleportEvent, bindings::teleport_event_type::TeleportEvent, {
    id, recipient_identity, mover_guid, map_id, x, y, z, orientation, created_micros, cross_map,
});
parity_test!(parity_game_addon_message, "game_addon_message", lyracore_module::AddonMessage, bindings::addon_message_type::AddonMessage, {
    id, recipient_identity, cmd, payload, created_at,
});
parity_test!(parity_game_xp_event, "game_xp_event", lyracore_module::XpEvent, bindings::xp_event_type::XpEvent, {
    id, recipient_identity, killed_guid, total_exp, created_at, is_kill,
});
parity_test!(parity_game_levelup_event, "game_levelup_event", lyracore_module::LevelupEvent, bindings::levelup_event_type::LevelupEvent, {
    id, recipient_identity, new_level, health_gained, created_at, mana_gained, strength_gained, agility_gained, stamina_gained, intellect_gained, spirit_gained,
});
parity_test!(parity_game_character_explored, "game_character_explored", lyracore_module::CharacterExplored, bindings::character_explored_type::CharacterExplored, {
    id, character_guid, area_bit, area_id, experience,
});
parity_test!(parity_game_account, "game_account", lyracore_module::Account, bindings::account_type::Account, {
    id, username, salt, verifier, identity, banned, alpha_test_tools,
});
parity_test!(parity_game_session, "game_session", lyracore_module::Session, bindings::session_type::Session, {
    account_id, session_key, identity, created_at, expires_at,
});
parity_test!(parity_game_character_shard, "game_character_shard", lyracore_module::CharacterShard, bindings::character_shard_type::CharacterShard, {
    character_guid, map_id, instance_id, updated_micros,
});
parity_test!(parity_game_map_region, "game_map_region", lyracore_module::MapRegion, bindings::map_region_type::MapRegion, {
    key, map_id, region_id, gx_min, gx_max, gy_min, gy_max,
});
parity_test!(parity_game_region_assignment, "game_region_assignment", lyracore_module::RegionAssignment, bindings::region_assignment_type::RegionAssignment, {
    key, map_id, region_id, shard, epoch, updated_micros,
});
// Party state, authoritative on realm-core and mirrored onto each world shard.
// `game_group_event` earns its entry twice over — it is the one table this slice CHANGED (the
// END-appended `recipient_guid`), and the gateway decodes it on two different connections.
parity_test!(parity_game_group, "game_group", lyracore_module::Group, bindings::group_type::Group, {
    group_id, leader_guid, loot_method, loot_threshold, rr_cursor, master_looter_guid,
});
parity_test!(parity_game_group_member, "game_group_member", lyracore_module::GroupMember, bindings::group_member_type::GroupMember, {
    id, group_id, character_guid, owner_identity,
});
parity_test!(parity_game_creature_quest_tap, "game_creature_quest_tap", lyracore_module::CreatureQuestTap, bindings::creature_quest_tap_type::CreatureQuestTap, {
    creature_guid, character_guid,
});
parity_test!(parity_game_creature_quest_tap_member, "game_creature_quest_tap_member", lyracore_module::CreatureQuestTapMember, bindings::creature_quest_tap_member_type::CreatureQuestTapMember, {
    id, creature_guid, character_guid,
});
parity_test!(parity_game_creature_loot_tag_group, "game_creature_loot_tag_group", lyracore_module::CreatureLootTagGroup, bindings::creature_loot_tag_group_type::CreatureLootTagGroup, {
    creature_guid, group_id,
});
parity_test!(parity_game_corpse_loot_eligible, "game_corpse_loot_eligible", lyracore_module::CorpseLootEligible, bindings::corpse_loot_eligible_type::CorpseLootEligible, {
    id, corpse_guid, eligible_guid,
});
parity_test!(parity_game_group_event, "game_group_event", lyracore_module::GroupEvent, bindings::group_event_type::GroupEvent, {
    id, recipient_identity, kind, other_guid, other_name, created_at, payload, recipient_guid,
});
// The private per-recipient trade-status relay (#120) — the `game_group_event` shape minus the
// name/payload columns (no trade status carries either).
parity_test!(parity_game_trade_event, "game_trade_event", lyracore_module::TradeEvent, bindings::trade_event_type::TradeEvent, {
    id, recipient_identity, kind, other_guid, created_at, recipient_guid, payload,
});
parity_test!(parity_game_duel_event, "game_duel_event", lyracore_module::DuelEvent, bindings::duel_event_type::DuelEvent, {
    id, recipient_identity, recipient_guid, kind, completion_kind, duel_id, flag_guid, flag_entry,
    initiator_guid, challenged_guid, winner_guid, loser_guid, map_id, instance_id, flag_x, flag_y,
    flag_z, flag_orientation, created_at, winner_name, loser_name,
});
// A bot's serendipity invite DECISION, picked up by the coordinator's
// `world::party::run_bot_invite` relay (`stdb/subscriptions.rs`) — not a client-facing table, but
// the gateway decodes it off the wire the same as everything else here.
parity_test!(parity_game_bot_invite_intent, "game_bot_invite_intent", lyracore_module::BotInviteIntent, bindings::bot_invite_intent_type::BotInviteIntent, {
    id, inviter_guid, target_guid, created_at, op,
});
// A session-less character's Shard crossing, picked up by the coordinator's
// `world::transfer::run_bot_transfer` relay — the transfer twin of the row above.
parity_test!(parity_game_bot_transfer_intent, "game_bot_transfer_intent", lyracore_module::BotTransferIntent, bindings::bot_transfer_intent_type::BotTransferIntent, {
    id, bot_guid, destination_map, destination_instance, reason, created_at,
});
// The private per-recipient whisper relay, now readable on TWO connections — the
// per-player one under RLS (unchanged) and realm-core's coordinator, which self-filters on the
// END-appended `recipient_guid`. Both decodes go through this binding, so a drifted column here is a
// mis-decoded private chat line rather than a compile error.
parity_test!(parity_game_whisper_event, "game_whisper_event", lyracore_module::WhisperEvent, bindings::whisper_event_type::WhisperEvent, {
    id, recipient_identity, other_guid, is_inform, message, created_at, recipient_guid,
});
parity_test!(parity_game_system_message_event, "game_system_message_event", lyracore_module::SystemMessageEvent, bindings::system_message_event_type::SystemMessageEvent, {
    id, recipient_identity, recipient_guid, message, created_at,
});
// Private (no per-player subscriber to decode these — every wire-visible roll transition still
// rides `game_group_event`, unchanged). Subscribed so the gateway's loot-roll relay
// (`world::loot::relay_tick`) can promote a world shard's staging roll onto realm-core and read
// realm-core's votes back — a drifted column here breaks that relay silently, not a client packet.
parity_test!(parity_game_loot_roll, "game_loot_roll", lyracore_module::LootRoll, bindings::loot_roll_type::LootRoll, {
    id, corpse_guid, slot, item_entry, deadline_micros, resolved,
});
parity_test!(parity_game_loot_roll_vote, "game_loot_roll_vote", lyracore_module::LootRollVote, bindings::loot_roll_vote_type::LootRollVote, {
    id, roll_id, voter_guid, voted, vote, rolled,
});
// The guid-range trio. `game_guid_allocator` + `game_guid_range` are subscribed on EVERY
// deployment (a database without a range refuses to create characters), the registry only when
// sharded. Drift here is silent and expensive: a mis-decoded `base` would install the wrong range.
parity_test!(parity_game_guid_allocator, "game_guid_allocator", lyracore_module::GuidAllocator, bindings::guid_allocator_type::GuidAllocator, {
    id, high_water,
});
parity_test!(parity_game_guid_range, "game_guid_range", lyracore_module::GuidRange, bindings::guid_range_type::GuidRange, {
    id, base, size,
});
parity_test!(parity_game_guid_range_registry, "game_guid_range_registry", lyracore_module::GuidRangeAssignment, bindings::guid_range_assignment_type::GuidRangeAssignment, {
    shard_name, slot, base, size, assigned_micros,
});
parity_test!(parity_game_character, "game_character", lyracore_module::Character, bindings::character_type::Character, {
    guid, account_id, owner_identity, name, race, class, gender, skin, face, hair_style,
    hair_color, facial_hair, level, xp, next_level_xp, map_id, zone_id, x, y, z, orientation,
    first_login, online, money, rested_xp, last_logout_micros, home_map, home_zone, home_x,
    home_y, home_z, played_total_secs, session_start_micros, health, power, respec_count,
    death_expire_micros, pending_instance_id, gm_level, pending_ghost, resting, rested_since_micros,
    pending_godmode, pending_run_speed_mult_bp, bank_bag_slots,
});
parity_test!(parity_game_world_entity, "game_world_entity", lyracore_module::WorldEntity, bindings::world_entity_type::WorldEntity, {
    guid, owner_identity, account_id, map_id, x, y, z, orientation, grid_x, grid_y,
    last_move_ms, type_mask, entry, scale_x, health, max_health, power, max_power, level,
    faction_template, unit_bytes_0, display_id, native_display_id, unit_flags,
    base_attack_time_ms, dynamic_flags, dead, player_bytes, player_bytes_2, player_bytes_3,
    player_flags, xp, next_level_xp, target_guid, money, unit_bytes_1, strength, agility,
    stamina, intellect, spirit, npc_flags, armor, leg_ends_ms, wp_target, movement_flags,
    combat_until_ms, pickpocketed, next_swing_spell, overpower_until_ms, revenge_until_ms,
    stance, owner_guid, skinned, mana_regen_paused_until_ms, death_expire_micros, instance_id,
    run_speed_mult_bp, godmode, resting, cell,
    sheet_str_bonus, sheet_agi_bonus, sheet_sta_bonus, sheet_int_bonus, sheet_spi_bonus,
    sheet_ap_base, sheet_ap_mods, sheet_dmg_min, sheet_dmg_max, sheet_crit_bp, unit_bytes_2,
    bank_bag_slots, mount_display_id, zone_id, sheet_ranged_ap, sheet_ranged_dmg_min,
    sheet_ranged_dmg_max,
});
// The live sky, one row per zone. Gateway-subscribed: world entry and zone entry read it, and its
// updates are the weather relay.
parity_test!(parity_game_zone_weather, "game_zone_weather", lyracore_module::ZoneWeather, bindings::zone_weather_type::ZoneWeather, {
    zone_id, weather_type, intensity, changed_at_micros,
});
parity_test!(parity_game_hunter_pet_protocol, "game_hunter_pet_protocol", lyracore_module::HunterPetProtocol, bindings::hunter_pet_protocol_type::HunterPetProtocol, {
    pet_id, owner_guid, live_pet_guid, creature_entry, name, name_timestamp, level, pet_xp,
    next_level_xp, happiness, loyalty_level,
});
// `game_config` became gateway-subscribed so the startup instance-hosting check can read
// `hosts_instances` back instead of guessing. The generated binding was STALE when that subscription
// landed — a later change END-appended `hosts_instances` to the module table and
// `server_config_type.rs` had never been regenerated, so this manifest line is the guard that made
// the drift a red test.
parity_test!(parity_game_config, "game_config", lyracore_module::ServerConfig, bindings::server_config_type::ServerConfig, {
    id, xp_rate, nav_enabled, hosts_instances, bots_idle, vmap_enabled, nav_coverage_enabled,
});
parity_test!(parity_game_creature_template, "game_creature_template", lyracore_module::CreatureTemplate, bindings::creature_template_type::CreatureTemplate, {
    entry, name, subname, display_id, level, health, faction_template, npc_flags, unit_flags,
    creature_type, creature_family, type_flags, rank, scale, base_attack_time_ms, money_min,
    money_max, max_level, max_level_health, aggro_range, damage_min, damage_max, armor,
    pickpocket_loot_id, skin_loot_id, trainer_type, trainer_class,
});
parity_test!(parity_game_taxi_node, "game_taxi_node", lyracore_module::GameTaxiNode, bindings::game_taxi_node_type::GameTaxiNode, {
    id, client_node_id, map_id, x, y, z, name, mount_display_horde, mount_display_alliance,
});
parity_test!(parity_game_taxi_path, "game_taxi_path", lyracore_module::GameTaxiPath, bindings::game_taxi_path_type::GameTaxiPath, {
    id, source_node_id, destination_node_id, fare,
});
parity_test!(parity_game_taxi_path_node, "game_taxi_path_node", lyracore_module::GameTaxiPathNode, bindings::game_taxi_path_node_type::GameTaxiPathNode, {
    id, path_id, node_index, map_id, x, y, z, flags, delay_ms,
});
parity_test!(parity_game_character_taxi_node, "game_character_taxi_node", lyracore_module::CharacterTaxiNode, bindings::character_taxi_node_type::CharacterTaxiNode, {
    id, character_guid, node_id,
});
parity_test!(parity_game_active_taxi_flight, "game_active_taxi_flight", lyracore_module::ActiveTaxiFlight, bindings::active_taxi_flight_type::ActiveTaxiFlight, {
    character_guid, path_id, source_node_id, destination_node_id, mount_display_id, fare,
    current_node_index, started_micros,
});
parity_test!(parity_game_taxi_service_reply, "game_taxi_service_reply", lyracore_module::TaxiServiceReply, bindings::taxi_service_reply_type::TaxiServiceReply, {
    request_id, character_guid, operation, npc_guid, accepted, known, source_client_node_id,
    available_client_node_ids, refusal, created_micros, result_code,
});
parity_test!(parity_game_taxi_passenger_spline, "game_taxi_passenger_spline", lyracore_module::TaxiPassengerSpline, bindings::taxi_passenger_spline_type::TaxiPassengerSpline, {
    character_guid, map_id, instance_id, grid_x, grid_y, cell, start_x, start_y, start_z, points,
    duration_ms, spline_id,
});
parity_test!(parity_game_start_position, "game_start_position", lyracore_module::StartPosition, bindings::start_position_type::StartPosition, {
    race_class, race, class, map_id, zone_id, x, y, z, orientation, display_id,
});
parity_test!(parity_game_corpse, "game_corpse", lyracore_module::Corpse, bindings::corpse_type::Corpse, {
    guid, owner_guid, map_id, x, y, z, orientation, display_id, bytes_1, bytes_2, created_at,
    reclaim_delay_micros, is_bones, instance_id,
});
parity_test!(parity_game_item_template, "game_item_template", lyracore_module::ItemTemplate, bindings::item_template_type::ItemTemplate, {
    entry, class, subclass, name, display_id, quality, inventory_type, item_level,
    required_level, max_durability, buy_price, sell_price, max_stack, damage_min, damage_max,
    delay_ms, stat_strength, stat_agility, stat_stamina, stat_intellect, stat_spirit, stat_crit,
    stat_hit, stat_armor, block_value, restores_power, spellid_1, spelltrigger_1, spellid_2,
    spelltrigger_2, container_slots, sheath, bonding, holy_res, fire_res, nature_res, frost_res,
    shadow_res, arcane_res, spellid_3, spelltrigger_3, spellid_4, spelltrigger_4, spellid_5,
    spelltrigger_5, required_skill, required_skill_rank, required_reputation_faction,
    required_reputation_rank, max_count, item_flags, page_text, start_quest, bag_family,
    buy_count, food_type, allowed_class, allowed_race,
});
parity_test!(parity_game_spell, "game_spell", lyracore_module::Spell, bindings::spell_type::Spell, {
    spell_id, name, power_type, cost, cast_time_ms, gcd_ms, cooldown_ms, range_yd, duration_ms,
    school_mask, dispel_type, mechanic, max_stacks, aura_interrupt, attributes, spell_level,
    max_level, is_negative, cast_flags, stances, family_name, family_flags, proc_flags,
    proc_chance, proc_charges,
});
parity_test!(parity_game_spell_effect, "game_spell_effect", lyracore_module::SpellEffect, bindings::spell_effect_type::SpellEffect, {
    id, spell_id, effect_index, kind, base_points, die_sides, per_level, period_ms, target,
    radius_yd, chain_targets, trigger_spell, effect_mechanic, p_0, p_0_kind, p_1, script_id,
    enters_combat,
}, renames: {
    // spacetime generate (CLI 2.6.1) inserts `_` before a bare trailing digit; module's own
    // field names (p0/p0_kind/p1) don't have it. Cosmetic — BSATN decode is positional.
    p0 => p_0, p0_kind => p_0_kind, p1 => p_1,
});
parity_test!(parity_game_item_instance, "game_item_instance", lyracore_module::ItemInstance, bindings::item_instance_type::ItemInstance, {
    guid, entry, owner_identity, owner_guid, slot, stack_count, durability, created_at,
    enchant_id, soulbound,
});
parity_test!(parity_game_corpse_loot, "game_corpse_loot", lyracore_module::CorpseLoot, bindings::corpse_loot_type::CorpseLoot, {
    id, corpse_guid, slot, item_entry, count, quest_only, reserved_for, designated_looter_guid,
    master_only, withheld,
});
parity_test!(parity_game_npc_vendor, "game_npc_vendor", lyracore_module::NpcVendor, bindings::npc_vendor_type::NpcVendor, {
    id, creature_entry, item_entry, slot, max_count,
});
parity_test!(parity_game_gossip_menu, "game_gossip_menu", lyracore_module::GossipMenu, bindings::gossip_menu_type::GossipMenu, {
    entry, text_id,
});
parity_test!(parity_game_gossip_option, "game_gossip_option", lyracore_module::GossipOption, bindings::gossip_option_type::GossipOption, {
    row_id, entry, option_index, icon, text, action, action_menu_id, cond_type, cond_value1,
    cond_value2,
});
parity_test!(parity_game_gossip_menu_profile, "game_gossip_menu_profile", lyracore_module::GossipMenuProfile, bindings::gossip_menu_profile_type::GossipMenuProfile, {
    menu_id, text_id,
});
parity_test!(parity_game_gossip_menu_profile_option, "game_gossip_menu_profile_option", lyracore_module::GossipMenuProfileOption, bindings::gossip_menu_profile_option_type::GossipMenuProfileOption, {
    row_id, menu_id, option_index, icon, text, action, action_menu_id, cond_type, cond_value1,
    cond_value2,
});
parity_test!(parity_game_creature_gossip_menu_override, "game_creature_gossip_menu_override", lyracore_module::CreatureGossipMenuOverride, bindings::creature_gossip_menu_override_type::CreatureGossipMenuOverride, {
    creature_guid, menu_id, map_id, instance_id,
});
parity_test!(parity_game_npc_text, "game_npc_text", lyracore_module::NpcText, bindings::npc_text_type::NpcText, {
    text_id, text,
});
parity_test!(parity_game_npc_text_slot, "game_npc_text_slot", lyracore_module::NpcTextSlot, bindings::npc_text_slot_type::NpcTextSlot, {
    id, text_id, slot_index, text_male, text_female, probability,
});
parity_test!(parity_game_quest_template, "game_quest_template", lyracore_module::QuestTemplate, bindings::quest_template_type::QuestTemplate, {
    entry, min_level, quest_level, title, reward_money, reward_xp, prev_quest_id,
    required_races, required_classes, zone_or_sort, rew_rep_faction_1, rew_rep_value_1,
    rew_rep_faction_2, rew_rep_value_2, src_item, src_item_count, repeatable,
    next_quest_id, limit_time, reward_money_max_level,
});
parity_test!(parity_game_quest_text, "game_quest_text", lyracore_module::QuestText, bindings::quest_text_type::QuestText, {
    quest_entry, details, objectives, offer_reward_text, request_items_text,
});
parity_test!(parity_game_quest_objective, "game_quest_objective", lyracore_module::QuestObjective, bindings::quest_objective_type::QuestObjective, {
    id, quest_entry, obj_index, kind, target_entry, required_count,
});
parity_test!(parity_game_quest_reward_item, "game_quest_reward_item", lyracore_module::QuestRewardItem, bindings::quest_reward_item_type::QuestRewardItem, {
    id, quest_entry, item_entry, count,
});
parity_test!(parity_game_quest_reward_choice, "game_quest_reward_choice", lyracore_module::QuestRewardChoice, bindings::quest_reward_choice_type::QuestRewardChoice, {
    id, quest_entry, choice_index, item_entry, count,
});
parity_test!(parity_game_creature_quest, "game_creature_quest", lyracore_module::CreatureQuest, bindings::creature_quest_type::CreatureQuest, {
    id, creature_entry, quest_entry, role,
});
parity_test!(parity_game_gameobject_quest, "game_gameobject_quest", lyracore_module::GameObjectQuest, bindings::game_object_quest_type::GameObjectQuest, {
    id, go_entry, quest_entry, role,
});
parity_test!(parity_game_character_quest, "game_character_quest", lyracore_module::CharacterQuest, bindings::character_quest_type::CharacterQuest, {
    id, character_guid, owner_identity, quest_entry, counts, rewarded, deadline_micros, failed,
});
parity_test!(parity_game_player_spell, "game_player_spell", lyracore_module::PlayerSpell, bindings::player_spell_type::PlayerSpell, {
    id, character_guid, owner_identity, spell_id,
});
parity_test!(parity_game_player_action, "game_player_action", lyracore_module::PlayerAction, bindings::player_action_type::PlayerAction, {
    id, character_guid, owner_identity, button, action, action_type,
});
parity_test!(parity_game_trainer_spell, "game_trainer_spell", lyracore_module::TrainerSpell, bindings::trainer_spell_type::TrainerSpell, {
    id, trainer_entry, spell_id, cost, required_level, learn_skill_line, learn_skill_cap,
});
// The SOURCE-side escrow row. The only module->gateway data flow the cross-database transfer
// adds, and the one binding in this tree that was hand-authored rather than generated — so this
// parity check is doing more work here than anywhere else in the file.
parity_test!(parity_game_transfer_out, "game_transfer_out", lyracore_module::TransferOut, bindings::transfer_out_type::TransferOut, {
    transfer_id, character_guid, dest_map_id, dest_instance_id, dest_x, dest_y, dest_z, dest_o,
    blob, created_micros, cross_database,
});
parity_test!(parity_game_spell_chain, "game_spell_chain", lyracore_module::SpellChain, bindings::spell_chain_type::SpellChain, {
    spell_id, prev_spell, first_spell, rank, req_spell,
});
// Generated even though the gateway does not currently subscribe to or read it. The binding was
// stale once and therefore earns an explicit regression guard outside the subscribed-table
// manifest below; recreating the old two-field binding must make this test fail to compile.
parity_test!(parity_game_spell_group_rule, "game_spell_group_rule", lyracore_module::SpellGroupRule, bindings::spell_group_rule_type::SpellGroupRule, {
    group_id, rule, rank_is_comparable,
});
parity_test!(parity_game_player_skill, "game_player_skill", lyracore_module::PlayerSkill, bindings::player_skill_type::PlayerSkill, {
    id, character_guid, owner_identity, skill_line, current, max_rank,
});
parity_test!(parity_game_gameobject, "game_gameobject", lyracore_module::GameObject, bindings::game_object_type::GameObject, {
    guid, template_entry, map_id, x, y, z, orientation, state, created_at, respawn_at_micros,
    instance_id, grid_x, grid_y, cell, rotation_0, rotation_1, rotation_2, rotation_3,
});
// The AOI-index fix pinned these two when they still rode the per-player AOI box; the
// shared-connection model moved them onto the coordinator's `SELECT *` list, so they are now in
// `MANIFEST_TABLES` like every other coordinator-subscribed table and the completeness guard asks
// for them by itself.
parity_test!(parity_game_entity_motion, "game_entity_motion", lyracore_module::EntityMotion, bindings::entity_motion_type::EntityMotion, {
    guid, map_id, instance_id, grid_x, grid_y, opcode, movement_info, seq, cell,
});
parity_test!(parity_game_creature_spline, "game_creature_spline", lyracore_module::CreatureSpline, bindings::creature_spline_type::CreatureSpline, {
    guid, start_micros, dur_ms, sx, sy, sz, dx, dy, dz, map_id, instance_id, grid_x, grid_y,
    spline_id, run, cell, facing, facing_angle,
});
parity_test!(parity_game_character_buyback, "game_character_buyback", lyracore_module::BuybackEntry, bindings::buyback_entry_type::BuybackEntry, {
    id, player_guid, item_entry, stack_count, price, soulbound,
});
parity_test!(parity_game_gameobject_template, "game_gameobject_template", lyracore_module::GameObjectTemplate, bindings::game_object_template_type::GameObjectTemplate, {
    entry, type_id, display_id, name, data_0, data_1, gather_skill_line, respawn_secs,
    gather_gray, lock_id, size,
}, renames: {
    // Same `spacetime generate` (CLI 2.6.1) trailing-digit normalization as SpellEffect above.
    data0 => data_0, data1 => data_1,
});
parity_test!(parity_game_talent, "game_talent", lyracore_module::Talent, bindings::talent_type::Talent, {
    talent_id, name, tree_id, tier, column, max_rank, spell_id, required_talent_id,
    required_points_in_tree, grant_spell_id, tab_id, rank_spell_2, rank_spell_3, rank_spell_4,
    rank_spell_5, required_talent_rank, required_spell_id,
});
parity_test!(parity_game_character_talent, "game_character_talent", lyracore_module::CharacterTalent, bindings::character_talent_type::CharacterTalent, {
    id, character_guid, owner_identity, talent_id, rank,
});
parity_test!(parity_game_roll_event, "game_roll_event", lyracore_module::RollEvent, bindings::roll_event_type::RollEvent, {
    id, roller_guid, min_roll, max_roll, result, created_at, map_id, instance_id, grid_x, grid_y,
});
parity_test!(parity_game_rest_state_event, "game_rest_state_event", lyracore_module::RestStateEvent, bindings::rest_state_event_type::RestStateEvent, {
    id, character_guid, player_bytes_2, created_at,
});
parity_test!(parity_game_breath_relay_event, "game_breath_relay_event", lyracore_module::BreathRelayEvent, bindings::breath_relay_event_type::BreathRelayEvent, {
    id, character_guid, kind, time_remaining_ms, duration_ms, damage, created_at,
});
parity_test!(parity_game_dynamic_object, "game_dynamic_object", lyracore_module::DynamicObject, bindings::dynamic_object_type::DynamicObject, {
    guid, caster_guid, spell_id, map_id, instance_id, x, y, z, radius_yd,
});
parity_test!(parity_game_combat_event, "game_combat_event", lyracore_module::CombatEvent, bindings::combat_event_type::CombatEvent, {
    id, attacker_guid, target_guid, damage, hit_info, killing_blow, created_at, blocked_amount,
    ranged_spell_id, ammo_display_id, spell_swing, impact_delay_ms, map_id, instance_id, grid_x,
    grid_y,
});
parity_test!(parity_game_melee_attack, "game_melee_attack", lyracore_module::MeleeAttack, bindings::melee_attack_type::MeleeAttack, {
    attacker_guid, target_guid, last_swing_ms, ranged_spell_id, last_offhand_swing_ms, rout_ends_ms,
    pursuit_ends_ms, leash_x, leash_y,
});
parity_test!(parity_game_spell_cast_event, "game_spell_cast_event", lyracore_module::SpellCastEvent, bindings::spell_cast_event_type::SpellCastEvent, {
    id, caster_guid, spell_id, created_at, target_guid, cast_time_ms, is_completion, damage,
    school, is_crit, resisted, absorbed, is_interrupted, cooldown_ms, delay_ms, healed,
    is_proc_log, swing_hit_info, client_initiated, map_id, instance_id, grid_x, grid_y,
    failure_reason,
});
parity_test!(parity_game_spell_impact_event, "game_spell_impact_event", lyracore_module::SpellImpactEvent, bindings::spell_impact_event_type::SpellImpactEvent, {
    id, caster_guid, target_guid, spell_id, created_at, damage, school, is_crit, resisted,
    absorbed, map_id, instance_id, grid_x, grid_y,
});
parity_test!(parity_game_creature_cast, "game_creature_cast", lyracore_module::CreatureCast, bindings::creature_cast_type::CreatureCast, {
    creature_entry, spell_id,
});
parity_test!(parity_game_resurrect_request, "game_resurrect_request", lyracore_module::ResurrectRequest, bindings::resurrect_request_type::ResurrectRequest, {
    target_guid, target_identity, caster_guid, caster_name, points, created_at,
});
parity_test!(parity_game_channel_event, "game_channel_event", lyracore_module::ChannelEvent, bindings::channel_event_type::ChannelEvent, {
    id, channel, channel_display, sender_guid, message, created_at,
});
parity_test!(parity_game_channel_member, "game_channel_member", lyracore_module::ChannelMember, bindings::channel_member_type::ChannelMember, {
    id, channel, character_guid, owner_identity,
});
parity_test!(parity_game_aura, "game_aura", lyracore_module::Aura, bindings::aura_type::Aura, {
    id, target_guid, caster_guid, spell_id, slot, level, flags, applied_at, expires_at, effect_id,
    eff_kind, amount, eff_p0, eff_p0_kind, eff_p1, period_ms, amount_remaining, stacks,
    next_tick_micros, channel_target, enters_combat, proc_flags, proc_chance, proc_ppm, proc_ex,
    proc_school_mask, proc_family_name, proc_family_flags, proc_charges, proc_icd_ms,
    proc_ready_micros,
});
parity_test!(parity_game_player_reputation, "game_player_reputation", lyracore_module::PlayerReputation, bindings::player_reputation_type::PlayerReputation, {
    id, character_guid, owner_identity, faction_id, standing, reputation_index, at_war,
});
parity_test!(parity_game_character_contact, "game_character_contact", lyracore_module::ContactEntry, bindings::contact_entry_type::ContactEntry, {
    id, owner_guid, owner_identity, target_guid, is_ignore,
});
parity_test!(parity_game_mail, "game_mail", lyracore_module::Mail, bindings::mail_type::Mail, {
    id, recipient_guid, sender_guid, subject, body, item_entry, item_stack_count, item_durability,
    item_enchant_id, item_soulbound, money, cod, was_read, created_at,
});
parity_test!(parity_game_mail_escrow, "game_mail_escrow", lyracore_module::MailEscrow, bindings::mail_escrow_type::MailEscrow, {
    escrow_id, sender_guid, recipient_guid, subject, body, money, postage, created_micros,
    delivered, payout, mail_id, item_entry, item_stack_count, item_durability, item_enchant_id,
    item_soulbound, cod,
});
parity_test!(parity_game_auction, "game_auction", lyracore_module::Auction, bindings::auction_type::Auction, {
    id, listing_operation_id, house, owner_guid, item_guid, item_entry, item_stack_count,
    item_durability, item_enchant_id, item_soulbound, start_bid, buyout, highest_bidder_guid,
    highest_bid, deposit, created_at, expires_at, revision, deposit_rate, consignment_rate,
});
parity_test!(parity_game_auction_bid_decision, "game_auction_bid_decision", lyracore_module::AuctionBidDecision, bindings::auction_bid_decision_type::AuctionBidDecision, {
    operation_id, bidder_guid, auction_id, offer, outcome, revision, result_bidder_guid,
    result_bid, minimum_increment, deferred_refund, accepted_price, house,
});
parity_test!(parity_game_auction_bid_hold, "game_auction_bid_hold", lyracore_module::AuctionBidHold, bindings::auction_bid_hold_type::AuctionBidHold, {
    operation_id, bidder_guid, auction_id, offer, outcome, revision, result_bidder_guid,
    result_bid, minimum_increment, deferred_refund, accepted_price, house,
});
parity_test!(parity_game_auction_hold, "game_auction_hold", lyracore_module::AuctionHold, bindings::auction_hold_type::AuctionHold, {
    operation_id, seller_guid, item_guid, item_entry, item_stack_count, item_durability,
    item_enchant_id, item_soulbound, start_bid, buyout, duration_minutes, deposit, created_micros,
    expires_micros, house, deposit_rate, consignment_rate,
});
parity_test!(parity_game_auction_operation_receipt, "game_auction_operation_receipt", lyracore_module::AuctionOperationReceipt, bindings::auction_operation_receipt_type::AuctionOperationReceipt, {
    operation_id, auction_id, actor_guid, item_guid, item_entry, item_stack_count, item_durability,
    item_enchant_id, item_soulbound, start_bid, buyout, duration_minutes, deposit, created_micros,
    expires_micros, house, deposit_rate, consignment_rate,
});
parity_test!(parity_game_auction_house, "game_auction_house", lyracore_module::AuctionHouseDefinition, bindings::auction_house_definition_type::AuctionHouseDefinition, {
    id, faction, deposit_rate, consignment_rate, name,
});
parity_test!(parity_game_auction_expiry, "game_auction_expiry", lyracore_module::AuctionExpiry, bindings::auction_expiry_type::AuctionExpiry, {
    scheduled_id, scheduled_at, auction_id,
});
parity_test!(parity_game_faction, "game_faction", lyracore_module::Faction, bindings::faction_type::Faction, {
    faction_id, reputation_index, base_standing,
});
parity_test!(parity_game_faction_template, "game_faction_template", lyracore_module::FactionTemplate, bindings::faction_template_type::FactionTemplate, {
    id, faction, faction_group, friend_group, enemy_group,
    enemy_0, enemy_1, enemy_2, enemy_3, friend_0, friend_1, friend_2, friend_3,
});
parity_test!(parity_game_encounter_equip, "game_encounter_equip", lyracore_module::EncounterEquip, bindings::encounter_equip_type::EncounterEquip, {
    creature_guid, instance_id, main_hand, off_hand, ranged,
});
// The per-player connections subscribe these two for the say/yell + emote relays, so they stay in
// the manifest for the same reason every coordinator table is — a column added module-side without
// the matching binding edit corrupts every row the relay decodes.
parity_test!(parity_game_chat_event, "game_chat_event", lyracore_module::ChatEvent, bindings::chat_event_type::ChatEvent, {
    id, sender_guid, chat_type, language, message, created_at, target_guid,
});
parity_test!(parity_game_emote_event, "game_emote_event", lyracore_module::EmoteEvent, bindings::emote_event_type::EmoteEvent, {
    id, sender_guid, text_emote, emote_anim, created_at, target_guid, map_id, instance_id,
    grid_x, grid_y,
});

/// Every table name that has a `parity_test!` line above. The completeness guard cross-checks
/// this against `stdb/connection.rs`'s real subscription list.
const MANIFEST_TABLES: &[&str] = &[
    "game_realm",
    // Shared-connection model: moved from the per-player AOI box onto the coordinator's global
    // subscription.
    "game_entity_motion",
    "game_creature_spline",
    "game_taxi_passenger_spline",
    "game_chat_event",
    "game_emote_event",
    "game_teleport_event",
    "game_xp_event",
    "game_levelup_event",
    "game_bot_invite_intent",
    "game_bot_transfer_intent",
    "game_addon_message",
    "game_character_explored",
    "game_account",
    "game_session",
    "game_character",
    "game_character_shard",
    "game_map_region",
    "game_region_assignment",
    "game_group",
    "game_group_member",
    "game_creature_quest_tap",
    "game_creature_quest_tap_member",
    "game_creature_loot_tag_group",
    "game_corpse_loot_eligible",
    "game_group_event",
    "game_trade_event",
    "game_duel_event",
    "game_whisper_event",
    "game_system_message_event",
    "game_loot_roll",
    "game_loot_roll_vote",
    "game_guid_allocator",
    "game_guid_range",
    "game_guid_range_registry",
    "game_world_entity",
    "game_zone_weather",
    "game_hunter_pet_protocol",
    "game_config",
    "game_creature_template",
    "game_taxi_node",
    "game_taxi_path",
    "game_taxi_path_node",
    "game_character_taxi_node",
    "game_active_taxi_flight",
    "game_taxi_service_reply",
    "game_start_position",
    "game_corpse",
    "game_item_template",
    "game_spell",
    "game_spell_effect",
    "game_item_instance",
    "game_corpse_loot",
    "game_npc_vendor",
    "game_gossip_menu",
    "game_gossip_option",
    "game_gossip_menu_profile",
    "game_gossip_menu_profile_option",
    "game_creature_gossip_menu_override",
    "game_npc_text",
    "game_npc_text_slot",
    "game_quest_template",
    "game_quest_text",
    "game_quest_objective",
    "game_quest_reward_item",
    "game_quest_reward_choice",
    "game_creature_quest",
    "game_gameobject_quest",
    "game_character_quest",
    "game_player_spell",
    "game_player_action",
    "game_trainer_spell",
    "game_transfer_out",
    "game_player_skill",
    "game_gameobject",
    "game_gameobject_template",
    "game_character_buyback",
    "game_spell_chain",
    "game_talent",
    "game_character_talent",
    "game_roll_event",
    "game_rest_state_event",
    "game_breath_relay_event",
    "game_dynamic_object",
    "game_combat_event",
    "game_melee_attack",
    "game_channel_event",
    "game_channel_member",
    "game_spell_cast_event",
    "game_spell_impact_event",
    "game_creature_cast",
    "game_resurrect_request",
    "game_aura",
    "game_player_reputation",
    "game_character_contact",
    "game_mail",
    "game_mail_escrow",
    "game_auction",
    "game_auction_bid_decision",
    "game_auction_bid_hold",
    "game_auction_hold",
    "game_auction_house",
    "game_auction_operation_receipt",
    "game_faction",
    "game_faction_template",
    "game_encounter_equip",
];

// ---------------------------------------------------------------------------------------------
// Completeness guard: a new coordinator subscription without a parity manifest line must fail
// loudly (catches the FORGOTTEN pair, not just a mistyped one).
// ---------------------------------------------------------------------------------------------

const CONNECTION_RS: &str = include_str!("../src/stdb/connection.rs");

/// Scan `source` for every `"SELECT * FROM game_<x>"` subscription-string literal and return the
/// `game_<x>` table names, in the order they appear. A plain substring scan rather than a
/// `regex` dependency — the subscription strings are a fixed, simple literal shape.
fn extract_select_star_tables(source: &str) -> Vec<String> {
    let needle = "SELECT * FROM ";
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(pos) = rest.find(needle) {
        let after = &rest[pos + needle.len()..];
        let end = after
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        out.push(after[..end].to_string());
        rest = &after[end..];
    }
    out
}

#[test]
fn completeness_guard_extraction_flags_a_missing_manifest_entry() {
    // Synthetic input (NOT the real connection.rs) proving the extraction+diff logic itself
    // works, independent of today's real subscription list.
    let fake = r#"
        .subscribe(vec![
            "SELECT * FROM game_real_one",
            "SELECT * FROM game_totally_fake_table",
        ]);
    "#;
    let subscribed = extract_select_star_tables(fake);
    let manifest = ["game_real_one"];
    let missing: Vec<&String> = subscribed
        .iter()
        .filter(|t| !manifest.contains(&t.as_str()))
        .collect();
    assert_eq!(
        missing,
        vec!["game_totally_fake_table"],
        "the completeness-guard extraction/diff should flag exactly the one subscribed table \
         missing from the manifest"
    );
}

#[test]
fn every_subscribed_table_in_connection_rs_has_a_parity_manifest_entry() {
    let subscribed = extract_select_star_tables(CONNECTION_RS);
    assert!(
        subscribed.len() >= 30,
        "sanity check: found only {} `SELECT * FROM game_...` subscriptions in connection.rs — \
         did the subscription-string format change? (the extraction scan needs updating)",
        subscribed.len()
    );
    let missing: Vec<&String> = subscribed
        .iter()
        .filter(|t| !MANIFEST_TABLES.contains(&t.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "connection.rs subscribes {missing:?} but gateway/tests/schema_parity.rs has no \
         `parity_test!` line for it — add one (copy the pattern from any existing entry above) \
         AND add the table name to MANIFEST_TABLES, or a gateway binding drift on this table will \
         break live BSATN decode silently. See docs/agent-playbook.md failure-mode §1."
    );
}
