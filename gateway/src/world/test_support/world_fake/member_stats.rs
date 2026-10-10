use super::super::*;

#[derive(Default)]
pub(crate) struct MemberStatsState {
    /// How many `member_presence` reads reached this handle.
    pub(crate) member_presence_reads: std::sync::atomic::AtomicUsize,
}

/// Member Stats over the same party state the routing tests use: Realm-core's `party` when this
/// handle has a realm, its own `mirror` on a single database. Presence reads this shard and its
/// peers, every World Shard, like `Coordinator::member_presence`.
impl MemberStatsStore for WorldFake {
    fn group_mates(&self, self_guid: u64) -> Result<Vec<u64>> {
        let roster = match &self.topology.realm {
            Some(realm) => {
                let party = realm.party.party.lock().unwrap();
                party
                    .group_of(self_guid)
                    .and_then(|group| party.roster(group))
            }
            None => self
                .party
                .mirror
                .lock()
                .unwrap()
                .iter()
                .find(|roster| roster.has_member(self_guid))
                .cloned(),
        };
        Ok(roster
            .map(|roster| roster.member_guids())
            .unwrap_or_default()
            .into_iter()
            .filter(|member| *member != self_guid)
            .collect())
    }

    fn member_presence(&self, guid: u64) -> Result<MemberPresence> {
        self.member_stats
            .member_presence_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Aura slots and a live pet are Member Stats' own overlay in production
        // (`Coordinator::with_member_shard_stats`, keyed by `ShardId`) — this Fake has no
        // `AuraIndex` to key into, so `entity` carries whatever `member_entities`/`live_entity`'s
        // fallback already gave it (empty auras, no pet, unless a test seeded `member_entities`
        // with its own).
        Ok(
            match presence::of(self, guid)?.map(|presence| presence.whereabouts) {
                Some(presence::Whereabouts::InWorld { entity, .. }) => {
                    MemberPresence::Live(Box::new(codec::MemberStats::from_entity(&entity)))
                }
                Some(presence::Whereabouts::InTransit) => MemberPresence::InTransit,
                Some(presence::Whereabouts::Offline) | None => MemberPresence::Offline,
            },
        )
    }
}
