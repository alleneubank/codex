pub(crate) fn format_cli_version(package_version: &str, revision: Option<&str>) -> String {
    revision.map_or_else(
        || package_version.to_string(),
        |revision| {
            let separator = if package_version.contains('+') {
                "."
            } else {
                "+"
            };
            format!("{package_version}{separator}fork.{revision}")
        },
    )
}
