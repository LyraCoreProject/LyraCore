//! Realm-core Account claims and their World Shard fences.

use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::bindings::*;
use super::connection::{call_reducer, Coordinator};
use crate::world::{SessionTx, WorldSessionToken as Token};

pub(crate) struct SessionOwnership {
    token: Token,
    character_guid: u64,
    deadline: AtomicI64,
    closed: AtomicBool,
    lost: Mutex<Option<SessionTx>>,
}

fn wire(token: Token) -> WorldSessionToken {
    WorldSessionToken {
        account_id: token.account_id,
        generation: token.generation,
        request_nonce: token.request_nonce,
    }
}

fn utc_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_micros().min(i64::MAX as u128) as i64)
        .unwrap_or(i64::MAX)
}

fn shadow_account(account: &Account) -> bool {
    account.username == format!("#{}", account.id)
        && account.salt.is_empty()
        && account.verifier.is_empty()
}

fn owner_matches(
    owner: &AccountCharacterOwner,
    realm_account_id: u64,
    realm_account_name: &str,
    character_guid: u64,
) -> bool {
    owner.account_id == realm_account_id
        && owner.account_name == realm_account_name
        && owner.character_guid == character_guid
}

fn account_owns_character(
    local_account: &Account,
    owner: Option<&AccountCharacterOwner>,
    prior_fence: Option<&AccountFence>,
    realm_account_id: u64,
    realm_account_name: &str,
    character_guid: u64,
) -> bool {
    if owner.is_some_and(|owner| {
        !owner_matches(owner, realm_account_id, realm_account_name, character_guid)
    }) {
        return false;
    }
    if local_account.username == realm_account_name {
        return true;
    }
    shadow_account(local_account)
        && match owner {
            Some(_) => true,
            None => prior_fence.is_some_and(|fence| {
                fence.account_id == realm_account_id
                    && fence.account_name == realm_account_name
                    && fence.character_guid == character_guid
            }),
        }
}

impl SessionOwnership {
    fn lose(&self) {
        self.closed.store(true, Ordering::Release);
        if let Some(tx) = self.lost.lock().unwrap_or_else(|p| p.into_inner()).take() {
            tx.close();
        }
    }
}

impl Coordinator {
    pub(crate) fn session_actor(&self, guid: u64) -> SessionActor {
        SessionActor {
            guid: if guid == 0 {
                self.2.as_ref().map_or(0, |owner| owner.character_guid)
            } else {
                guid
            },
            ownership: self.2.as_ref().map(|owner| wire(owner.token)),
        }
    }

    pub(crate) fn watch_session(&self, tx: SessionTx) {
        if let Some(owner) = &self.2 {
            *owner.lost.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
            if owner.closed.load(Ordering::Acquire)
                || owner.deadline.load(Ordering::Acquire) <= utc_micros()
            {
                owner.lose();
            }
        }
    }

    fn claim_receipt(
        &self,
        token: Option<Token>,
        account_id: u64,
        nonce: u128,
    ) -> Result<AccountClaim> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let row = self
                .0
                .coord()
                .conn
                .db
                .game_account_claim()
                .account_id()
                .find(&account_id);
            if let Some(row) = row.filter(|row| {
                row.request_nonce == nonce && token.is_none_or(|t| t.generation == row.generation)
            }) {
                if row.closed || row.expires_micros <= utc_micros() {
                    return Err(anyhow!("STALE_WORLD_SESSION"));
                }
                return Ok(row);
            }
            if Instant::now() >= deadline {
                return Err(anyhow!("Account claim receipt was not visible within 3s"));
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    pub fn claim_session(&self, account_id: u64, character_guid: u64) -> Result<Token> {
        let shards = self.configured_world_shards()?;
        let name = self
            .0
            .coord()
            .conn
            .db
            .game_account()
            .id()
            .find(&account_id)
            .ok_or_else(|| anyhow!("no Account {account_id} on {}", self.shard_name()))?
            .username;
        let realm = self.realm_core()?;
        let realm_account = realm
            .account_by_username(&name)?
            .ok_or_else(|| anyhow!("no Account {name} on Realm-core"))?;
        let realm_account_id = realm_account.id;
        if shards.iter().any(|shard| {
            shard
                .0
                .coord()
                .conn
                .db
                .game_account_character_owner()
                .character_guid()
                .find(&character_guid)
                .is_some_and(|owner| {
                    !owner_matches(&owner, realm_account_id, &name, character_guid)
                })
        }) {
            return Err(anyhow!("Character ownership changed"));
        }
        let owns_character = shards.iter().any(|shard| {
            shard
                .character_row(character_guid)
                .is_some_and(|character| {
                    let guard = shard.0.coord();
                    let db = &guard.conn.db;
                    db.game_account()
                        .id()
                        .find(&character.account_id)
                        .is_some_and(|account| {
                            let owner = db
                                .game_account_character_owner()
                                .character_guid()
                                .find(&character_guid);
                            let prior_fence =
                                db.game_account_fence().account_id().find(&realm_account_id);
                            account_owns_character(
                                &account,
                                owner.as_ref(),
                                prior_fence.as_ref(),
                                realm_account_id,
                                &name,
                                character_guid,
                            )
                        })
                })
        });
        if !owns_character {
            return Err(anyhow!("Character does not belong to Account"));
        }
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|error| anyhow!("Account request nonce: {error}"))?;
        let nonce = u128::from_le_bytes(bytes).max(1);
        call_reducer!(
            realm.0.call_pipe().conn.reducers,
            "claim_account",
            claim_account_then(realm_account_id, character_guid, nonce)
        )?;
        let receipt = realm.claim_receipt(None, realm_account_id, nonce)?;
        let token = Token {
            account_id: receipt.account_id,
            generation: receipt.generation,
            request_nonce: nonce,
        };
        for shard in shards {
            call_reducer!(
                shard.0.call_pipe().conn.reducers,
                "fence_account",
                fence_account_then(
                    wire(token),
                    name.clone(),
                    character_guid,
                    receipt.expires_micros
                )
            )?;
        }
        // No renewal is started for an incomplete admission. Its claim expires so another
        // generation can finish fencing Shards that the interrupted attempt did not reach.
        Ok(token)
    }

    pub(crate) fn bind_session(&self, token: Token) -> Result<Coordinator> {
        let receipt =
            self.realm_core()?
                .claim_receipt(Some(token), token.account_id, token.request_nonce)?;
        let owner = Arc::new(SessionOwnership {
            token,
            character_guid: receipt.character_guid,
            deadline: AtomicI64::new(receipt.expires_micros),
            closed: AtomicBool::new(false),
            lost: Mutex::new(None),
        });
        let weak = Arc::downgrade(&owner);
        let mut unbound = self.clone();
        unbound.2 = None;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| anyhow!("Account renewal runtime: {error}"))?;
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                let Some(owner) = weak.upgrade() else {
                    break;
                };
                if owner.closed.load(Ordering::Acquire) {
                    break;
                }
                let coord = unbound.clone();
                let token = owner.token;
                // World Sessions can occupy every Tokio blocking slot. Renewal must keep
                // progressing independently of those long-lived readers.
                let (reply, result) = tokio::sync::oneshot::channel();
                let started = std::thread::Builder::new()
                    .name("account-renewal".into())
                    .spawn(move || {
                        let _ = reply.send(coord.renew_session(token));
                    });
                let renewed = match started {
                    Ok(_) => result
                        .await
                        .map_err(|error| anyhow!("Account renewal thread: {error}"))
                        .and_then(|result| result),
                    Err(error) => Err(anyhow!("start Account renewal thread: {error}")),
                };
                match renewed {
                    Ok(expires) => {
                        owner.deadline.store(expires, Ordering::Release);
                    }
                    Err(error) => {
                        log::warn!(
                            "Account {} generation {} lost ownership renewal: {error:#}",
                            token.account_id,
                            token.generation
                        );
                        owner.lose();
                        break;
                    }
                }
            }
        });
        Ok(Coordinator(self.0.clone(), self.1.clone(), Some(owner)))
    }

    fn renew_session(&self, token: Token) -> Result<i64> {
        let realm = self.realm_core()?;
        let prior = realm
            .claim_receipt(Some(token), token.account_id, token.request_nonce)?
            .expires_micros;
        call_reducer!(
            realm.0.call_pipe().conn.reducers,
            "renew_account_claim",
            renew_account_claim_then(wire(token))
        )?;
        let visible_until = Instant::now() + Duration::from_secs(3);
        let receipt = loop {
            let receipt =
                realm.claim_receipt(Some(token), token.account_id, token.request_nonce)?;
            if receipt.expires_micros > prior {
                break receipt;
            }
            if Instant::now() >= visible_until {
                return Err(anyhow!("Account renewal receipt was not visible within 3s"));
            }
            std::thread::sleep(Duration::from_millis(15));
        };
        for shard in self.configured_world_shards()? {
            call_reducer!(
                shard.0.call_pipe().conn.reducers,
                "renew_account_fence",
                renew_account_fence_then(wire(token), receipt.expires_micros)
            )?;
        }
        Ok(receipt.expires_micros)
    }

    pub fn release_session(&self, token: Token) -> Result<()> {
        if let Some(owner) = &self.2 {
            owner.closed.store(true, Ordering::Release);
            owner.lost.lock().unwrap_or_else(|p| p.into_inner()).take();
        }
        for shard in self.configured_world_shards()? {
            call_reducer!(
                shard.0.call_pipe().conn.reducers,
                "close_account_fence",
                close_account_fence_then(wire(token))
            )?;
        }
        let realm = self.realm_core()?;
        call_reducer!(
            realm.0.call_pipe().conn.reducers,
            "release_account_claim",
            release_account_claim_then(wire(token))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accept::BlockingTaskCapacity;
    use crate::config::GatewayConfig;
    use crate::durable_test_support::Standalone;

    fn account(id: u64, username: &str, salt: &[u8], verifier: &[u8]) -> Account {
        Account {
            id,
            username: username.into(),
            salt: salt.into(),
            verifier: verifier.into(),
            identity: None,
            banned: false,
            alpha_test_tools: false,
        }
    }

    #[test]
    fn transferred_character_ownership_requires_exact_realm_provenance() {
        let real = account(17, "TEST", &[1], &[2]);
        assert!(account_owns_character(&real, None, None, 4, "TEST", 99));

        let shadow = account(17, "#17", &[], &[]);
        assert!(!account_owns_character(&shadow, None, None, 4, "TEST", 99));
        let owner = AccountCharacterOwner {
            character_guid: 99,
            account_id: 4,
            account_name: "TEST".into(),
        };
        assert!(account_owns_character(
            &shadow,
            Some(&owner),
            None,
            4,
            "TEST",
            99
        ));
        for (account_id, account_name, character_guid) in
            [(5, "TEST", 99), (4, "OTHER", 99), (4, "TEST", 100)]
        {
            assert!(!account_owns_character(
                &shadow,
                Some(&owner),
                None,
                account_id,
                account_name,
                character_guid,
            ));
        }
        assert!(!account_owns_character(
            &account(17, "OTHER", &[1], &[2]),
            Some(&owner),
            None,
            4,
            "TEST",
            99,
        ));
        assert!(!account_owns_character(
            &account(17, "#17", &[1], &[]),
            Some(&owner),
            None,
            4,
            "TEST",
            99,
        ));

        let prior_fence = AccountFence {
            account_id: 4,
            account_name: "TEST".into(),
            generation: 7,
            request_nonce: 8,
            character_guid: 99,
            expires_micros: 0,
            closed: true,
        };
        assert!(account_owns_character(
            &shadow,
            None,
            Some(&prior_fence),
            4,
            "TEST",
            99,
        ));
        let conflicting_owner = AccountCharacterOwner {
            account_id: 5,
            ..owner.clone()
        };
        assert!(!account_owns_character(
            &shadow,
            Some(&conflicting_owner),
            Some(&prior_fence),
            4,
            "TEST",
            99,
        ));
        assert!(!account_owns_character(
            &real,
            Some(&conflicting_owner),
            None,
            4,
            "TEST",
            99,
        ));
    }

    fn fenced_destination(
        runtime: &tokio::runtime::Runtime,
        cfg: GatewayConfig,
        token: Token,
        expires_micros: i64,
    ) -> Result<(Standalone, Coordinator)> {
        let mut fixture = Standalone::start("account-transfer-destination");
        fixture.publish_module();
        fixture.assert_call("claim_operator", &[]);
        fixture.assert_call("install_guid_range", &["1000000000"]);
        let destination = runtime.block_on(Coordinator::connect(&GatewayConfig {
            stdb_uri: fixture.server().into(),
            module_name: fixture.shard_name().into(),
            coordinator_token: Some(fixture.owner_token()),
            gateway_id: "account-transfer-destination".into(),
            ..cfg
        }))?;
        call_reducer!(
            destination.0.call_pipe().conn.reducers,
            "fence_account",
            fence_account_then(wire(token), "TEST".into(), 1, expires_micros)
        )?;
        Ok((fixture, destination))
    }

    fn assert_transfer_preserves_ownership(
        runtime: &tokio::runtime::Runtime,
        cfg: GatewayConfig,
        fixture: &Standalone,
        winner: &Coordinator,
        old: &Coordinator,
        second: Token,
        expires_micros: i64,
    ) {
        let (destination_fixture, destination_raw) =
            fenced_destination(runtime, cfg, second, expires_micros).unwrap();
        let destination = Coordinator(
            destination_raw.0.clone(),
            destination_raw.1.clone(),
            winner.2.clone(),
        );
        let stale_destination = Coordinator(
            destination_raw.0.clone(),
            destination_raw.1.clone(),
            old.2.clone(),
        );
        let transfer_id = crate::world::transfer::transfer_id_for(1);
        winner
            .begin_transfer(&crate::world::transfer::TransferPlan {
                transfer_id,
                character_guid: 1,
                dest_map_id: 0,
                dest_instance_id: 0,
                dest_x: 100.0,
                dest_y: 100.0,
                dest_z: 20.0,
                dest_o: 0.0,
            })
            .unwrap();
        assert!(crate::durable_test_support::poll_until(
            Duration::from_secs(5),
            || winner.escrow_row(1).is_some()
        ));
        let escrow = winner.escrow_row(1).unwrap();
        assert_eq!(escrow.transfer_id, transfer_id);
        assert_eq!(escrow.character_guid, 1);
        let source = crate::world::transfer::RealmLocatorPredecessor {
            map_id: 0,
            instance_id: 0,
            revision: 1,
        };
        let refused = |result: Result<()>| {
            let error = result.unwrap_err();
            assert!(
                error.to_string().contains("STALE_WORLD_SESSION"),
                "{error:#}"
            );
        };
        refused(stale_destination.import_player_character_blob(transfer_id, &escrow.blob, source));
        assert!(destination_fixture
            .query_rows("SELECT * FROM game_transfer_in")
            .is_empty());
        destination
            .import_player_character_blob(transfer_id, &escrow.blob, source)
            .unwrap();
        let arrivals = destination_fixture.query_rows("SELECT * FROM game_transfer_in");
        assert_eq!(arrivals[0]["transfer_id"], transfer_id.to_string());
        assert_eq!(arrivals[0]["character_guid"], "1");
        refused(old.confirm_import(transfer_id));
        assert!(fixture
            .query_rows("SELECT * FROM game_transfer_in")
            .is_empty());
        winner.confirm_import(transfer_id).unwrap();
        refused(old.finish_transfer(transfer_id));
        assert_eq!(
            fixture
                .query_rows("SELECT guid FROM game_character WHERE guid = 1")
                .len(),
            1
        );
        winner.finish_transfer(transfer_id).unwrap();
        refused(stale_destination.release_player_transfer_arrival(transfer_id, 1, source));
        assert_eq!(
            destination_fixture.query_rows("SELECT * FROM game_transfer_in"),
            arrivals
        );
        destination
            .release_player_transfer_arrival(transfer_id, 1, source)
            .unwrap();
        assert!(fixture
            .query_rows("SELECT guid FROM game_character WHERE guid = 1")
            .is_empty());
        assert!(fixture
            .query_rows("SELECT * FROM game_transfer_out")
            .is_empty());
        assert!(destination_fixture
            .query_rows("SELECT * FROM game_transfer_in")
            .is_empty());
        assert_eq!(
            destination_fixture
                .query_rows("SELECT guid FROM game_character WHERE guid = 1")
                .len(),
            1
        );
    }

    #[test]
    #[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
    fn independent_coordinators_preserve_the_winner_after_delayed_cleanup() {
        for name in [
            "LYRACORE_SHARD_MAP",
            "LYRACORE_SHARD_MAP_FILE",
            "LYRACORE_REALM_CORE",
        ] {
            assert!(
                std::env::var_os(name).is_none(),
                "unset {name} for this private fixture"
            );
        }
        let mut fixture = Standalone::start("account-ownership");
        fixture.publish_module();
        fixture.assert_call("claim_operator", &[]);
        fixture.assert_call("install_guid_range", &["0"]);
        fixture.assert_call("gw_heartbeat", &[]);
        let cfg = GatewayConfig {
            logon_bind: "127.0.0.1:0".into(),
            world_bind: "127.0.0.1:0".into(),
            stdb_uri: fixture.server().into(),
            module_name: fixture.shard_name().into(),
            coordinator_token: Some(fixture.owner_token()),
            gateway_id: "account-ownership-a".into(),
            blocking_task_capacity: BlockingTaskCapacity::new(2),
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let a = runtime.block_on(Coordinator::connect(&cfg)).unwrap();
        let b = runtime
            .block_on(Coordinator::connect(&GatewayConfig {
                gateway_id: "account-ownership-b".into(),
                ..cfg.clone()
            }))
            .unwrap();
        let account_id = a.account_by_username("TEST").unwrap().unwrap().id;
        let identity = a.bound_identity(account_id).unwrap();
        assert_eq!(b.bound_identity(account_id).unwrap(), identity);
        a.establish_session(account_id, &[7; 40], identity).unwrap();
        let first = a.claim_session(account_id, 1).unwrap();
        let old = a.bind_session(first).unwrap();
        old.player_login(account_id, 1).unwrap();
        assert!(matches!(
            b.delete_character(account_id, 1).unwrap(),
            crate::codec::CharDeleteOutcome::Failed
        ));
        assert_eq!(
            fixture
                .query_rows("SELECT guid FROM game_character WHERE guid = 1")
                .len(),
            1
        );
        let queued_before_takeover = GwMove {
            actor: old.session_actor(1),
            opcode: lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT as u16,
            movement_info: vec![],
            x: 500.0,
            y: 100.0,
            z: 20.0,
            o: 0.0,
            move_time_ms: 200,
        };
        assert!(b
            .claim_session(account_id, 1)
            .unwrap_err()
            .to_string()
            .contains("ACCOUNT_IN_USE"));
        assert_eq!(
            fixture
                .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
                .len(),
            1
        );

        // Hold A's cleanup while its Realm claim expires. The World Shard still has A's old
        // generation, which B must advance before entering.
        fixture.assert_sql(&format!(
            "UPDATE game_account_claim SET expires_micros = 0 WHERE account_id = {}",
            first.account_id
        ));
        let second = b.claim_session(account_id, 1).unwrap();
        assert!(second.generation > first.generation);
        let winner = b.bind_session(second).unwrap();
        winner.player_login(account_id, 1).unwrap();
        old.release_session(first).unwrap();
        old.release_session(first).unwrap();
        let batch = super::super::movement_batch::MovementBatch::new();
        batch.push(GwMove {
            actor: winner.session_actor(1),
            x: 100.0,
            move_time_ms: 100,
            ..queued_before_takeover.clone()
        });
        batch.push(queued_before_takeover);
        let failures = batch.drain(|moves| {
            call_reducer!(
                winner.0.call_pipe().conn.reducers,
                "gw_movement_batch",
                gw_movement_batch_then(moves)
            )
        });
        assert!(failures.is_empty());
        let entity =
            fixture.query_rows("SELECT x,last_move_ms FROM game_world_entity WHERE guid = 1");
        assert_eq!(entity[0]["x"].parse::<f32>().unwrap(), 100.0);
        assert_eq!(entity[0]["last_move_ms"], "100");
        let stale = old.stop_attack(account_id, 1).unwrap_err();
        assert!(
            stale.to_string().contains("STALE_WORLD_SESSION"),
            "{stale:#}"
        );
        let entities = fixture.query_rows("SELECT * FROM game_world_entity WHERE guid = 1");
        assert_eq!(
            entities.len(),
            1,
            "delayed cleanup deleted the winner's Character"
        );
        let claims = fixture.query_rows("SELECT * FROM game_account_claim");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0]["generation"], second.generation.to_string());
        assert_eq!(claims[0]["closed"], "false");
        assert_transfer_preserves_ownership(
            &runtime,
            cfg,
            &fixture,
            &winner,
            &old,
            second,
            claims[0]["expires_micros"].parse().unwrap(),
        );
        winner.release_session(second).unwrap();
        assert!(fixture
            .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
            .is_empty());
    }
}
