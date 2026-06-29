use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use crate::GitToolingError;

const DISABLED_HOOKS_PATH: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };
const EXECUTABLE_CHECKOUT_FILTER_CONFIG_PATTERN: &str = r"^filter\..*\.(clean|smudge|process)$";

pub(crate) fn ensure_git_repository(path: &Path) -> Result<(), GitToolingError> {
    match run_git_for_stdout(
        path,
        vec![
            OsString::from("rev-parse"),
            OsString::from("--is-inside-work-tree"),
        ],
        /*env*/ None,
    ) {
        Ok(output) if output.trim() == "true" => Ok(()),
        Ok(_) => Err(GitToolingError::NotAGitRepository {
            path: path.to_path_buf(),
        }),
        Err(GitToolingError::GitCommand { status, .. }) if status.code() == Some(128) => {
            Err(GitToolingError::NotAGitRepository {
                path: path.to_path_buf(),
            })
        }
        Err(err) => Err(err),
    }
}

pub(crate) fn resolve_head(path: &Path) -> Result<Option<String>, GitToolingError> {
    match run_git_for_stdout(
        path,
        vec![
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from("HEAD"),
        ],
        /*env*/ None,
    ) {
        Ok(sha) => Ok(Some(sha)),
        Err(GitToolingError::GitCommand { status, .. }) if status.code() == Some(128) => Ok(None),
        Err(other) => Err(other),
    }
}

pub(crate) fn resolve_repository_root(path: &Path) -> Result<PathBuf, GitToolingError> {
    let root = run_git_for_stdout(
        path,
        vec![
            OsString::from("rev-parse"),
            OsString::from("--show-toplevel"),
        ],
        /*env*/ None,
    )?;
    Ok(PathBuf::from(root))
}

pub(crate) fn run_git_for_status<I, S>(
    dir: &Path,
    args: I,
    env: Option<&[(OsString, OsString)]>,
) -> Result<(), GitToolingError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_git(dir, args, env)?;
    Ok(())
}

pub(crate) fn run_git_for_stdout<I, S>(
    dir: &Path,
    args: I,
    env: Option<&[(OsString, OsString)]>,
) -> Result<String, GitToolingError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let run = run_git(dir, args, env)?;
    String::from_utf8(run.output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|source| GitToolingError::GitOutputUtf8 {
            command: run.command,
            source,
        })
}

pub(crate) fn checkout_filter_config_env_overrides(
    dir: &Path,
) -> Result<Vec<(OsString, OsString)>, GitToolingError> {
    let run = match run_git(
        dir,
        [
            "config",
            "--null",
            "--name-only",
            "--get-regexp",
            EXECUTABLE_CHECKOUT_FILTER_CONFIG_PATTERN,
        ],
        /*env*/ None,
    ) {
        Ok(run) => run,
        Err(GitToolingError::GitCommand { status, .. }) if status.code() == Some(1) => {
            return Ok(Vec::new());
        }
        Err(err) => return Err(err),
    };
    let output =
        String::from_utf8(run.output.stdout).map_err(|source| GitToolingError::GitOutputUtf8 {
            command: run.command,
            source,
        })?;
    let mut drivers = output
        .split('\0')
        .filter_map(|key| {
            key.strip_suffix(".clean")
                .or_else(|| key.strip_suffix(".smudge"))
                .or_else(|| key.strip_suffix(".process"))
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    drivers.sort();
    drivers.dedup();

    let config_overrides = drivers
        .into_iter()
        .flat_map(|driver| {
            [
                (format!("{driver}.clean"), String::new()),
                (format!("{driver}.smudge"), String::new()),
                (format!("{driver}.process"), String::new()),
                (format!("{driver}.required"), "false".to_string()),
            ]
        })
        .collect::<Vec<_>>();
    let mut env = Vec::with_capacity(config_overrides.len() * 2 + 1);
    env.push((
        OsString::from("GIT_CONFIG_COUNT"),
        OsString::from(config_overrides.len().to_string()),
    ));
    for (index, (key, value)) in config_overrides.into_iter().enumerate() {
        env.push((
            OsString::from(format!("GIT_CONFIG_KEY_{index}")),
            OsString::from(key),
        ));
        env.push((
            OsString::from(format!("GIT_CONFIG_VALUE_{index}")),
            OsString::from(value),
        ));
    }
    Ok(env)
}

fn run_git<I, S>(
    dir: &Path,
    args: I,
    env: Option<&[(OsString, OsString)]>,
) -> Result<GitRun, GitToolingError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let iterator = args.into_iter();
    let (lower, upper) = iterator.size_hint();
    let mut args_vec = Vec::with_capacity(upper.unwrap_or(lower) + 2);
    // Keep internal Git helper commands independent of configured hook directories.
    args_vec.push(OsString::from("-c"));
    args_vec.push(OsString::from(format!(
        "core.hooksPath={DISABLED_HOOKS_PATH}"
    )));
    for arg in iterator {
        args_vec.push(OsString::from(arg.as_ref()));
    }
    let command_string = build_command_string(&args_vec);
    let mut command = Command::new("git");
    command.current_dir(dir);
    if let Some(envs) = env {
        for (key, value) in envs {
            command.env(key, value);
        }
    }
    command.args(&args_vec);
    let output = command.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(GitToolingError::GitCommand {
            command: command_string,
            status: output.status,
            stderr,
        });
    }
    Ok(GitRun {
        command: command_string,
        output,
    })
}

fn build_command_string(args: &[OsString]) -> String {
    if args.is_empty() {
        return "git".to_string();
    }
    let joined = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    format!("git {joined}")
}

struct GitRun {
    command: String,
    output: std::process::Output,
}
