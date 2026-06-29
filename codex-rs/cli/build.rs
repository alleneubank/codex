fn main() {
    let version = git_output(&["rev-parse", "HEAD"])
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is set"));
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
