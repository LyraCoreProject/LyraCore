//! GameObject use resolves its interaction before entering a quest or loot operation.
use super::super::*;

pub(crate) struct GameObject;

impl<
        St: DeathStore
            + LootRollStore
            + LootWindowStore
            + NpcStore
            + ShardRoutingStore
            + QuestActionStore
            + ?Sized,
    > ProtocolFamily<St> for GameObject
{
    fn handle(
        store: &St,
        session: &mut ProtocolSession,
        request: ProtocolRequest,
    ) -> Result<ProtocolReply> {
        let message = request.message()?;
        let ClientOpcodeMessage::CMSG_GAMEOBJ_USE(request) = &message else {
            return Err(anyhow!("non-GameObject request routed to GameObject"));
        };
        let guid = request.guid.guid();
        match store.gameobject_type(guid)? {
            Some(lyracore_shared::constants::go_type::QUESTGIVER) => {
                Ok(super::quest_giver_menu(store, guid, session.self_guid().unwrap_or(0))?.into())
            }
            Some(lyracore_shared::constants::go_type::CHEST) => {
                super::LootWindow::handle(store, session, message.into())
            }
            _ => super::Loot::handle(store, session, message.into()),
        }
    }
}
