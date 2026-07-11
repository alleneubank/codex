use pretty_assertions::assert_eq;

use super::build_version::format_cli_version;

#[test]
fn formats_package_version_with_optional_fork_revision() {
    assert_eq!(
        [
            format_cli_version("0.144.0", /*revision*/ None),
            format_cli_version("0.144.0", Some("abcdef123456")),
            format_cli_version("0.144.0-alpha.1", Some("abcdef123456")),
            format_cli_version("0.144.0+preview", Some("abcdef123456")),
        ],
        [
            "0.144.0",
            "0.144.0+fork.abcdef123456",
            "0.144.0-alpha.1+fork.abcdef123456",
            "0.144.0+preview.fork.abcdef123456",
        ]
    );
}
