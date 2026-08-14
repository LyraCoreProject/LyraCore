//! The ranged auto-repeat route: Auto Shot and wand Shoot are attack LOOPS, not one-shot casts.
//!
//! Activation arms the durable loop and parks a cast in the client's auto-repeat slot; each real
//! shot belongs to the durable swing tick and its combat-event relay. Cancellation only clears the
//! loop. Both directions return a session transition — whether a loop is armed is gateway session
//! state, and melee start, melee stop and this route all read it.

use super::*;

/// Arm or refuse a ranged auto-repeat loop (`CMSG_CAST_SPELL` for Auto Shot / wand Shoot).
pub(super) fn activate<St: CastStore + ?Sized>(
    store: &St,
    player: CastPlayer,
    c: &CMSG_CAST_SPELL,
) -> Result<CastOutcome> {
    let spell = c.spell;
    // The shot's target rides the cast's UNIT block. No current-selection fallback: Auto Shot and
    // Shoot are cast ON a target, so the client always sends one.
    let target = unit_target(c);
    log::info!(
        "world[autoshot]: activate spell={spell} target={target} already_repeating={} (account {})",
        player.ranged_repeat,
        player.account_id
    );

    // Arm the durable loop FIRST, before any success outcome exists. A refusal then answers only
    // the raw failure result, and the 5875 client drops its auto-repeat toggle on that — client and
    // server stay in lockstep. Acknowledging first and failing after left the client toggled ON
    // over a dead loop, so the next press sent a cancel instead of a cast.
    if let Err(e) = store.start_ranged_attack(
        player.account_id,
        player.self_guid.unwrap_or(0),
        target,
        spell,
    ) {
        return refuse(store, player, spell, e);
    }

    // The activation ack is `SMSG_SPELL_START` alone: timer 0 (the wind-up is an attack timer, not
    // a cast bar — the client animates its own), the ammo block that nocks the projectile, and the
    // real unit target. No CAST_RESULT(OK) and no GO: this cast parks in the client's auto-repeat
    // slot and never resolves, and each shot's GO comes from the swing-tick relay.
    let outbound = player.self_guid.map_or_else(Vec::new, |caster| {
        vec![Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(
            Box::new(codec::build_spell_start(
                caster,
                spell,
                0,
                target,
                ammo_display(store, caster),
            )),
        ))]
    });
    Ok(CastOutcome::Handled {
        transition: CastTransition {
            ranged_repeat: Some(true),
        },
        outbound,
    })
}

/// A refused activation. Gameplay refusals are handled outcomes; a dead reducer transport is not.
fn refuse<St: CastStore + ?Sized>(
    store: &St,
    player: CastPlayer,
    spell: u32,
    e: anyhow::Error,
) -> Result<CastOutcome> {
    if is_transport_failure(&e) {
        return Err(e);
    }
    log::info!(
        "world[autoshot]: start_ranged_attack refused spell={spell} (account {}): {e}",
        player.account_id
    );
    let outbound = vec![Outbound::Raw {
        opcode: OP_CAST_RESULT,
        body: codec::build_cast_result_failed(spell, codec::cast_failure_reason_for(&e.to_string())),
    }];
    // A refused RETARGET drops the client's toggle on that failure result, so the still-firing OLD
    // loop has to go too — otherwise the server keeps shooting a target the client believes it
    // stopped. A fresh activation has no older loop, and must not touch unrelated engagement state.
    if !player.ranged_repeat {
        return Ok(CastOutcome::Handled {
            transition: CastTransition::default(),
            outbound,
        });
    }
    stop_loop(store, player, "reject-teardown");
    Ok(CastOutcome::Handled {
        transition: CastTransition {
            ranged_repeat: Some(false),
        },
        outbound,
    })
}

/// `CMSG_CANCEL_AUTO_REPEAT_SPELL`: the client toggled the loop off, or auto-switched to melee.
pub(super) fn cancel<St: CastStore + ?Sized>(store: &St, player: CastPlayer) -> CastOutcome {
    log::info!(
        "world[autoshot]: cancel auto-repeat active={} (account {})",
        player.ranged_repeat,
        player.account_id
    );
    // The durable stop runs ONLY when a ranged loop was armed. A melee press sends CMSG_ATTACKSWING
    // and this cancel back to back, and the swing handler has already overwritten the shared
    // engagement row to melee — an unconditional stop deleted that just-armed melee row. No inline
    // ack either: the engagement's on_delete relay is the one sender of SMSG_CANCEL_AUTO_REPEAT.
    if player.ranged_repeat {
        stop_loop(store, player, "cancel_auto_repeat");
    }
    CastOutcome::Handled {
        transition: CastTransition {
            ranged_repeat: Some(false),
        },
        outbound: Vec::new(),
    }
}

/// Ask the module to tear the engagement down. Best effort: a refused stop (nothing armed, a race
/// with the swing tick) must not end the session or reach the client.
fn stop_loop<St: CastStore + ?Sized>(store: &St, player: CastPlayer, context: &str) {
    if let Err(e) = store.stop_attack(player.account_id, player.self_guid.unwrap_or(0)) {
        log::debug!(
            "world: {context} stop_attack ignored (account {}): {e}",
            player.account_id
        );
    }
}

/// The `(display_id, inv_type)` ammo block for the activation START. `Some` only when the ranged
/// slot (17) holds a launcher (weapon class 2, subclass 2/3/18 — bow/gun/crossbow) and a live
/// projectile stack is in the bags. A wand fires its own bolt, and a launcher with no live stack
/// omits the block so the durable loop alone decides whether a shot can happen. Inventory type 24
/// (INVTYPE_AMMO) is hardcoded, matching the GO relay.
fn ammo_display<St: CastStore + ?Sized>(store: &St, self_guid: u64) -> Option<(u32, u32)> {
    let items = store.player_items(self_guid).ok()?;
    let launcher = items
        .iter()
        .find(|i| i.slot == 17)
        .and_then(|i| store.item_template(i.entry).ok().flatten())?;
    if launcher.class != 2 || !matches!(launcher.subclass, 2 | 3 | 18) {
        return None;
    }
    // Lowest slot among live stacks mirrors the module's per-shot `find_ammo` pick, so the
    // projectile nocked on the START is the one each shot consumes. The client's item cache is
    // unsorted, and a spent stack must never be nocked.
    items
        .iter()
        .filter(|i| i.stack_count > 0)
        .filter(|i| {
            store
                .item_template(i.entry)
                .ok()
                .flatten()
                .is_some_and(|t| t.class == 6)
        })
        .min_by_key(|i| i.slot)
        .and_then(|i| store.item_template(i.entry).ok().flatten())
        .map(|t| (t.display_id, 24))
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;
    use codec::{ItemInstanceView, ItemTemplateView};

    const AUTO_SHOT: u32 = 75;

    /// One owned item: `entry` in inventory `slot`, holding `stack_count`.
    fn owned(slot: u8, entry: u32, stack_count: u32) -> ItemInstanceView {
        ItemInstanceView {
            guid: u64::from(entry),
            entry,
            owner_guid: CASTER,
            slot,
            stack_count,
            ..Default::default()
        }
    }

    /// A weapon template (class 2). Subclass 2/3/18 is a bow/gun/crossbow; 19 is a wand.
    fn launcher(entry: u32, subclass: u8) -> ItemTemplateView {
        ItemTemplateView {
            entry,
            class: 2,
            subclass,
            ..Default::default()
        }
    }

    /// A projectile template (class 6) with its own display id.
    fn projectile(entry: u32, display_id: u32) -> ItemTemplateView {
        ItemTemplateView {
            entry,
            class: 6,
            display_id,
            ..Default::default()
        }
    }

    /// A store whose spell 75 routes as ranged auto-repeat.
    fn ranged_store() -> InMemoryCasts {
        InMemoryCasts {
            ranged_auto_repeat: vec![AUTO_SHOT],
            ..InMemoryCasts::instant()
        }
    }

    /// A player with an older ranged loop already armed.
    fn repeating() -> CastPlayer {
        CastPlayer {
            ranged_repeat: true,
            ..player()
        }
    }

    /// The single `SMSG_SPELL_START` in the batch.
    fn spell_start(outbound: &[Outbound]) -> &wow_world_messages::vanilla::SMSG_SPELL_START {
        outbound
            .iter()
            .find_map(|out| match out {
                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(start)) => Some(&**start),
                _ => None,
            })
            .expect("the batch has no SMSG_SPELL_START")
    }

    /// The ammo block the activation START carries, if any.
    fn ammo_block(outbound: &[Outbound]) -> Option<(u32, u32)> {
        spell_start(outbound)
            .flags
            .get_ammo()
            .map(|a| (a.ammo_display_id, a.ammo_inventory_type))
    }

    // ── Activation ───────────────────────────────────────────────────────────

    #[test]
    fn activation_arms_the_durable_loop_and_acknowledges_with_a_zero_timer_start() {
        let store = ranged_store();

        let (transition, outbound) =
            handled(dispatch_cast(&store, player(), cast(AUTO_SHOT, unit_targets(88))).unwrap());

        assert_eq!(
            store.ranged_attacks.lock().unwrap().as_slice(),
            &[(ACCOUNT, CASTER, 88, AUTO_SHOT)],
            "the cast's unit target arms the loop, with no selection fallback"
        );
        assert_eq!(
            transition,
            CastTransition {
                ranged_repeat: Some(true)
            }
        );
        assert_eq!(
            sequence(&outbound),
            ["START"],
            "no CAST_RESULT(OK) and no GO: the activation is not a completed shot"
        );
        let start = spell_start(&outbound);
        assert_eq!(start.timer, 0, "the wind-up is an attack timer, not a cast bar");
        assert_eq!(
            start
                .targets
                .target_flags
                .get_unit()
                .map(|u| u.unit_target.guid()),
            Some(88)
        );
        assert!(store.casts.lock().unwrap().is_empty());
        assert!(store.stop_attacks.lock().unwrap().is_empty());
    }

    #[test]
    fn fresh_activation_failure_sends_only_the_failure_and_stops_nothing() {
        let store = InMemoryCasts {
            ranged_error: Some("no ranged weapon equipped".into()),
            ..ranged_store()
        };

        let (transition, outbound) =
            handled(dispatch_cast(&store, player(), cast(AUTO_SHOT, unit_targets(88))).unwrap());

        assert_eq!(sequence(&outbound), ["CAST_RESULT(FAILED 0x17)"]);
        assert_eq!(
            transition,
            CastTransition::default(),
            "a refused activation must not arm ranged-repeat state"
        );
        assert!(
            store.stop_attacks.lock().unwrap().is_empty(),
            "there is no older loop to tear down"
        );
    }

    #[test]
    fn retarget_failure_over_an_active_loop_clears_the_state_and_stops_the_old_loop() {
        let store = InMemoryCasts {
            ranged_error: Some("target is out of range".into()),
            ..ranged_store()
        };

        let (transition, outbound) =
            handled(dispatch_cast(&store, repeating(), cast(AUTO_SHOT, unit_targets(99))).unwrap());

        assert_eq!(sequence(&outbound), ["CAST_RESULT(FAILED 0x59)"]);
        assert_eq!(
            transition,
            CastTransition {
                ranged_repeat: Some(false)
            }
        );
        assert_eq!(
            store.stop_attacks.lock().unwrap().as_slice(),
            &[(ACCOUNT, CASTER)],
            "the client dropped its toggle, so the still-firing old loop must go too"
        );
    }

    #[test]
    fn a_dead_reducer_transport_during_activation_is_session_fatal() {
        let store = InMemoryCasts {
            ranged_error: Some("start_ranged_attack reducer transport disconnected".into()),
            ..ranged_store()
        };

        assert!(dispatch_cast(&store, player(), cast(AUTO_SHOT, unit_targets(88))).is_err());
    }

    // ── Cancellation ─────────────────────────────────────────────────────────

    #[test]
    fn cancelling_an_active_loop_clears_the_state_and_stops_it_with_no_inline_ack() {
        let store = ranged_store();

        let (transition, outbound) = handled(
            dispatch_cast(
                &store,
                repeating(),
                ClientOpcodeMessage::CMSG_CANCEL_AUTO_REPEAT_SPELL,
            )
            .unwrap(),
        );

        assert_eq!(
            transition,
            CastTransition {
                ranged_repeat: Some(false)
            }
        );
        assert_eq!(
            store.stop_attacks.lock().unwrap().as_slice(),
            &[(ACCOUNT, CASTER)]
        );
        assert!(
            outbound.is_empty(),
            "the engagement's on_delete relay is the one sender of the cancel signal"
        );
    }

    #[test]
    fn cancelling_with_no_active_loop_clears_the_state_without_a_durable_stop() {
        let store = ranged_store();

        let (transition, outbound) = handled(
            dispatch_cast(
                &store,
                player(),
                ClientOpcodeMessage::CMSG_CANCEL_AUTO_REPEAT_SPELL,
            )
            .unwrap(),
        );

        assert_eq!(
            transition,
            CastTransition {
                ranged_repeat: Some(false)
            }
        );
        assert!(
            store.stop_attacks.lock().unwrap().is_empty(),
            "a late cancel must not delete a just-armed melee engagement"
        );
        assert!(outbound.is_empty());
    }

    // ── Ammo display ─────────────────────────────────────────────────────────

    /// Ranged slot 17 holds `launcher`, and the given projectile stacks sit in the bags.
    fn armed_with(launcher: ItemTemplateView, ammo: Vec<(u8, u32, u32)>) -> InMemoryCasts {
        let mut items = vec![owned(17, launcher.entry, 1)];
        let mut templates = vec![launcher];
        for (slot, entry, stack_count) in ammo {
            items.push(owned(slot, entry, stack_count));
            templates.push(projectile(entry, entry * 10));
        }
        InMemoryCasts {
            items,
            templates,
            ..ranged_store()
        }
    }

    fn activate_ammo(store: &InMemoryCasts) -> Option<(u32, u32)> {
        let (_, outbound) =
            handled(dispatch_cast(store, player(), cast(AUTO_SHOT, unit_targets(88))).unwrap());
        ammo_block(&outbound)
    }

    #[test]
    fn every_launcher_class_nocks_its_projectile() {
        for subclass in [2u8, 3, 18] {
            let store = armed_with(launcher(100, subclass), vec![(5, 200, 40)]);

            assert_eq!(
                activate_ammo(&store),
                Some((2000, 24)),
                "weapon subclass {subclass} is a bow, gun or crossbow"
            );
        }
    }

    #[test]
    fn a_wand_omits_the_ammo_block() {
        let store = armed_with(launcher(100, 19), vec![(5, 200, 40)]);

        assert_eq!(
            activate_ammo(&store),
            None,
            "a wand fires its own bolt and renders it itself"
        );
    }

    #[test]
    fn an_empty_ranged_slot_omits_the_ammo_block() {
        let store = InMemoryCasts {
            items: vec![owned(5, 200, 40)],
            templates: vec![projectile(200, 2000)],
            ..ranged_store()
        };

        assert_eq!(activate_ammo(&store), None);
    }

    #[test]
    fn a_launcher_with_no_projectile_omits_the_ammo_block() {
        let store = armed_with(launcher(100, 2), Vec::new());

        assert_eq!(
            activate_ammo(&store),
            None,
            "the durable loop decides whether a shot can occur"
        );
    }

    #[test]
    fn a_spent_projectile_stack_is_never_nocked() {
        let store = armed_with(launcher(100, 2), vec![(5, 200, 0)]);

        assert_eq!(activate_ammo(&store), None);
    }

    #[test]
    fn the_lowest_slot_live_stack_wins() {
        let store = armed_with(launcher(100, 2), vec![(9, 300, 20), (4, 201, 60), (2, 200, 0)]);

        assert_eq!(
            activate_ammo(&store),
            Some((2010, 24)),
            "slot 2 is spent, so slot 4 is the deterministic pick"
        );
    }
}
