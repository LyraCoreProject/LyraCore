use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=LYRACORE_COORDINATOR_TEST_SOURCE");
    let source = env::var_os("LYRACORE_COORDINATOR_TEST_SOURCE").map(PathBuf::from);
    let declaration = source.map_or_else(String::new, |source| {
        let source = source
            .canonicalize()
            .expect("Package coordinator tests are missing");
        assert!(
            source.is_file(),
            "Package coordinator test source must be a file"
        );
        println!("cargo:rerun-if-changed={}", source.display());
        format!("#[path = {source:?}] mod package_tests;\n")
    });
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    fs::write(output.join("package-coordinator-tests.rs"), declaration)
        .expect("could not write Package coordinator test entry");
}
