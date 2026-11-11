use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::vmap::hex_decode;

#[table(accessor = game_go_model)]
pub struct GoModel {
    #[primary_key]
    pub entry: u32,
    pub scale: f32,
    pub radius: f32,
    pub blob: Vec<u8>,
}

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
    let tris = lyracore_shared::vmap::decode(&blob)
        .map_err(|err| format!("invalid go_model blob: {err:?}"))?;
    if !scale.is_finite()
        || scale <= 0.0
        || !radius.is_finite()
        || radius < 0.0
        || tris
            .iter()
            .flat_map(|t| t.verts)
            .flatten()
            .any(|v| !v.is_finite())
    {
        return Err(
            "go_model needs finite geometry, a positive scale and a nonnegative radius".to_string(),
        );
    }
    Ok((entry, scale, radius, blob))
}

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
    crate::go_collider::rebuild_go_colliders(ctx)
}

#[reducer]
pub fn import_go_models_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_go_model_batch(ctx, &packed)?;
    crate::go_collider::rebuild_go_colliders(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn malformed_transform_and_non_ascii_hex_are_refused() {
        let blob = lyracore_shared::vmap::encode(&[]);
        let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
        for (scale, radius) in [
            ("0", "1"),
            ("-1", "1"),
            ("NaN", "1"),
            ("1", "NaN"),
            ("1", "-1"),
        ] {
            assert!(parse_row(&format!("1,{scale},{radius},{hex}")).is_err());
        }
        assert!(parse_row("1,1,1,€a").is_err());
    }
}
