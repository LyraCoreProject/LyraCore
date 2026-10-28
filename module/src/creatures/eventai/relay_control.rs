use super::{CreatureInstruction, IncrementPhaseInstruction, SetPhaseInstruction};

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayCreatureControl {
    Phase(u8),
    IncrementPhase(i32),
    Melee(bool),
    CombatMovement(bool),
    ReactState(super::CreatureReactState),
    RemoveAura(u32),
    QuestEvent(u32),
    KillCredit(u32),
    ScriptQuestEvent(crate::quest::ScriptQuestEvent),
}

impl RelayCreatureControl {
    pub(super) fn parse(fields: &[&str]) -> Result<Self, String> {
        if let ["script-quest-event", quest, distance, group] = fields {
            return Ok(Self::ScriptQuestEvent(crate::quest::ScriptQuestEvent {
                quest_entry: quest
                    .parse::<u32>()
                    .ok()
                    .filter(|entry| *entry != 0)
                    .ok_or("invalid quest entry")?,
                distance_yd: distance.parse().map_err(|_| "invalid quest distance")?,
                group: match *group {
                    "0" => false,
                    "1" => true,
                    _ => return Err("invalid quest group switch".into()),
                },
            }));
        }
        let [kind, value] = fields else {
            return Err("creature control needs kind and value".into());
        };
        let number = || {
            value
                .parse::<u32>()
                .map_err(|_| format!("invalid control value: {value}"))
        };
        let enabled = || match *value {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(format!("invalid control switch: {value}")),
        };
        match *kind {
            "phase" if number()? < 32 => Ok(Self::Phase(number()? as u8)),
            "phase-inc" => Ok(Self::IncrementPhase(
                value.parse().map_err(|_| "invalid phase increment")?,
            )),
            "melee" => Ok(Self::Melee(enabled()?)),
            "combat-movement" => Ok(Self::CombatMovement(enabled()?)),
            "react-state" => Ok(Self::ReactState(match *value {
                "passive" => super::CreatureReactState::Passive,
                "defensive" => super::CreatureReactState::Defensive,
                "aggressive" => super::CreatureReactState::Aggressive,
                _ => return Err("invalid script react state".into()),
            })),
            "remove-aura" => Ok(Self::RemoveAura(number()?)),
            "quest-event" => Ok(Self::QuestEvent(number()?)),
            "kill-credit" => Ok(Self::KillCredit(number()?)),
            _ => Err(format!("invalid creature control: {kind}")),
        }
    }

    pub(super) fn instruction(self) -> CreatureInstruction {
        match self {
            Self::ScriptQuestEvent(request) => CreatureInstruction::QuestCredit(
                crate::quest::EventAiQuestCredit::ScriptEvent(request),
            ),
            Self::Phase(phase) => CreatureInstruction::SetPhase(SetPhaseInstruction { phase }),
            Self::IncrementPhase(amount) => {
                CreatureInstruction::CheckedIncrementPhase(IncrementPhaseInstruction { amount })
            }
            Self::Melee(enabled) => CreatureInstruction::SetAutoAttack(enabled),
            Self::CombatMovement(enabled) => CreatureInstruction::Movement(
                super::MovementOperation::SetCombatMovement(super::MovementSwitch { enabled }),
            ),
            Self::ReactState(state) => {
                CreatureInstruction::SetReactState(super::SetReactStateInstruction { state })
            }
            Self::RemoveAura(spell_id) => {
                CreatureInstruction::RemoveAura(super::RemoveAuraInstruction {
                    spell_id,
                    target: super::InstructionTarget::SelfActor,
                })
            }
            Self::QuestEvent(quest_entry) => CreatureInstruction::QuestCredit(
                crate::quest::EventAiQuestCredit::QuestEvent(crate::quest::QuestEvent {
                    quest_entry,
                    recipient_policy: crate::quest::QuestCreditRecipientPolicy::SelectedCharacter,
                }),
            ),
            Self::KillCredit(creature_entry) => CreatureInstruction::QuestCredit(
                crate::quest::EventAiQuestCredit::KillCredit(crate::quest::KillCredit {
                    creature_entry,
                    recipient_policy: crate::quest::QuestCreditRecipientPolicy::SelectedCharacter,
                }),
            ),
        }
    }
}
