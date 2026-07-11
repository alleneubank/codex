use anyhow::Result;
use pretty_assertions::assert_eq;

#[test]
fn top_level_version_exposes_a_non_source_semver() -> Result<()> {
    let output = std::process::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?)
        .arg("--version")
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    let version = stdout
        .trim()
        .strip_prefix("codex-cli ")
        .expect("version output should use the codex-cli package name");
    let version = semver::Version::parse(version)?;
    assert!(
        version.major != 0 || version.minor != 0 || version.patch != 0 || !version.pre.is_empty()
    );
    if !version.build.is_empty() {
        let mut build_components = version.build.as_str().rsplit('.');
        let revision = build_components
            .next()
            .expect("non-empty build metadata should contain a revision");
        assert_eq!(build_components.next(), Some("fork"));
        assert_eq!(revision.len(), 12);
        assert!(
            revision
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
    }

    Ok(())
}
