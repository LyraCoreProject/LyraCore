#![cfg(feature = "native_simulation_database")]

use lyracore_module::native_prototype::compare;
use serde_json::json;

#[test]
fn comparison_catches_missing_rows_flags_time_and_float_drift() {
    let expected = json!({"entities": [{"guid": 9_007_199_254_740_993_u64, "x": 3.0, "dead": false}], "sequence": 1});
    for actual in [
        json!({"entities": [], "sequence": 1}),
        json!({"entities": [{"guid": 9_007_199_254_740_992_u64, "x": 3.0, "dead": false}], "sequence": 1}),
        json!({"entities": [{"guid": 9_007_199_254_740_993_u64, "x": 3.01, "dead": false}], "sequence": 1}),
        json!({"entities": [{"guid": 9_007_199_254_740_993_u64, "x": 3.0, "dead": true}], "sequence": 1}),
        json!({"entities": [{"guid": 9_007_199_254_740_993_u64, "x": 3.0, "dead": false}], "sequence": 2}),
    ] {
        assert!(compare(&expected, &actual).is_err(), "{actual}");
    }
    let mut close = expected.clone();
    close["entities"][0]["x"] = json!(3.000001);
    compare(&expected, &close).unwrap();
    close["entities"][0]["x"] = json!(4.0);
    assert!(compare(&expected, &close)
        .unwrap_err()
        .contains("entities[guid=9007199254740993].x"));
}
