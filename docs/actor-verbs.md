# Actor verbs

Actor verbs expose the same action operations to player reducers, debug reducers, and Package bots.
They accept an explicit Actor guid and apply the operation's gameplay Gates.

Existing verbs retain their `Result<(), String>` contract. Typed requests expose accepted
work and pending cast identity without changing the client operations. All Gates remain
in the operation that owns them. Callers authorize the Actor before entering these functions.

| verb | core | Gates and outcomes |
|------|------|------------------------------------------------------------|
| `attack` | `combat::apply_start_attack` | CC-blocked rejected; no self/corpse/cross-map; friendly (green) target rejected when faction data exists; re-arm retargets |
| `request_attack` | `combat::request_attack` | typed acceptance or Refusal; keeps a matching swing timer; an in-range Character turns toward the exact target when the facing Gate would block its swing unless an active movement leg owns facing; acceptance does not imply damage |
| `ranged_attack` | `combat::apply_start_ranged_attack` | `attack` gates + ranged weapon equipped (slot 17) |
| `stop_attack` | `combat::stop_attack_for` | unconditional disarm of the actor's outgoing melee row |
| `cast_at` | `spell::request_cast` | normal cast lifecycle; an existing timed cast waits; level comes from the live entity |
| `cast_readiness` | `spell::cast_readiness` | read-only spellbook, supported-lifecycle, and cast Gate check |
| `request_cast` | `spell::request_cast` | typed start, waiting, and Refusal; completion uses `on_cast_finished` |
| `accept_quest` | `quest::apply_accept_quest` | alive + giver in range offering the quest + level/race/class/prereq/duplicate gates |
| `stage_quest` | `quest::grant_quest_unchecked` | Debug and bot staging: same row shape, all accept gates SKIPPED (giver-less) |
| `turn_in_quest` | `quest::apply_turn_in_quest` | alive + giver in range ending the quest + objectives complete; rewards atomic |
| `open_creature_loot` | `loot::open_creature_corpse` | legacy result adapter used by the Gateway |
| `request_open_creature_loot` | `loot::request_open_creature_corpse` | typed open acceptance or Refusal; applies the same corpse and Loot Tag Gates |
| `take_loot` | `items::apply_take_loot` | legacy result adapter used by the existing rest-and-loot goal |
| `request_take_loot` | `items::request_take_loot` | typed take completion or Refusal; inventory-full leaves the item and Loot Source unchanged |
| `loot_money` | `loot::apply_loot_money` | alive + dead creature corpse, same map, 10yd, money > 0 + Loot Tag eligibility |
| `buy_item` | `items::apply_buy_item` | vendor in range + stocked + money; stacks/slots validated |
| `sell_item` | `items::apply_item_sell` | vendor in range + sellable item in slot; feeds the buyback ring |
| `use_item` | `items::apply_item_use` | alive + usable item in slot (consumable/on-use gates) |
| `cast_item_target` | `creatures::apply_item_target_spell` | spell kind + owned carried item + effect-specific gates |
| `equip_item` | `items::apply_equip_item` | slot type/level gates |
| `trainer_buy` | `trainer::apply_trainer_buy` | trainer resolved by guid + offering gates (class/level/money) |
| `reconcile_profile_spell` | `trainer::reconcile_profile_spell` | free profile grant + trainer/class/level/rank gates; explicit no-import demo fallback |
| `reconcile_profile_skill` | `skill::reconcile_profile_skill` | free profile grant + race/class/level availability gates |
| `select_profile_talent` | `talent::select_profile_talent` | bounded preferred-tree selection through the owning talent Gates |
| `learn_profile_talent` | `talent::reconcile_profile_talent` | class/race/level/point/tier/prerequisite gates |
| `reconcile_profile_item` | `items::request_profile_item` | bounded top-up + capacity/uniqueness gates |
| `equip_profile_upgrade` | `items::apply_equip_profile_upgrade` | normal equip gates + preserves equal or stronger gear |
| `reconcile_starter_role_spell_levels` | `seed::reconcile_curated_starter_role_levels` | repairs only exact old curated level-zero headers; preserves imported or tuned rows |
| `use_gameobject` | `gameobject::apply_use_gameobject` | GO resolved by guid + range/use gates |
| `request_use_gameobject` | `gameobject::request_use_gameobject` | typed target, partition, range, and use Refusal |
| `area_trigger_route` | `quest::area_trigger_route` | exact imported source volume and target map; landing remains private |
| `enter_sessionless_areatrigger` | `quest::enter_sessionless_areatrigger` | imported source volume + expected certified party partition + session-less action Gates |
| `import_revision` | `game_import_meta` read | current importer source and file identities for one family |
| `repop` | `world::do_repop` | dead actor releases to the graveyard ghost |
| `respond_resurrect` | `spell::do_resurrect_response` | consume the actor's pending rez offer; accept revives IN PLACE at the offer's % |
| `spirit_res` | `world::do_spirit_healer_res` | ghost actor res at the spirit healer (sickness applies) |
| `self_resurrect` | `spell::do_self_resurrect` | dead actor with a Self-Resurrection Option revives in place; no sickness |
| `accept_group_invite` | `group::accept_invite_for` | pending invite exists + its Group still exists (or a solo inviter is still ungrouped) + group not full; roster events fire |
| `set_sessionless_action_consent` | `sessionless::set_sessionless_action_consent` | update Package consent and clear unclaimed Group Intents atomically |
| `companion_target_facts` | `group::companion_target_facts` | exact hostile creature + partition/death/control gates; never selects a substitute |
| `system_message` | `chat::emit_system_message` | recipient exists and is online on this Shard; text is trimmed, bounded, and non-empty |

Add a consumed verb to this table and the re-exports in `module/src/actor.rs`. If its operation
is inlined in a sender-bound reducer, extract the operation and let both callers use it. Delete
unused verbs. `debug_only!` and `package_only!` allow unused imports only when their consumer tree
is absent from the build.
