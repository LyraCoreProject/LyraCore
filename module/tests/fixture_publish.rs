mod support;

use std::sync::Barrier;

use support::Standalone;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn concurrent_fixtures_publish_complete_modules() {
    let fixtures: Vec<_> = (0..4)
        .map(|index| Standalone::start(&format!("concurrent-publish-{index}")))
        .collect();
    let ready = Barrier::new(fixtures.len());
    std::thread::scope(|threads| {
        for (index, mut standalone) in fixtures.into_iter().enumerate() {
            let ready = &ready;
            threads.spawn(move || {
                ready.wait();
                if index == 0 {
                    standalone.publish_module_anonymous();
                } else {
                    standalone.publish_module();
                }
                let counted =
                    standalone.sql_anonymous("SELECT COUNT(*) AS count FROM game_item_template");
                let text = String::from_utf8_lossy(&counted.stdout);
                let counts: Vec<&str> = text
                    .lines()
                    .map(str::trim)
                    .filter(|line| line.parse::<u64>().is_ok())
                    .collect();
                assert_eq!(
                    counts,
                    ["10"],
                    "init seeds six core and four fixture item templates: {text}"
                );
            });
        }
    });
}
