use spacetimedb::ReducerContext;

#[cfg(feature = "debug_reducers")]
mod fixture;

pub(crate) type PackageCommandHandler =
    fn(&ReducerContext, u64, &str, &str) -> Result<String, String>;

const MAX_COMMAND_BYTES: usize = 64;
const MAX_PAYLOAD_BYTES: usize = 1024;

fn command_parts<'a>(cmd: &'a str, payload: &str) -> Result<(&'a str, &'a str), String> {
    if cmd.len() > MAX_COMMAND_BYTES || payload.len() > MAX_PAYLOAD_BYTES || payload.contains('\0')
    {
        return Err("Malformed".into());
    }
    let (namespace, action) = cmd
        .strip_prefix("package.")
        .and_then(|command| command.split_once('.'))
        .ok_or("Malformed")?;
    if [namespace, action].iter().any(|part| {
        part.is_empty()
            || !part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }) {
        return Err("Malformed".into());
    }
    Ok((namespace, action))
}

pub(crate) fn apply_package_command(
    ctx: &ReducerContext,
    actor_guid: u64,
    cmd: &str,
    payload: &str,
) -> Result<(), String> {
    let (namespace, action) = command_parts(cmd, payload)?;
    let handler = crate::GAME_ADDON_COMMANDS
        .iter()
        .find_map(|(registered, handler)| (*registered == namespace).then_some(*handler));
    #[cfg(feature = "debug_reducers")]
    let handler = handler
        .or_else(|| (namespace == "fixture").then_some(fixture::handle as PackageCommandHandler));
    let handler = handler.ok_or("UnknownCommand")?;
    // Propagate Refusals so SpacetimeDB rolls back every change made by the Package.
    let reply = handler(ctx, actor_guid, action, payload)?;
    if reply.len() > MAX_PAYLOAD_BYTES || reply.contains('\0') {
        return Err("InvalidPackageReply".into());
    }
    super::send(ctx, actor_guid, &format!("{cmd}.result"), &reply);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_commands_name_an_action_without_an_actor() {
        assert_eq!(
            command_parts("package.wave_defense.status", ""),
            Ok(("wave_defense", "status"))
        );
        assert_eq!(
            command_parts("package.wave_defense.select", "healer|42"),
            Ok(("wave_defense", "select"))
        );
    }

    #[test]
    fn malformed_package_commands_are_refused() {
        for command in [
            "package.",
            "package.wave_defense",
            "package..status",
            "package.wave_defense.",
            "package.wave_defense.status.extra",
            "package.wave_defense.status|42",
            "example.order",
        ] {
            assert_eq!(command_parts(command, ""), Err("Malformed".into()));
        }
        assert_eq!(
            command_parts(&format!("package.{}.status", "x".repeat(64)), ""),
            Err("Malformed".into())
        );
    }

    #[test]
    fn package_payloads_are_bounded_and_cannot_truncate_the_reply_frame() {
        assert!(command_parts("package.wave_defense.status", &"a".repeat(1024)).is_ok());
        for payload in ["a".repeat(1025), "status\0other".to_string()] {
            assert_eq!(
                command_parts("package.wave_defense.status", &payload),
                Err("Malformed".into())
            );
        }
    }
}
