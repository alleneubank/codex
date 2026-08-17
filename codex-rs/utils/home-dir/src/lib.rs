use codex_utils_absolute_path::AbsolutePathBuf;
use dirs::home_dir;
use std::path::PathBuf;

/// Returns the path to the Codex configuration directory, which can be
/// specified by the `CODEX_HOME` environment variable. If not set, defaults to
/// `~/.codex`.
///
/// - If `CODEX_HOME` is set, the value must exist and be a directory. The
///   value will be canonicalized and this function will Err otherwise.
/// - If `CODEX_HOME` is not set, this function does not verify that the
///   directory exists.
pub fn find_codex_home() -> std::io::Result<AbsolutePathBuf> {
    let codex_home_env = std::env::var("CODEX_HOME")
        .ok()
        .filter(|val| !val.is_empty());
    find_codex_home_from_env(codex_home_env.as_deref())
}

/// Returns the directory used exclusively for local CLI credentials.
///
/// When `CODEX_AUTH_HOME` is unset, this is the resolved `CODEX_HOME`. A set
/// value must identify an existing directory and is canonicalized before use.
pub fn find_auth_home(codex_home: &AbsolutePathBuf) -> std::io::Result<AbsolutePathBuf> {
    let auth_home = find_auth_home_for_path(codex_home)?;
    AbsolutePathBuf::from_absolute_path(auth_home)
}

/// Returns the directory used exclusively for local CLI credentials for a
/// supplied Codex home path.
///
/// This preserves a caller-provided path when `CODEX_AUTH_HOME` is unset, so
/// bootstrap configuration can defer canonicalization of its normal state root.
pub fn find_auth_home_for_path(codex_home: &std::path::Path) -> std::io::Result<PathBuf> {
    let auth_home_env = std::env::var("CODEX_AUTH_HOME")
        .ok()
        .filter(|val| !val.is_empty());
    find_auth_home_from_env(codex_home, auth_home_env.as_deref())
}

fn find_auth_home_from_env(
    codex_home: &std::path::Path,
    auth_home_env: Option<&str>,
) -> std::io::Result<PathBuf> {
    match auth_home_env {
        Some(val) => find_existing_directory_from_env("CODEX_AUTH_HOME", val).map(Into::into),
        None => Ok(codex_home.to_path_buf()),
    }
}

fn find_codex_home_from_env(codex_home_env: Option<&str>) -> std::io::Result<AbsolutePathBuf> {
    // Honor the `CODEX_HOME` environment variable when it is set to allow users
    // (and tests) to override the default location.
    match codex_home_env {
        Some(val) => find_existing_directory_from_env("CODEX_HOME", val),
        None => {
            let mut p = home_dir().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Could not find home directory",
                )
            })?;
            p.push(".codex");
            AbsolutePathBuf::from_absolute_path(p)
        }
    }
}

fn find_existing_directory_from_env(
    variable_name: &str,
    value: &str,
) -> std::io::Result<AbsolutePathBuf> {
    let path = PathBuf::from(value);
    let metadata = std::fs::metadata(&path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{variable_name} points to {value:?}, but that path does not exist"),
        ),
        _ => std::io::Error::new(
            err.kind(),
            format!("failed to read {variable_name} {value:?}: {err}"),
        ),
    })?;

    if !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{variable_name} points to {value:?}, but that path is not a directory"),
        ));
    }

    let canonical = path.canonicalize().map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!("failed to canonicalize {variable_name} {value:?}: {err}"),
        )
    })?;
    AbsolutePathBuf::from_absolute_path(canonical)
}

#[cfg(test)]
mod tests {
    use super::find_auth_home_from_env;
    use super::find_codex_home_from_env;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use dirs::home_dir;
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::io::ErrorKind;
    use tempfile::TempDir;

    #[test]
    fn find_codex_home_env_missing_path_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let missing = temp_home.path().join("missing-codex-home");
        let missing_str = missing
            .to_str()
            .expect("missing codex home path should be valid utf-8");

        let err = find_codex_home_from_env(Some(missing_str)).expect_err("missing CODEX_HOME");
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(
            err.to_string().contains("CODEX_HOME"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_codex_home_env_file_path_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let file_path = temp_home.path().join("codex-home.txt");
        fs::write(&file_path, "not a directory").expect("write temp file");
        let file_str = file_path
            .to_str()
            .expect("file codex home path should be valid utf-8");

        let err = find_codex_home_from_env(Some(file_str)).expect_err("file CODEX_HOME");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("not a directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_codex_home_env_valid_directory_canonicalizes() {
        let temp_home = TempDir::new().expect("temp home");
        let temp_str = temp_home
            .path()
            .to_str()
            .expect("temp codex home path should be valid utf-8");

        let resolved = find_codex_home_from_env(Some(temp_str)).expect("valid CODEX_HOME");
        let expected = temp_home
            .path()
            .canonicalize()
            .expect("canonicalize temp home");
        let expected = AbsolutePathBuf::from_absolute_path(expected).expect("absolute home");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn find_codex_home_without_env_uses_default_home_dir() {
        let resolved =
            find_codex_home_from_env(/*codex_home_env*/ None).expect("default CODEX_HOME");
        let mut expected = home_dir().expect("home dir");
        expected.push(".codex");
        let expected = AbsolutePathBuf::from_absolute_path(expected).expect("absolute home");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn find_auth_home_uses_override_without_changing_codex_home() {
        let codex_home = TempDir::new().expect("temp Codex home");
        let auth_home = TempDir::new().expect("temp auth home");
        let auth_home_str = auth_home
            .path()
            .to_str()
            .expect("auth home path should be valid utf-8");

        let codex_home = AbsolutePathBuf::from_absolute_path(
            codex_home
                .path()
                .canonicalize()
                .expect("canonicalize Codex home"),
        )
        .expect("absolute Codex home");
        let resolved = find_auth_home_from_env(&codex_home, Some(auth_home_str))
            .expect("valid CODEX_AUTH_HOME");
        let expected = auth_home
            .path()
            .canonicalize()
            .expect("canonicalize auth home");

        assert_eq!(resolved, expected);
        assert_ne!(resolved, codex_home.to_path_buf());
    }

    #[test]
    fn find_auth_home_without_override_uses_codex_home() {
        let codex_home = TempDir::new().expect("temp Codex home");
        let codex_home = AbsolutePathBuf::from_absolute_path(
            codex_home
                .path()
                .canonicalize()
                .expect("canonicalize Codex home"),
        )
        .expect("absolute Codex home");

        assert_eq!(
            find_auth_home_from_env(&codex_home, /*auth_home_env*/ None)
                .expect("default auth home"),
            codex_home.to_path_buf()
        );
    }

    #[test]
    fn find_auth_home_missing_path_is_fatal_and_names_variable() {
        let temp_home = TempDir::new().expect("temp home");
        let codex_home =
            AbsolutePathBuf::from_absolute_path(temp_home.path()).expect("absolute Codex home");
        let missing = temp_home.path().join("missing-auth-home");
        let missing = missing
            .to_str()
            .expect("missing auth home path should be valid utf-8");

        let err = find_auth_home_from_env(&codex_home, Some(missing))
            .expect_err("missing CODEX_AUTH_HOME");
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(err.to_string().contains("CODEX_AUTH_HOME"));
    }
}
