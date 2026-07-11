fn main() {
    let manifest_dir = match std::env::var_os("CARGO_MANIFEST_DIR") {
        Some(manifest_dir) => manifest_dir,
        None => panic!("CARGO_MANIFEST_DIR must be set for build scripts"),
    };
    let version_path = std::path::PathBuf::from(manifest_dir).join("../fork-version.txt");
    println!("cargo:rerun-if-changed={}", version_path.display());
    let semver = std::fs::read_to_string(&version_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", version_path.display()));
    let semver = semver::Version::parse(semver.trim()).unwrap_or_else(|err| {
        panic!(
            "{} must contain valid SemVer: {err}",
            version_path.display()
        )
    });
    assert!(
        semver != semver::Version::new(0, 0, 0),
        "{} must contain a non-zero SemVer",
        version_path.display()
    );
    println!("cargo:rustc-env=CODEX_CLI_VERSION={semver}");
}
