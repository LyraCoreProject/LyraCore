mod support;

use serde_json::{json, Map, Value};
use support::{schema_section, Standalone};

const OPEN: &[&str] = &["claim_operator"];

const OPEN_PREFIX: &str = "debug_";

const GATE_REFUSALS: &[&str] = &["operator only", "loot:boundary_operator_rejected"];

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn every_operator_reducer_refuses_an_anonymous_caller() {
    let mut shard = Standalone::start("operator-gates");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);

    let gated = operator_reducers(&shard.describe());
    for known in [
        "gw_player_login",
        "realm_mail_send",
        "set_realm_address",
        "begin_transfer",
    ] {
        assert!(
            gated.iter().any(|(name, _)| name == known),
            "{known} is missing from the gated set"
        );
    }
    assert!(gated.iter().all(|(name, _)| name != "claim_operator"));
    let claim = shard.call_anonymous("claim_operator", &[]);
    let claim_text = output_text(&claim);
    assert!(
        !claim.status.success() && claim_text.contains("operator already claimed"),
        "an open reducer answers an anonymous caller itself: {claim_text}"
    );

    let open: Vec<String> = gated
        .iter()
        .filter_map(|(reducer, args)| {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let output = shard.call_anonymous(reducer, &args);
            let text = output_text(&output);
            let refused =
                !output.status.success() && GATE_REFUSALS.iter().any(|gate| text.contains(gate));
            (!refused).then(|| format!("{reducer}: {text}"))
        })
        .collect();
    assert!(
        open.is_empty(),
        "{} of {} gated reducers let a non-Operator through:\n{}",
        open.len(),
        gated.len(),
        open.join("\n")
    );
}

fn operator_reducers(schema: &Value) -> Vec<(String, Vec<String>)> {
    let typespace = schema_section(schema, "Typespace")["types"]
        .as_array()
        .expect("typespace");
    let function_names = |kind| -> Vec<&str> {
        schema_section(schema, kind)
            .as_array()
            .expect(kind)
            .iter()
            .map(|entry| entry["function_name"].as_str().expect("function name"))
            .collect()
    };
    let scheduled = function_names("Schedules");
    let lifecycle = function_names("LifeCycleReducers");
    schema_section(schema, "Reducers")
        .as_array()
        .expect("reducers")
        .iter()
        .filter_map(|reducer| {
            let name = reducer["source_name"].as_str().expect("reducer name");
            let open = OPEN.contains(&name)
                || name.starts_with(OPEN_PREFIX)
                || scheduled.contains(&name)
                || lifecycle.contains(&name);
            let args = reducer["params"]["elements"]
                .as_array()
                .expect("reducer params")
                .iter()
                .map(|param| minimal(&param["algebraic_type"], typespace).to_string())
                .collect();
            (!open).then(|| (name.to_owned(), args))
        })
        .collect()
}

fn minimal(ty: &Value, typespace: &[Value]) -> Value {
    let (kind, inner) = ty
        .as_object()
        .and_then(|object| object.iter().next())
        .expect("an algebraic type");
    match kind.as_str() {
        "Ref" => minimal(&typespace[inner.as_u64().unwrap() as usize], typespace),
        "Product" => Value::Object(
            inner["elements"]
                .as_array()
                .unwrap()
                .iter()
                .map(|field| {
                    let name = field["name"]["some"].as_str().expect("a named field");
                    (
                        name.to_owned(),
                        minimal(&field["algebraic_type"], typespace),
                    )
                })
                .collect::<Map<_, _>>(),
        ),
        "Sum" => {
            let variant = &inner["variants"][0];
            let name = variant["name"]["some"].as_str().expect("a named variant");
            json!({ name: minimal(&variant["algebraic_type"], typespace) })
        }
        "Array" => json!([]),
        "String" => json!(""),
        "Bool" => json!(false),
        "F32" | "F64" => json!(0.0),
        "U256" | "I256" => json!("0x1"),
        _ => json!(1),
    }
}

fn output_text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
