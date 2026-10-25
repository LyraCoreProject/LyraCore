use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::{
    EnvironmentalDamageType, TimerType, SMSG_ENVIRONMENTAL_DAMAGE_LOG, SMSG_START_MIRROR_TIMER,
    SMSG_STOP_MIRROR_TIMER,
};

const BREATH_TIMER: TimerType = TimerType::Breath;

pub fn build_breath_timer_start(time_remaining_ms: u32, duration_ms: u32) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_START_MIRROR_TIMER(Box::new(SMSG_START_MIRROR_TIMER {
        timer: BREATH_TIMER,
        time_remaining: time_remaining_ms,
        duration: duration_ms,
        // The binding uses u32 for this signed wire field; -1 drains the timer.
        scale: (-1i32) as u32,
        is_frozen: false,
        id: 0,
    }))
}

pub fn build_breath_timer_stop() -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_STOP_MIRROR_TIMER(SMSG_STOP_MIRROR_TIMER {
        timer: BREATH_TIMER,
    })
}

pub fn build_drowning_damage_log(character_guid: u64, damage: u32) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_ENVIRONMENTAL_DAMAGE_LOG(Box::new(SMSG_ENVIRONMENTAL_DAMAGE_LOG {
        guid: wow_world_messages::Guid::new(character_guid),
        damage_type: EnvironmentalDamageType::Drowning,
        damage,
        absorb: 0,
        resist: 0,
    }))
}
