//! Cast opcodes through their dispatcher. Route selection, target decoding and durable dispatch
//! are covered per route by the focused seam tests in `handlers/cast`; these tests pin what the
//! session sees: the ordered batch, the ranged auto-repeat state it carries between casts, and
//! the effect of each cancellation.

use super::handlers::InMemoryCasts;
use super::*;
use std::sync::Mutex;

/// `SMSG_CAST_RESULT`, a raw frame. The synchronous ORDER across raw and typed packets is the
/// contract under test.
const OP_CAST_RESULT: u16 = 0x0130;

/// What the session keeps between two casts.
struct Caster {
    self_guid: Option<u64>,
    ranged_repeat: bool,
}

impl Caster {
    fn in_world() -> Self {
        Self {
            self_guid: Some(1),
            ranged_repeat: false,
        }
    }

    fn at_character_select() -> Self {
        Self {
            self_guid: None,
            ranged_repeat: false,
        }
    }
}

/// Dispatch one cast message, apply the session transition it asks for, and return the packets
/// the session would send for it.
fn run(
    store: &InMemoryCasts,
    caster: &mut Caster,
    msg: impl Into<ClientOpcodeMessage>,
) -> Vec<Outbound> {
    let player = CastPlayer {
        account_id: 7,
        self_guid: caster.self_guid,
        ranged_repeat: caster.ranged_repeat,
    };
    match dispatch_cast(store, player, msg.into()).unwrap() {
        CastOutcome::Handled {
            transition,
            outbound,
        } => {
            if let Some(armed) = transition.ranged_repeat {
                caster.ranged_repeat = armed;
            }
            outbound
        }
        CastOutcome::PassThrough(_) => panic!("the cast dispatcher passed the message on"),
    }
}

/// `SpellCastTargets` carrying a UNIT target (the client's selected mob).
fn unit_targets(guid: u64) -> SpellCastTargets {
    SpellCastTargets {
        target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
            SpellCastTargets_SpellCastTargetFlags_Unit {
                unit_target: Guid::new(guid),
            },
        ),
    }
}

#[test]
fn instant_cast_sends_start_then_raw_cast_result_ok_then_go_and_threads_the_target() {
    // Root-cause client-wedge fix: an INSTANT cast must emit START(0) -> raw CAST_RESULT(OK,
    // opcode 0x0130, 5-byte body) -> GO, IN THAT ORDER. That order is a real invariant: the 5875
    // client needs the OK ack between START and GO to clear its cast slot. The unit target must
    // reach the cast, which shows in the GO's hit list.
    let store = InMemoryCasts::instant();
    let sent = run(
        &store,
        &mut Caster::in_world(),
        CMSG_CAST_SPELL {
            spell: 100,
            targets: unit_targets(77),
        },
    );

    let mut want = 100u32.to_le_bytes().to_vec();
    want.push(0x00); // SPELL_RESULT_STATUS_OKAY: 5 bytes, NO trailing reason byte
    match sent.as_slice() {
        [Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(_)), Outbound::Raw {
            opcode: OP_CAST_RESULT,
            body,
        }, Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(go))] => {
            assert_eq!(
                *body, want,
                "CAST_RESULT body is spell_id(u32 LE) + OKAY(0x00)"
            );
            let hits: Vec<u64> = go.hits.iter().map(|g| g.guid()).collect();
            assert_eq!(hits, [77], "the client's unit target is the GO's hit");
        }
        _ => panic!(
            "expected START, raw CAST_RESULT, GO; got {} packets",
            sent.len()
        ),
    }
}

#[test]
fn auto_shot_intercept_starts_the_ranged_attack_instead_of_casting() {
    // Vanilla shape: Auto Shot (75) and wand Shoot (5019) are auto-repeat ranged attacks. The
    // activation arms the loop with the cast's unit target, then the ack is SMSG_SPELL_START ALONE
    // with timer 0 (no CAST_RESULT, no GO: the cast parks in the client's AUTOREPEAT slot and
    // never resolves; each shot's GO comes from the swing-tick relay). The ordinary cast route
    // would send all three packets, so a lone START also shows `cast_spell` did not run.
    for spell in [75u32, 5019] {
        let store = InMemoryCasts {
            ranged_auto_repeat: vec![75, 5019],
            ..InMemoryCasts::instant()
        };
        let mut caster = Caster::in_world();
        let sent = run(
            &store,
            &mut caster,
            CMSG_CAST_SPELL {
                spell,
                targets: unit_targets(88),
            },
        );
        match sent.as_slice() {
            [Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(start))] => {
                assert_eq!(start.spell, spell, "spell id in START");
                assert_eq!(
                    start.timer, 0,
                    "spell {spell}: START timer must be 0 (the 0.5s wind-up is an attack-timer, not a cast bar)"
                );
                let target = start
                    .targets
                    .target_flags
                    .get_unit()
                    .map(|u| u.unit_target.guid());
                assert_eq!(
                    target,
                    Some(88),
                    "spell {spell}: START names the unit target"
                );
            }
            _ => panic!(
                "spell {spell}: expected the activation START alone, got {} packets",
                sent.len()
            ),
        }
        assert!(
            caster.ranged_repeat,
            "spell {spell}: the session arms the loop"
        );
        assert_eq!(
            store.engaged.lock().unwrap().as_slice(),
            &[1],
            "spell {spell}: the durable loop engages the caster"
        );
    }
}

#[test]
fn a_cast_before_entering_the_world_answers_nothing_and_does_not_fail() {
    // A cast can arrive while the session is still at character select: a stale addon macro, a
    // reconnect race. The seam answers no frames and names a zero caster; the dispatcher returns
    // Ok, so the session serves the next opcode.
    let store = InMemoryCasts::instant();
    let sent = run(
        &store,
        &mut Caster::at_character_select(),
        CMSG_CAST_SPELL {
            spell: 100,
            targets: unit_targets(77),
        },
    );
    assert!(sent.is_empty());
}

#[test]
fn cancelling_auto_repeat_still_tears_the_ranged_loop_down_through_stop_attack() {
    // The cancel tears the loop down only when one is armed (the `was_repeat` gate). Arm Auto
    // Shot, then cancel, and check that the durable engagement for this attacker is gone.
    let store = InMemoryCasts {
        ranged_auto_repeat: vec![75],
        ..InMemoryCasts::instant()
    };
    let mut caster = Caster::in_world();
    run(
        &store,
        &mut caster,
        CMSG_CAST_SPELL {
            spell: 75,
            targets: unit_targets(88),
        },
    );
    assert_eq!(store.engaged.lock().unwrap().as_slice(), &[1]);

    // The cancel sends no ack of its own (the on_delete relay does).
    let sent = run(&store, &mut caster, CMSG_CANCEL_AUTO_REPEAT_SPELL {});
    assert!(sent.is_empty());
    assert!(!caster.ranged_repeat);
    assert!(store.engaged.lock().unwrap().is_empty());
}

#[test]
fn cancel_aura_removes_the_aura_the_wire_spell_id_names() {
    let store = InMemoryCasts {
        auras: Mutex::new(vec![5555, 6000]),
        ..Default::default()
    };
    let sent = run(
        &store,
        &mut Caster::in_world(),
        CMSG_CANCEL_AURA { id: 5555 },
    );
    assert!(sent.is_empty());
    assert_eq!(store.auras.lock().unwrap().as_slice(), &[6000]);
}

#[test]
fn cancel_cast_drops_the_pending_cast_of_the_caller_only() {
    let store = InMemoryCasts {
        pending_casts: Mutex::new(vec![1, 2]),
        ..Default::default()
    };
    let sent = run(
        &store,
        &mut Caster::in_world(),
        CMSG_CANCEL_CAST { id: 133 },
    );
    assert!(sent.is_empty());
    assert_eq!(store.pending_casts.lock().unwrap().as_slice(), &[2]);
}
