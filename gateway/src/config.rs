//! Gateway configuration. All operational, none of it game state.

use crate::accept::BlockingTaskCapacity;

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
    /// This gateway PROCESS's own identity (issue #308) — `LYRACORE_GATEWAY_ID`, default
    /// `<hostname>:<world_bind port>`. Exists so a load sample this process writes onto realm-core
    /// (`load_sample::sample_and_record` → `record_shard_load`) can be told apart from another
    /// gateway process's sample for the SAME shard, instead of one clobbering the other
    /// (`docs/region-sharding.md`'s "Load sampling" section). Also the natural place to hang a
    /// per-process label on the other per-process health signals (`MOTIONSTAT`/`AOISTAT`) later —
    /// not done here, out of this issue's scope.
    pub gateway_id: String,
    /// Shared non-waiting gate for blocking logon and World Session tasks. Its size mirrors the
    /// Tokio blocking-pool ceiling configured before this value is built.
    pub(crate) blocking_task_capacity: BlockingTaskCapacity,
}

/// The hostname half of the default `gateway_id` — `HOSTNAME` if set (containers commonly export
/// it), else `/proc/sys/kernel/hostname` (Linux, no extra dependency), else a fixed fallback so a
/// gateway always starts rather than failing to compute its own identity.
fn default_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

impl GatewayConfig {
    pub fn from_env() -> Self {
        let get = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        let world_bind = get("LYRACORE_WORLD_BIND", "0.0.0.0:8085");
        let default_gateway_id = format!(
            "{}:{}",
            default_hostname(),
            world_bind.rsplit(':').next().unwrap_or(&world_bind)
        );
        Self {
            logon_bind: get("LYRACORE_LOGON_BIND", "0.0.0.0:3724"),
            world_bind: world_bind.clone(),
            stdb_uri: get("LYRACORE_SPACETIMEDB_URL", "http://127.0.0.1:3000"),
            module_name: get("LYRACORE_DATABASE", "lyracore"),
            coordinator_token: std::env::var("LYRACORE_COORDINATOR_TOKEN").ok(),
            gateway_id: get("LYRACORE_GATEWAY_ID", &default_gateway_id),
            blocking_task_capacity: BlockingTaskCapacity::new(max_blocking_threads()),
        }
    }
}

// ===============================================================================================
//  Shard map — gateway multi-shard routing: the shard map, per-shard coordinators, login routed
//  by character location.
// ===============================================================================================

/// Instances of one map are spread over a shard pool by `instance_id % INSTANCE_BUCKETS`, so a rule
/// can name a slice of a map's instances instead of all of them.
///
/// The shipped topology only ever uses map-level wildcards (`<map_id>:*=<db>`); per-bucket rules
/// (splitting one map's instances across several databases) are documented and tested below but not
/// deployed — that is the upgrade path, not a live feature to go hunting for elsewhere.
// Deliberate simplification: a hardcoded modulus IS the whole bucketing scheme. Ceiling — you
// cannot resize the pool without every gateway agreeing on the same number, and rebalancing means
// restarting them. Upgrade path: realm-core owns the assignment table and
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
/// session's reducer calls + AOI scoping to the shard that owns their character's location.
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
    /// `LYRACORE_REALM_CORE`: the database that owns accounts, sessions, and the character→shard
    /// index. `None` = unconfigured, which is *exactly* today — auth lives on the default database.
    realm_core: Option<String>,
}

impl ShardMap {
    /// Read the shard map from `LYRACORE_SHARD_MAP` (or the file named by `LYRACORE_SHARD_MAP_FILE`), defaulting
    /// every unmatched location to `default_db` (the `LYRACORE_DATABASE` database). Unset → single entry.
    /// `LYRACORE_REALM_CORE` names the realm-core database; unset → auth stays on `default_db`.
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
    }

    /// Point auth (accounts / sessions / the character→shard index) at a separate `realm-core`
    /// database. Blank, absent, or *equal to the default database* all mean "unconfigured" —
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
    //  The instance POOL — the instance-shard pool (instance lifecycle at scale).
    // ==========================================================================================

    /// Every database an instance of `map_id` can be routed to — the map's **instance pool**,
    /// deduped, in bucket order.
    ///
    /// Note: this is [`resolve`](Self::resolve) read the other way round, and the instance-shard
    /// pool adds **no new assignment mechanism at all**. The shard map's `<map_id>:<bucket>=<db>`
    /// rule already IS the pool hook:
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
    /// config alone, and existing runs are undisturbed" true rather than aspirational.
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
    /// index yet, so this is the only durable evidence either database has — and it is
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
    /// there is nothing to be sticky about). Closing that needs the instance→shard index —
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
    /// ⚠ **Default stays FIRST.** `resolve_home_shard`'s fallback probe walks the connected set in
    /// this order and must try the default database before any rule shard.
    pub fn shards(&self) -> Vec<String> {
        let mut dbs = vec![self.default_db.clone()];
        for r in &self.rules {
            if !dbs.contains(&r.db) {
                dbs.push(r.db.clone());
            }
        }
        dbs
    }

    /// **Will this realm's dungeon populations actually get spawned, exactly once, on the database
    /// that runs them?**
    ///
    /// Two facts have to agree and until now no single process held both: the ROUTING is this
    /// gateway's shard map, and `hosts_instances` is a column on each DATABASE. The gateway is the
    /// layer that can hold both — it subscribes `game_config` on every connection in the set (see
    /// `stdb/connection.rs`), so `hosts_instances(db)` below is a real read, not a guess. The MODULE
    /// cannot own this check even in principle: `create_instance_with_id` knows its own flag but has
    /// no idea whether any *other* database exists, let alone whether the shard map points dungeon
    /// traffic at one — and the elastic-sharding spec forbids it from learning
    /// (`no_module_game_logic_reads_a_shard_id`).
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
    ///   population. Measured as 8 consecutive empty instances and 4 bot-test failures
    ///   that read as gameplay regressions. FATAL — a realm whose dungeons are all empty rooms should
    ///   not come up quietly.
    /// - [`InstanceHosting::LoadNotMoved`] — the DEFAULT database hosts populations while a dungeon
    ///   map's instances are owned elsewhere. The run works; it just spawns ~207 creatures + 28
    ///   gameobject copies on the world writer and evicts them again after the transfer, i.e. exactly
    ///   the load Phase A exists to remove. A WARNING, and — unlike the old reminder, which fired on
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
                     a completely empty instance. The open world is unaffected and the \
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
                     completely empty instance and no other database will ever populate it \
                     (measured: 8 consecutive instances with 0 entities, 4 bot tests failing as if \
                     gameplay had regressed). Fix ONE of these and restart: (a) let that database \
                     host its own dungeons — `spacetime sql {owner} \"UPDATE game_config SET \
                     hosts_instances = true WHERE id = 0\"`; or (b) configure LYRACORE_SHARD_MAP so map \
                     {map_id}'s instances are routed to a database that DOES host populations, \
                     e.g. LYRACORE_SHARD_MAP=\"{map_id}:*=<instances-database>\". Note that EVERY \
                     instance bucket is checked, so a pool whose members disagree about the flag \
                     lands here on the one member that forgot it, not on all eight."
                ));
            }
            // The inverse: the portal runs on the default database, so if THAT
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
                     {0} \"UPDATE game_config SET hosts_instances = false WHERE id = 0\"`.",
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
/// populations actually be spawned, and on the right database?
///
/// Exactly one variant means "correctly configured", and it carries no message: the ordinary
/// single-database realm and a correctly-configured Phase A deployment are both SILENT. That is
/// deliberate — the reminder this check replaced warned on every sharded startup whether or not
/// anything was wrong, the operator confirmed it firing on a correct gateway, and a warning that
/// always fires gets filtered, which defeats the one startup it needed to catch.
#[derive(Clone, Debug, PartialEq)]
pub enum InstanceHosting {
    /// Every dungeon map's instances land on a database that hosts populations, and no database
    /// spawns a population it will not run. Nothing to report.
    Consistent,
    /// The default database will spawn dungeon populations it does not run: they are evicted
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
    /// NOBODY will spawn the population: every dungeon is created empty, and no amount of
    /// waiting fixes it because the owning database is CONNECTED and says it hosts nothing. Fatal.
    NoHost(String),
}

impl InstanceHosting {
    /// Apply the verdict at startup: log the warning, or refuse to start.
    ///
    /// `Err` is the whole point here — an empty-dungeon realm must not come up quietly, because
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
/// connected world shards. Pure, so the whole self-heal rule is unit-testable without a
/// node — deliberately, because an earlier shard-routing change shipped a bug hidden by a mock that
/// hardcoded exactly the answer under test.
///
/// - `hint` — the realm-core `game_character_shard` entry, or `None` when the index has no idea.
/// - `connected` — the world shards that actually connected, DEFAULT FIRST (probe order).
/// - `probe(db)` — "does `db` hold this character, and at what location" (the coordinator's
///   `character_location` against that shard's cache; an in-memory lookup, not a round trip).
///
/// The rule: **the index is a hint, the row is the truth.** A hint is accepted only when the shard it
/// names actually holds the character; anything else falls through to walking the shards, and the
/// answer is marked for write-back. With ONE connected shard this degenerates to "probe the default
/// shard and resolve its location through the map" — i.e. the original shard-routing behavior, byte
/// for byte.
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
    //    location the owning shard reports — exactly how the shard map routes, just sourced by
    //    probe rather than by assuming the default shard still holds the row.
    connected.iter().find_map(|db| probe(db)).map(settle)
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

/// The address the realm list advertises, overriding the `game_realm` row.
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

/// What the realm list advertises: the override when set, the `game_realm` row otherwise. One
/// implementation, so the realm list and the startup check cannot disagree about what is advertised.
pub fn advertised_realm_address_or(row_address: String) -> String {
    advertised_realm_address().unwrap_or(row_address)
}

/// Loopback advertised while the world listener is reachable from elsewhere — the one combination
/// that is wrong whatever the surrounding network. NAT and firewalls are unknowable from here, and
/// unparseable input reads as reachable because this only drives a warning.
pub fn advertised_address_is_unreachable(advertised: &str, world_bind: &str) -> bool {
    let bind = host_of(world_bind);
    // An empty or absent bind proves nothing about who can reach this listener, so it is not
    // grounds to report the advertisement.
    is_loopback_host(host_of(advertised)) && !bind.is_empty() && !is_loopback_host(bind)
}

/// The host half of a `host:port`, with IPv6 brackets stripped. A value with no port reads whole.
fn host_of(address: &str) -> &str {
    let host = match address.rsplit_once(':') {
        // `[::1]:8085` — the split is the port separator only because the host is bracketed.
        Some((host, _)) if !host.is_empty() => host,
        _ => address,
    };
    host.trim_start_matches('[').trim_end_matches(']')
}

fn is_loopback_host(host: &str) -> bool {
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        // `127.example.com` is a hostname that merely starts like one, not a loopback address.
        Err(_) => host.eq_ignore_ascii_case("localhost"),
    }
}

/// Area-of-Interest gate (scaling): the per-player `game_world_entity` subscription is SCOPED to a
/// grid-cell box (before the shared-connection model, a re-subscribing per-player subscription; now
/// an in-process cell index — `stdb::world_index`), instead of the global `SELECT *`. Default ON;
/// the 5×5 50-yd box ≈ vanilla view distance. `LYRACORE_AOI=0` is the escape hatch back to the
/// global subscription.
pub fn aoi_enabled() -> bool {
    std::env::var("LYRACORE_AOI").map_or(true, |v| v != "0")
}

/// Total reducer-call connections per shard (the call-pipe pool). Default 4 — sized from the
/// measured ~1000-seats-per-pipe wall; `LYRACORE_CALL_PIPES=1` restores the single-connection
/// call path. Values are clamped to [1, 16].
pub fn call_pipes() -> usize {
    std::env::var("LYRACORE_CALL_PIPES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(4)
        .clamp(1, 16)
}

/// The per-account OWNER identity bound by `establish_session` — deterministic and derived, so
/// logins build NO per-player connection just to mint one. Never node-issued (real identities
/// carry the node's c200 prefix; this is a tagged literal), unique per account. The module stamps
/// it as row owner exactly like a real bound identity (`gw_player_login`'s fail-closed check is
/// satisfied); no client connection ever presents it, so the GATEWAY predicates are the
/// visibility layer for owned rows.
pub fn synthetic_owner_identity(account_id: u64) -> [u8; 32] {
    let mut id = [0u8; 32];
    id[..8].copy_from_slice(b"LYRAGWID");
    id[24..].copy_from_slice(&account_id.to_le_bytes());
    id
}


/// A crash-diagnosis probe for the SMSG_COMPRESSED_MOVES corruption at crowd scale:
/// `LYRACORE_WRITER_TRACE=1` turns on a per-session black-box ring inside `spawn_writer` — the
/// last 32 (opcode, declared size, rolling checksum) triples for the exact bytes handed to the
/// socket, dumped to `/tmp/gw-writer-crash/<account>.txt` when a session's writer loop ends on a
/// write error. Default OFF: the ring costs a checksum pass over every outbound body, which is not
/// a cost production should pay to stand still. Mirrors the wire harness's own crash ring
/// (`/tmp/wire-crash/`) so the two can be diffed frame-for-frame at a corruption offset.
pub fn writer_trace_enabled() -> bool {
    std::env::var("LYRACORE_WRITER_TRACE").is_ok_and(|v| v == "1")
}

/// Maximum established World Sessions. Unset or zero means unlimited; malformed values fail startup.
pub fn max_sessions() -> anyhow::Result<usize> {
    admission_limit("LYRACORE_MAX_SESSIONS")
}

/// Queued admissions per tick. Unset or zero means unlimited; malformed values fail startup.
pub fn admit_concurrency() -> anyhow::Result<usize> {
    admission_limit("LYRACORE_ADMIT_CONCURRENCY")
}

fn admission_limit(name: &str) -> anyhow::Result<usize> {
    parse_admission_limit(name, std::env::var_os(name).as_deref())
}

fn parse_admission_limit(name: &str, raw: Option<&std::ffi::OsStr>) -> anyhow::Result<usize> {
    let Some(raw) = raw else {
        return Ok(0);
    };
    raw.to_str()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "invalid {name}={raw:?}: expected a decimal integer from 0 to {}, with 0 for unlimited",
                usize::MAX
            )
        })
}

/// tokio's own default `max_blocking_threads`, restated here so an unset
/// `LYRACORE_MAX_BLOCKING_THREADS` is today's behaviour byte for byte.
///
/// This constant is a *mirror* of a tokio internal (`runtime::Builder::new_multi_thread` seeds
/// `max_blocking_threads: 512`), not a value tokio exposes — there is no accessor to read it back.
/// If a tokio bump moves it, this mirror is what keeps "unset == before" true for THIS gateway,
/// which is the property that matters; it is not trying to track tokio.
pub const DEFAULT_MAX_BLOCKING_THREADS: usize = 512;

/// The ceiling on tokio's blocking pool, which is the real ceiling on concurrent players.
///
/// Both accept loops hand admitted sockets to `spawn_blocking` (the wire codecs in
/// `wow_login_messages` / `wow_world_messages` are blocking `std::io`), and a World Session holds
/// its blocking thread for the session's entire life. So `max_blocking_threads` is both the pool
/// ceiling and the size of the listeners' shared, non-waiting [`BlockingTaskCapacity`].
///
/// It was previously tokio's inherited default of 512, configured by nothing and reported by
/// nothing. Before the admission gate, a full pool did not refuse: `spawn_blocking` queued the
/// socket without a bound. The shared capacity now rejects excess sockets before task submission.
/// Measured 2026-08-07 on an 8-core box: 600 clients offered seated **477** at 512 and **535** at
/// 4096.
///
/// Default 512 preserves the former pool size. A malformed or zero value falls back to this finite
/// default. Zero blocking threads would leave the Gateway unable to serve accepted sockets.
pub fn max_blocking_threads() -> usize {
    max_blocking_threads_from_env(
        std::env::var("LYRACORE_MAX_BLOCKING_THREADS")
            .ok()
            .as_deref(),
    )
}

/// The pure half of [`max_blocking_threads`] (house pattern: [`realm_address_override`]) — the
/// variable is process-global, so the parse is exercised through this rather than by mutating the
/// test process's own environment.
pub fn max_blocking_threads_from_env(raw: Option<&str>) -> usize {
    raw.map(str::trim)
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_BLOCKING_THREADS)
}

#[cfg(test)]
mod max_blocking_threads_tests {
    use super::*;

    /// The safety property of this knob: **unset changes nothing**. Every existing deployment
    /// keeps the exact 512-thread pool it has been running on.
    #[test]
    fn unset_is_tokios_own_default() {
        assert_eq!(max_blocking_threads_from_env(None), 512);
        assert_eq!(
            max_blocking_threads_from_env(None),
            DEFAULT_MAX_BLOCKING_THREADS
        );
    }

    #[test]
    fn a_plain_number_is_taken_as_written() {
        assert_eq!(max_blocking_threads_from_env(Some("4096")), 4096);
        assert_eq!(max_blocking_threads_from_env(Some("1")), 1);
        assert_eq!(max_blocking_threads_from_env(Some(" 2048 ")), 2048);
    }

    /// Zero is not "unlimited" the way it is for `max_sessions` — a zero-thread blocking pool is a
    /// gateway that accepts every socket and services none of them, silently, which is precisely
    /// the failure shape this exists to make impossible. Fall back rather than obey.
    #[test]
    fn zero_falls_back_rather_than_seating_nobody() {
        assert_eq!(
            max_blocking_threads_from_env(Some("0")),
            DEFAULT_MAX_BLOCKING_THREADS
        );
    }

    /// A malformed value falls back to the finite default.
    /// `-1` matters specifically: it is the shape an operator reaches for to mean "no limit", and
    /// it does not parse as `usize`.
    #[test]
    fn a_malformed_value_falls_back_to_the_default() {
        for junk in ["", "   ", "-1", "lots", "4096x", "4_096", "1e3", "512.0"] {
            assert_eq!(
                max_blocking_threads_from_env(Some(junk)),
                DEFAULT_MAX_BLOCKING_THREADS,
                "{junk:?} should have fallen back to the default"
            );
        }
    }
}

#[cfg(test)]
mod realm_address_tests {
    use super::*;

    /// The whole safety property of this override: **unset changes nothing**. Every existing
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

    /// The reported failure: seeded loopback row, wildcard bind, every startup marker green, and no
    /// remote client can enter the world.
    #[test]
    fn advertising_loopback_while_listening_for_remote_clients_is_unreachable() {
        for bind in ["0.0.0.0:8085", "159.69.88.70:8085", "192.168.1.50:8085"] {
            assert!(
                advertised_address_is_unreachable("127.0.0.1:8085", bind),
                "loopback advertised against {bind} must be reported"
            );
        }
    }

    /// The contributor fixture. Loopback everywhere is the supported local setup, so it must stay
    /// silent or the warning becomes noise every developer learns to skip.
    #[test]
    fn a_loopback_fixture_is_never_reported() {
        for bind in ["127.0.0.1:8085", "localhost:8085", "[::1]:8085"] {
            assert!(
                !advertised_address_is_unreachable("127.0.0.1:8085", bind),
                "the loopback fixture bound to {bind} must stay silent"
            );
        }
    }

    #[test]
    fn a_routable_advertised_address_is_never_reported_whatever_the_bind() {
        for bind in ["0.0.0.0:8085", "127.0.0.1:8085", "10.0.0.5:8085"] {
            assert!(!advertised_address_is_unreachable(
                "159.69.88.70:8085",
                bind
            ));
            assert!(!advertised_address_is_unreachable(
                "realm.example.com:8085",
                bind
            ));
        }
    }

    /// Every spelling of loopback the row or the bind can carry — a check that only knows
    /// `127.0.0.1` would miss the same fault written another way.
    #[test]
    fn every_spelling_of_loopback_counts_as_loopback() {
        for advertised in [
            "127.0.0.1:8085",
            "127.1.2.3:8085",
            "localhost:8085",
            "LOCALHOST:8085",
            "[::1]:8085",
        ] {
            assert!(
                advertised_address_is_unreachable(advertised, "0.0.0.0:8085"),
                "{advertised} is loopback"
            );
        }
    }

    /// `127.0.0.1` is loopback; a hostname that merely starts with those characters is not.
    #[test]
    fn a_host_that_only_looks_like_loopback_is_left_alone() {
        assert!(!advertised_address_is_unreachable(
            "127.example.com:8085",
            "0.0.0.0:8085"
        ));
        assert!(!advertised_address_is_unreachable(
            "localhost.example.com:8085",
            "0.0.0.0:8085"
        ));
    }

    /// This drives a log line, nothing more. Anything unparseable reads as reachable rather than
    /// panicking the gateway on the way up.
    #[test]
    fn malformed_input_stays_silent_instead_of_panicking() {
        for (advertised, bind) in [
            ("", ""),
            ("", "0.0.0.0:8085"),
            ("127.0.0.1:8085", ""),
            (":", ":"),
            ("::::", "0.0.0.0:8085"),
            ("no-port", "also-no-port"),
            ("127.0.0.1:notaport", "0.0.0.0:8085"),
        ] {
            let _ = advertised_address_is_unreachable(advertised, bind);
        }
        // The one malformed case with a definite answer: a loopback host and an unparseable bind
        // must not be reported, since nothing proves the listener is remote-reachable.
        assert!(!advertised_address_is_unreachable("127.0.0.1:8085", ""));
    }
}

#[cfg(test)]
mod shard_map_tests {
    use super::*;

    #[test]
    fn an_unconfigured_map_is_a_single_entry_that_never_routes() {
        // The safety property: absent config → every location resolves to the one database,
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
    //  Instance hosting: the detector that replaced the old always-firing reminder.
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

    /// **The damaging direction.** No shard map + `hosts_instances = false` = every
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
    /// half the old reminder promised and this detector must not spend.
    #[test]
    fn the_unconfigured_single_database_realm_is_silent() {
        let single = ShardMap::parse("lyracore", "");
        let up = |d: &str| d == "lyracore";
        assert_eq!(
            single.check_instance_hosting(flags(&[("lyracore", true)]), up),
            InstanceHosting::Consistent,
            "a single database that hosts its own dungeons is correctly configured — saying \
             ANYTHING here is noise the detector was built to remove"
        );
        assert_eq!(
            single.check_instance_hosting(|_| None, up),
            InstanceHosting::Consistent,
            "a database with no game_config row reads as HOSTING (the module's own unwrap_or(true)) \
             — an unreadable config must never fabricate a fatal"
        );
    }

    /// **The wasteful direction, now actually detected.** A shard map that routes Deadmines away
    /// while the world shard still hosts populations warns; the same shard map with the flag
    /// correctly `false` says NOTHING. The second half fixes the old reminder's always-fires noise —
    /// the operator confirmed it firing on a correctly-configured gateway.
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
             warning gets filtered, which is exactly how the empty-dungeon failure went unnoticed"
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
    /// of the empty-dungeon failure to diagnose from the outside, and the reason the check walks
    /// every bucket instead of asking about one representative instance id.
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
    /// This is the availability half of the empty-dungeon failure, and it cuts the other way from
    /// the rest of the file. The operator's config is CORRECT here; a different process is down,
    /// transiently — a node that has not finished coming up, a gateway launched a few seconds
    /// early in a restart, an instances
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
    /// Every assertion here exists because a mutation satisfied its predecessor while reintroducing
    /// the empty-dungeon failure with the whole 438-test suite green:
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
             empty, silently"
        );
        // Defeat 2: the check must be wired to the NAMED reader, not to an inline closure. Otherwise
        // the real read can sit next to it as a dead binding and satisfy the assertions below.
        assert!(
            body.contains("check_instance_hosting(hosts_instances,"),
            "the hosting check is not being passed the `hosts_instances` reader by name — an inline \
             closure (`|_| Some(true)`) keeps every other assertion here true, leaves the real \
             reader as a dead binding, and silently reintroduces the empty-dungeon failure"
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
             fatal and reintroduces the empty-dungeon failure with every other test green: {stmt}"
        );
        // Defeat 3: comments are stripped, so this now needs the real subscription. It lives in
        // `coordinator_queries`, not in `connect`'s body, so this one assertion scans the whole file.
        assert!(
            connection_rs.contains("\"SELECT * FROM game_config\""),
            "the coordinator no longer subscribes game_config — the detector cannot read the flag \
             back and degrades to the old guess-and-warn reminder"
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
        // Adversarial review: a shard that is named by the map but failed to connect must
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
    //  The instance pool: assignment by the EXISTING bucket rule, and the stickiness that
    //  keeps a live run where it is when the pool grows.
    // ==========================================================================================

    /// The requirement that instance creation targets the pool via the shard map, and that the
    /// pool can grow by config alone — in one test: **the pool is the bucket rule, and nothing
    /// else.** Two pool members, ids allocated consecutively by the world shard's `auto_inc`, and
    /// the runs alternate — no new assignment machinery was added or needed.
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
        // The byte-identical-when-unset property, for the pool's two new resolvers. With no rules the
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
                     the stickiness rule when nothing is configured"
                );
            }
        }
    }

    /// **Growing the pool by config alone.** The operator adds `instances-1` by editing
    /// `LYRACORE_SHARD_MAP` alone. New runs land on
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
    //  Realm-core: the auth database, and the character→shard index's self-heal rule.
    // ==========================================================================================

    #[test]
    fn realm_core_unconfigured_keeps_auth_on_the_default_database() {
        // The safety property (mirroring the shard map's): with no LYRACORE_REALM_CORE the auth
        // database IS the world database, so every account/session read and write goes exactly
        // where it goes today, and no second coordinator connection is opened.
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
        // plausible deliberate "hub == world" single-realm posture (the shared-auth-hub design's
        // zero-migration default). Both must collapse to today rather than opening a second
        // connection to the same database.
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
        // shard where the character is, resolve that through the map" verbatim.
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
        // The index says the character is in a Deadmines instance; it actually walked out and
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
    //  The SINGLE-DATABASE realm, and the parser's totality
    // ==========================================================================================

    /// The configuration nearly every deployment runs, and exactly what you silently get when a
    /// topology env var is omitted: nothing configured at all.
    ///
    /// Individual accessors are pinned elsewhere; this asserts the whole surface AT ONCE, because
    /// the failure mode is not "one accessor is wrong" — it is "the gateway looks like it is running
    /// a five-database realm and is actually running one", or the reverse. Every routing question
    /// must answer with the one database, and no accessor may invent a second name.
    ///
    /// It is also the contract every sharding feature is built against: each shard-map and
    /// instance-pool rule is a no-op here, which is what makes them safe to ship default-off.
    #[test]
    fn the_unconfigured_realm_is_exactly_one_database_in_every_accessor() {
        // Exactly what `from_env` produces when LYRACORE_SHARD_MAP / _REALM_CORE are unset: an
        // empty rule text and no realm-core configured.
        let m = ShardMap::parse("lyracore", "").with_realm_core(None);

        assert_eq!(m.default_db(), "lyracore");
        assert_eq!(m.shards(), vec!["lyracore".to_string()]);
        assert_eq!(m.databases(), vec!["lyracore".to_string()]);
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

        // And the hosting check is inert rather than merely quiet.
        assert_eq!(
            m.check_instance_hosting(|_| Some(true), |_| true),
            InstanceHosting::Consistent,
            "a single-database realm hosting its own instances has nothing to report"
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

    /// PROPERTY: `LYRACORE_SHARD_MAP` is operator input, read at startup, and a mistake in it
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
}

#[cfg(test)]
mod admission_limit_tests {
    use super::parse_admission_limit;
    use std::ffi::OsStr;

    #[test]
    fn absent_or_zero_limits_allow_unlimited_admission() {
        assert_eq!(parse_admission_limit("LIMIT", None).unwrap(), 0);
        assert_eq!(
            parse_admission_limit("LIMIT", Some(OsStr::new("0"))).unwrap(),
            0
        );
    }

    #[test]
    fn decimal_limits_preserve_the_configured_value() {
        for (raw, expected) in [("1", 1), ("1000", 1000), ("  2048 ", 2048)] {
            assert_eq!(
                parse_admission_limit("LIMIT", Some(OsStr::new(raw))).unwrap(),
                expected
            );
        }
        assert_eq!(
            parse_admission_limit("LIMIT", Some(OsStr::new(&usize::MAX.to_string()))).unwrap(),
            usize::MAX
        );
    }

    #[test]
    fn malformed_limits_do_not_become_unlimited() {
        let overflow = (usize::MAX as u128 + 1).to_string();
        for name in ["LYRACORE_MAX_SESSIONS", "LYRACORE_ADMIT_CONCURRENCY"] {
            for raw in [
                "", "  ", "-1", "+1", "1,000", "1_000", "1e3", "1.5", "many", &overflow,
            ] {
                let error = parse_admission_limit(name, Some(OsStr::new(raw)))
                    .unwrap_err()
                    .to_string();
                assert!(error.contains(name), "{error}");
                assert!(error.contains(&format!("{raw:?}")), "{error}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_limit_is_an_error() {
        use std::os::unix::ffi::OsStrExt;
        assert!(parse_admission_limit("LIMIT", Some(OsStr::from_bytes(&[0xff]))).is_err());
    }
}
