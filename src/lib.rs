pub mod cli;
pub mod colors;
pub mod config;
pub mod execute;
pub mod lock;
pub mod spec;

use crate::cli::build_command;
use crate::colors::Theme;
use crate::config::{bootstrap_help, BootstrapConfig, ENV_SPEC};
use crate::execute::run as run_commands;
use crate::lock::{run_lock_command, LockCli};
use crate::spec::{load_spec_text, OpenApiSpec};
use anyhow::{Context, Result};
use clap::error::ErrorKind;
use clap::Parser;
use figlet_rs::FIGlet;
use std::env;
use std::path::Path;

/// Run the CLI with a fixed OpenAPI spec and lock manifest (used by generated locked binaries).
pub fn run_locked(lock_dir: &Path) -> Result<()> {
    let manifest = lock::read_manifest(lock_dir)?;
    manifest.apply_to_env(lock_dir)?;
    run_cli_inner()
}

fn run_cli_inner() -> Result<()> {
    let args = env::args().collect::<Vec<_>>();
    let bootstrap = BootstrapConfig::from_env_and_args(&args)?;
    let bin_name = executable_name(&args);
    let theme = Theme::from_env_and_mode(bootstrap.color_scheme.as_deref(), bootstrap.color_mode)?;

    if bootstrap.wants_version {
        println!("{} {}", bin_name, env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let spec_source = match &bootstrap.spec_source {
        Some(source) => source.clone(),
        None if bootstrap.wants_help => {
            print_banner(&theme, bootstrap.title.as_deref(), bootstrap.no_banner);
            println!("{}", bootstrap_help(&bin_name));
            return Ok(());
        }
        None => {
            eprintln!("{}", bootstrap_help(&bin_name));
            anyhow::bail!("missing OpenAPI spec source; set {ENV_SPEC} or pass --spec")
        }
    };

    let spec_text = load_spec_text(&spec_source)
        .with_context(|| format!("failed to load OpenAPI spec from '{spec_source}'"))?;
    let spec = OpenApiSpec::from_json_with_source(&spec_text, Some(&spec_source))?;

    let command = build_command(&bin_name, &spec, &theme).color(bootstrap.color_mode.clap_choice());
    let matches = match command.clone().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => match error.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                print_banner(&theme, bootstrap.title.as_deref(), bootstrap.no_banner);
                error.print()?;
                return Ok(());
            }
            _ => {
                error.print()?;
                std::process::exit(2);
            }
        },
    };

    let should_print_banner = matches
        .subcommand_name()
        .map(|name| name != "completions")
        .unwrap_or(false);
    if should_print_banner {
        print_banner(&theme, bootstrap.title.as_deref(), bootstrap.no_banner);
    }

    run_commands(&bin_name, &spec, &theme, &matches, command)?;
    Ok(())
}

/// Full CLI entry including bootstrap `lock` handling (used by the main `acli` binary).
pub fn run() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if let Some(lock_idx) = find_lock_subcommand(&args) {
        let argv = lock_cli_argv(&args, lock_idx);
        let lock_cli = LockCli::parse_from(argv);
        return run_lock_command(lock_cli);
    }
    run_cli_inner()
}

/// Position of the standalone `lock` token (not a value for a preceding flag).
fn find_lock_subcommand(args: &[String]) -> Option<usize> {
    let mut i = 1;
    while i < args.len() {
        if args[i] == "lock" && !lock_token_is_flag_value(args, i) {
            return Some(i);
        }
        if current_flag_consumes_following_value(args, i) {
            i += 2;
        } else {
            i += 1;
        }
    }
    None
}

fn lock_token_is_flag_value(args: &[String], lock_idx: usize) -> bool {
    lock_idx > 0 && current_flag_consumes_following_value(args, lock_idx - 1)
}

/// Whether `args[i]` is a flag that takes its value from `args[i + 1]` (not `NAME=value` form).
fn current_flag_consumes_following_value(args: &[String], i: usize) -> bool {
    if i + 1 >= args.len() {
        return false;
    }
    let a = args[i].as_str();
    if a == "-o" {
        return true;
    }
    let Some(name) = a.strip_prefix("--") else {
        return false;
    };
    if a.contains('=') {
        return false;
    }
    long_flag_takes_separate_value(name)
}

fn long_flag_takes_separate_value(flag: &str) -> bool {
    matches!(
        flag,
        "spec"
            | "title"
            | "color-scheme"
            | "color"
            | "server-url"
            | "server-index"
            | "server-var"
            | "bearer-token"
            | "basic-user"
            | "basic-pass"
            | "api-key"
            | "auth"
            | "timeout"
            | "output"
            | "acli-crate-path"
            | "crate-name"
            | "binary-name"
            | "secrets"
            | "default-header"
    )
}

/// `argv[0]` is the program name; includes every token except the `lock` word, with tokens after
/// `lock` before tokens before `lock` so `clap` still parses `lock`-specific flags when globals
/// precede `lock` (e.g. `acli --spec URL lock --output ./out`).
fn lock_cli_argv(args: &[String], lock_idx: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    out.push(args[0].clone());
    out.extend(args[lock_idx + 1..].iter().cloned());
    out.extend(args[1..lock_idx].iter().cloned());
    out
}

#[cfg(test)]
mod lock_bootstrap_tests {
    use super::*;

    #[test]
    fn find_lock_after_globals() {
        let args = vec![
            "acli".into(),
            "--no-banner".into(),
            "--server-index".into(),
            "2".into(),
            "lock".into(),
            "--output".into(),
            "/tmp/x".into(),
        ];
        assert_eq!(find_lock_subcommand(&args), Some(4));
    }

    #[test]
    fn lock_not_subcommand_when_value_for_spec() {
        let args = vec!["acli".into(), "--spec".into(), "lock".into()];
        assert_eq!(find_lock_subcommand(&args), None);
    }

    #[test]
    fn lock_cli_argv_merges_before_and_after() {
        let args = vec![
            "acli".into(),
            "--spec".into(),
            "s.json".into(),
            "lock".into(),
            "--output".into(),
            "/out".into(),
        ];
        let v = lock_cli_argv(&args, 3);
        assert_eq!(v, vec!["acli", "--output", "/out", "--spec", "s.json",]);
    }
}

fn print_banner(theme: &Theme, title: Option<&str>, disabled: bool) {
    if disabled {
        return;
    }
    let Some(title) = title.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };

    let rendered = std::panic::catch_unwind(|| {
        let font = FIGlet::standard().unwrap();
        font.convert(title).unwrap().to_string()
    })
    .unwrap_or_else(|_| title.to_string());

    eprintln!("{}", theme.banner(rendered));
}

fn executable_name(args: &[String]) -> String {
    args.first()
        .and_then(|arg| Path::new(arg).file_name())
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| env!("CARGO_PKG_NAME").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_name_falls_back_to_app_name() {
        assert_eq!(executable_name(&[]), "acli");
    }
}
