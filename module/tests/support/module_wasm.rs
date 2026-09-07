use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Build once per test process and retain immutable bytes. Publishers copy these into their own
/// directories so concurrent fixtures never share the CLI optimizer's output file.
pub fn module_bytes() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(build_module_bytes)
}

fn build_module_bytes() -> Vec<u8> {
    let module_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = module_dir.parent().unwrap();
    let output = Command::new("cargo")
        .current_dir(workspace)
        .args([
            "build",
            "--locked",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "-p",
            "lyracore-module",
            "--features=debug_reducers",
            "--message-format=json-render-diagnostics",
        ])
        .output()
        .expect("failed to run the test Module build");
    assert!(
        output.status.success(),
        "the test Module build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let wasm = String::from_utf8(output.stdout)
        .expect("Cargo artifact output was not UTF-8")
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter(|message| message["target"]["name"] == "lyracore_module")
        .filter_map(|message| message["filenames"].as_array().cloned())
        .flatten()
        .filter_map(|filename| filename.as_str().map(PathBuf::from))
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "wasm")
        })
        .expect("Cargo did not report the built Module Wasm artifact");
    let bytes = std::fs::read(&wasm).expect("the test Module output is missing");
    assert!(bytes.starts_with(b"\0asm"), "the built module is not Wasm");
    bytes
}
