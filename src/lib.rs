pub mod app_config;
pub mod cli;
pub mod colors;
pub mod config;
pub mod execute;
pub mod lock;
pub mod spec;

use crate::app_config::config_schema_json;
use crate::cli::build_command;
use crate::colors::Theme;
use crate::config::{bootstrap_help, schema_help, BootstrapConfig, ENV_SPEC};
use crate::execute::run as run_commands;
use crate::lock::{
    launcher_lock_dir, run_install_command, run_uninstall_command, InstallCli, UninstallCli,
};
use crate::spec::{load_spec_text, OpenApiSpec};
use anyhow::{Context, Result};
use clap::error::ErrorKind;
use clap::Parser;
use figlet_rs::FIGlet;
use std::env;
use std::path::Path;

#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run the CLI with a fixed OpenAPI spec and lock manifest (used by locked CLI launchers).
pub fn run_locked(lock_dir: &Path) -> Result<()> {
    let manifest = lock::read_manifest(lock_dir)?;
    manifest.apply_to_env(lock_dir)?;
    run_cli_inner()
}

/// Run the CLI with an embedded OpenAPI spec and lock manifest (kept for older generated locked binaries).
pub fn run_locked_embedded(manifest_json: &str, spec_json: &str) -> Result<()> {
    let manifest: lock::LockManifest =
        serde_json::from_str(manifest_json).context("failed to parse embedded acli.lock.json")?;
    if manifest.version != 1 {
        anyhow::bail!(
            "unsupported embedded acli.lock.json version {} (expected 1)",
            manifest.version
        );
    }
    manifest.apply_to_env_with_spec_source("<embedded OpenAPI spec>")?;
    run_cli_inner_with_spec(Some(spec_json))
}

fn run_cli_inner() -> Result<()> {
    run_cli_inner_with_spec(None)
}

fn run_cli_inner_with_spec(locked_spec_text: Option<&str>) -> Result<()> {
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

    let spec_text = match locked_spec_text {
        Some(text) => text.to_string(),
        None => load_spec_text(&spec_source)
            .with_context(|| format!("failed to load OpenAPI spec from '{spec_source}'"))?,
    };
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

    run_commands(
        &bin_name,
        &spec,
        &theme,
        &matches,
        command,
        bootstrap.app_config.as_ref(),
    )?;
    Ok(())
}

/// Full CLI entry including locked launcher and bootstrap command handling.
pub fn run() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if let Some(lock_dir) = launcher_lock_dir()? {
        return run_locked(&lock_dir);
    }
    if let Some(schema_idx) = find_bootstrap_subcommand(&args, "schema") {
        if schema_wants_help(&args, schema_idx) {
            println!("{}", schema_help(&executable_name(&args)));
            return Ok(());
        }
        println!("{}", config_schema_json()?);
        return Ok(());
    }
    if let Some(install_idx) = find_bootstrap_subcommand(&args, "install") {
        let argv = install_cli_argv(&args, install_idx);
        let install_cli = InstallCli::parse_from(argv);
        return run_install_command(install_cli);
    }
    if let Some(uninstall_idx) = find_bootstrap_subcommand(&args, "uninstall") {
        let argv = subcommand_only_argv(&args, uninstall_idx);
        let uninstall_cli = UninstallCli::parse_from(argv);
        return run_uninstall_command(uninstall_cli);
    }
    run_cli_inner()
}

/// Position of a standalone bootstrap command token (not a value for a preceding flag).
fn find_bootstrap_subcommand(args: &[String], command: &str) -> Option<usize> {
    let mut i = 1;
    while i < args.len() {
        if args[i] == command && !bootstrap_token_is_flag_value(args, i) {
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

fn bootstrap_token_is_flag_value(args: &[String], token_idx: usize) -> bool {
    token_idx > 0 && current_flag_consumes_following_value(args, token_idx - 1)
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
            | "config"
            | "title"
            | "color-scheme"
            | "color"
            | "server-url"
            | "server-index"
            | "server-var"
            | "bearer-token"
            | "bearer-token-env"
            | "basic-user"
            | "basic-user-env"
            | "basic-pass"
            | "basic-pass-env"
            | "api-key"
            | "api-key-env"
            | "auth"
            | "auth-env"
            | "timeout"
            | "output"
            | "acli-crate-path"
            | "crate-name"
            | "binary-name"
            | "cargo"
            | "data-dir"
            | "install-root"
            | "secrets"
            | "default-header"
    )
}

/// `argv[0]` is the program name; includes every token except the `install` word, with tokens after
/// `install` before tokens before `install` so `clap` still parses install-specific flags when
/// globals precede `install` (e.g. `acli --spec URL install --output ./out`).
fn install_cli_argv(args: &[String], install_idx: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    out.push(args[0].clone());
    out.extend(args[install_idx + 1..].iter().cloned());
    out.extend(args[1..install_idx].iter().cloned());
    out
}

fn subcommand_only_argv(args: &[String], command_idx: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len() - command_idx);
    out.push(args[0].clone());
    out.extend(args[command_idx + 1..].iter().cloned());
    out
}

fn schema_wants_help(args: &[String], schema_idx: usize) -> bool {
    args[schema_idx + 1..]
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
}

#[cfg(test)]
mod lock_bootstrap_tests {
    use super::*;

    #[test]
    fn find_install_after_globals() {
        let args = vec![
            "acli".into(),
            "--no-banner".into(),
            "--server-index".into(),
            "2".into(),
            "install".into(),
            "--output".into(),
            "/tmp/x".into(),
        ];
        assert_eq!(find_bootstrap_subcommand(&args, "install"), Some(4));
    }

    #[test]
    fn install_not_subcommand_when_value_for_spec() {
        let args = vec!["acli".into(), "--spec".into(), "install".into()];
        assert_eq!(find_bootstrap_subcommand(&args, "install"), None);
    }

    #[test]
    fn find_uninstall_after_globals() {
        let args = vec![
            "acli".into(),
            "--color".into(),
            "never".into(),
            "uninstall".into(),
            "my_service".into(),
        ];
        assert_eq!(find_bootstrap_subcommand(&args, "uninstall"), Some(3));
    }

    #[test]
    fn finds_schema_bootstrap_command() {
        let args = vec!["acli".into(), "--config".into(), "schema".into()];
        assert_eq!(find_bootstrap_subcommand(&args, "schema"), None);

        let args = vec![
            "acli".into(),
            "--color".into(),
            "never".into(),
            "schema".into(),
        ];
        assert_eq!(find_bootstrap_subcommand(&args, "schema"), Some(3));
    }

    #[test]
    fn schema_help_is_detected_after_schema_command() {
        let args = vec!["acli".into(), "schema".into(), "--help".into()];
        assert!(schema_wants_help(&args, 1));

        let args = vec!["acli".into(), "--help".into(), "schema".into()];
        assert!(!schema_wants_help(&args, 2));
    }

    #[test]
    fn install_cli_argv_merges_before_and_after() {
        let args = vec![
            "acli".into(),
            "--spec".into(),
            "s.json".into(),
            "install".into(),
            "--output".into(),
            "/out".into(),
        ];
        let v = install_cli_argv(&args, 3);
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
