use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::{
    EnvironmentalDamageType, TimerType, SMSG_ENVIRONMENTAL_DAMAGE_LOG, SMSG_START_MIRROR_TIMER,
    SMSG_STOP_MIRROR_TIMER,
};

const DRAIN_SCALE: i32 = -1;
const FATIGUE_RECOVERY_SCALE: i32 = 10;

fn mirror_timer_start(
    timer: TimerType,
    time_remaining_ms: u32,
    duration_ms: u32,
    scale: i32,
) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_START_MIRROR_TIMER(Box::new(SMSG_START_MIRROR_TIMER {
        timer,
        time_remaining: time_remaining_ms,
        duration: duration_ms,
        // The binding uses u32 for this signed wire field.
        scale: scale as u32,
        is_frozen: false,
        id: 0,
    }))
}

fn environmental_damage_log(
    character_guid: u64,
    damage_type: EnvironmentalDamageType,
    damage: u32,
) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_ENVIRONMENTAL_DAMAGE_LOG(Box::new(SMSG_ENVIRONMENTAL_DAMAGE_LOG {
        guid: wow_world_messages::Guid::new(character_guid),
        damage_type,
        damage,
        absorb: 0,
        resist: 0,
    }))
}

pub fn build_breath_timer_start(time_remaining_ms: u32, duration_ms: u32) -> ServerOpcodeMessage {
    mirror_timer_start(
        TimerType::Breath,
        time_remaining_ms,
        duration_ms,
        DRAIN_SCALE,
    )
}

pub fn build_breath_timer_stop() -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_STOP_MIRROR_TIMER(SMSG_STOP_MIRROR_TIMER {
        timer: TimerType::Breath,
    })
}

pub fn build_drowning_damage_log(character_guid: u64, damage: u32) -> ServerOpcodeMessage {
    environmental_damage_log(character_guid, EnvironmentalDamageType::Drowning, damage)
}

pub fn build_fatigue_timer_start(
    in_deep_water: bool,
    time_remaining_ms: u32,
    duration_ms: u32,
) -> ServerOpcodeMessage {
    let scale = if in_deep_water {
        DRAIN_SCALE
    } else {
        FATIGUE_RECOVERY_SCALE
    };
    mirror_timer_start(TimerType::Fatigue, time_remaining_ms, duration_ms, scale)
}

pub fn build_fatigue_timer_stop() -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_STOP_MIRROR_TIMER(SMSG_STOP_MIRROR_TIMER {
        timer: TimerType::Fatigue,
    })
}

pub fn build_exhaustion_damage_log(character_guid: u64, damage: u32) -> ServerOpcodeMessage {
    environmental_damage_log(character_guid, EnvironmentalDamageType::Exhausted, damage)
}
