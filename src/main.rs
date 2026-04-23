mod cli;
mod colors;
mod config;
mod execute;
mod spec;

use crate::cli::build_command;
use crate::colors::Theme;
use crate::config::{bootstrap_help, BootstrapConfig, ENV_SPEC};
use crate::execute::run;
use crate::spec::{load_spec_text, OpenApiSpec};
use anyhow::{Context, Result};
use clap::error::ErrorKind;
use figlet_rs::FIGlet;
use std::env;
use std::path::Path;

fn main() {
    if let Err(error) = real_main() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
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

    run(&bin_name, &spec, &theme, &matches, command)?;
    Ok(())
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
