use pretty_assertions::assert_eq;

use super::runtime_check;

#[test]
fn runtime_check_reports_embedded_product_version() {
    let check = runtime_check();
    let version_details = check
        .details
        .iter()
        .filter(|detail| detail.starts_with("version: "))
        .cloned()
        .collect::<Vec<_>>();

    assert_eq!(
        version_details,
        vec![format!("version: {}", env!("CODEX_CLI_VERSION"))]
    );
}
