//! Gateway configuration. All operational, none of it game state.

#[derive(Clone, Debug)]
pub struct GatewayConfig {
    /// Bind address for the logon listener (SRP6 + realm list). Default 0.0.0.0:3724.
    pub logon_bind: String,
    /// Bind address for the world listener. Default 0.0.0.0:8085.
    pub world_bind: String,
    /// SpacetimeDB host URI, e.g. http://127.0.0.1:3000.
    pub stdb_uri: String,
    /// Module/database name to connect to.
    pub module_name: String,
    /// Auth token for the privileged coordination connection (reads account/session).
    pub coordinator_token: Option<String>,
}

impl GatewayConfig {
    pub fn from_env() -> Self {
        let get = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        Self {
            logon_bind: get("LYRACORE_LOGON_BIND", "0.0.0.0:3724"),
            world_bind: get("LYRACORE_WORLD_BIND", "0.0.0.0:8085"),
            stdb_uri: get("LYRACORE_SPACETIMEDB_URL", "http://127.0.0.1:3000"),
            module_name: get("LYRACORE_DATABASE", "lyracore"),
            coordinator_token: std::env::var("LYRACORE_COORDINATOR_TOKEN").ok(),
        }
    }
}

// ===============================================================================================
//  Shard map (#17 — the routing half of Phase A of the elastic-sharding spec, #12)
// ===============================================================================================

/// Instances of one map are spread over a shard pool by `instance_id % INSTANCE_BUCKETS`, so a rule
/// can name a slice of a map's instances instead of all of them.
// Deliberate simplification: a hardcoded modulus IS the whole bucketing scheme. Ceiling — you
// cannot resize the pool without every gateway agreeing on the same number, and rebalancing means
// restarting them. Upgrade path: realm-core owns the assignment table (#12 Phase B/C) and
// gateways subscribe to it with an epoch, at which point this constant and the env parsing below
// both go away.
pub const INSTANCE_BUCKETS: u64 = 8;

/// One `<map_id>:<bucket>=<database>` rule. `None` = the `*` wildcard.
#[derive(Clone, Debug, PartialEq)]
struct ShardRule {
    map_id: Option<u32>,
    bucket: Option<u64>,
    db: String,
}

/// Multi-shard routing table: `(map_id, instance-bucket) → database name`.
///
/// The gateway holds one coordinator connection per database named here, and pins each player's
/// per-player connection + AOI subscriptions to the shard that owns their character's location.
///
/// **Unconfigured = one entry** (the `LYRACORE_DATABASE` database): every lookup resolves to it, nothing
/// ever routes, and the gateway behaves byte-identically to the single-database build. That is the
/// safety property — a missing, empty, or malformed shard map can only collapse back to today.
///
/// Config format (env `LYRACORE_SHARD_MAP`, or a file path in `LYRACORE_SHARD_MAP_FILE`; the env var wins).
/// Rules are separated by commas, semicolons, or newlines, and `#` starts a comment:
///
/// ```text
/// LYRACORE_SHARD_MAP="1:*=lyracore-instances, 389:*=lyracore-instances"
/// ```
///
/// A rule is `<map_id>:<bucket>=<database>`, where `map_id` is a number or `*` and `bucket` is a
/// number in `0..INSTANCE_BUCKETS` or `*`. `<map_id>=<db>` is shorthand for `<map_id>:*=<db>`.
/// ⚠ The OPEN WORLD is `instance_id == 0`, i.e. **bucket 0** — so `*:0=pool-a`, which reads like
/// "the pool's first slot", moves every open-world location there as well. Name the map when you
/// mean instances (`389:0=pool-a`).
/// **The first matching rule wins**; anything unmatched falls through to the default database.
/// A rule that fails to parse is logged and DROPPED — a typo can never silently reroute players.
#[derive(Clone, Debug)]
pub struct ShardMap {
    default_db: String,
    rules: Vec<ShardRule>,
    /// `LYRACORE_REALM_CORE` (#20): the database that owns accounts, sessions, and the character→shard
    /// index. `None` = unconfigured, which is *exactly* today — auth lives on the default database.
    realm_core: Option<String>,
    /// `LYRACORE_REGION_SHARDS` (#72): world shards a REGION assignment may name, connected but never
    /// routed to by the shard map itself.
    ///
    /// Without this there is no way to express "connect to this database, but let the region overlay
    /// decide who goes there". A world shard is only connected if it appears in [`shards`], and
    /// before this that meant it had to be some rule's `db` — but every rule routes a whole map (or
    /// a map+bucket) to it, which is precisely the decision regions exist to take instead. Naming a
    /// region shard as `0:*=<db>` would hand it every map-0 location the seam menu does NOT cover,
    /// i.e. all of Eastern Kingdoms outside the drawn regions, on a database holding none of it.
    ///
    /// These carry NO rules, so [`resolve`](Self::resolve) can never return one and
    /// [`check_instance_hosting`](Self::check_instance_hosting) never considers one an instance
    /// owner. The only thing that changes is that `connected(db)` in
    /// [`resolve_region_shard`] can now succeed for them.
    region_shards: Vec<String>,
}

impl ShardMap {
    /// Read the shard map from `LYRACORE_SHARD_MAP` (or the file named by `LYRACORE_SHARD_MAP_FILE`), defaulting
    /// every unmatched location to `default_db` (the `LYRACORE_DATABASE` database). Unset → single entry.
    /// `LYRACORE_REALM_CORE` names the realm-core database (#20); unset → auth stays on `default_db`.
    pub fn from_env(default_db: &str) -> Self {
        let text = match std::env::var("LYRACORE_SHARD_MAP") {
            Ok(t) => t,
            Err(_) => match std::env::var("LYRACORE_SHARD_MAP_FILE") {
                Ok(path) => std::fs::read_to_string(&path).unwrap_or_else(|e| {
                    log::error!(
                        "LYRACORE_SHARD_MAP_FILE {path}: {e} — falling back to a single shard"
                    );
                    String::new()
                }),
                Err(_) => String::new(),
            },
        };
        Self::parse(default_db, &text)
            .with_realm_core(std::env::var("LYRACORE_REALM_CORE").ok().as_deref())
            .with_region_shards(std::env::var("LYRACORE_REGION_SHARDS").ok().as_deref())
    }

    /// Declare the world shards a REGION assignment may name (`LYRACORE_REGION_SHARDS`, comma/semicolon/
    /// newline separated, `#` comments) — see the field docs for why a shard-map rule cannot do this.
    ///
    /// Filtered so the degenerate configs collapse to "unconfigured" rather than opening a second
    /// connection to somewhere already in the set, or a first one to somewhere that must not hold
    /// characters:
    /// - the **default** database and any database a rule already names are dropped (already
    ///   connected, and `shards()` must not list one twice — it is what a character-location probe
    ///   walks)
    /// - **realm-core** is dropped: it owns no characters, so routing a player there would land them
    ///   on a database with nothing of theirs in it. `resolve_region_shard` already refuses an
    ///   assignment naming an unconnected shard; this keeps realm-core unconnectable *as a shard*
    ///   even though the gateway does connect to it for auth.
    ///
    /// ⚠ Order matters: this runs AFTER `with_realm_core` in `from_env` so realm-core is known.
    pub fn with_region_shards(mut self, names: Option<&str>) -> Self {
        let mut out: Vec<String> = Vec::new();
        for raw in names.unwrap_or("").split(['\n', ',', ';']) {
            let name = raw.split('#').next().unwrap_or("").trim();
            if name.is_empty() || name == self.default_db {
                continue;
            }
            if self.realm_core.as_deref() == Some(name) {
                log::error!(
                    "LYRACORE_REGION_SHARDS names {name}, which is the realm-core database — it owns no \
                     characters, so a region assigned there would route players onto a database \
                     holding nothing of theirs. Ignoring it."
                );
                continue;
            }
            if self.rules.iter().any(|r| r.db == name) || out.iter().any(|d| d == name) {
                continue; // already connected via a rule, or listed twice
            }
            out.push(name.to_string());
        }
        self.region_shards = out;
        self
    }

    /// The databases declared connectable for region routing only (`LYRACORE_REGION_SHARDS`).
    /// Test-only accessor: production reads the overlay through [`ShardMap::databases`] /
    /// [`resolve_region_shard`], which fold it in already.
    #[cfg(test)]
    pub fn region_shards(&self) -> &[String] {
        &self.region_shards
    }

    /// Point auth (accounts / sessions / the character→shard index) at a separate `realm-core`
    /// database (#20). Blank, absent, or *equal to the default database* all mean "unconfigured" —
    /// the degenerate `LYRACORE_REALM_CORE=<the world db>` config must collapse to today's single-database
    /// behavior rather than opening a second connection to the same place.
    pub fn with_realm_core(mut self, name: Option<&str>) -> Self {
        self.realm_core = name
            .map(str::trim)
            .filter(|n| !n.is_empty() && *n != self.default_db)
            .map(str::to_string);
        self
    }

    /// The configured realm-core database, or `None` when auth stays on the default database.
    pub fn realm_core_db(&self) -> Option<&str> {
        self.realm_core.as_deref()
    }

    /// The database every auth read/write must target, given which databases actually connected.
    ///
    /// `Ok(db)` = use it. `Err(name)` = realm-core is CONFIGURED but not connected — the caller must
    /// **fail closed** (refuse to authenticate) rather than silently fall back to the world DB's
    /// stale auth cache. That fallback would be a real security regression: a ban or a password
    /// rotation applied on realm-core would stop being enforced the moment realm-core blinked, which
    /// is precisely when an attacker would want it to. Backward compatibility is served by the
    /// *unconfigured* case, which never reaches the `Err` arm at all.
    pub fn auth_db(&self, connected: impl Fn(&str) -> bool) -> std::result::Result<&str, &str> {
        match self.realm_core.as_deref() {
            None => Ok(&self.default_db),
            Some(db) if connected(db) => Ok(db),
            Some(db) => Err(db),
        }
    }

    /// Parse the rule text (see the type docs). Malformed rules are logged and skipped.
    pub fn parse(default_db: &str, text: &str) -> Self {
        let mut rules = Vec::new();
        for raw in text.split(['\n', ',', ';']) {
            let rule = raw.split('#').next().unwrap_or("").trim();
            if rule.is_empty() {
                continue;
            }
            match parse_shard_rule(rule) {
                Some(r) => rules.push(r),
                None => log::error!(
                    "shard map: ignoring malformed rule {rule:?} (want `<map_id|*>:<bucket|*>=<db>`)"
                ),
            }
        }
        Self {
            default_db: default_db.to_string(),
            rules,
            realm_core: None,
            region_shards: Vec::new(),
        }
    }

    /// The database that owns `(map_id, instance_id)`. Never fails: unmatched → the default shard.
    pub fn resolve(&self, map_id: u32, instance_id: u64) -> &str {
        let bucket = instance_id % INSTANCE_BUCKETS;
        self.rules
            .iter()
            .find(|r| r.map_id.is_none_or(|m| m == map_id) && r.bucket.is_none_or(|b| b == bucket))
            .map_or(self.default_db.as_str(), |r| r.db.as_str())
    }

    /// [`resolve`](Self::resolve), degraded to the DEFAULT database when the owning shard is not in
    /// `connected` (named by the map but unreachable at startup). The degradation target is the
    /// default and never "wherever the caller already is": that is what makes a bad or partly-down
    /// shard map collapse to today's single-database behavior instead of pinning a session to a
    /// database that owns nothing of theirs. Pure, so the rule is unit-testable without a node.
    pub fn resolve_connected(
        &self,
        map_id: u32,
        instance_id: u64,
        connected: impl Fn(&str) -> bool,
    ) -> &str {
        let db = self.resolve(map_id, instance_id);
        if connected(db) {
            return db;
        }
        log::warn!(
            "shard {db} owns map {map_id}/instance {instance_id} but is not connected — falling \
             back to the default database {}",
            self.default_db
        );
        &self.default_db
    }

    /// The default (realm/home) database — where accounts, sessions, and the character list live.
    pub fn default_db(&self) -> &str {
        &self.default_db
    }

    // ==========================================================================================
    //  The instance POOL (#21 — the "pool" half of Phase A)
    // ==========================================================================================

    /// Every database an instance of `map_id` can be routed to — the map's **instance pool**,
    /// deduped, in bucket order.
    ///
    /// Note: this is [`resolve`](Self::resolve) read the other way round, and #21 adds **no new
    /// assignment mechanism at all**. #17's `<map_id>:<bucket>=<db>` rule already IS the pool hook:
    ///
    /// ```text
    /// LYRACORE_SHARD_MAP="36:0=lyracore-instances-1, 36:1=lyracore-instances-1, 36:*=lyracore-instances-0"
    /// ```
    ///
    /// spreads Deadmines runs over two databases with zero code, because `bucket = instance_id %
    /// INSTANCE_BUCKETS` and instance ids come from one `auto_inc` allocator on the world shard, so
    /// consecutive runs land in consecutive buckets. This method exists only so the routing code can
    /// ask *"is this database one of the places this map's instances live"* — the question that
    /// protects an in-flight run from a pool resize (see [`instance_owner`](Self::instance_owner)).
    ///
    /// Unconfigured it is `[default_db]` — "the pool is the one database" — which is why every rule
    /// built on it is a no-op on a single-database gateway.
    pub fn instance_pool(&self, map_id: u32) -> Vec<&str> {
        let mut pool: Vec<&str> = Vec::new();
        for bucket in 0..INSTANCE_BUCKETS {
            // `bucket < INSTANCE_BUCKETS`, so `bucket % INSTANCE_BUCKETS == bucket`: passing it as
            // the instance id enumerates exactly the buckets, and only the buckets.
            let db = self.resolve(map_id, bucket);
            if !pool.contains(&db) {
                pool.push(db);
            }
        }
        pool
    }

    /// Which database owns instance `instance_id` of `map_id`, for a character the `holder`
    /// database currently holds. [`resolve_connected`](Self::resolve_connected) plus **one**
    /// stickiness rule — and that rule is what makes "a second instances database can be added by
    /// config alone, and existing runs are undisturbed" (#21 AC#3) true rather than aspirational.
    ///
    /// # Why the shard map alone is not sufficient
    ///
    /// Growing the pool means re-pointing buckets, and a bucket is a property of the instance ID,
    /// not of the run: an instance minted while bucket 5 belonged to `instances-0` is still bucket 5
    /// when the operator hands bucket 5 to `instances-1`. The shard map alone would then answer
    /// `instances-1` for a run that is LIVE on `instances-0`, and the gateway would dutifully mirror
    /// a second, empty copy of the dungeon there and transfer the player into it — forking the party
    /// away from the population it was fighting.
    ///
    /// # What identifies an in-flight instance
    ///
    /// **The shard that already holds a character inside it.** There is no realm-wide instance→shard
    /// index yet (that is #22), so this is the only durable evidence either database has — and it is
    /// evidence the caller has already read, because finding the holder is how world entry starts.
    /// So: if the holder is itself a member of this map's pool, and it reports the character inside a
    /// non-open-world instance, the holder wins.
    ///
    /// It deliberately cannot capture the INITIAL hop, and that is the whole trick: a player stepping
    /// through the portal is held by the open-world shard, which is not in a dungeon map's pool, so
    /// the shard map decides and the new run lands on the pool as designed. New runs spread; live
    /// runs stay put.
    ///
    /// # Residual, stated out loud
    ///
    /// A party member who has **not yet entered** a re-bucketed live instance is still routed by the
    /// map, and will fork it (they are held by the open-world shard, which is not in the pool, so
    /// there is nothing to be sticky about). Closing that needs the instance→shard index (#22) —
    /// until then the operator procedure is "grow the pool while it is idle".
    ///
    /// # Unconfigured
    ///
    /// `instance_pool == [default_db]` and the holder is the default database, so this returns
    /// exactly what `resolve_connected` returns, for every input. (And a single-database gateway
    /// never calls it at all — `Coordinator::is_sharded` short-circuits first.)
    pub fn instance_owner<'a>(
        &'a self,
        map_id: u32,
        instance_id: u64,
        holder: &'a str,
        connected: impl Fn(&str) -> bool,
    ) -> &'a str {
        // `instance_id != 0` first: the OPEN WORLD is instance 0 and is not an instance at all.
        // Without it, every character on any second shard would pin itself there forever — the
        // `*:0=` footgun the type docs warn about, re-created in code instead of in config.
        if instance_id != 0 && connected(holder) && self.instance_pool(map_id).contains(&holder) {
            return holder;
        }
        self.resolve_connected(map_id, instance_id, connected)
    }

    /// Every distinct WORLD SHARD this map can resolve to, default first. These are the databases
    /// that hold characters, so these — and only these — are what a character-location probe walks.
    /// Realm-core is deliberately not here: it holds no characters, so probing it is pure waste.
    /// ⚠ **Default stays FIRST.** `Coordinator::region_db_for` reads the seam menu from
    /// `world_shards().first()`, so anything appended here must go AFTER the default.
    pub fn shards(&self) -> Vec<String> {
        let mut dbs = vec![self.default_db.clone()];
        for r in &self.rules {
            if !dbs.contains(&r.db) {
                dbs.push(r.db.clone());
            }
        }
        // Region shards (#72) hold characters and are probed like any other world shard, but no rule
        // routes to them — the region overlay is their only way in. `with_region_shards` has already
        // dropped the default, realm-core, and anything a rule names, so this cannot duplicate.
        for d in &self.region_shards {
            if !dbs.contains(d) {
                dbs.push(d.clone());
            }
        }
        dbs
    }

    /// **Will this realm's dungeon populations actually get spawned, exactly once, on the database
    /// that runs them?** (issue #48, subsuming #39's startup reminder.)
    ///
    /// Two facts have to agree and until now no single process held both: the ROUTING is this
    /// gateway's shard map, and `hosts_instances` is a column on each DATABASE. The gateway is the
    /// layer that can hold both — it subscribes `game_config` on every connection in the set (see
    /// `stdb/connection.rs`), so `hosts_instances(db)` below is a real read, not a guess. The MODULE
    /// cannot own this check even in principle: `create_instance_with_id` knows its own flag but has
    /// no idea whether any *other* database exists, let alone whether the shard map points dungeon
    /// traffic at one — and spec #12 forbids it from learning (`no_module_game_logic_reads_a_shard_id`).
    ///
    /// # The two failure directions
    ///
    /// A dungeon portal's areatrigger runs on the database the PLAYER is standing on — the default
    /// (open-world) shard — which files the `game_instance` row. The POPULATION is spawned by
    /// whichever database owns that instance, when the gateway mirrors the id there
    /// (`ensure_instance`). So:
    ///
    /// - [`InstanceHosting::NoHost`] — the owning database has `hosts_instances = false`. Every
    ///   dungeon is created as a LEASE with **0 entities** and nothing, anywhere, will ever spawn its
    ///   population. This is #48: measured as 8 consecutive empty instances and 4 bot-test failures
    ///   that read as gameplay regressions. FATAL — a realm whose dungeons are all empty rooms should
    ///   not come up quietly.
    /// - [`InstanceHosting::LoadNotMoved`] — the DEFAULT database hosts populations while a dungeon
    ///   map's instances are owned elsewhere. The run works; it just spawns ~207 creatures + 28
    ///   gameobject copies on the world writer and evicts them again after the transfer, i.e. exactly
    ///   the load Phase A exists to remove. A WARNING, and — unlike #39's reminder, which fired on
    ///   every sharded startup regardless of the flag — it now fires only when the flag really is
    ///   wrong.
    ///
    /// Everything else is [`InstanceHosting::Consistent`] and says nothing at all. In particular the
    /// ordinary single-database realm (no `LYRACORE_SHARD_MAP`, flag at its `true` default) is silent, and
    /// so is a correctly-configured Phase A deployment.
    ///
    /// # Inputs
    ///
    /// - `hosts_instances(db)` — that database's `game_config.hosts_instances`, or `None` when the
    ///   row is missing/unreadable. `None` reads as **true**, mirroring the module's own
    ///   `hosts_instance_populations` default: a database that predates the column behaves as it did
    ///   before the column existed, and an unreadable config can never fabricate a fatal.
    /// - `connected(db)` — the databases that actually connected. Routing to an unreachable shard
    ///   degrades to the default database ([`resolve_connected`](Self::resolve_connected)), and this
    ///   check follows that degradation: if the instances shard is down AND the world shard does not
    ///   host populations, dungeons would be empty. That is [`InstanceHosting::UnreachableHost`] —
    ///   the same *symptom* as `NoHost`, deliberately NOT the same severity, see that variant.
    pub fn check_instance_hosting(
        &self,
        hosts_instances: impl Fn(&str) -> Option<bool>,
        connected: impl Fn(&str) -> bool,
    ) -> InstanceHosting {
        let hosts = |db: &str| hosts_instances(db).unwrap_or(true);
        // Every (dungeon map, instance bucket) pair this realm can mint — i.e. every database a
        // dungeon run can land on. `bucket < INSTANCE_BUCKETS`, so passing the bucket straight in as
        // the instance id enumerates exactly the buckets and only the buckets (`instance_pool`'s
        // trick, same expression). Bucket 0 is probed as instance id 0, which is the OPEN WORLD id
        // and never a real instance — that is fine and deliberately not special-cased: `resolve`
        // reads `instance_id % INSTANCE_BUCKETS` and nothing else, so 0 and 8 are the same probe.
        let owners = lyracore_shared::instance::DUNGEON_MAPS
            .iter()
            .flat_map(|&map_id| (0..INSTANCE_BUCKETS).map(move |bucket| (map_id, bucket)));
        let mut load_not_moved: Option<String> = None;
        for (map_id, instance_id) in owners {
            // `resolve_connected`'s degradation rule, inlined so this stays a SILENT pure function:
            // it logs one warning per call, and calling it 8× per dungeon map at startup would put
            // eight identical lines in front of the one message that matters.
            let resolved = self.resolve(map_id, instance_id);
            let degraded = !connected(resolved);
            let owner = if degraded {
                self.default_db.as_str()
            } else {
                resolved
            };
            // The owner hosts nothing ⇒ dungeons WILL be empty. But how we got here decides the
            // severity, and conflating the two turns a blip into an outage (see `UnreachableHost`):
            // a routed-but-unreachable shard is a TRANSIENT condition on a config the operator got
            // right, and `Coordinator::connect` deliberately keeps serving the open world through it.
            if !hosts(owner) && degraded {
                return InstanceHosting::UnreachableHost(format!(
                    "instances of DUNGEON map {map_id} are routed to database {resolved}, which is \
                     NOT CONNECTED — so dungeon entries degrade back to {owner}, whose \
                     `game_config.hosts_instances` is FALSE. Until {resolved} is reachable every \
                     dungeon entry files a lease row and spawns NOTHING: players and bots walk into \
                     a completely empty instance (issue #48). The open world is unaffected and the \
                     routing itself is correct, so the gateway is COMING UP ANYWAY — bring \
                     {resolved} up and restart the gateway to restore dungeons (the coordinator \
                     watchdog does not re-run this check, and it does not connect shards that were \
                     absent at startup)."
                ));
            }
            if !hosts(owner) {
                return InstanceHosting::NoHost(format!(
                    "instances of DUNGEON map {map_id} are owned by database {owner}, and \
                     {owner}'s `game_config.hosts_instances` is FALSE — every dungeon entry there \
                     files a lease row and spawns NOTHING, so players and bots walk into a \
                     completely empty instance and no other database will ever populate it (issue \
                     #48: 8 consecutive instances with 0 entities, 4 bot tests failing as if \
                     gameplay had regressed). Fix ONE of these and restart: (a) let that database \
                     host its own dungeons — `spacetime sql {owner} \"UPDATE game_config SET \
                     hosts_instances = true WHERE id = 0\"`; or (b) configure LYRACORE_SHARD_MAP so map \
                     {map_id}'s instances are routed to a database that DOES host populations, \
                     e.g. LYRACORE_SHARD_MAP=\"{map_id}:*=<instances-database>\". Note that EVERY \
                     instance bucket is checked, so a pool whose members disagree about the flag \
                     lands here on the one member that forgot it, not on all eight."
                ));
            }
            // The inverse (#39's direction): the portal runs on the default database, so if THAT
            // one hosts populations while the run happens elsewhere, it pays for a dungeon twice
            // over — spawn then evict. Recorded, not returned: `NoHost` is strictly worse, so a
            // configuration that is somehow both must report the fatal.
            if owner != self.default_db && hosts(&self.default_db) && load_not_moved.is_none() {
                load_not_moved = Some(format!(
                    "instances of DUNGEON map {map_id} are owned by database {owner}, but the \
                     dungeon portal's areatrigger runs on {0} — and {0}'s \
                     `game_config.hosts_instances` is still TRUE, so every entry spawns the whole \
                     population (~207 creatures + 28 gameobject copies) on the world writer and \
                     evicts it again after the transfer. The run works; the load Phase A exists to \
                     remove does not go away, and nothing else will tell you. Fix: `spacetime sql \
                     {0} \"UPDATE game_config SET hosts_instances = false WHERE id = 0\"` \
                     (issue #39).",
                    self.default_db
                ));
            }
        }
        match load_not_moved {
            Some(msg) => InstanceHosting::LoadNotMoved(msg),
            None => InstanceHosting::Consistent,
        }
    }

    /// Every database the gateway opens a coordinator connection to, default first: the world shards
    /// plus realm-core when configured.
    pub fn databases(&self) -> Vec<String> {
        let mut dbs = self.shards();
        if let Some(rc) = &self.realm_core {
            if !dbs.contains(rc) {
                dbs.push(rc.clone());
            }
        }
        dbs
    }
}

/// The startup verdict of [`ShardMap::check_instance_hosting`] — will this realm's dungeon
/// populations actually be spawned, and on the right database? (issues #39 + #48.)
///
/// Exactly one variant means "correctly configured", and it carries no message: the ordinary
/// single-database realm and a correctly-configured Phase A deployment are both SILENT. That is
/// deliberate — #39's predecessor warned on every sharded startup whether or not anything was wrong,
/// the operator confirmed it firing on a correct gateway, and a warning that always fires gets
/// filtered, which defeats the one startup it needed to catch.
#[derive(Clone, Debug, PartialEq)]
pub enum InstanceHosting {
    /// Every dungeon map's instances land on a database that hosts populations, and no database
    /// spawns a population it will not run. Nothing to report.
    Consistent,
    /// The default database will spawn dungeon populations it does not run (#39): they are evicted
    /// again after the transfer. Wasteful, not broken — a warning.
    LoadNotMoved(String),
    /// Dungeons WOULD be empty right now, but only because the database the map is routed to did not
    /// connect — so entries degrade back to a default database that hosts nothing. **Not fatal**, and
    /// that split is the point of this variant existing.
    ///
    /// The configuration is CORRECT; another process is down. `Coordinator::connect` already treats
    /// an unreachable non-default shard as a degradation rather than a failure, on purpose and in
    /// writing ("routing to a missing shard degrades to the default database, i.e. to today's
    /// behavior") — because the open world, every login, and every non-dungeon session are fine.
    /// Refusing to boot here would convert a transient blip in the instances database into a total
    /// realm outage, and there is no retry to recover it: `main` propagates the error and the process
    /// exits, so a gateway started seconds too early in a node restart would need a human. Empty
    /// dungeons for the duration is strictly less bad than no realm at all, so: log it as an error
    /// (the shard's own "unreachable" error is already on the line above) and come up.
    UnreachableHost(String),
    /// NOBODY will spawn the population: every dungeon is created empty (#48), and no amount of
    /// waiting fixes it because the owning database is CONNECTED and says it hosts nothing. Fatal.
    NoHost(String),
}

impl InstanceHosting {
    /// Apply the verdict at startup: log the warning, or refuse to start.
    ///
    /// `Err` is the whole point of #48 — an empty-dungeon realm must not come up quietly, because
    /// the symptom (bot tests failing as if combat or questing had regressed) points at everything
    /// except the config line that caused it. The severity split is here, in a pure function, so a
    /// test can pin "the damaging direction is fatal and the wasteful one is not" without a node.
    ///
    /// Only a MISCONFIGURATION is fatal. The two recoverable verdicts log and return `Ok`, because a
    /// startup check that refuses to boot on a transient condition trades a recoverable degradation
    /// for an outage — see [`InstanceHosting::UnreachableHost`].
    pub fn enforce(self) -> std::result::Result<(), String> {
        match self {
            Self::Consistent => Ok(()),
            Self::LoadNotMoved(msg) => {
                log::warn!("{msg}");
                Ok(())
            }
            Self::UnreachableHost(msg) => {
                log::error!("{msg}");
                Ok(())
            }
            Self::NoHost(msg) => Err(msg),
        }
    }
}

/// Where the realm believes a character lives, and whether the realm-core index needs correcting.
#[derive(Clone, Debug, PartialEq)]
pub struct HomeShard {
    /// The database that owns the character.
    pub db: String,
    /// The `(map_id, instance_id)` that resolved it — the value written back on a heal.
    pub location: (u32, u64),
    /// The realm-core index disagreed (or had nothing) — write `location` back to it.
    pub heal: bool,
}

/// Resolve which database owns `character_guid`, from the realm-core index hint plus a probe of the
/// connected world shards (#20 AC#3). Pure, so the whole self-heal rule is unit-testable without a
/// node — deliberately, because #17 shipped a routing bug in exactly the layer its mock hardcoded.
///
/// - `hint` — the realm-core `game_character_shard` entry, or `None` when the index has no idea.
/// - `connected` — the world shards that actually connected, DEFAULT FIRST (probe order).
/// - `probe(db)` — "does `db` hold this character, and at what location" (the coordinator's
///   `character_location` against that shard's cache; an in-memory lookup, not a round trip).
///
/// The rule: **the index is a hint, the row is the truth.** A hint is accepted only when the shard it
/// names actually holds the character; anything else falls through to walking the shards, and the
/// answer is marked for write-back. With ONE connected shard this degenerates to "probe the default
/// shard and resolve its location through the map" — i.e. #17's behavior, byte for byte.
pub fn resolve_home_shard(
    map: &ShardMap,
    connected: &[String],
    hint: Option<(u32, u64)>,
    probe: impl Fn(&str) -> Option<(u32, u64)>,
) -> Option<HomeShard> {
    let is_connected = |d: &str| connected.iter().any(|c| c == d);
    let settle = |location: (u32, u64)| HomeShard {
        db: map
            .resolve_connected(location.0, location.1, is_connected)
            .to_string(),
        location,
        heal: hint != Some(location),
    };

    // 1. Fast path: trust the hint only far enough to know WHICH shard to ask, then confirm with
    //    that shard's own row. A wrong hint therefore costs one extra probe, never a wrong route.
    if let Some((map_id, instance_id)) = hint {
        let hinted = map.resolve_connected(map_id, instance_id, is_connected);
        if let Some(location) = probe(hinted) {
            return Some(settle(location));
        }
        log::info!(
            "realm-core index points at map {map_id}/instance {instance_id} (shard {hinted}), but \
             that shard does not hold the character — probing; the index will self-heal"
        );
    }

    // 2. Fallback probe (the self-heal): ask every connected shard, default first, and route by the
    //    location the owning shard reports — exactly how #17 routes, just sourced by probe rather
    //    than by assuming the default shard still holds the row.
    connected.iter().find_map(|db| probe(db)).map(settle)
}

// ===============================================================================================
//  Region assignment (#23 — the Phase C foundation of the elastic-sharding spec, #12)
// ===============================================================================================

/// One row of realm-core's `region_assignment { map_id, region_id, shard, epoch }`, as the gateway
/// reads it. `shard` is a DATABASE NAME; `epoch` is monotonic per `(map_id, region_id)`.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionAssignment {
    pub map_id: u32,
    pub region_id: u32,
    pub shard: String,
    pub epoch: u64,
}

/// **The whole of region routing.** Which database owns the region containing `(map_id, x, y)` —
/// or `None`, meaning "no region owns this point; route it through the ordinary `(map, instance)`
/// shard map", i.e. exactly as #17 did.
///
/// `None` is the answer in every degenerate case, and that is the ticket's safety property: no
/// region definitions, no assignment row, an assignment naming an empty or unconnected shard, a
/// point in `DEFAULT_REGION`, or anything inside an instance. **With every region of a map assigned
/// to one shard the result is the same database the shard map would have produced anyway**, so the
/// seams are drawn and dormant and nothing about the wire changes.
///
/// # Epoch semantics
///
/// An operator flip bumps `epoch`. The HIGHEST epoch for a `(map_id, region_id)` wins, so a stale
/// row — a retried write, or a delete/insert pair delivered in either order by the subscription —
/// can never resurrect a superseded assignment. A flip re-routes **new entrants only**: this
/// function runs at world entry (`WorldStore::home_shard`) and nowhere else, and a session already
/// in the world keeps the shard handle it pinned there. Moving residents is warm handoff, a later
/// ticket; this one deliberately builds none of it.
///
/// # Instances are not region-routed
///
/// Regions partition the OPEN WORLD (`instance_id == 0`). An instance is already its own partition
/// with its own owner (#19), and layering a region overlay on top of it would give one player two
/// answers to "which database am I on".
pub fn resolve_region_shard<'a>(
    regions: &lyracore_shared::region::RegionMap,
    assignments: &'a [RegionAssignment],
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    connected: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    if instance_id != 0 {
        return None;
    }
    let region_id = regions.region_at(map_id, x, y);
    if region_id == lyracore_shared::region::DEFAULT_REGION {
        return None;
    }
    winning_region_shard(assignments, map_id, region_id, connected)
}

/// [`resolve_region_shard`]'s tail, factored out so the CELL-keyed resolver below (#73 view-merge)
/// and the world-position resolver above share one "which shard won this region" decision instead of
/// the epoch/connectivity rules drifting apart between them.
fn winning_region_shard(
    assignments: &[RegionAssignment],
    map_id: u32,
    region_id: u32,
    connected: impl Fn(&str) -> bool,
) -> Option<&str> {
    let winner = assignments
        .iter()
        .filter(|a| a.map_id == map_id && a.region_id == region_id)
        .max_by_key(|a| a.epoch)?;
    let db = winner.shard.trim();
    if db.is_empty() {
        return None;
    }
    if !connected(db) {
        // Same degradation rule as `ShardMap::resolve_connected`: a shard the assignment names but
        // the gateway never reached routes NOBODY to it. Falling through to the shard map is the
        // pre-region answer, which is always a database this gateway is actually talking to.
        log::warn!(
            "region {region_id} on map {map_id} is assigned to shard {db} (epoch {}), which is not \
             connected — falling through to the (map, instance) shard map",
            winner.epoch
        );
        return None;
    }
    Some(db)
}

/// The CELL-keyed twin of [`resolve_region_shard`] — issue #73's view-merge rebuild. Which database
/// owns the region containing grid cell `(gx, gy)` on `map_id`, or `None` for "home" — the same
/// degenerate cases `resolve_region_shard` collapses on (no definitions, no assignment, an
/// unconnected/empty shard, or an instance).
///
/// This is the RESOLVER `aoi::split_box_by_shard` calls once per cell while it scans a box's rows —
/// the boundary-line splitter, not a separately-materialized per-cell partition map (the shape #73's
/// first attempt built and the rebuild's design note calls out as unnecessary complexity: the split
/// and the resolve are one pass, not two). Both resolvers must answer the SAME shard for the cell a
/// given `(x, y)` falls in — `resolve_region_shard_for_cell_agrees_with_resolve_region_shard` below
/// pins that the two conversions cannot drift.
pub fn resolve_region_shard_for_cell<'a>(
    regions: &lyracore_shared::region::RegionMap,
    assignments: &'a [RegionAssignment],
    map_id: u32,
    instance_id: u64,
    gx: i32,
    gy: i32,
    connected: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    if instance_id != 0 {
        return None;
    }
    let region_id = regions.region_of(map_id, gx, gy);
    if region_id == lyracore_shared::region::DEFAULT_REGION {
        return None;
    }
    winning_region_shard(assignments, map_id, region_id, connected)
}

/// #73 view-merge (bug found LIVE 2026-08-04, `test-view-merge.sh` gate): a region explicitly
/// assigned to the CALLER's OWN shard resolves to HOME, not to an away bucket named after itself.
/// `resolve_region_shard_for_cell` (like `resolve_region_shard` before it) answers purely "which
/// database does this assignment name", with no notion of who is asking — a caller that hands the
/// name straight to `Coordinator::shard_handle` stays correct for free, because THAT method's own
/// `db == self.shard_name()` guard folds "my own shard" back to "stay" before anything acts on it.
/// `Coordinator::split_box_by_shard` cannot rely on that fold: it builds `(shard, rects)` buckets
/// directly from the resolver's raw answer, one call per cell, before any `shard_handle` call ever
/// runs — so a region drawn "for symmetry" onto the shard the viewer is ALREADY on (an entirely
/// ordinary menu: one region per shard, e.g. "region 1 = the rest of Elwynn, pinned to
/// lyracore" beside "region 2 = Goldshire, pinned to lyracore-world-2") lands its own HOME
/// coverage in an away bucket that can never resolve — worse than a missed away leg, because
/// `update()`'s home bucket only ever picks up a `None` entry, so a box that resolves ENTIRELY to
/// "my own shard, by name" ends up subscribing NOTHING at all, home or away.
pub fn fold_home_shard(resolved: Option<String>, my_shard: &str) -> Option<String> {
    resolved.filter(|name| name != my_shard)
}

/// `<map_id|*>[:<bucket|*>]=<db>` → a rule, or `None` if it doesn't parse.
fn parse_shard_rule(rule: &str) -> Option<ShardRule> {
    let (loc, db) = rule.split_once('=')?;
    let db = db.trim();
    if db.is_empty() {
        return None;
    }
    let (map, bucket) = match loc.split_once(':') {
        Some((m, b)) => (m.trim(), b.trim()),
        None => (loc.trim(), "*"),
    };
    let map_id = if map == "*" {
        None
    } else {
        Some(map.parse::<u32>().ok()?)
    };
    let bucket = if bucket == "*" {
        None
    } else {
        Some(bucket.parse::<u64>().ok()?)
    };
    if bucket.is_some_and(|b| b >= INSTANCE_BUCKETS) {
        return None; // a bucket outside the modulus can never match — that's a typo, not a rule
    }
    Some(ShardRule {
        map_id,
        bucket,
        db: db.to_string(),
    })
}

/// Emit the `PLAYER_QUEST_LOG_*` descriptor fields (the L-window quest log). Default ON — the layout
/// (index 198, 20 slots × 3 u32, 6-bit-per-counter packing) was VERIFIED
/// against a live 5875 client on 2026-06-24 (empty log + multi-quest log render with no crash; a kill
/// updated the objective to "1/10" live). Set `LYRACORE_QUEST_LOG=0` to disable (escape hatch if a future
/// content edge case misbehaves). Pure config, shared by the world login-sync path and the stdb quest-log
/// relay — it lives here so the stdb tier doesn't reach up into `world` for it.
pub fn quest_log_fields_enabled() -> bool {
    std::env::var("LYRACORE_QUEST_LOG").map_or(true, |v| v != "0")
}

/// #228 — the address the realm list advertises, overriding the `game_realm` row.
///
/// The row is seeded `127.0.0.1:8085`, which is right for the loopback contributor fixture and
/// wrong for every other way of reaching this gateway: a client that completed the SRP6 handshake
/// against a LAN (or hosted) logon tier is then told to open the world connection to *its own*
/// loopback. That failure is silent and looks like "the realm doesn't work", because the logon half
/// succeeded.
///
/// Unset — production and every existing deployment — is byte-identical to before: the row wins.
/// `lyracore dev up [--lan IP]` sets it to the address it actually bound the world listener to, so
/// what is advertised is a property of how the gateway was launched rather than of a row somebody
/// may have edited on one shard.
pub fn advertised_realm_address() -> Option<String> {
    realm_address_override(std::env::var("LYRACORE_REALM_ADDRESS").ok())
}

/// The pure half of [`advertised_realm_address`]. An empty or blank value reads as unset — an
/// exported-but-empty variable is a shell accident (`LYRACORE_REALM_ADDRESS=$MAYBE_UNSET`), and
/// advertising "" would break the realm list rather than override it.
pub fn realm_address_override(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Area-of-Interest gate (scaling): the per-player `game_world_entity` subscription is SCOPED to a
/// grid-cell box that re-subscribes as the player crosses cells (an `AreaOfInterestTracker`), instead
/// of the global `SELECT *`. Default ON since 2026-07-10 (operator live-verified the 2-client
/// enter/leave-scope behavior; the 5×5 50-yd box ≈ vanilla view distance). `LYRACORE_AOI=0` is the
/// escape hatch back to the proven global subscription.
pub fn aoi_enabled() -> bool {
    std::env::var("LYRACORE_AOI").map_or(true, |v| v != "0")
}

/// #72 slice 2 — the warm mid-session handoff gate (house pattern: [`aoi_enabled`]). Default ON:
/// a confirmed [`crate::world::seam::SeamCrossing`] drives the escrowed transfer and re-homes the
/// session with no loading screen. `LYRACORE_WARM_HANDOFF=0` is the escape hatch back to slice 1's
/// log-only behavior (detect + log, never move anyone) — useful if a live handoff misbehaves and
/// the operator wants residents to keep draining onto their login shard instead.
pub fn warm_handoff_enabled() -> bool {
    std::env::var("LYRACORE_WARM_HANDOFF").map_or(true, |v| v != "0")
}

/// #73 — the cross-seam view-merge gate (house pattern: [`aoi_enabled`]/[`warm_handoff_enabled`]).
/// Default ON: when a straddling player's box crosses a region boundary owned by another shard, the
/// `AreaOfInterestTracker` splits the box into per-shard `GridRect`s (`aoi::split_box_by_shard`) and
/// opens that shard's per-account connection to merge its deltas into the same view instead of only
/// ever seeing its own shard. `LYRACORE_VIEW_MERGE=0` collapses to home-shard-only AOI (today's
/// behavior) — the escape hatch if the away leg misbehaves live. Structurally a no-op on a
/// single-database realm or an un-imported seam menu either way: `split_box_by_shard` already
/// answers every cell "home" the instant no seam menu is imported, so this flag only ever matters
/// when a seam menu AND a region assignment both exist.
pub fn view_merge_enabled() -> bool {
    std::env::var("LYRACORE_VIEW_MERGE").map_or(true, |v| v != "0")
}

/// The seam-crossing notice: each warm handoff's start/complete as a System chat line to the
/// crossing player. Default ON since #326 (house pattern: [`aoi_enabled`]), where it stopped being
/// #72's opt-in operator testing aid and became a player-facing status line for the alpha. The
/// no-loading-screen crossing is the project's flagship differentiator and it is otherwise
/// *invisible* — it works so quietly that nobody can tell it happened — so for the alpha every
/// player sees it announced without an operator exporting anything. `LYRACORE_SEAM_NOTIFY=0` is
/// the operator opt-out, for a realm that would rather its seams stay silent; flip the default
/// back once seams are mature and boring (the follow-up #326 asks for).
pub fn seam_notify_enabled() -> bool {
    seam_notify_from_env(std::env::var("LYRACORE_SEAM_NOTIFY").ok().as_deref())
}

/// The pure half of [`seam_notify_enabled`] (house pattern: [`realm_address_override`]) — the
/// variable is process-global, so the parse is exercised through this rather than by mutating the
/// test process's own environment. Only an exact `0` disables — the same rule as [`aoi_enabled`]
/// and friends, spelled as a comparison because clippy rejects `map_or(true, ..)` on a `&str`.
pub fn seam_notify_from_env(raw: Option<&str>) -> bool {
    raw != Some("0")
}

/// #209 probe: `LYRACORE_WRITER_TRACE=1` turns on a per-session black-box ring inside `spawn_writer` — the
/// last 32 (opcode, declared size, rolling checksum) triples for the exact bytes handed to the
/// socket, dumped to `/tmp/gw-writer-crash/<account>.txt` when a session's writer loop ends on a
/// write error. Default OFF: the ring costs a checksum pass over every outbound body, which is not
/// a cost production should pay to stand still. Mirrors the wire harness's own crash ring
/// (`/tmp/wire-crash/`) so the two can be diffed frame-for-frame at a corruption offset.
pub fn writer_trace_enabled() -> bool {
    std::env::var("LYRACORE_WRITER_TRACE").is_ok_and(|v| v == "1")
}

/// #180 login queue: the hard ceiling on concurrently ESTABLISHED world sessions (a seat is held
/// from `AUTH_OK` until the socket closes — see `world::login_queue`). Default `0` = unlimited,
/// i.e. today's behavior byte for byte: a single-player dev realm, or any gateway that never sets
/// this, never queues anyone. A malformed value falls back to unlimited rather than guessing.
pub fn max_sessions() -> usize {
    std::env::var("LYRACORE_MAX_SESSIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// #180 login queue: additionally rate-limits how many queued sessions may be admitted per tick
/// (`world::login_queue::LoginQueue`'s tick, 1s in production) — independent of `max_sessions`, so
/// a burst of simultaneously freed seats (a raid wipe, a restart draining a full queue) trickles
/// admissions instead of releasing its whole backlog into `CMSG_PLAYER_LOGIN` at once. Default `0`
/// = unlimited: seats fill as fast as they free, subject only to `max_sessions`.
pub fn admit_concurrency() -> usize {
    std::env::var("LYRACORE_ADMIT_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod realm_address_tests {
    use super::*;

    /// The whole safety property of #228's override: **unset changes nothing**. Every existing
    /// deployment, production included, keeps advertising the `game_realm` row it always did.
    #[test]
    fn an_unset_override_leaves_the_game_realm_row_in_charge() {
        assert_eq!(realm_address_override(None), None);
    }

    #[test]
    fn a_blank_override_reads_as_unset_rather_than_advertising_nothing() {
        // `LYRACORE_REALM_ADDRESS=$SOMETHING_UNSET` is one exported-but-empty variable away, and
        // advertising an empty address would break the realm list instead of overriding it.
        for blank in ["", "   ", "\t\n"] {
            assert_eq!(
                realm_address_override(Some(blank.to_string())),
                None,
                "{blank:?} must read as unconfigured"
            );
        }
    }

    #[test]
    fn a_configured_address_wins_over_the_row_and_is_trimmed() {
        assert_eq!(
            realm_address_override(Some("192.168.1.50:8085".to_string())),
            Some("192.168.1.50:8085".to_string())
        );
        // Shell/env-file whitespace is not part of the address the client is told to connect to.
        assert_eq!(
            realm_address_override(Some("  192.168.1.50:8085\n".to_string())),
            Some("192.168.1.50:8085".to_string())
        );
    }
}

#[cfg(test)]
mod seam_notify_tests {
    use super::*;

    /// #326 inverted this gate: what was #72's `=1` opt-in is now on unless the operator opts out.
    /// Driven through the pure half for the reason [`seam_notify_from_env`] documents — `std::env`
    /// is process-global and the test binary is one process, so a `set_var` here would leak into
    /// every other test running beside it.
    #[test]
    fn an_unset_variable_now_announces_the_crossing() {
        assert!(
            seam_notify_from_env(None),
            "the alpha's flagship crossing must be visible with nothing exported"
        );
    }

    #[test]
    fn exactly_zero_is_the_operators_opt_out() {
        assert!(!seam_notify_from_env(Some("0")));
    }

    #[test]
    fn one_still_enables_so_an_old_launcher_does_not_invert() {
        // #72-era recipes and scripts export `=1`. Under the old polarity that meant "on"; it must
        // not now read as "the operator asked for something other than the default".
        assert!(seam_notify_from_env(Some("1")));
    }

    #[test]
    fn any_other_value_leaves_the_notice_on() {
        // Deliberately including "false"/"off": this file's gates all test `!= "0"` and nothing
        // else (see `aoi_enabled`), so a truthiness word is NOT an opt-out and must not silently
        // read as one.
        for other in ["", "  ", "1 ", "true", "false", "off", "no", "2"] {
            assert!(
                seam_notify_from_env(Some(other)),
                "{other:?} is not the documented opt-out and must leave the notice on"
            );
        }
    }
}

#[cfg(test)]
mod shard_map_tests {
    use super::*;

    #[test]
    fn an_unconfigured_map_is_a_single_entry_that_never_routes() {
        // The safety property (#17): absent config → every location resolves to the one database,
        // so no reducer call and no subscription can target anything else.
        let m = ShardMap::parse("lyracore", "");
        assert_eq!(m.resolve(0, 0), "lyracore");
        assert_eq!(m.resolve(389, 12345), "lyracore");
        assert_eq!(m.databases(), vec!["lyracore".to_string()]);
    }

    #[test]
    fn a_map_rule_routes_that_map_only() {
        let m = ShardMap::parse("world", "1:*=instances");
        assert_eq!(m.resolve(1, 0), "instances");
        assert_eq!(m.resolve(1, 99), "instances");
        assert_eq!(
            m.resolve(0, 0),
            "world",
            "unmatched maps stay on the default shard"
        );
        assert_eq!(
            m.databases(),
            vec!["world".to_string(), "instances".to_string()]
        );
    }

    #[test]
    fn a_bucket_rule_splits_one_maps_instances_across_a_pool() {
        let m = ShardMap::parse("world", "389:0=pool-a\n389:1=pool-b\n389:*=pool-c");
        assert_eq!(m.resolve(389, 0), "pool-a"); // 0 % 8 == 0
        assert_eq!(m.resolve(389, 8), "pool-a");
        assert_eq!(m.resolve(389, 1), "pool-b");
        assert_eq!(m.resolve(389, 2), "pool-c"); // falls through to the map-wide rule
        assert_eq!(m.resolve(0, 0), "world");
    }

    #[test]
    fn first_matching_rule_wins_and_the_shorthand_omits_the_bucket() {
        let m = ShardMap::parse("world", "1=first; 1:*=second");
        assert_eq!(m.resolve(1, 3), "first");
    }

    // ==========================================================================================
    //  Instance hosting (#48): the detector that replaced #39's always-firing reminder.
    //
    //  Every test here drives the same pure function with the same two inputs — the routing (this
    //  gateway's) and the flag (each database's) — because the bug was precisely that no process
    //  held both.
    // ==========================================================================================

    /// Convenience: `hosts_instances` reader from a `(db, flag)` list; anything unlisted reads
    /// `None`, i.e. the module's own "no row ⇒ hosts" default.
    fn flags<'a>(pairs: &'a [(&'a str, bool)]) -> impl Fn(&str) -> Option<bool> + 'a {
        move |db: &str| pairs.iter().find(|(d, _)| *d == db).map(|(_, f)| *f)
    }

    /// **Issue #48, the damaging direction.** No shard map + `hosts_instances = false` = every
    /// dungeon created with 0 entities, forever, because there is no instances shard to route to.
    /// Measured live as 8 consecutive empty instances and four bot-test failures (`bot_deadmines`
    /// "no Defias death within 300s", `party_brains`, `bot_goals`, `class_roles`) that looked like
    /// gameplay regressions until someone counted instance rows.
    #[test]
    fn no_shard_map_plus_hosting_off_is_fatal_and_names_both_remedies() {
        let single = ShardMap::parse("lyracore", "");
        let verdict =
            single.check_instance_hosting(flags(&[("lyracore", false)]), |d| d == "lyracore");
        let msg = match &verdict {
            InstanceHosting::NoHost(m) => m.clone(),
            other => panic!(
                "a single-database gateway against a database that does not host instance \
                 populations must be FATAL — nothing else can spawn the dungeon. Got: {other:?}"
            ),
        };
        // The AC's exact-remedy requirement, both branches of it.
        assert!(
            msg.contains("UPDATE game_config SET hosts_instances = true WHERE id = 0"),
            "the message must name the exact SQL that fixes it: {msg}"
        );
        assert!(
            msg.contains("LYRACORE_SHARD_MAP"),
            "…or the routing that fixes it instead: {msg}"
        );
        assert!(
            msg.contains("lyracore"),
            "…and the database to run it on: {msg}"
        );
        // Fatal means fatal.
        assert!(
            verdict.enforce().is_err(),
            "the empty-dungeon verdict must refuse to start the gateway"
        );
    }

    /// **The ordinary single-database realm** — `LYRACORE_SHARD_MAP` unset, flag at its `true` default,
    /// and the same again for a database with no config row at all. Silent, both ways. This is the
    /// half #39 promised and #48 must not spend.
    #[test]
    fn the_unconfigured_single_database_realm_is_silent() {
        let single = ShardMap::parse("lyracore", "");
        let up = |d: &str| d == "lyracore";
        assert_eq!(
            single.check_instance_hosting(flags(&[("lyracore", true)]), up),
            InstanceHosting::Consistent,
            "a single database that hosts its own dungeons is correctly configured — saying \
             ANYTHING here is the noise #48 removed"
        );
        assert_eq!(
            single.check_instance_hosting(|_| None, up),
            InstanceHosting::Consistent,
            "a database with no game_config row reads as HOSTING (the module's own unwrap_or(true)) \
             — an unreadable config must never fabricate a fatal"
        );
    }

    /// **Issue #39's direction, now actually detected.** A shard map that routes Deadmines away
    /// while the world shard still hosts populations warns; the same shard map with the flag
    /// correctly `false` says NOTHING. The second half is the fix for #44's always-fires noise —
    /// the operator confirmed the old reminder firing on a correctly-configured gateway.
    #[test]
    fn a_routed_dungeon_warns_only_while_the_world_shard_still_hosts() {
        let phase_a = ShardMap::parse("lyracore", "36:*=lyracore-instances");
        let up = |_: &str| true;
        match phase_a.check_instance_hosting(
            flags(&[("lyracore", true), ("lyracore-instances", true)]),
            up,
        ) {
            InstanceHosting::LoadNotMoved(msg) => {
                assert!(
                    msg.contains("UPDATE game_config SET hosts_instances = false WHERE id = 0"),
                    "the warning must name the exact SQL: {msg}"
                );
                assert!(msg.contains("lyracore"), "…on the world shard: {msg}");
            }
            other => panic!(
                "a world shard that still hosts populations for a map routed elsewhere spawns the \
                 dungeon then evicts it — that must warn. Got: {other:?}"
            ),
        }
        assert_eq!(
            phase_a.check_instance_hosting(
                flags(&[("lyracore", false), ("lyracore-instances", true)]),
                up
            ),
            InstanceHosting::Consistent,
            "the CORRECT Phase A configuration must be completely silent — an always-firing \
             warning gets filtered, which is exactly how #48 got missed"
        );
    }

    /// The partial misconfiguration the old reminder's "does the map route anywhere" test could
    /// never see: a shard map that routes the open world but not the DUNGEON, with the flag off.
    /// There IS a second database, it DOES host instances, and dungeons are still empty — because
    /// nothing routes map 36 to it.
    #[test]
    fn a_shard_map_that_routes_everything_except_the_dungeon_is_still_fatal() {
        let m = ShardMap::parse("lyracore", "1:*=lyracore-kalimdor");
        let verdict = m.check_instance_hosting(
            flags(&[("lyracore", false), ("lyracore-kalimdor", true)]),
            |_| true,
        );
        assert!(
            matches!(verdict, InstanceHosting::NoHost(_)),
            "map 36 still resolves to the world shard, which does not host populations — the \
             presence of some OTHER hosting database does not populate Deadmines. Got: {verdict:?}"
        );
    }

    /// A pool member that forgot the flag. `36:1=` sends every 8th run to a database whose
    /// `hosts_instances` is false, so one dungeon in eight is empty — the hardest possible version
    /// of #48 to diagnose from the outside, and the reason the check walks every bucket instead of
    /// asking about one representative instance id.
    #[test]
    fn one_unhosted_pool_member_out_of_eight_is_still_fatal() {
        let m = ShardMap::parse("lyracore", "36:1=instances-1, 36:*=instances-0");
        let verdict = m.check_instance_hosting(
            flags(&[
                ("lyracore", false),
                ("instances-0", true),
                ("instances-1", false), // the operator published this one and forgot the SQL
            ]),
            |_| true,
        );
        assert!(
            matches!(verdict, InstanceHosting::NoHost(_)),
            "bucket 1 of map 36 lands on a database that hosts nothing — every 8th Deadmines run \
             would be an empty dungeon. Got: {verdict:?}"
        );
    }

    /// An instances shard the map names but that failed to connect degrades to the default database
    /// (`resolve_connected`'s documented rule) — and on a Phase A world shard the default database
    /// is exactly the one that hosts nothing, so dungeons WOULD be empty. Reported, and deliberately
    /// **not fatal**.
    ///
    /// This is the availability half of #48 and it cuts the other way from the rest of the file. The
    /// operator's config is CORRECT here; a different process is down, transiently — a node that has
    /// not finished coming up, a gateway launched a few seconds early in a restart, an instances
    /// database mid-republish. `Coordinator::connect` decided in writing that an unreachable
    /// non-default shard degrades rather than fails, precisely so the open world and every login
    /// survive it, and this check must not quietly overrule that one call below: `main` propagates
    /// the error and the process EXITS, with no retry anywhere, so a fatal here turns a blip in the
    /// instances database into a total realm outage that needs a human. Empty dungeons for the
    /// duration beats no realm at all.
    #[test]
    fn an_unreachable_instances_shard_is_reported_but_does_not_stop_the_gateway() {
        let phase_a = ShardMap::parse("lyracore", "36:*=lyracore-instances");
        let verdict = phase_a.check_instance_hosting(
            flags(&[("lyracore", false), ("lyracore-instances", true)]),
            |d| d == "lyracore", // the instances database never connected
        );
        match &verdict {
            InstanceHosting::UnreachableHost(msg) => {
                assert!(
                    msg.contains("lyracore-instances") && msg.contains("NOT CONNECTED"),
                    "the message must name the shard that is down and why dungeons are empty: {msg}"
                );
                assert!(
                    msg.contains("restart the gateway"),
                    "…and the recovery, since the watchdog never re-runs this check: {msg}"
                );
            }
            other => panic!(
                "a correctly-routed realm whose instances shard is merely DOWN must come up and say \
                 so, not refuse to boot — the fatal is for a misconfiguration nothing can recover \
                 from. Got: {other:?}"
            ),
        }
        assert_eq!(
            verdict.enforce(),
            Ok(()),
            "refusing to start here converts a transient blip in one database into a whole-realm \
             outage: `main` exits, nothing retries, and the open world was never affected"
        );
    }

    /// **The operator's live three-database stack, exactly as deployed** (`lyracore` with the
    /// flag correctly off, map 36 routed to `lyracore-instances`, auth on `realm-core`). It must be
    /// `Consistent` — completely silent. A false fatal on the one configuration that actually works
    /// would be worse than the bug this PR fixes, and realm-core is the wrinkle worth pinning: it is
    /// in `databases()` (so it gets a connection and a flag) but it is NOT in `rules`, so it must
    /// never be resolved as a dungeon owner and its own flag must never matter.
    #[test]
    fn the_operators_live_three_database_stack_is_silent() {
        let live = ShardMap::parse("lyracore", "36:*=lyracore-instances")
            .with_realm_core(Some("lyracore-realm"));
        let up = |d: &str| matches!(d, "lyracore" | "lyracore-instances" | "lyracore-realm");
        assert_eq!(
            live.check_instance_hosting(
                flags(&[
                    ("lyracore", false),          // the world shard does not host populations
                    ("lyracore-instances", true), // the instances shard does
                    ("lyracore-realm", false),    // auth only — never spawns anything, never asked
                ]),
                up
            ),
            InstanceHosting::Consistent,
            "the deployed Phase A stack is correct and must produce NO output at all"
        );
        // realm-core's flag is irrelevant either way — it owns no map, so flipping it changes nothing.
        assert_eq!(
            live.check_instance_hosting(
                flags(&[
                    ("lyracore", false),
                    ("lyracore-instances", true),
                    ("lyracore-realm", true),
                ]),
                up
            ),
            InstanceHosting::Consistent,
            "realm-core is not in `rules`, so it can never be a dungeon owner — its flag must not \
             be able to produce a verdict in either direction"
        );
    }

    /// The severity split, pinned on its own: exactly ONE verdict stops the gateway, and it is the
    /// one nothing but an operator edit can fix. Swapping any two is a one-line mutation in `enforce`
    /// that every other test in this file survives.
    #[test]
    fn only_the_unrecoverable_verdict_refuses_to_start() {
        assert!(InstanceHosting::Consistent.enforce().is_ok());
        assert!(
            InstanceHosting::LoadNotMoved("waste".into())
                .enforce()
                .is_ok(),
            "spawning a population that gets evicted is wasteful, not broken — it must not stop a \
             live realm from coming up"
        );
        assert!(
            InstanceHosting::UnreachableHost("down".into()).enforce().is_ok(),
            "another database being DOWN is transient and recoverable, and the open world is fine — \
             failing startup on it trades a degradation for an outage with no retry to escape it"
        );
        assert_eq!(
            InstanceHosting::NoHost("empty".into()).enforce(),
            Err("empty".into()),
            "an every-dungeon-is-empty realm whose owning database is CONNECTED and says it hosts \
             nothing cannot recover by waiting — that one must fail startup, with the message as the \
             reason"
        );
    }

    /// Strip Rust comments from `src` so a source scan cannot be satisfied by prose.
    ///
    /// Adversarial review of this PR defeated the tripwire below FOUR ways, and three of them were
    /// comments: the original scan filtered only lines whose FIRST token is `//`, so a trailing
    /// `// game_config() … find(&0)` on any code line, a `/* … */` block, or a commented-out
    /// `// "SELECT * FROM game_config",` all satisfied it while the code did the opposite. Removing
    /// comments before scanning is the only version of this that means anything.
    ///
    /// String literals are left alone (the scan wants `"SELECT * FROM game_config"`), which is why
    /// the `?` assertion below is positional rather than a `contains`.
    fn strip_comments(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut chars = src.char_indices().peekable();
        let (mut in_str, mut in_line, mut block) = (false, false, 0usize);
        while let Some((i, c)) = chars.next() {
            if in_line {
                if c == '\n' {
                    in_line = false;
                    out.push(c);
                }
                continue;
            }
            if block > 0 {
                if c == '/' && src[i..].starts_with("/*") {
                    block += 1;
                    chars.next();
                } else if c == '*' && src[i..].starts_with("*/") {
                    block -= 1;
                    chars.next();
                } else if c == '\n' {
                    out.push(c);
                }
                continue;
            }
            if in_str {
                out.push(c);
                if c == '\\' {
                    if let Some((_, n)) = chars.next() {
                        out.push(n);
                    }
                } else if c == '"' {
                    in_str = false;
                }
                continue;
            }
            match c {
                '"' => {
                    in_str = true;
                    out.push(c);
                }
                '/' if src[i..].starts_with("//") => in_line = true,
                '/' if src[i..].starts_with("/*") => {
                    block = 1;
                    chars.next();
                }
                _ => out.push(c),
            }
        }
        out
    }

    /// The verdict has to be ENFORCED where the databases are known. `Coordinator::connect` needs a
    /// live node, so this is the tripwire for its one call site: the pure decision above is worth
    /// nothing if the result is dropped on the floor (`let _ = …`, `.ok()`, a bare `if let`).
    ///
    /// # This scan was defeated four ways in review — do not weaken it back
    ///
    /// Every assertion here exists because a mutation satisfied its predecessor while restoring #48
    /// with the whole 438-test suite green:
    ///
    /// 1. **`?` inside a string.** `.enforce().map_err(|m| log::warn!("hosting? {m}"))` contains a
    ///    `?` and propagates NOTHING. Now the `?` must be the LAST token of the statement, which is
    ///    what "propagates" actually means and still survives the `.map_err` the real code needs.
    /// 2. **Dead binding.** Keeping the real reader closure, adding `let _ = &hosts_instances;`, and
    ///    passing `|_| Some(true)` to the check left every "the reader names `game_config()` /
    ///    `find(&0)`" assertion true and the check wired to a constant. Now the call site must pass
    ///    the READER BY NAME and the read must live inside that binding.
    /// 3. **Comment.** `// dropped for now: "SELECT * FROM game_config",` satisfied the subscription
    ///    assertion with the subscription deleted. Comments are stripped now.
    /// 4. **`if false { … }`.** A decoy block satisfies any `contains` scan. Rejected outright.
    #[test]
    fn coordinator_connect_enforces_the_hosting_verdict_fatally() {
        const CONNECTION_RS_RAW: &str = include_str!("stdb/connection.rs");
        let connection_rs = strip_comments(CONNECTION_RS_RAW);
        let body = {
            let start = connection_rs
                .find("pub async fn connect(cfg: &GatewayConfig)")
                .expect("Coordinator::connect moved — re-point this tripwire");
            let rest = &connection_rs[start..];
            let open = rest.find('{').expect("connect has a body");
            let mut depth = 0i32;
            let mut end = 0usize;
            for (i, c) in rest[open..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest[open..=end].to_string()
        };
        // Defeat 4: a decoy block satisfies every `contains` below while running nothing.
        assert!(
            !body.contains("if false"),
            "`if false` in Coordinator::connect — a dead block satisfies every source assertion in \
             this tripwire while executing none of it"
        );
        assert!(
            body.contains("check_instance_hosting("),
            "Coordinator::connect no longer checks instance hosting — a gateway with no shard map \
             against a database with hosts_instances = false is back to creating every dungeon \
             empty, silently (#48)"
        );
        // Defeat 2: the check must be wired to the NAMED reader, not to an inline closure. Otherwise
        // the real read can sit next to it as a dead binding and satisfy the assertions below.
        assert!(
            body.contains("check_instance_hosting(hosts_instances,"),
            "the hosting check is not being passed the `hosts_instances` reader by name — an inline \
             closure (`|_| Some(true)`) keeps every other assertion here true, leaves the real \
             reader as a dead binding, and silently restores #48"
        );
        let at = body.find(".enforce()").expect(
            "the hosting verdict is no longer enforced — `check_instance_hosting` is a pure \
             function whose return value IS the decision",
        );
        // Defeat 1: the statement containing `.enforce()` must PROPAGATE. `contains('?')` was
        // satisfied by a question mark inside a log-format string; the `?` has to be the last token
        // before the `;`, which is the definition of propagating and still allows the intermediate
        // `.map_err(...)` the real code needs (`enforce` yields `String`, `connect` yields
        // `anyhow::Result`).
        let stmt = &body[at..];
        let stmt = &stmt[..stmt.find(';').unwrap_or(stmt.len())];
        assert!(
            stmt.trim_end().ends_with('?'),
            "the enforced verdict must PROPAGATE — the statement has to END in `?` so `NoHost` \
             aborts startup. `let _ = …`, `.ok()`, or a `?` buried in a log string swallows the \
             fatal and restores #48 with every other test green: {stmt}"
        );
        // Defeat 3: comments are stripped, so this now needs the real subscription. It lives in
        // `coordinator_queries`, not in `connect`'s body, so this one assertion scans the whole file.
        assert!(
            connection_rs.contains("\"SELECT * FROM game_config\""),
            "the coordinator no longer subscribes game_config — the detector cannot read the flag \
             back and degrades to #39's guess-and-warn reminder"
        );
        // The flag reader must actually READ THE COLUMN, from the id=0 SINGLETON — and the read must
        // be INSIDE the `hosts_instances` binding that the call site above passes by name, so the two
        // cannot be decoupled. The module-side twin of the `find(&0)` mutation (any other id finds
        // nothing and defaults to hosting) is recorded in `module/src/instance.rs` as one that
        // SURVIVED review once already.
        let reader = {
            let from = body
                .find("let hosts_instances =")
                .expect("the `hosts_instances` reader binding is gone — see the assertion above");
            let rest = &body[from..];
            &rest[..rest.find("};").map(|e| e + 2).unwrap_or(rest.len())]
        };
        assert!(
            reader.contains("game_config()") && reader.contains("hosts_instances"),
            "the `hosts_instances` reader does not read game_config out of the coordinator cache — \
             the hosting check is wired to a constant: {reader}"
        );
        assert!(
            reader.contains("find(&0)"),
            "the flag reader is not looking at the game_config id=0 singleton — it would find \
             nothing, default to `true`, and never report an empty-dungeon realm: {reader}"
        );
    }

    #[test]
    fn a_full_wildcard_rule_moves_the_whole_world() {
        let m = ShardMap::parse("world", "*:*=elsewhere");
        assert_eq!(m.resolve(0, 0), "elsewhere");
        assert_eq!(m.resolve(571, 7), "elsewhere");
    }

    #[test]
    fn an_unreachable_shard_degrades_to_the_default_database_not_to_the_asker() {
        // Adversarial review of #17: a shard that is named by the map but failed to connect must
        // route to the DEFAULT database. Degrading to "whatever handle asked" is invisible from the
        // default handle but wrong from any other — a session already pinned to `pool-a` that ports
        // to a map owned by the down `pool-b` would keep its `pool-a` pin and be served by a
        // database that owns neither its old location nor its new one.
        let m = ShardMap::parse("world", "1:*=pool-a, 2:*=pool-b");
        let up = |db: &str| db != "pool-b"; // pool-b is down
        assert_eq!(
            m.resolve_connected(1, 0, up),
            "pool-a",
            "a reachable shard still routes"
        );
        assert_eq!(
            m.resolve_connected(2, 0, up),
            "world",
            "an unreachable shard falls back"
        );
        assert_eq!(
            m.resolve_connected(0, 0, up),
            "world",
            "unmatched maps are unaffected"
        );
    }

    // ==========================================================================================
    //  The instance pool (#21): assignment by the EXISTING bucket rule, and the stickiness that
    //  keeps a live run where it is when the pool grows.
    // ==========================================================================================

    /// AC#1 + AC#3's mechanism, in one test: **the pool is the bucket rule, and nothing else.**
    /// Two pool members, ids allocated consecutively by the world shard's `auto_inc`, and the runs
    /// alternate — no new assignment machinery was added or needed.
    #[test]
    fn the_existing_bucket_rule_spreads_consecutive_runs_across_the_pool() {
        let m = ShardMap::parse(
            "world",
            "36:1=instances-1, 36:3=instances-1, 36:5=instances-1, 36:7=instances-1, \
             36:*=instances-0",
        );
        // Instance ids 1..=8 as `auto_inc` mints them, one per dungeon run.
        let landed: Vec<&str> = (1..=8).map(|id| m.resolve(36, id)).collect();
        assert_eq!(
            landed,
            [
                "instances-1",
                "instances-0",
                "instances-1",
                "instances-0",
                "instances-1",
                "instances-0",
                "instances-1",
                "instances-0",
            ],
            "consecutive runs must alternate across the pool — that IS the load spread"
        );
        assert_eq!(m.instance_pool(36), vec!["instances-0", "instances-1"]);
        // The open world is untouched: it is map 0 / instance 0, which no `36:` rule can match.
        assert_eq!(m.resolve(0, 0), "world");
        assert_eq!(
            m.instance_pool(0),
            vec!["world"],
            "map 0's 'pool' is the world shard itself"
        );
    }

    /// The `*:0=` footgun, pinned from the POOL side. A wildcard bucket rule captures the open
    /// world (instance 0 is bucket 0), and `instance_pool` must report that honestly rather than
    /// quietly hiding it — the pool listing is what an operator reads to check a config.
    #[test]
    fn a_wildcard_bucket_rule_puts_the_open_world_in_the_pool_and_says_so() {
        let trap = ShardMap::parse("world", "*:0=pool-a");
        assert_eq!(
            trap.resolve(0, 0),
            "pool-a",
            "the whole open world moved — the documented trap"
        );
        assert_eq!(trap.instance_pool(0), vec!["pool-a", "world"]);
        // Naming the map is the fix the type docs prescribe, and it keeps the open world home.
        let named = ShardMap::parse("world", "36:0=pool-a");
        assert_eq!(named.resolve(0, 0), "world");
        assert_eq!(named.instance_pool(36), vec!["pool-a", "world"]);
    }

    #[test]
    fn an_unconfigured_map_has_a_one_database_pool_and_instance_owner_is_resolve() {
        // The byte-identical-when-unset property, for #21's two new resolvers. With no rules the
        // pool is the single database and stickiness has nothing to stick to, so `instance_owner`
        // answers exactly what `resolve` answers for every input it can be asked about.
        let m = ShardMap::parse("lyracore", "");
        assert_eq!(m.instance_pool(0), vec!["lyracore"]);
        assert_eq!(m.instance_pool(36), vec!["lyracore"]);
        for map_id in [0u32, 1, 36, 389] {
            for instance_id in [0u64, 1, 5, 8, 12345] {
                assert_eq!(
                    m.instance_owner(map_id, instance_id, "lyracore", |_| true),
                    m.resolve(map_id, instance_id),
                    "map {map_id}/instance {instance_id} must route identically with and without \
                     the #21 stickiness rule when nothing is configured"
                );
            }
        }
    }

    /// **AC#3.** The operator adds `instances-1` by editing `LYRACORE_SHARD_MAP` alone. New runs land on
    /// it; the run already live on `instances-0` — whose bucket the edit just re-pointed — keeps
    /// resolving to `instances-0` for the players who are inside it.
    #[test]
    fn adding_a_second_instances_database_moves_new_runs_and_leaves_a_live_run_where_it_is() {
        let up = |_: &str| true;
        // Before: one instances database owns every bucket of map 36.
        let before = ShardMap::parse("world", "36:*=instances-0");
        assert_eq!(before.instance_pool(36), vec!["instances-0"]);
        // A party walks into Deadmines. The world shard mints instance 5 and holds them, so the
        // shard map decides — the holder is not in the pool — and the run lands on instances-0.
        assert_eq!(before.instance_owner(36, 5, "world", up), "instances-0");

        // The operator adds instances-1, by config alone. Bucket 5 is one of the buckets it takes.
        let after = ShardMap::parse(
            "world",
            "36:4=instances-1, 36:5=instances-1, 36:6=instances-1, 36:7=instances-1, \
             36:*=instances-0",
        );
        assert_eq!(after.instance_pool(36), vec!["instances-0", "instances-1"]);
        // NEW runs land on the new database…
        assert_eq!(
            after.instance_owner(36, 13, "world", up),
            "instances-1",
            "13 % 8 == 5"
        );
        assert_eq!(
            after.instance_owner(36, 10, "world", up),
            "instances-0",
            "10 % 8 == 2"
        );
        // …and the LIVE run does not move. instances-0 is in the pool and is where the character
        // actually is, so it outranks the map's new answer for that bucket. Without this, the
        // player's next world entry would mirror an empty instance 5 onto instances-1 and transfer
        // them into it, away from the party and the population they were fighting.
        assert_eq!(
            after.instance_owner(36, 5, "instances-0", up),
            "instances-0",
            "a run already live on a pool member must survive that bucket being re-pointed"
        );
        // The shard map on its own would have moved them — that is precisely what is being fixed.
        assert_eq!(after.resolve(36, 5), "instances-1");
    }

    #[test]
    fn stickiness_never_applies_to_the_open_world_or_to_a_shard_outside_the_pool() {
        let up = |_: &str| true;
        let m = ShardMap::parse("world", "36:*=instances-0");
        // Instance 0 is the OPEN WORLD, not an instance: a character standing on instances-0 in
        // the open world routes by the map, or they would be pinned to the instance shard forever.
        assert_eq!(m.instance_owner(0, 0, "instances-0", up), "world");
        // Adversarial review: the assertion above SURVIVES deleting the `instance_id != 0` guard,
        // because `instances-0` is not in map 0's pool anyway — that mutation stayed green. The
        // sharp form is a map whose bucket 0 and bucket 1 belong to different databases: the open
        // world is bucket 0, so BOTH are in that map's pool. A character standing in the open world
        // on `world-b` must still be routed by the map to `world-a`; without the guard they answer
        // `world-b` and are pinned to whichever database last held them, permanently, with no
        // instance anywhere in the picture.
        let split = ShardMap::parse("world", "0:0=world-a, 0:*=world-b");
        assert_eq!(split.instance_pool(0), vec!["world-a", "world-b"]);
        assert_eq!(split.resolve(0, 0), "world-a");
        assert_eq!(
            split.instance_owner(0, 0, "world-b", up),
            "world-a",
            "the open world is never sticky — stickiness is a property of INSTANCES"
        );
        // A holder that is not in this map's pool never wins — this is the INITIAL HOP, and it is
        // what makes instances reach the pool at all.
        assert_eq!(m.instance_owner(36, 5, "world", up), "instances-0");
        // A pool member for a DIFFERENT map is not a pool member for this one.
        let two = ShardMap::parse("world", "36:*=instances-0, 389:*=instances-1");
        assert_eq!(two.instance_owner(36, 5, "instances-1", up), "instances-0");
    }

    #[test]
    fn a_sticky_holder_that_is_not_connected_falls_back_to_the_shard_map() {
        // Same degradation rule as everywhere else in this file: a database the gateway never
        // reached can never be routed to, not even by stickiness. Anything else would strand a
        // login on a shard that is not answering.
        let m = ShardMap::parse("world", "36:*=instances-0");
        assert_eq!(
            m.instance_owner(36, 5, "instances-0", |db| db != "instances-0"),
            "world"
        );
        assert_eq!(
            m.instance_owner(36, 5, "instances-0", |_| true),
            "instances-0"
        );
    }

    // ==========================================================================================
    //  Realm-core (#20): the auth database, and the character→shard index's self-heal rule.
    // ==========================================================================================

    #[test]
    fn realm_core_unconfigured_keeps_auth_on_the_default_database() {
        // The safety property (#20, mirroring #17's): with no LYRACORE_REALM_CORE the auth database IS
        // the world database, so every account/session read and write goes exactly where it goes
        // today, and no second coordinator connection is opened.
        let m = ShardMap::parse("world", "1:*=instances");
        assert_eq!(m.realm_core_db(), None);
        assert_eq!(m.auth_db(|_| true), Ok("world"));
        assert_eq!(
            m.auth_db(|_| false),
            Ok("world"),
            "an unconfigured realm-core cannot fail closed"
        );
        assert_eq!(m.databases(), m.shards(), "nothing extra to connect to");
    }

    #[test]
    fn a_configured_realm_core_owns_auth_and_gets_its_own_connection() {
        let m = ShardMap::parse("world", "1:*=instances").with_realm_core(Some("lyracore-realm"));
        assert_eq!(m.realm_core_db(), Some("lyracore-realm"));
        assert_eq!(m.auth_db(|_| true), Ok("lyracore-realm"));
        assert_eq!(
            m.databases(),
            vec![
                "world".to_string(),
                "instances".to_string(),
                "lyracore-realm".to_string()
            ],
            "realm-core is connected but is NOT a shard"
        );
        assert_eq!(
            m.shards(),
            vec!["world".to_string(), "instances".to_string()],
            "realm-core holds no characters, so a character probe must never walk it"
        );
    }

    #[test]
    fn an_unreachable_realm_core_fails_closed_instead_of_falling_back_to_the_world_cache() {
        // THE auth-boundary rule. The world DB keeps `game_account`/`game_session` as a
        // write-through cache (removing the tables would be a destructive migration), so a
        // fallback is *possible* — and must not happen: it would resurrect a rotated password or
        // un-enforce a ban exactly when realm-core is down. Degrade to nobody logs in, never to
        // "everybody logs in against yesterday's credentials".
        let m = ShardMap::parse("world", "").with_realm_core(Some("lyracore-realm"));
        assert_eq!(
            m.auth_db(|db| db != "lyracore-realm"),
            Err("lyracore-realm")
        );
    }

    #[test]
    fn realm_core_naming_the_world_database_or_nothing_is_treated_as_unconfigured() {
        // `LYRACORE_REALM_CORE=lyracore` (== LYRACORE_DATABASE) is a plausible operator mistake and a
        // plausible deliberate "hub == world" single-realm posture (work-item 238's slice 0). Both
        // must collapse to today rather than opening a second connection to the same database.
        for cfg in [Some("world"), Some("  "), Some(""), None] {
            let m = ShardMap::parse("world", "").with_realm_core(cfg);
            assert_eq!(
                m.realm_core_db(),
                None,
                "LYRACORE_REALM_CORE={cfg:?} must read as unconfigured"
            );
            assert_eq!(m.databases(), vec!["world".to_string()]);
        }
        // Surrounding whitespace is trimmed, not treated as part of the name.
        let m = ShardMap::parse("world", "").with_realm_core(Some(" lyracore-realm "));
        assert_eq!(m.realm_core_db(), Some("lyracore-realm"));
    }

    #[test]
    fn one_shard_resolves_home_from_the_probe_alone_exactly_like_before_realm_core() {
        // Backward compatibility, stated as a test: single shard, no index entry → "ask the default
        // shard where the character is, resolve that through the map". #17's behavior verbatim.
        let m = ShardMap::parse("world", "");
        let connected = vec!["world".to_string()];
        let got = resolve_home_shard(&m, &connected, None, |db| (db == "world").then_some((0, 0)));
        assert_eq!(
            got,
            Some(HomeShard {
                db: "world".into(),
                location: (0, 0),
                heal: true
            })
        );
        // Nobody holds the character → no answer at all (the caller keeps its current handle).
        assert_eq!(resolve_home_shard(&m, &connected, None, |_| None), None);
    }

    #[test]
    fn a_matching_index_entry_routes_without_a_heal() {
        let m = ShardMap::parse("world", "389:*=instances");
        let connected = vec!["world".to_string(), "instances".to_string()];
        let got = resolve_home_shard(&m, &connected, Some((389, 12)), |db| {
            (db == "instances").then_some((389, 12))
        });
        assert_eq!(
            got,
            Some(HomeShard {
                db: "instances".into(),
                location: (389, 12),
                heal: false
            }),
            "an index entry the owning shard confirms is used as-is, with no write-back"
        );
    }

    #[test]
    fn a_deliberately_stale_index_entry_self_heals_via_the_fallback_probe() {
        // AC#3. The index says the character is in a Deadmines instance; it actually walked out and
        // is standing in the open world. The hinted shard doesn't hold it, so the probe walks the
        // shards, finds it on `world`, routes there, and flags the entry for correction.
        let m = ShardMap::parse("world", "389:*=instances");
        let connected = vec!["world".to_string(), "instances".to_string()];
        let got = resolve_home_shard(&m, &connected, Some((389, 12)), |db| {
            (db == "world").then_some((0, 0))
        });
        assert_eq!(
            got,
            Some(HomeShard {
                db: "world".into(),
                location: (0, 0),
                heal: true
            }),
            "a stale entry must route by the row that actually exists, and mark itself for repair"
        );
    }

    #[test]
    fn an_index_entry_naming_a_shard_that_lies_about_the_location_still_routes_by_the_row() {
        // The hinted shard DOES hold the character, but at a location that belongs to a different
        // shard (mid-transfer, or a shard-map edit since the entry was written). The location the
        // row reports is the authority for routing, and the entry gets corrected.
        let m = ShardMap::parse("world", "389:*=instances");
        let connected = vec!["world".to_string(), "instances".to_string()];
        let got = resolve_home_shard(&m, &connected, Some((0, 0)), |db| {
            (db == "world").then_some((389, 3))
        });
        assert_eq!(
            got,
            Some(HomeShard {
                db: "instances".into(),
                location: (389, 3),
                heal: true
            })
        );
    }

    #[test]
    fn an_index_entry_pointing_at_a_disconnected_shard_degrades_to_the_default_then_probes() {
        // `resolve_connected` sends the hint to the default database when the named shard is down;
        // if the default doesn't hold the character either, the probe still walks the rest.
        let m = ShardMap::parse("world", "389:*=instances, 1:*=pool-b");
        let connected = vec!["world".to_string(), "instances".to_string()]; // pool-b never connected
        let got = resolve_home_shard(&m, &connected, Some((1, 0)), |db| {
            (db == "instances").then_some((389, 7))
        });
        assert_eq!(
            got,
            Some(HomeShard {
                db: "instances".into(),
                location: (389, 7),
                heal: true
            })
        );
    }

    // ==========================================================================================
    //  Region assignment (#23): the cell→region→shard overlay, and its no-op guarantee.
    // ==========================================================================================

    use lyracore_shared::region::RegionMap;

    /// The seam menu every region test below routes against: two 20×20-cell regions side by side on
    /// map 0, drawn around the cells that contain `POINT_IN_1` / `POINT_IN_2`.
    fn seam_menu() -> RegionMap {
        let (gx, gy) = lyracore_shared::spatial::grid_cell(POINT_IN_1.0, POINT_IN_1.1);
        let (m, rejected) = RegionMap::parse(&format!(
            "0:1 = {}..{}, {}..{}\n0:2 = {}..{}, {}..{}",
            gx,
            gx + 19,
            gy,
            gy + 19,
            gx + 20,
            gx + 39,
            gy,
            gy + 19,
        ));
        assert!(rejected.is_empty(), "{rejected:?}");
        m
    }

    /// A position inside region 1, and one inside region 2 (20 cells = 1000yd west, and the cell
    /// axis is inverted, so a LOWER x is a HIGHER cell index).
    const POINT_IN_1: (f32, f32) = (-8949.95, -132.493);
    const POINT_IN_2: (f32, f32) = (
        -8949.95 - 20.0 * lyracore_shared::spatial::GRID_CELL_SIZE,
        -132.493,
    );

    fn assignment(region_id: u32, shard: &str, epoch: u64) -> RegionAssignment {
        RegionAssignment {
            map_id: 0,
            region_id,
            shard: shard.into(),
            epoch,
        }
    }

    #[test]
    fn region_routing_with_no_definitions_or_no_assignments_is_a_strict_no_op() {
        // THE backward-compatibility property (#23 AC#3), as a test: nothing imported, nothing
        // assigned, nothing routes — every caller falls through to the `(map, instance)` shard map
        // it used before regions existed. Byte-identical, because there is no other branch to take.
        let empty = RegionMap::default();
        let none: [RegionAssignment; 0] = [];
        let (x, y) = POINT_IN_1;
        assert_eq!(
            resolve_region_shard(&empty, &none, 0, 0, x, y, |_| true),
            None
        );
        // Definitions imported but nothing assigned → still nothing routes: the seams are DRAWN
        // and DORMANT, which is exactly what "a seam between same-shard regions costs nothing"
        // means operationally.
        assert_eq!(
            resolve_region_shard(&seam_menu(), &none, 0, 0, x, y, |_| true),
            None
        );
        // Assignments exist but the point is outside every region (DEFAULT_REGION → the shard map).
        let a = [assignment(1, "pool-a", 1)];
        assert_eq!(
            resolve_region_shard(&seam_menu(), &a, 0, 0, 5000.0, 5000.0, |_| true),
            None
        );
        // Even an assignment row that NAMES region 0 must not route it. The module refuses to write
        // one, but the gateway must not depend on that: region 0 is "the rest of the map", and
        // routing it would hand the whole unmapped world to one database on a single bad row.
        let claims_the_rest = [assignment(
            lyracore_shared::region::DEFAULT_REGION,
            "pool-a",
            9,
        )];
        assert_eq!(
            resolve_region_shard(&seam_menu(), &claims_the_rest, 0, 0, 5000.0, 5000.0, |_| {
                true
            }),
            None
        );
        // A different map is untouched by map 0's menu.
        assert_eq!(
            resolve_region_shard(&seam_menu(), &a, 1, 0, x, y, |_| true),
            None
        );
    }

    #[test]
    fn all_regions_of_a_map_on_one_shard_resolves_to_that_one_shard_for_every_point() {
        // AC#3's premise, stated where it can be checked without a node: with every region assigned
        // to the SAME database, the overlay is constant — every point in the zone answers the same
        // shard, so no seam is ever crossed and the topology is indistinguishable from a single
        // shard that happens to have a menu drawn on it.
        let menu = seam_menu();
        let all = [assignment(1, "world", 1), assignment(2, "world", 1)];
        for (x, y) in [POINT_IN_1, POINT_IN_2] {
            assert_eq!(
                resolve_region_shard(&menu, &all, 0, 0, x, y, |_| true),
                Some("world")
            );
        }
    }

    #[test]
    fn an_assignment_flip_routes_the_next_entrant_and_flipping_back_restores() {
        // AC#4, the tracer demo, minus the node: region 2 is empty, so flipping it moves nobody —
        // the NEXT entrant to that region resolves to `pool-b`, and region 1 is untouched by the
        // flip. Flipping back (a higher epoch, empty shard = unassign) restores the old answer.
        let menu = seam_menu();
        let before = [assignment(1, "world", 1)];
        assert_eq!(
            resolve_region_shard(&menu, &before, 0, 0, POINT_IN_2.0, POINT_IN_2.1, |_| true),
            None
        );

        let flipped = [assignment(1, "world", 1), assignment(2, "pool-b", 1)];
        assert_eq!(
            resolve_region_shard(&menu, &flipped, 0, 0, POINT_IN_2.0, POINT_IN_2.1, |_| true),
            Some("pool-b"),
            "the next entrant to the flipped region lands on the second shard"
        );
        assert_eq!(
            resolve_region_shard(&menu, &flipped, 0, 0, POINT_IN_1.0, POINT_IN_1.1, |_| true),
            Some("world"),
            "the region next door is untouched — a flip is per region, not per map"
        );

        // Flipping back: the module deletes the row on an empty-shard write, and an operator who
        // instead re-assigns it to the original database gets the same answer. Both are tested
        // because both are documented as "flip it back".
        let unassigned = [assignment(1, "world", 1)];
        assert_eq!(
            resolve_region_shard(&menu, &unassigned, 0, 0, POINT_IN_2.0, POINT_IN_2.1, |_| {
                true
            }),
            None
        );
        let reassigned = [assignment(1, "world", 1), assignment(2, "world", 3)];
        assert_eq!(
            resolve_region_shard(&menu, &reassigned, 0, 0, POINT_IN_2.0, POINT_IN_2.1, |_| {
                true
            }),
            Some("world")
        );
    }

    #[test]
    fn the_highest_epoch_wins_so_a_stale_row_can_never_resurrect_an_old_assignment() {
        // Epoch semantics. The rows arrive out of a live subscription, in whatever order the SDK
        // applies a delete/insert pair; routing must not depend on that order. Both orderings of
        // the same two epochs must answer the NEWER one.
        let menu = seam_menu();
        let (x, y) = POINT_IN_1;
        let newer_last = [assignment(1, "pool-a", 1), assignment(1, "pool-b", 2)];
        let newer_first = [assignment(1, "pool-b", 2), assignment(1, "pool-a", 1)];
        assert_eq!(
            resolve_region_shard(&menu, &newer_last, 0, 0, x, y, |_| true),
            Some("pool-b")
        );
        assert_eq!(
            resolve_region_shard(&menu, &newer_first, 0, 0, x, y, |_| true),
            Some("pool-b")
        );
        // An UNASSIGN at a higher epoch beats an older assignment the same way.
        let unassigned_later = [assignment(1, "pool-a", 1), assignment(1, "", 2)];
        assert_eq!(
            resolve_region_shard(&menu, &unassigned_later, 0, 0, x, y, |_| true),
            None
        );
    }

    #[test]
    fn an_assignment_naming_an_unconnected_shard_falls_through_to_the_shard_map() {
        // Same rule `resolve_connected` applies to the shard map: a database the gateway never
        // reached can only ever collapse to the pre-region answer. Routing a login into a shard
        // that isn't there would strand the player; routing them by map is always serviceable.
        let menu = seam_menu();
        let (x, y) = POINT_IN_1;
        let a = [assignment(1, "pool-a", 1)];
        assert_eq!(
            resolve_region_shard(&menu, &a, 0, 0, x, y, |db| db != "pool-a"),
            None
        );
        assert_eq!(
            resolve_region_shard(&menu, &a, 0, 0, x, y, |_| true),
            Some("pool-a")
        );
    }

    #[test]
    fn a_player_inside_an_instance_is_never_region_routed() {
        // Regions partition the OPEN WORLD. An instance already has an owner (#19's shard pool),
        // and a region overlay on top of it would give one player two answers to "which database".
        let menu = seam_menu();
        let (x, y) = POINT_IN_1;
        let a = [assignment(1, "pool-a", 1)];
        assert_eq!(
            resolve_region_shard(&menu, &a, 0, 0, x, y, |_| true),
            Some("pool-a")
        );
        assert_eq!(
            resolve_region_shard(&menu, &a, 0, 42, x, y, |_| true),
            None,
            "the same coordinates inside instance 42 route by the shard map, not by region"
        );
    }

    // ==========================================================================================
    //  Cell-keyed region resolution (#73 view-merge rebuild): the boundary-line resolver
    //  `aoi::split_box_by_shard` calls once per cell while it scans a box's rows.
    // ==========================================================================================

    fn cell_of(x: f32, y: f32) -> (i32, i32) {
        lyracore_shared::spatial::grid_cell(x, y)
    }

    #[test]
    fn resolve_region_shard_for_cell_agrees_with_resolve_region_shard() {
        // The two resolvers must never disagree about the shard a given world position's cell
        // belongs to — `resolve_region_shard` converts x/y internally (`RegionMap::region_at`); this
        // one is handed the cell directly (the box splitter's native unit). Same menu, same
        // assignment, same answer, for a point in EACH region and one point outside both.
        let menu = seam_menu();
        let a = [assignment(1, "pool-a", 1), assignment(2, "pool-b", 1)];
        for (x, y) in [POINT_IN_1, POINT_IN_2, (5000.0, 5000.0)] {
            let (gx, gy) = cell_of(x, y);
            assert_eq!(
                resolve_region_shard(&menu, &a, 0, 0, x, y, |_| true),
                resolve_region_shard_for_cell(&menu, &a, 0, 0, gx, gy, |_| true),
                "cell ({gx},{gy}) disagrees with the point resolver at ({x},{y})"
            );
        }
    }

    #[test]
    fn resolve_region_shard_for_cell_refuses_inside_an_instance() {
        let menu = seam_menu();
        let a = [assignment(1, "pool-a", 1)];
        let (gx, gy) = cell_of(POINT_IN_1.0, POINT_IN_1.1);
        assert_eq!(
            resolve_region_shard_for_cell(&menu, &a, 0, 0, gx, gy, |_| true),
            Some("pool-a")
        );
        assert_eq!(
            resolve_region_shard_for_cell(&menu, &a, 0, 42, gx, gy, |_| true),
            None,
            "the same cell inside instance 42 is never region-routed"
        );
    }

    #[test]
    fn resolve_region_shard_for_cell_region_zero_is_always_home() {
        let menu = seam_menu();
        let a = [assignment(1, "pool-a", 1), assignment(2, "pool-b", 1)];
        assert_eq!(
            resolve_region_shard_for_cell(&menu, &a, 0, 0, 0, 0, |_| true),
            None,
            "a cell outside every drawn region is DEFAULT_REGION, which never routes"
        );
    }

    #[test]
    fn resolve_region_shard_for_cell_falls_through_on_an_unconnected_target() {
        let menu = seam_menu();
        let a = [assignment(1, "pool-a", 1)];
        let (gx, gy) = cell_of(POINT_IN_1.0, POINT_IN_1.1);
        assert_eq!(
            resolve_region_shard_for_cell(&menu, &a, 0, 0, gx, gy, |db| db != "pool-a"),
            None,
            "an unreachable target degrades to home, never to a wrong shard"
        );
    }

    /// The exact bug caught live 2026-08-04: a region explicitly assigned to the CALLER's OWN shard
    /// must fold to `None` (home) — never survive as `Some(my_own_name)`, which
    /// `Coordinator::split_box_by_shard` would otherwise group into an away bucket that can never
    /// resolve (`shard_handle("my own name")` legitimately answers "already home", which
    /// `ensure_away` cannot tell apart from "unreachable" without this fold happening first).
    #[test]
    fn fold_home_shard_folds_the_callers_own_name_to_home() {
        assert_eq!(
            fold_home_shard(Some("lyracore".to_string()), "lyracore"),
            None
        );
        assert_eq!(
            fold_home_shard(Some("lyracore-world-2".to_string()), "lyracore"),
            Some("lyracore-world-2".to_string()),
            "a genuinely different shard must pass through unchanged"
        );
        assert_eq!(
            fold_home_shard(None, "lyracore"),
            None,
            "already-home stays home"
        );
    }

    // ==========================================================================================
    //  #223 — the SINGLE-DATABASE realm, and the parser's totality
    // ==========================================================================================

    /// The configuration nearly every deployment runs, and exactly what you silently get when a
    /// topology env var is omitted: nothing configured at all.
    ///
    /// Individual accessors are pinned elsewhere; this asserts the whole surface AT ONCE, because
    /// the failure mode is not "one accessor is wrong" — it is "the gateway looks like it is running
    /// a five-database realm and is actually running one", or the reverse. Every routing question
    /// must answer with the one database, and no accessor may invent a second name.
    ///
    /// It is also the contract every sharding feature is built against: each of #17/#21/#72's rules
    /// is a no-op here, which is what makes them safe to ship default-off.
    #[test]
    fn the_unconfigured_realm_is_exactly_one_database_in_every_accessor() {
        // Exactly what `from_env` produces when LYRACORE_SHARD_MAP / _REALM_CORE / _REGION_SHARDS
        // are all unset: an empty rule text and neither overlay configured.
        let m = ShardMap::parse("lyracore", "")
            .with_realm_core(None)
            .with_region_shards(None);

        assert_eq!(m.default_db(), "lyracore");
        assert_eq!(m.shards(), vec!["lyracore".to_string()]);
        assert_eq!(m.databases(), vec!["lyracore".to_string()]);
        assert_eq!(m.region_shards(), Vec::<String>::new());
        assert_eq!(
            m.auth_db(|_| true),
            Ok("lyracore"),
            "unsharded, auth is served from the one database — there is no realm-core to consult"
        );
        assert_eq!(
            m.auth_db(|_| false),
            Ok("lyracore"),
            "and with no realm-core configured, connectivity of OTHER databases cannot make auth \
             fail closed"
        );

        // Routing is the identity function over the whole address space.
        for map_id in [
            0u32,
            1,
            30,
            36,
            43,
            47,
            70,
            90,
            109,
            129,
            189,
            209,
            389,
            4_000_000_000,
        ] {
            for instance_id in [0u64, 1, 7, 63, 64, 65, 1_000_000, u64::MAX] {
                assert_eq!(
                    m.resolve(map_id, instance_id),
                    "lyracore",
                    "map {map_id}/instance {instance_id} routed off the one database"
                );
            }
            assert_eq!(
                m.instance_pool(map_id),
                vec!["lyracore"],
                "map {map_id}'s instance pool must be the one database"
            );
        }

        // And the two overlays are inert rather than merely quiet.
        assert_eq!(
            m.check_instance_hosting(|_| Some(true), |_| true),
            InstanceHosting::Consistent,
            "a single-database realm hosting its own instances has nothing to report"
        );
        assert_eq!(
            resolve_region_shard(
                &lyracore_shared::region::RegionMap::default(),
                &[],
                0,
                0,
                -9000.0,
                100.0,
                |_| true
            ),
            None,
            "with no regions defined and no assignments, region routing must be a strict no-op"
        );
    }

    /// The rule text is split on `\n`, `,` AND `;`, so an operator may write the map on one line, on
    /// several, or with semicolons — and `docs/danger-zones.md`'s stack recipe uses one form while a
    /// `LYRACORE_SHARD_MAP_FILE` naturally uses another.
    ///
    /// All three must produce an IDENTICAL map. A separator that quietly failed to split would fold
    /// several rules into one malformed rule, which `parse` then drops — so a whole realm's routing
    /// would silently collapse to the default database with only a log line to show for it.
    #[test]
    fn every_separator_spelling_of_the_same_map_parses_identically() {
        let rules = ["1:*=world-1", "36:0=inst-a", "36:*=inst-b", "*:*=world-2"];
        let spellings = [
            rules.join(","),
            rules.join(";"),
            rules.join("\n"),
            // Mixed, with the whitespace and blank lines a hand-edited file accumulates.
            format!(
                "  {}  ;\n\n  {} ,\n{}\n\n;  {}  ",
                rules[0], rules[1], rules[2], rules[3]
            ),
            // With comments, which are stripped per-rule at the first `#`.
            format!(
                "# the realm\n{} # eastern kingdoms\n{},{}\n{}",
                rules[0], rules[1], rules[2], rules[3]
            ),
        ];

        let canonical = ShardMap::parse("core", &spellings[0]);
        assert_eq!(
            canonical.shards(),
            vec![
                "core".to_string(),
                "world-1".to_string(),
                "inst-a".to_string(),
                "inst-b".to_string(),
                "world-2".to_string()
            ],
            "precondition: the comma spelling parses all four rules — otherwise this test compares \
             two equally-broken maps and proves nothing"
        );

        for (i, text) in spellings.iter().enumerate().skip(1) {
            let m = ShardMap::parse("core", text);
            assert_eq!(
                m.shards(),
                canonical.shards(),
                "spelling {i} ({text:?}) parsed to a different set of databases"
            );
            for map_id in [0u32, 1, 36, 189] {
                for instance_id in [0u64, 1, 2, 64, 65] {
                    assert_eq!(
                        m.resolve(map_id, instance_id),
                        canonical.resolve(map_id, instance_id),
                        "spelling {i} routes map {map_id}/instance {instance_id} differently"
                    );
                }
            }
        }
    }

    /// Every malformed rule shape, one per case rather than the existing test's single mixed blob —
    /// so a parser change that started ACCEPTING one of them names which one.
    ///
    /// The rule that matters is uniform: a rule the parser cannot fully understand is DROPPED, never
    /// half-applied. A half-applied rule (say, a bad bucket treated as a wildcard) would route a
    /// whole map's instances somewhere the operator never wrote, and the log line would describe a
    /// rule that was accepted.
    #[test]
    fn each_malformed_rule_shape_is_dropped_whole_and_never_half_applied() {
        let garbage = [
            ("no separator at all", "nonsense"),
            ("empty database", "1:0="),
            ("whitespace-only database", "1:0=   "),
            ("no location", "=db"),
            ("non-numeric map id", "x:1=db"),
            ("non-numeric bucket", "1:x=db"),
            ("negative map id", "-1:0=db"),
            ("bucket outside the modulus", "1:99=db"),
            ("map id past u32", "4294967296:0=db"),
            ("empty rule", ""),
            ("comment only", "# just a note"),
        ];
        for (what, rule) in garbage {
            let m = ShardMap::parse("core", rule);
            assert_eq!(
                m.shards(),
                vec!["core".to_string()],
                "the {what} rule {rule:?} was accepted and added a database — a rule the parser \
                 cannot fully read must be dropped, not partly applied"
            );
            for map_id in [0u32, 1, 36] {
                for instance_id in [0u64, 1, 99] {
                    assert_eq!(
                        m.resolve(map_id, instance_id),
                        "core",
                        "the {what} rule {rule:?} rerouted map {map_id}/instance {instance_id}"
                    );
                }
            }
        }
    }

    /// PROPERTY (#223): `LYRACORE_SHARD_MAP` is operator input, read at startup, and a mistake in it
    /// fails silently into a working-looking realm. The parser's whole job is therefore to
    /// DEGRADE — never to abort the process, and never to invent a destination.
    ///
    /// Two invariants over generated rule text:
    ///
    ///   * `parse` returns for any input at all (no panic — an aborted parse is a gateway that will
    ///     not start, on a config change made at 3am);
    ///   * whatever it produces, `resolve` only ever answers with a database that appears in
    ///     `databases()`. A resolver that could name an unlisted database would route players to a
    ///     handle the gateway never connected, which surfaces as a mid-session hang rather than a
    ///     startup error.
    ///
    /// Deterministic: a fixed seed, replayed identically every run.
    #[test]
    fn any_operator_shard_map_text_degrades_and_never_routes_off_its_own_database_list() {
        use crate::codec::property_tests::Rng;
        let mut rng = Rng::new(0x5348_4152_4400);
        // Fragments that individually look plausible, so the generator produces near-miss configs
        // (the realistic typo) as well as noise.
        let pieces = [
            "1",
            "36",
            "*",
            "x",
            "-1",
            "0",
            "99",
            "4294967296",
            ":",
            "=",
            ",",
            ";",
            "\n",
            "#",
            "world-1",
            "core",
            " ",
            "",
            "::",
            "==",
            "1:",
            ":0",
            "=db",
        ];

        for _ in 0..3_000 {
            let mut text = String::new();
            for _ in 0..rng.below(12) {
                text.push_str(pieces[rng.below(pieces.len())]);
            }
            let m = ShardMap::parse("core", &text);
            let known = m.databases();
            assert!(
                known.contains(&"core".to_string()),
                "the default database must always be in the list; text was {text:?}"
            );
            for map_id in [0u32, 1, 36, 4_000_000_000] {
                for instance_id in [0u64, 1, 63, u64::MAX] {
                    let db = m.resolve(map_id, instance_id).to_string();
                    assert!(
                        known.contains(&db),
                        "shard map {text:?} routed map {map_id}/instance {instance_id} to {db:?}, \
                         which is not in its own database list {known:?} — the gateway never \
                         connects to it, so this is a mid-session hang rather than a startup error"
                    );
                }
            }
        }
    }

    #[test]
    fn malformed_rules_are_dropped_and_the_map_degrades_to_the_default() {
        // Every one of these is garbage; none may reroute a player. Comments + blanks are skipped.
        let m = ShardMap::parse(
            "world",
            "# a comment\n\n nonsense , 1:=  , =db , x:1=db , 1:99=db , 1:*=good",
        );
        assert_eq!(m.resolve(0, 0), "world");
        assert_eq!(m.resolve(1, 0), "good", "the one VALID rule still applies");
        assert_eq!(m.databases(), vec!["world".to_string(), "good".to_string()]);
    }

    // ==========================================================================================
    //  Region shards (#72): connected, but reachable ONLY through the region overlay.
    // ==========================================================================================

    #[test]
    fn a_region_shard_is_connected_but_the_shard_map_never_routes_to_it() {
        // The whole point. Before this, a database regions could name had to be some rule's `db`,
        // and every rule routes a whole map to it — so a map-0 region shard had to be `0:*=<db>`,
        // which hands it every map-0 location the seam menu does NOT cover (all of Eastern Kingdoms
        // outside the drawn regions) on a database holding none of that content.
        let m = ShardMap::parse("core", "1:*=world-1").with_region_shards(Some("world-2"));
        assert_eq!(
            m.shards(),
            vec![
                "core".to_string(),
                "world-1".to_string(),
                "world-2".to_string()
            ],
            "a region shard IS a world shard: it holds characters, so location probes must walk it"
        );
        // …and yet nothing routes there by map.
        for map_id in [0u32, 1, 36, 389] {
            assert_ne!(
                m.resolve(map_id, 0),
                "world-2",
                "map {map_id} must not resolve to a region shard — regions are its only way in"
            );
        }
        assert_eq!(
            m.resolve(0, 0),
            "core",
            "unmatched map 0 still falls to the default"
        );
    }

    #[test]
    fn the_default_stays_first_so_the_seam_menu_is_still_read_from_it() {
        // `Coordinator::region_db_for` reads the seam menu from `world_shards().first()`. If a
        // region shard ever sorted ahead of the default, the menu would be read from a database that
        // has none — and `resolve_region_shard` answers None on an empty menu, so region routing
        // would silently switch itself off rather than fail.
        let m = ShardMap::parse("core", "1:*=world-1").with_region_shards(Some("aaa-sorts-first"));
        assert_eq!(m.shards().first().map(String::as_str), Some("core"));
        assert_eq!(m.databases().first().map(String::as_str), Some("core"));
    }

    #[test]
    fn region_shards_collapse_to_nothing_in_every_degenerate_config() {
        // Unset / blank / comments-only — the ordinary realm names none of these, and must be
        // byte-identical to before.
        let base = ShardMap::parse("core", "1:*=world-1");
        for input in [
            None,
            Some(""),
            Some("   "),
            Some("# just a comment"),
            Some(",,;;"),
        ] {
            let m = base.clone().with_region_shards(input);
            assert!(
                m.region_shards().is_empty(),
                "{input:?} must configure nothing"
            );
            assert_eq!(
                m.shards(),
                base.shards(),
                "{input:?} must not change the connected set"
            );
        }
        // Naming the DEFAULT, or a database a rule already routes to, is a no-op rather than a
        // duplicate — `shards()` is what a character-location probe walks, so a repeat would make it
        // probe the same database twice.
        for dup in ["core", "world-1"] {
            let m = base.clone().with_region_shards(Some(dup));
            assert_eq!(m.shards(), base.shards(), "{dup} is already connected");
        }
        // Listed twice in one value.
        let m = base.clone().with_region_shards(Some("world-2, world-2"));
        assert_eq!(m.shards().iter().filter(|d| *d == "world-2").count(), 1);
    }

    #[test]
    fn realm_core_is_refused_as_a_region_shard() {
        // realm-core owns accounts, not characters. An assignment naming it would route a login onto
        // a database with nothing of the player's in it — the exact hazard `region_db_for`'s
        // "WORLD SHARDS only" comment calls out. Refuse it at config time, not at routing time.
        let m = ShardMap::parse("core", "1:*=world-1")
            .with_realm_core(Some("lyracore-realm"))
            .with_region_shards(Some("lyracore-realm, world-2"));
        assert_eq!(m.region_shards(), ["world-2".to_string()]);
        assert!(
            !m.shards().contains(&"lyracore-realm".to_string()),
            "realm-core must never be walked as a world shard"
        );
        assert!(
            m.databases().contains(&"lyracore-realm".to_string()),
            "…but the gateway still CONNECTS to it, for auth"
        );
    }

    #[test]
    fn a_region_assignment_naming_a_region_shard_now_resolves() {
        // The end-to-end reason this exists: `resolve_region_shard` refuses an assignment whose
        // shard is not connected, and `region_db_for` supplies `world_shards()` as that set. So this
        // assertion is the one that was previously impossible to satisfy without hijacking a map.
        let m = ShardMap::parse("core", "1:*=world-1").with_region_shards(Some("world-2"));
        let connected = |db: &str| m.shards().iter().any(|d| d == db);
        let menu = lyracore_shared::region::RegionMap::parse("0:2=530..570,300..410").0;
        let assigned = |shard: &str| {
            vec![RegionAssignment {
                map_id: 0,
                region_id: 2,
                shard: shard.to_string(),
                epoch: 1,
            }]
        };
        // A point inside region 2 — cell (533, 341)-ish, i.e. Goldshire.
        let (x, y) = (-9583.0f32, 30.0f32);
        assert_eq!(
            menu.region_at(0, x, y),
            2,
            "the fixture point must be inside region 2"
        );
        assert_eq!(
            resolve_region_shard(&menu, &assigned("world-2"), 0, 0, x, y, connected),
            Some("world-2"),
            "a region shard is a legal assignment target"
        );
        // Control: an unconnected name still degrades to the shard map, unchanged by this feature.
        assert_eq!(
            resolve_region_shard(&menu, &assigned("world-9"), 0, 0, x, y, connected),
            None,
            "an assignment naming a database the gateway never reached routes nobody"
        );
    }

    #[test]
    fn adding_a_region_shard_cannot_change_the_instance_hosting_verdict() {
        // `check_instance_hosting` walks DUNGEON_MAPS through `resolve`, and no rule names a region
        // shard — so adding one must not be able to turn a correct realm into a warning, which is
        // the kind of startup regression that gets a real message filtered out.
        let base = ShardMap::parse("core", "36:*=instances");
        let with = base.clone().with_region_shards(Some("world-2"));
        let hosts = |db: &str| Some(db == "instances");
        assert_eq!(
            base.check_instance_hosting(hosts, |_| true),
            with.check_instance_hosting(hosts, |_| true)
        );
        assert_eq!(
            with.check_instance_hosting(hosts, |_| true),
            InstanceHosting::Consistent
        );
    }
}
