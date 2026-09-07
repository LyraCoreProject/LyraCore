mod support;

use std::sync::Barrier;

use support::Standalone;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn concurrent_fixtures_publish_complete_modules() {
    let ready = Barrier::new(4);
    std::thread::scope(|threads| {
        for index in 0..4 {
            let ready = &ready;
            threads.spawn(move || {
                let mut standalone = Standalone::start(&format!("concurrent-publish-{index}"));
                ready.wait();
                if index == 0 {
                    standalone.publish_module_anonymous();
                } else {
                    standalone.publish_module();
                }
                standalone.assert_sql("SELECT COUNT(*) AS count FROM game_item_template");
            });
        }
    });
}
