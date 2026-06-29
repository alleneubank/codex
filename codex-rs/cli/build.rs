#[path = "src/build_version.rs"]
mod build_version;

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

    // Keep the comparable upstream version separate from fork provenance. Source archives without
    // Git metadata honestly fall back to the pinned SemVer instead of inventing a revision.
    let revision = git_output(&["rev-parse", "--short=12", "HEAD"]);
    let version = build_version::format_cli_version(&semver.to_string(), revision.as_deref());
    println!("cargo:rustc-env=CODEX_CLI_VERSION={version}");
    println!("cargo:rerun-if-changed=build.rs");
    track_git_path("HEAD");
    if let Some(head_ref) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        track_git_path(&head_ref);
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-ObjC");
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn track_git_path(path: &str) {
    if let Some(git_path) = git_output(&["rev-parse", "--git-path", path]) {
        println!("cargo:rerun-if-changed={git_path}");
    }
}
