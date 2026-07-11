use pretty_assertions::assert_eq;

use super::normalize_snapshot_version;

#[test]
fn snapshot_version_normalization_preserves_rendered_width() {
    let rendered = format!("|{}|", crate::version::CODEX_CLI_VERSION);
    let normalized = normalize_snapshot_version(&rendered);

    assert_eq!(normalized.chars().count(), rendered.chars().count());
}
