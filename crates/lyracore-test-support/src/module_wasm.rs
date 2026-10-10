use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Build once per test process and retain immutable bytes. Publishers copy these into their own
/// directories so concurrent fixtures never share the CLI optimizer's output file.
pub fn module_bytes() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(build_module_bytes)
}

fn build_module_bytes() -> Vec<u8> {
    let wasm = build_artifact(
        "lyracore-module",
        "lyracore_module",
        &[
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--features=debug_reducers,package_test_fixture",
        ],
    );
    let bytes = std::fs::read(&wasm).expect("the test Module output is missing");
    assert!(bytes.starts_with(b"\0asm"), "the built module is not Wasm");
    bytes
}

/// Build the managed Gateway source once per test process and return its exact artifact.
pub fn gateway_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| build_artifact("lyracore-gateway", "lyracore-gateway", &[]))
}

fn build_artifact(package: &str, target: &str, options: &[&str]) -> PathBuf {
    let workspace = super::core_root();
    let output = Command::new("cargo")
        .current_dir(workspace)
        .args([
            "build",
            "--locked",
            "-p",
            package,
            "--message-format=json-render-diagnostics",
        ])
        .args(options)
        .output()
        .expect("failed to run the test artifact build");
    assert!(
        output.status.success(),
        "the test {package} build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .expect("Cargo artifact output was not UTF-8")
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter(|message| message["target"]["name"] == target)
        .find_map(|message| {
            message["executable"]
                .as_str()
                .map(PathBuf::from)
                .or_else(|| {
                    message["filenames"]
                        .as_array()?
                        .iter()
                        .filter_map(|filename| filename.as_str().map(PathBuf::from))
                        .find(|path| {
                            path.extension()
                                .is_some_and(|extension| extension == "wasm")
                        })
                })
        })
        .unwrap_or_else(|| panic!("Cargo did not report the built {target} artifact"))
}
