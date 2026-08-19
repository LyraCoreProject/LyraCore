use spacetimedb::table;

/// One ordered action from a native EventAI source rule. The fields through `repeat_max_ms` are the
/// migration shape. New rules use the compact fields appended after them.
#[table(
    accessor = game_creature_ai_event,
    index(accessor = by_entry, btree(columns = [creature_entry])),
    index(accessor = by_guid, btree(columns = [creature_guid]))
)]
#[derive(Clone)]
pub struct CreatureAiEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub event_type: u8,
    pub action_type: u8,
    pub text: String,
    #[default(0u32)]
    pub spell_id: u32,
    #[default(0u32)]
    pub initial_min_ms: u32,
    #[default(0u32)]
    pub initial_max_ms: u32,
    #[default(0u32)]
    pub repeat_min_ms: u32,
    #[default(0u32)]
    pub repeat_max_ms: u32,
    #[default(0u64)]
    pub source_rule_id: u64,
    #[default(0u8)]
    pub action_order: u8,
    #[default(0u64)]
    pub creature_guid: u64,
    #[default(0u8)]
    pub chance_pct: u8,
    #[default(0u32)]
    pub allowed_phase_mask: u32,
    #[default(0u32)]
    pub source_flags: u32,
    #[default(0u8)]
    pub repeat_policy: u8,
    #[default(0u32)]
    pub event_param_1: u32,
    #[default(0u32)]
    pub event_param_2: u32,
    #[default(0u32)]
    pub event_param_3: u32,
    #[default(0u32)]
    pub event_param_4: u32,
    #[default(0u32)]
    pub event_param_5: u32,
    #[default(0u32)]
    pub event_param_6: u32,
    #[default(0u32)]
    pub action_param_1: u32,
    #[default(0u32)]
    pub action_param_2: u32,
    #[default(0u32)]
    pub action_param_3: u32,
    #[default(0u8)]
    pub target_policy: u8,
    #[default(0u32)]
    pub cast_options: u32,
}

/// Broadcast text referenced by EventAI actions. Module only.
#[table(accessor = game_creature_ai_broadcast_text)]
pub struct CreatureAiBroadcastText {
    #[primary_key]
    pub id: u32,
    pub male_text: String,
    pub female_text: String,
    pub chat_type: u8,
    pub language_id: u8,
    pub emote_delay_1_ms: u32,
    pub emote_id_1: u32,
    pub emote_delay_2_ms: u32,
    pub emote_id_2: u32,
    pub emote_delay_3_ms: u32,
    pub emote_id_3: u32,
}

/// Creature-wide EventAI state. Edge resets advance its lifecycle identities. Module only.
#[table(accessor = game_creature_ai_state)]
pub struct CreatureAiState {
    #[primary_key]
    pub creature_guid: u64,
    pub phase: u8,
    pub lifecycle_id: u64,
    pub engagement_id: u64,
    pub ranged_distance: f32,
    pub ranged_angle: f32,
}

/// Timing and consumption state for one creature and one effective source rule. Module only.
#[table(
    accessor = game_creature_ai_rule_state,
    index(accessor = by_creature, btree(columns = [creature_guid]))
)]
pub struct CreatureAiRuleState {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_guid: u64,
    pub source_rule_id: u64,
    pub next_eligible_ms: u64,
    pub consumed: bool,
    pub lifecycle_id: u64,
    pub engagement_id: u64,
}

/// Migration table retained for timers written by the previous timer pass. The native evaluator
/// ignores these rows and writes `game_creature_ai_rule_state` instead.
#[table(
    accessor = game_creature_ai_timer,
    index(accessor = by_creature, btree(columns = [creature_guid]))
)]
pub struct CreatureAiTimer {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_guid: u64,
    pub rule_id: u64,
    pub next_at_ms: u64,
}

/// Imported EventAI summon placement. Module only.
#[table(accessor = game_creature_ai_summon)]
pub struct CreatureAiSummon {
    #[primary_key]
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub lifetime_ms: u32,
}
