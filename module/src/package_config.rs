use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::helpers::require_operator;

#[table(
    accessor = game_package_config,
    public,
    index(accessor = by_package_key, btree(columns = [package_name, key]))
)]
pub struct PackageConfig {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub package_name: String,
    pub key: String,
    pub value: String,
}

fn config_row(ctx: &ReducerContext, package_name: &str, key: &str) -> Option<PackageConfig> {
    ctx.db
        .game_package_config()
        .by_package_key()
        .filter((package_name, key))
        .next()
}

fn known_keys(ctx: &ReducerContext, package_name: &str) -> Vec<String> {
    let mut keys: Vec<String> = ctx
        .db
        .game_package_config()
        .by_package_key()
        .filter(package_name)
        .map(|row| row.key)
        .collect();
    keys.sort();
    keys
}

pub(crate) fn unknown_key_message(package_name: &str, key: &str, known_keys: &[String]) -> String {
    if known_keys.is_empty() {
        format!("package '{package_name}' has no config keys yet; pass allow_new to set '{key}'")
    } else {
        format!(
            "package '{package_name}' has no config key '{key}'; known keys: {}. Pass allow_new to set it anyway",
            known_keys.join(", ")
        )
    }
}

#[reducer]
pub fn set_package_config(
    ctx: &ReducerContext,
    package_name: String,
    key: String,
    value: String,
    allow_new: bool,
) -> Result<(), String> {
    require_operator(ctx)?;
    match config_row(ctx, &package_name, &key) {
        Some(mut row) => {
            row.value = value;
            ctx.db.game_package_config().id().update(row);
            Ok(())
        }
        None if allow_new => {
            ctx.db.game_package_config().insert(PackageConfig {
                id: 0,
                package_name,
                key,
                value,
            });
            Ok(())
        }
        None => Err(unknown_key_message(
            &package_name,
            &key,
            &known_keys(ctx, &package_name),
        )),
    }
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn ensure_package_config_default(
    ctx: &ReducerContext,
    package: &str,
    key: &str,
    value: &str,
) {
    if config_row(ctx, package, key).is_some() {
        return;
    }
    ctx.db.game_package_config().insert(PackageConfig {
        id: 0,
        package_name: package.to_string(),
        key: key.to_string(),
        value: value.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_names_the_packages_existing_keys_sorted() {
        let known = vec!["max_bots".to_string(), "spawn_rate".to_string()];
        let message = unknown_key_message("sample_package", "spwan_rate", &known);
        assert!(message.contains("sample_package"), "{message}");
        assert!(message.contains("spwan_rate"), "{message}");
        assert!(
            message.contains("max_bots, spawn_rate"),
            "keys must be named sorted: {message}"
        );
    }

    #[test]
    fn a_package_with_no_keys_yet_still_names_itself_in_the_refusal() {
        let message = unknown_key_message("brand_new_package", "any_key", &[]);
        assert!(message.contains("brand_new_package"), "{message}");
        assert!(message.contains("any_key"), "{message}");
    }
}
