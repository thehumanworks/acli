use crate::colors::ColorMode;
use anyhow::{anyhow, Result};

pub const ENV_SPEC: &str = "OPENAPI_CLI_SPEC";
pub const ENV_TITLE: &str = "OPENAPI_CLI_TITLE";
pub const ENV_COLOR_SCHEME: &str = "OPENAPI_CLI_COLOR_SCHEME";
pub const ENV_COLOR: &str = "OPENAPI_CLI_COLOR";
pub const ENV_BASE_URL: &str = "OPENAPI_CLI_BASE_URL";
pub const ENV_SERVER_VARS: &str = "OPENAPI_CLI_SERVER_VARS";
pub const ENV_BEARER_TOKEN: &str = "OPENAPI_CLI_BEARER_TOKEN";
pub const ENV_BASIC_USER: &str = "OPENAPI_CLI_BASIC_USER";
pub const ENV_BASIC_PASS: &str = "OPENAPI_CLI_BASIC_PASS";
pub const ENV_API_KEY: &str = "OPENAPI_CLI_API_KEY";
pub const ENV_TIMEOUT: &str = "OPENAPI_CLI_TIMEOUT_SECS";
pub const ENV_INSECURE: &str = "OPENAPI_CLI_INSECURE";

#[derive(Debug, Clone)]
pub struct BootstrapConfig {
    pub spec_source: Option<String>,
    pub title: Option<String>,
    pub color_scheme: Option<String>,
    pub color_mode: ColorMode,
    pub no_banner: bool,
    pub wants_help: bool,
    pub wants_version: bool,
}

impl BootstrapConfig {
    pub fn from_env_and_args(args: &[String]) -> Result<Self> {
        let mut spec_source = std::env::var(ENV_SPEC).ok();
        let mut title = std::env::var(ENV_TITLE).ok();
        let mut color_scheme = std::env::var(ENV_COLOR_SCHEME).ok();
        let mut color_value = std::env::var(ENV_COLOR).ok();
        let mut no_banner = false;
        let mut wants_help = false;
        let mut wants_version = false;

        let mut index = 1usize;
        while index < args.len() {
            let arg = &args[index];

            match arg.as_str() {
                "-h" | "--help" => wants_help = true,
                "help" => wants_help = true,
                "-V" | "--version" | "version" => wants_version = true,
                "--no-banner" => no_banner = true,
                "--spec" => {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| anyhow!("--spec requires a value"))?;
                    spec_source = Some(value.clone());
                    index += 1;
                }
                "--title" => {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| anyhow!("--title requires a value"))?;
                    title = Some(value.clone());
                    index += 1;
                }
                "--color-scheme" => {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| anyhow!("--color-scheme requires a value"))?;
                    color_scheme = Some(value.clone());
                    index += 1;
                }
                "--color" => {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| anyhow!("--color requires a value"))?;
                    color_value = Some(value.clone());
                    index += 1;
                }
                _ => {
                    if let Some(value) = arg.strip_prefix("--spec=") {
                        spec_source = Some(value.to_string());
                    } else if let Some(value) = arg.strip_prefix("--title=") {
                        title = Some(value.to_string());
                    } else if let Some(value) = arg.strip_prefix("--color-scheme=") {
                        color_scheme = Some(value.to_string());
                    } else if let Some(value) = arg.strip_prefix("--color=") {
                        color_value = Some(value.to_string());
                    }
                }
            }

            index += 1;
        }

        let color_mode = ColorMode::parse(color_value.as_deref())?;

        Ok(Self {
            spec_source,
            title,
            color_scheme,
            color_mode,
            no_banner,
            wants_help,
            wants_version,
        })
    }
}

pub fn bootstrap_help(bin_name: &str) -> String {
    format!(
        r#"{bin_name}

Build a live CLI from an OpenAPI JSON document provided through environment variables.
The spec source can be an HTTPS URL, a local file path, or an inline JSON string.

Usage:
  {bin_name} [global options] <command> [args...]

Global options:
  --spec <VALUE>           Override {ENV_SPEC}
  --title <TEXT>           Override {ENV_TITLE}
  --color-scheme <VALUE>   Override {ENV_COLOR_SCHEME} (preset: default|mono|ocean|sunset, or JSON)
  --color <MODE>           auto|always|never, overrides {ENV_COLOR}
  --no-banner              Suppress ASCII banner output
  -h, --help               Show this bootstrap help when no spec is loaded
  -V, --version            Show version

Environment:
  {ENV_SPEC}           Required unless --spec is passed.
  {ENV_TITLE}          Optional ASCII banner title.
  {ENV_COLOR_SCHEME}   Optional preset or JSON color config.
  {ENV_COLOR}          Optional color mode (auto|always|never).
  {ENV_BASE_URL}       Optional base URL override when the spec has no usable server.
  {ENV_SERVER_VARS}    Optional JSON object for server template variable overrides.
  {ENV_BEARER_TOKEN}   Default bearer token.
  {ENV_BASIC_USER}     Default HTTP basic username.
  {ENV_BASIC_PASS}     Default HTTP basic password.
  {ENV_API_KEY}        Default API key fallback.
  {ENV_TIMEOUT}        Request timeout in seconds.
  {ENV_INSECURE}       Set to true/1 to disable TLS verification.

Examples:
  export {ENV_SPEC}='https://petstore3.swagger.io/api/v3/openapi.json'
  export {ENV_TITLE}='Petstore'
  export {ENV_COLOR_SCHEME}='{{"banner":"bright-cyan bold","header":"bright-blue bold"}}'
  {bin_name} list
  {bin_name} describe list-pets
  {bin_name} list-pets --query limit=50
  {bin_name} create-pet --body-file ./pet.json
"#
    )
}
