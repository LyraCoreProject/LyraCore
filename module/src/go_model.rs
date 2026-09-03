//! Module-private store for DOOR/BUTTON collision meshes. `importer --go-models` resolves each
//! DOOR/BUTTON `gameobject_template.display_id` to an M2 bounding mesh, pre-applies the MDDF axis
//! shuffle (`importer/src/go_model.rs::shuffle`), and packs the result into local-space triangles
//! via `lyracore_shared::vmap`'s per-triangle codec. This table only STORES that local-space
//! geometry — the state-gated ray merge that CONSUMES it (per-spawn world transform,
//! liveness/open-state gating against `game_gameobject`) is the `game_go_collider` registry.
//!
//! PRIVATE — no gateway binding needed (`docs/danger-zones.md` §1: "a table binding is only
//! needed if the gateway subscribes to or reads that table" — nothing outside this module does
//! yet; an operator inspects it with `spacetime sql`).

use spacetimedb::{reducer, table, ReducerContext, Table};

#[table(accessor = game_go_model)]
pub struct GoModel {
    #[primary_key]
    pub entry: u32,
    /// `gameobject_template.size` (render/collision scale), carried verbatim from the dump.
    pub scale: f32,
    /// Local-space bounding-sphere radius of the shuffled mesh — the cheap segment-reject the
    /// ray merge needs before decoding and transforming the full triangle blob.
    pub radius: f32,
    /// `lyracore_shared::vmap`-codec local-space triangles. The class tag the codec carries is
    /// unused here — a door's dynamic-ray participation is decided per `game_gameobject` row
    /// (state/liveness), not per triangle class.
    pub blob: Vec<u8>,
}

// ===========================================================================================
//  Import reducers — same packed-string convention as `vmap::import_vmap_chunks`/`_append`:
//  rows `;`, fields `,`, the blob hex LAST so `splitn` keeps it intact.
// ===========================================================================================

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex blob".to_string());
    }
    (0..s.len() / 2)
        .map(|i| {
            u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| format!("bad hex at {i}"))
        })
        .collect()
}

/// One packed row: `entry,scale,radius,blob_hex`.
fn parse_row(row: &str) -> Result<(u32, f32, f32, Vec<u8>), String> {
    let f: Vec<&str> = row.splitn(4, ',').collect();
    if f.len() != 4 {
        return Err(format!("go_model row needs 4 fields, got {}", f.len()));
    }
    let entry = f[0]
        .parse::<u32>()
        .map_err(|_| format!("bad entry: {}", f[0]))?;
    let scale = f[1]
        .parse::<f32>()
        .map_err(|_| format!("bad scale: {}", f[1]))?;
    let radius = f[2]
        .parse::<f32>()
        .map_err(|_| format!("bad radius: {}", f[2]))?;
    let blob = hex_decode(f[3])?;
    lyracore_shared::vmap::decode(&blob)
        .map_err(|err| format!("invalid go_model blob: {err:?}"))?;
    Ok((entry, scale, radius, blob))
}

/// Load one batch, REPLACING any row whose entry repeats (retry-safe: re-appending the same
/// entry with the same bytes is a no-op in effect, and a corrected re-extract simply overwrites).
fn load_go_model_batch(ctx: &ReducerContext, packed: &str) -> Result<u32, String> {
    let models = ctx.db.game_go_model();
    let mut loaded = 0u32;
    for row in packed.split(';').filter(|r| !r.is_empty()) {
        let (entry, scale, radius, blob) = parse_row(row)?;
        models.entry().delete(entry);
        models.insert(GoModel {
            entry,
            scale,
            radius,
            blob,
        });
        loaded += 1;
    }
    Ok(loaded)
}

/// Clear + load the first go-model batch (operator-only), mirroring `vmap::import_vmap_chunks`.
#[reducer]
pub fn import_go_models(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let models = ctx.db.game_go_model();
    let entries: Vec<u32> = models.iter().map(|m| m.entry).collect();
    for entry in entries {
        models.entry().delete(entry);
    }
    if load_go_model_batch(ctx, &packed)? == 0 {
        return Err("go_model import payload was empty".to_string());
    }
    Ok(())
}

/// Append a go-model batch WITHOUT the reset — a full catalogue can span many `spacetime call`
/// args, same reason `import_vmap_chunks_append` exists.
#[reducer]
pub fn import_go_models_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_go_model_batch(ctx, &packed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn parse_row_round_trips_a_well_formed_row() {
        let blob = lyracore_shared::vmap::encode(&[]);
        let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
        let row = format!("7,1.5,2.25,{hex}");
        let (entry, scale, radius, decoded) = parse_row(&row).unwrap();
        assert_eq!(entry, 7);
        assert_eq!(scale, 1.5);
        assert_eq!(radius, 2.25);
        assert_eq!(decoded, blob);
    }

    #[test]
    fn parse_row_rejects_a_short_row() {
        assert!(parse_row("1,2,3").is_err());
    }

    #[test]
    fn parse_row_rejects_an_invalid_blob() {
        let row = "1,1.0,1.0,zz";
        assert!(parse_row(row).is_err());
    }

    // -------------------------------------------------------------------------------------
    //  Reducer idempotency — a pure in-memory stand-in for the entry-keyed clear+load /
    //  replace-on-append semantics `load_go_model_batch` implements against a real table,
    //  mirroring `vmap.rs`'s `LifecycleHarness` test style (no live ReducerContext needed).
    // -------------------------------------------------------------------------------------

    #[derive(Default)]
    struct GoModelHarness {
        rows: BTreeMap<u32, (f32, f32, Vec<u8>)>,
    }

    impl GoModelHarness {
        fn import(&mut self, packed: &str) -> Result<u32, String> {
            self.rows.clear();
            self.append(packed)
        }

        fn append(&mut self, packed: &str) -> Result<u32, String> {
            let mut loaded = 0u32;
            for row in packed.split(';').filter(|r| !r.is_empty()) {
                let (entry, scale, radius, blob) = parse_row(row)?;
                self.rows.insert(entry, (scale, radius, blob));
                loaded += 1;
            }
            Ok(loaded)
        }
    }

    fn packed_row(entry: u32, scale: f32, radius: f32) -> String {
        let blob = lyracore_shared::vmap::encode(&[]);
        let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
        format!("{entry},{scale},{radius},{hex}")
    }

    #[test]
    fn importing_the_same_batch_twice_is_idempotent() {
        let mut h = GoModelHarness::default();
        let batch = format!("{};{}", packed_row(1, 1.0, 2.0), packed_row(2, 1.0, 3.0));
        h.import(&batch).unwrap();
        let after_first: Vec<_> = h.rows.keys().copied().collect();
        h.import(&batch).unwrap();
        let after_second: Vec<_> = h.rows.keys().copied().collect();
        assert_eq!(after_first, after_second);
        assert_eq!(
            h.rows.len(),
            2,
            "re-importing the same batch must not duplicate rows"
        );
    }

    #[test]
    fn re_appending_the_same_entry_replaces_rather_than_duplicates() {
        let mut h = GoModelHarness::default();
        h.import(&packed_row(1, 1.0, 2.0)).unwrap();
        h.append(&packed_row(1, 1.0, 5.0)).unwrap(); // corrected re-extract, same entry
        assert_eq!(h.rows.len(), 1);
        assert_eq!(
            h.rows[&1].1, 5.0,
            "the newer radius must win, not error or duplicate"
        );
    }

    #[test]
    fn a_fresh_import_clears_a_prior_generation() {
        let mut h = GoModelHarness::default();
        h.import(&packed_row(1, 1.0, 2.0)).unwrap();
        h.import(&packed_row(2, 1.0, 3.0)).unwrap();
        assert_eq!(
            h.rows.len(),
            1,
            "import (not append) must clear the prior batch"
        );
        assert!(h.rows.contains_key(&2));
        assert!(!h.rows.contains_key(&1));
    }
}
