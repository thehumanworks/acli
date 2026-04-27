//! Pin an OpenAPI JSON document with optional runtime config, store secrets in the OS keychain by default, and install an API-specific CLI launcher.

use crate::app_config::{load_config_source, AcliConfig, SecretConfig, SecretsModeConfig};
use crate::config::{
    env_truthy, sanitize_env_key, ENV_API_KEY, ENV_AUTH_PREFIX, ENV_BASE_URL, ENV_BASIC_PASS,
    ENV_BASIC_USER, ENV_BEARER_TOKEN, ENV_COLOR, ENV_COLOR_SCHEME, ENV_CONFIG, ENV_DATA_DIR,
    ENV_DEFAULT_HEADERS, ENV_INSECURE, ENV_INSTALL_ROOT, ENV_LOCK_DIR, ENV_NO_BANNER,
    ENV_SERVER_INDEX, ENV_SERVER_VARS, ENV_SPEC, ENV_TIMEOUT, ENV_TITLE,
};
use crate::spec::{load_spec_text, OpenApiSpec};
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use keyring::{Entry, Error as KeyringError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MANIFEST_FILE: &str = "acli.lock.json";
const SPEC_FILE: &str = "openapi.json";
const KEYCHAIN_SERVICE_PREFIX: &str = "acli-lock";
const LOCKS_DIR: &str = "locks";
const SIDECAR_SUFFIX: &str = ".acli-lock";

#[derive(Debug, Parser)]
#[command(
    name = "acli install",
    override_usage = "acli [global options] install [OPTIONS]",
    about = "Pin an OpenAPI JSON spec and config, store secrets in the keychain by default, and install an API-specific CLI launcher"
)]
pub struct InstallCli {
    /// Directory for the generated lock bundle (`openapi.json`, `acli.lock.json`)
    #[arg(long, value_name = "DIR")]
    pub output: Option<PathBuf>,

    /// acli JSON config source; accepts a local path or raw JSON
    #[arg(long, value_name = "PATH|JSON", env = ENV_CONFIG)]
    pub config: Option<String>,

    /// Deprecated compatibility option; no longer used because locked CLIs do not compile generated crates
    #[arg(long, value_name = "PATH", hide = true, default_value = "..")]
    pub acli_crate_path: String,

    /// Deprecated compatibility option; use --binary-name to set the installed command name
    #[arg(long, value_name = "NAME", hide = true)]
    pub crate_name: Option<String>,

    /// Installed command name (default: derived from the API title)
    #[arg(long, value_name = "NAME")]
    pub binary_name: Option<String>,

    /// Write the lock bundle without installing a launcher
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub no_install: bool,

    /// Deprecated compatibility option; no Cargo executable is needed
    #[arg(long, value_name = "PATH", hide = true)]
    pub cargo: Option<PathBuf>,

    /// Install root whose `bin` directory receives the locked CLI launcher
    #[arg(long, value_name = "DIR")]
    pub install_root: Option<PathBuf>,

    /// App-owned data directory for installed lock bundles
    #[arg(long, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,

    /// Where to persist sensitive values: `keychain` (default), `inline` in the manifest (not recommended), or `env` references
    #[arg(long, value_parser = ["keychain", "inline", "env"])]
    pub secrets: Option<String>,

    /// OpenAPI spec source (URL, path, or JSON); defaults to `ACLI_SPEC`
    #[arg(long, value_name = "URL|PATH|JSON")]
    pub spec: Option<String>,

    #[arg(long)]
    pub title: Option<String>,

    #[arg(long)]
    pub color_scheme: Option<String>,

    #[arg(long)]
    pub color: Option<String>,

    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub no_banner: bool,

    #[arg(long)]
    pub server_url: Option<String>,

    #[arg(long)]
    pub server_index: Option<usize>,

    #[arg(long, value_name = "NAME=VALUE", action = clap::ArgAction::Append)]
    pub server_var: Vec<String>,

    #[arg(long, value_name = "NAME=VALUE", action = clap::ArgAction::Append)]
    pub default_header: Vec<String>,

    #[arg(long)]
    pub timeout: Option<u64>,

    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub insecure: bool,

    #[arg(long)]
    pub bearer_token: Option<String>,

    /// Host environment variable to read for the default bearer token when `--secrets env` is used
    #[arg(long, value_name = "ENV_VAR")]
    pub bearer_token_env: Option<String>,

    #[arg(long)]
    pub basic_user: Option<String>,

    /// Host environment variable to read for the default basic auth username when `--secrets env` is used
    #[arg(long, value_name = "ENV_VAR")]
    pub basic_user_env: Option<String>,

    #[arg(long)]
    pub basic_pass: Option<String>,

    /// Host environment variable to read for the default basic auth password when `--secrets env` is used
    #[arg(long, value_name = "ENV_VAR")]
    pub basic_pass_env: Option<String>,

    #[arg(long)]
    pub api_key: Option<String>,

    /// Host environment variable to read for the default api key when `--secrets env` is used
    #[arg(long, value_name = "ENV_VAR")]
    pub api_key_env: Option<String>,

    #[arg(long, value_name = "SCHEME=VALUE", action = clap::ArgAction::Append)]
    pub auth: Vec<String>,

    /// Named auth env reference when `--secrets env` is used, e.g. `partner=PARTNER_TOKEN`
    #[arg(long, value_name = "SCHEME=ENV_VAR", action = clap::ArgAction::Append)]
    pub auth_env: Vec<String>,
}

#[derive(Debug, Parser)]
#[command(
    name = "acli uninstall",
    override_usage = "acli uninstall <BINARY_NAME> [OPTIONS]",
    about = "Remove an installed locked CLI launcher and its app-owned lock bundle"
)]
pub struct UninstallCli {
    /// Installed command name to remove
    #[arg(value_name = "BINARY_NAME")]
    pub binary_name: String,

    /// Install root whose `bin` directory contains the locked CLI launcher
    #[arg(long, value_name = "DIR")]
    pub install_root: Option<PathBuf>,

    /// App-owned data directory containing installed lock bundles
    #[arg(long, value_name = "DIR", env = ENV_DATA_DIR)]
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockManifest {
    pub version: u32,
    /// Path to the pinned OpenAPI JSON file, relative to the crate root
    pub spec_path: String,
    pub title: Option<String>,
    pub color_scheme: Option<String>,
    pub color: Option<String>,
    #[serde(default)]
    pub no_banner: bool,
    pub server_url: Option<String>,
    #[serde(default)]
    pub server_index: usize,
    #[serde(default)]
    pub server_vars: BTreeMap<String, String>,
    #[serde(default)]
    pub default_headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub operation_names: BTreeMap<String, String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub insecure: bool,
    /// When set, secrets are read from the platform keychain (`service` / account name = env var name).
    pub keychain_service: Option<String>,
    /// Keychain account names for `ACLI_AUTH_*` overrides (full env var names).
    #[serde(default)]
    pub keychain_auth_accounts: Vec<String>,
    #[serde(default, skip_serializing_if = "InlineSecrets::is_empty")]
    pub inline_secrets: InlineSecrets,
    #[serde(default, skip_serializing_if = "EnvSecrets::is_empty")]
    pub env_secrets: EnvSecrets,
}

fn default_timeout() -> u64 {
    30
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InlineSecrets {
    pub bearer_token: Option<String>,
    pub basic_user: Option<String>,
    pub basic_pass: Option<String>,
    pub api_key: Option<String>,
    #[serde(default)]
    pub auth: BTreeMap<String, String>,
}

impl InlineSecrets {
    fn is_empty(&self) -> bool {
        self.bearer_token.is_none()
            && self.basic_user.is_none()
            && self.basic_pass.is_none()
            && self.api_key.is_none()
            && self.auth.is_empty()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvSecrets {
    pub bearer_token: Option<String>,
    pub basic_user: Option<String>,
    pub basic_pass: Option<String>,
    pub api_key: Option<String>,
    #[serde(default)]
    pub auth: BTreeMap<String, String>,
}

impl EnvSecrets {
    fn is_empty(&self) -> bool {
        self.bearer_token.is_none()
            && self.basic_user.is_none()
            && self.basic_pass.is_none()
            && self.api_key.is_none()
            && self.auth.is_empty()
    }
}

impl LockManifest {
    pub fn apply_to_env(&self, lock_dir: &Path) -> Result<()> {
        let spec_abs = lock_dir.join(&self.spec_path);
        let spec_str = spec_abs
            .to_str()
            .ok_or_else(|| anyhow!("lock directory or spec path is not valid UTF-8"))?;
        self.apply_to_env_with_spec_source(spec_str)
    }

    pub fn apply_to_env_with_spec_source(&self, spec_source: &str) -> Result<()> {
        unsafe {
            std::env::set_var(ENV_SPEC, spec_source);
        }

        set_opt_env(ENV_TITLE, self.title.as_deref());
        set_opt_env(ENV_COLOR_SCHEME, self.color_scheme.as_deref());
        set_opt_env(ENV_COLOR, self.color.as_deref());
        set_opt_env(ENV_BASE_URL, self.server_url.as_deref());

        if self.no_banner {
            unsafe {
                std::env::set_var(ENV_NO_BANNER, "true");
            }
        }

        unsafe {
            std::env::set_var(ENV_SERVER_INDEX, self.server_index.to_string());
        }

        if !self.server_vars.is_empty() {
            let json = serde_json::to_string(&self.server_vars)?;
            unsafe {
                std::env::set_var(ENV_SERVER_VARS, json);
            }
        }

        if !self.default_headers.is_empty() {
            let json = serde_json::to_string(&self.default_headers)?;
            unsafe {
                std::env::set_var(ENV_DEFAULT_HEADERS, json);
            }
        }

        unsafe {
            std::env::set_var(ENV_TIMEOUT, self.timeout_secs.to_string());
        }
        if self.insecure {
            unsafe {
                std::env::set_var(ENV_INSECURE, "true");
            }
        }

        if let Some(service) = &self.keychain_service {
            apply_keychain_secrets(service, &self.keychain_auth_accounts)?;
        }

        apply_env_secret_refs(&self.env_secrets);

        if let Some(token) = &self.inline_secrets.bearer_token {
            unsafe {
                std::env::set_var(ENV_BEARER_TOKEN, token);
            }
        }
        if let Some(user) = &self.inline_secrets.basic_user {
            unsafe {
                std::env::set_var(ENV_BASIC_USER, user);
            }
        }
        if let Some(pass) = &self.inline_secrets.basic_pass {
            unsafe {
                std::env::set_var(ENV_BASIC_PASS, pass);
            }
        }
        if let Some(key) = &self.inline_secrets.api_key {
            unsafe {
                std::env::set_var(ENV_API_KEY, key);
            }
        }
        for (scheme, value) in &self.inline_secrets.auth {
            let env_key = format!("{ENV_AUTH_PREFIX}{}", sanitize_env_key(scheme));
            unsafe {
                std::env::set_var(env_key, value);
            }
        }

        Ok(())
    }
}

fn set_opt_env(key: &str, value: Option<&str>) {
    unsafe {
        match value {
            Some(v) if !v.is_empty() => std::env::set_var(key, v),
            _ => {}
        }
    }
}

fn apply_keychain_secrets(service: &str, auth_accounts: &[String]) -> Result<()> {
    let mut accounts: Vec<&str> = vec![
        ENV_BEARER_TOKEN,
        ENV_BASIC_USER,
        ENV_BASIC_PASS,
        ENV_API_KEY,
    ];
    for a in auth_accounts {
        accounts.push(a.as_str());
    }

    for username in accounts {
        let entry = Entry::new(service, username)
            .with_context(|| format!("failed to open keychain entry '{service}' / '{username}'"))?;
        if let Ok(secret) = entry.get_password() {
            if !secret.is_empty() {
                unsafe {
                    std::env::set_var(username, secret);
                }
            }
        }
    }
    Ok(())
}

fn apply_env_secret_refs(env_secrets: &EnvSecrets) {
    set_env_from_host_ref(ENV_BEARER_TOKEN, env_secrets.bearer_token.as_deref());
    set_env_from_host_ref(ENV_BASIC_USER, env_secrets.basic_user.as_deref());
    set_env_from_host_ref(ENV_BASIC_PASS, env_secrets.basic_pass.as_deref());
    set_env_from_host_ref(ENV_API_KEY, env_secrets.api_key.as_deref());
    for (scheme, host_env) in &env_secrets.auth {
        let target_env = format!("{ENV_AUTH_PREFIX}{}", sanitize_env_key(scheme));
        set_env_from_host_ref(&target_env, Some(host_env));
    }
}

fn set_env_from_host_ref(target_env: &str, host_env: Option<&str>) {
    let Some(host_env) = host_env.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    if let Ok(value) = std::env::var(host_env) {
        if !value.is_empty() {
            unsafe {
                std::env::set_var(target_env, value);
            }
        }
    }
}

pub fn read_manifest(lock_dir: &Path) -> Result<LockManifest> {
    let path = lock_dir.join(MANIFEST_FILE);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read lock manifest '{}'", path.display()))?;
    let manifest: LockManifest =
        serde_json::from_str(&text).context("failed to parse acli.lock.json")?;
    if manifest.version != 1 {
        bail!(
            "unsupported acli.lock.json version {} (expected 1)",
            manifest.version
        );
    }
    Ok(manifest)
}

pub fn run_install_command(cli: InstallCli) -> Result<()> {
    run_install_command_inner(cli)
}

pub fn run_uninstall_command(cli: UninstallCli) -> Result<()> {
    run_uninstall_command_inner(cli)
}

pub fn launcher_lock_dir() -> Result<Option<PathBuf>> {
    if let Some(lock_dir) = env::var_os(ENV_LOCK_DIR).filter(|value| !value.is_empty()) {
        return Ok(Some(PathBuf::from(lock_dir)));
    }

    let exe = env::current_exe().context("failed to determine current executable path")?;
    read_lock_dir_sidecar(&launcher_sidecar_path(&exe))
}

fn run_install_command_inner(cli: InstallCli) -> Result<()> {
    let config = cli.config.as_deref().map(load_config_source).transpose()?;
    let config = config.as_ref();

    let no_install = cli.no_install
        || config
            .and_then(|config| config.install.no_install)
            .unwrap_or(false);
    let no_banner = cli.no_banner
        || config
            .and_then(|config| config.cli.no_banner)
            .unwrap_or_else(|| env_truthy(ENV_NO_BANNER));

    let spec_source = cli
        .spec
        .clone()
        .or_else(|| config.and_then(|config| config.spec.clone()))
        .or_else(|| env::var(ENV_SPEC).ok())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("missing spec; pass --spec or set {ENV_SPEC}"))?;

    let spec_text = load_spec_text(&spec_source)
        .with_context(|| format!("failed to load OpenAPI spec from '{spec_source}'"))?;
    let mut parsed_spec = OpenApiSpec::from_json_with_source(&spec_text, Some(&spec_source))
        .with_context(|| format!("failed to parse OpenAPI spec from '{spec_source}'"))?;
    let operation_names = config
        .map(|config| config.cli.operation_names.clone())
        .unwrap_or_default();
    parsed_spec.apply_operation_name_overrides(&operation_names)?;
    let api_title = parsed_spec.info.title.clone().unwrap_or_else(|| "api".to_string());

    let binary_name = cli
        .binary_name
        .clone()
        .or_else(|| cli.crate_name.clone().map(|name| name.replace('-', "_")))
        .or_else(|| config.and_then(|config| config.cli.binary_name.clone()))
        .unwrap_or_else(|| slugify_binary_name(&api_title));
    validate_binary_name(&binary_name)?;

    let output = cli
        .output
        .clone()
        .or_else(|| config.and_then(|config| config.install.output.as_ref().map(PathBuf::from)))
        .unwrap_or_else(|| PathBuf::from("."));

    fs::create_dir_all(&output)
        .with_context(|| format!("failed to create {}", output.display()))?;

    fs::write(output.join(SPEC_FILE), spec_text.as_bytes())
        .with_context(|| format!("failed to write {SPEC_FILE}"))?;

    let server_vars = merge_server_vars(&cli, config)?;
    let default_headers = merge_default_headers(&cli, config)?;

    let mut keychain_auth_accounts = Vec::new();
    let mut inline = InlineSecrets::default();
    let mut env_secrets = EnvSecrets::default();
    let secrets = cli
        .secrets
        .clone()
        .or_else(|| {
            config.and_then(|config| {
                config
                    .install
                    .secrets
                    .as_ref()
                    .map(SecretsModeConfig::as_str)
                    .map(str::to_string)
            })
        })
        .unwrap_or_else(|| {
            if config_has_env_secret_refs(config) {
                "env".to_string()
            } else {
                "keychain".to_string()
            }
        });
    let keychain_service = match secrets.as_str() {
        "inline" => {
            reject_env_secret_refs(&cli, config)?;
            store_inline_from_cli(&cli, config, &mut inline)?;
            None
        }
        "env" => {
            reject_literal_secret_values(&cli, config)?;
            store_env_refs_from_cli(&cli, config, &mut env_secrets)?;
            None
        }
        _ => {
            reject_env_secret_refs(&cli, config)?;
            let service = format!(
                "{}-{}-{}",
                KEYCHAIN_SERVICE_PREFIX,
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0)
            );
            store_secrets_in_keychain(
                &service,
                &cli,
                config,
                &mut inline,
                &mut keychain_auth_accounts,
            )?;
            Some(service)
        }
    };

    let manifest = LockManifest {
        version: 1,
        spec_path: SPEC_FILE.to_string(),
        title: cli
            .title
            .clone()
            .or_else(|| config.and_then(|config| config.cli.title.clone()))
            .or_else(|| env::var(ENV_TITLE).ok()),
        color_scheme: cli
            .color_scheme
            .clone()
            .or_else(|| config.and_then(|config| config.cli.color_scheme.clone()))
            .or_else(|| env::var(ENV_COLOR_SCHEME).ok()),
        color: cli
            .color
            .clone()
            .or_else(|| {
                config.and_then(|config| {
                    config
                        .cli
                        .color
                        .as_ref()
                        .map(|mode| mode.as_str().to_string())
                })
            })
            .or_else(|| env::var(ENV_COLOR).ok()),
        no_banner,
        server_url: cli
            .server_url
            .clone()
            .or_else(|| config.and_then(|config| config.server.url.clone()))
            .or_else(|| env::var(ENV_BASE_URL).ok()),
        server_index: cli
            .server_index
            .or_else(|| config.and_then(|config| config.server.index))
            .or_else(|| {
                env::var(ENV_SERVER_INDEX)
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or(0),
        server_vars,
        default_headers,
        operation_names,
        timeout_secs: cli
            .timeout
            .or_else(|| config.and_then(|config| config.http.timeout_secs))
            .or_else(|| {
                env::var(ENV_TIMEOUT)
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .unwrap_or(30),
        insecure: cli.insecure
            || config
                .and_then(|config| config.http.insecure)
                .unwrap_or_else(|| env_truthy(ENV_INSECURE)),
        keychain_service,
        keychain_auth_accounts,
        inline_secrets: inline,
        env_secrets,
    };

    let manifest_path = output.join(MANIFEST_FILE);
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest)?.as_bytes(),
    )
    .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    if no_install {
        eprintln!(
            "Wrote API-specific lock bundle under {}:\n  - {}\n  - {}\n\nInstall later:\n  acli install --output {} --spec <URL|PATH|JSON>",
            output.display(),
            SPEC_FILE,
            MANIFEST_FILE,
            output.display(),
        );
    } else {
        let install_root = cli.install_root.as_deref().or_else(|| {
            config.and_then(|config| config.install.install_root.as_deref().map(Path::new))
        });
        let data_dir = cli.data_dir.as_deref().or_else(|| {
            config.and_then(|config| config.install.data_dir.as_deref().map(Path::new))
        });
        let installed = install_lock_bundle(&output, &binary_name, install_root, data_dir)?;
        eprintln!(
            "Wrote lock bundle under {}\nInstalled API-specific CLI '{}' at {}\nInstalled lock data at {}",
            output.display(),
            binary_name,
            installed.executable.display(),
            installed.lock_dir.display(),
        );
    }

    Ok(())
}

fn run_uninstall_command_inner(cli: UninstallCli) -> Result<()> {
    validate_binary_name(&cli.binary_name)?;
    let removed = uninstall_locked_cli(
        &cli.binary_name,
        cli.install_root.as_deref(),
        cli.data_dir.as_deref(),
    )?;
    eprintln!(
        "Removed locked CLI '{}':\n  - launcher: {}\n  - lock data: {}",
        cli.binary_name,
        removed.executable.display(),
        removed.lock_dir.display(),
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstalledLock {
    executable: PathBuf,
    sidecar: PathBuf,
    lock_dir: PathBuf,
}

fn install_lock_bundle(
    bundle_dir: &Path,
    binary_name: &str,
    install_root: Option<&Path>,
    data_dir: Option<&Path>,
) -> Result<InstalledLock> {
    let runtime = env::current_exe().context("failed to determine current executable path")?;
    install_lock_bundle_from_runtime(bundle_dir, &runtime, binary_name, install_root, data_dir)
}

fn install_lock_bundle_from_runtime(
    bundle_dir: &Path,
    runtime: &Path,
    binary_name: &str,
    install_root: Option<&Path>,
    data_dir: Option<&Path>,
) -> Result<InstalledLock> {
    validate_binary_name(binary_name)?;
    let lock_dir = lock_bundle_dir(&resolve_data_dir(data_dir)?, binary_name);
    let install_bin = resolve_install_root(install_root)?.join("bin");
    let executable = install_bin.join(installed_executable_name(binary_name));
    let sidecar = launcher_sidecar_path(&executable);

    fs::create_dir_all(&install_bin).with_context(|| {
        format!(
            "failed to create install bin dir '{}'",
            install_bin.display()
        )
    })?;
    if paths_refer_to_same_file(runtime, &executable)? {
        bail!(
            "installing locked CLI '{}' would overwrite the running acli runtime; choose a different --binary-name or --install-root",
            binary_name
        );
    }
    if lock_dir.exists() {
        if let Ok(manifest) = read_manifest(&lock_dir) {
            delete_keychain_secrets(&manifest)?;
        }
        fs::remove_dir_all(&lock_dir).with_context(|| {
            format!(
                "failed to replace existing lock bundle '{}'",
                lock_dir.display()
            )
        })?;
    }
    fs::create_dir_all(&lock_dir)
        .with_context(|| format!("failed to create lock data dir '{}'", lock_dir.display()))?;

    copy_bundle_file(bundle_dir, &lock_dir, SPEC_FILE)?;
    copy_bundle_file(bundle_dir, &lock_dir, MANIFEST_FILE)?;
    fs::copy(runtime, &executable).with_context(|| {
        format!(
            "failed to install runtime '{}' to '{}'",
            runtime.display(),
            executable.display()
        )
    })?;
    make_executable(&executable)?;
    write_lock_dir_sidecar(&sidecar, &lock_dir)?;

    Ok(InstalledLock {
        executable,
        sidecar,
        lock_dir,
    })
}

fn uninstall_locked_cli(
    binary_name: &str,
    install_root: Option<&Path>,
    data_dir: Option<&Path>,
) -> Result<InstalledLock> {
    validate_binary_name(binary_name)?;
    let executable = resolve_install_root(install_root)?
        .join("bin")
        .join(installed_executable_name(binary_name));
    let sidecar = launcher_sidecar_path(&executable);
    let lock_dir = match read_lock_dir_sidecar(&sidecar)? {
        Some(lock_dir) => lock_dir,
        None => lock_bundle_dir(&resolve_data_dir(data_dir)?, binary_name),
    };

    if lock_dir.exists() {
        if let Ok(manifest) = read_manifest(&lock_dir) {
            delete_keychain_secrets(&manifest)?;
        }
        fs::remove_dir_all(&lock_dir)
            .with_context(|| format!("failed to remove lock data '{}'", lock_dir.display()))?;
    }
    remove_file_if_exists(&sidecar)?;
    remove_file_if_exists(&executable)?;

    Ok(InstalledLock {
        executable,
        sidecar,
        lock_dir,
    })
}

fn copy_bundle_file(from_dir: &Path, to_dir: &Path, name: &str) -> Result<()> {
    let from = from_dir.join(name);
    let to = to_dir.join(name);
    fs::copy(&from, &to)
        .with_context(|| format!("failed to copy '{}' to '{}'", from.display(), to.display()))?;
    Ok(())
}

fn lock_bundle_dir(data_dir: &Path, binary_name: &str) -> PathBuf {
    data_dir.join(LOCKS_DIR).join(binary_name)
}

fn installed_executable_name(binary_name: &str) -> String {
    #[cfg(windows)]
    {
        if binary_name.to_ascii_lowercase().ends_with(".exe") {
            binary_name.to_string()
        } else {
            format!("{binary_name}.exe")
        }
    }
    #[cfg(not(windows))]
    {
        binary_name.to_string()
    }
}

fn launcher_sidecar_path(executable: &Path) -> PathBuf {
    let file_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("acli");
    executable.with_file_name(format!("{file_name}{SIDECAR_SUFFIX}"))
}

fn write_lock_dir_sidecar(sidecar: &Path, lock_dir: &Path) -> Result<()> {
    let lock_dir = absolute_existing_or_parent_path(lock_dir)?;
    let lock_dir = lock_dir
        .to_str()
        .ok_or_else(|| anyhow!("lock data path is not valid UTF-8"))?;
    fs::write(sidecar, format!("{lock_dir}\n"))
        .with_context(|| format!("failed to write launcher sidecar '{}'", sidecar.display()))?;
    Ok(())
}

fn read_lock_dir_sidecar(sidecar: &Path) -> Result<Option<PathBuf>> {
    if !sidecar.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(sidecar)
        .with_context(|| format!("failed to read launcher sidecar '{}'", sidecar.display()))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!(
            "launcher sidecar '{}' does not contain a lock data path",
            sidecar.display()
        );
    }
    Ok(Some(PathBuf::from(trimmed)))
}

fn resolve_install_root(install_root: Option<&Path>) -> Result<PathBuf> {
    if let Some(root) = install_root {
        return absolute_existing_or_parent_path(root);
    }
    if let Some(root) = env::var_os(ENV_INSTALL_ROOT).filter(|value| !value.is_empty()) {
        return absolute_existing_or_parent_path(Path::new(&root));
    }
    default_install_root()
}

fn default_install_root() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(root) = env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
            Ok(PathBuf::from(root).join("acli"))
        } else {
            Ok(home_dir()?.join("AppData").join("Local").join("acli"))
        }
    }

    #[cfg(not(windows))]
    {
        Ok(home_dir()?.join(".local"))
    }
}

fn resolve_data_dir(data_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(dir) = data_dir {
        return absolute_existing_or_parent_path(dir);
    }
    if let Some(dir) = env::var_os(ENV_DATA_DIR).filter(|value| !value.is_empty()) {
        return absolute_existing_or_parent_path(Path::new(&dir));
    }
    default_data_dir()
}

fn default_data_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(home_dir()?
            .join("Library")
            .join("Application Support")
            .join("acli"))
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(dir) = env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
            Ok(PathBuf::from(dir).join("acli"))
        } else if let Some(dir) = env::var_os("APPDATA").filter(|value| !value.is_empty()) {
            Ok(PathBuf::from(dir).join("acli"))
        } else {
            Ok(home_dir()?.join("AppData").join("Local").join("acli"))
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(dir) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
            Ok(PathBuf::from(dir).join("acli"))
        } else {
            Ok(home_dir()?.join(".local").join("share").join("acli"))
        }
    }
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("could not determine home directory"))
}

fn absolute_existing_or_parent_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()
            .context("failed to determine current directory")?
            .join(path))
    }
}

fn validate_binary_name(binary_name: &str) -> Result<()> {
    let trimmed = binary_name.trim();
    if trimmed.is_empty() {
        bail!("binary name cannot be empty");
    }
    if trimmed == "." || trimmed == ".." || trimmed.contains('/') || trimmed.contains('\\') {
        bail!("binary name must be a command name, not a path");
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to remove '{}'", path.display())),
    }
}

fn paths_refer_to_same_file(a: &Path, b: &Path) -> Result<bool> {
    if !a.exists() || !b.exists() {
        return Ok(false);
    }
    let a = fs::canonicalize(a).with_context(|| format!("failed to resolve '{}'", a.display()))?;
    let b = fs::canonicalize(b).with_context(|| format!("failed to resolve '{}'", b.display()))?;
    Ok(a == b)
}

fn delete_keychain_secrets(manifest: &LockManifest) -> Result<()> {
    let Some(service) = manifest.keychain_service.as_deref() else {
        return Ok(());
    };
    let mut accounts: Vec<&str> = vec![
        ENV_BEARER_TOKEN,
        ENV_BASIC_USER,
        ENV_BASIC_PASS,
        ENV_API_KEY,
    ];
    for account in &manifest.keychain_auth_accounts {
        accounts.push(account);
    }
    for account in accounts {
        let entry = Entry::new(service, account)
            .with_context(|| format!("failed to open keychain entry '{service}' / '{account}'"))?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to delete keychain secret '{account}'"));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .with_context(|| format!("failed to read permissions for '{}'", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to mark '{}' executable", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn slugify_crate_name(title: &str) -> String {
    let mut s: String = title
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s = s.trim_matches('-').to_string();
    if s.is_empty()
        || !s
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
    {
        s = "api-cli".to_string();
    }
    s
}

fn slugify_binary_name(title: &str) -> String {
    slugify_crate_name(title).replace('-', "_")
}

fn merge_server_vars(
    cli: &InstallCli,
    config: Option<&AcliConfig>,
) -> Result<BTreeMap<String, String>> {
    let mut map =
        parse_json_object_string(env::var(ENV_SERVER_VARS).ok().as_deref(), ENV_SERVER_VARS)?;
    if let Some(config) = config {
        map.extend(config.server.vars.clone());
    }
    for pair in &cli.server_var {
        let (k, v) = parse_one_pair(pair, "server-var")?;
        map.insert(k, v);
    }
    Ok(map)
}

fn merge_default_headers(
    cli: &InstallCli,
    config: Option<&AcliConfig>,
) -> Result<BTreeMap<String, String>> {
    let mut map = parse_json_object_string(
        env::var(ENV_DEFAULT_HEADERS).ok().as_deref(),
        ENV_DEFAULT_HEADERS,
    )?;
    if let Some(config) = config {
        map.extend(config.http.default_headers.clone());
    }
    for pair in &cli.default_header {
        let (k, v) = parse_one_pair(pair, "default-header")?;
        map.insert(k, v);
    }
    Ok(map)
}

fn parse_json_object_string(raw: Option<&str>, label: &str) -> Result<BTreeMap<String, String>> {
    let Some(trimmed) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(BTreeMap::new());
    };
    let value: Value =
        serde_json::from_str(trimmed).with_context(|| format!("{label} must be a JSON object"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("{label} must be a JSON object"))?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        if let Some(s) = v.as_str() {
            out.insert(k.clone(), s.to_string());
        } else {
            out.insert(k.clone(), v.to_string());
        }
    }
    Ok(out)
}

fn parse_one_pair(pair: &str, flag: &str) -> Result<(String, String)> {
    let (k, v) = pair
        .split_once('=')
        .ok_or_else(|| anyhow!("{flag} expects NAME=VALUE, got '{pair}'"))?;
    if k.trim().is_empty() {
        bail!("{flag} expects a non-empty name before '='");
    }
    Ok((k.trim().to_string(), v.to_string()))
}

fn store_inline_from_cli(
    cli: &InstallCli,
    config: Option<&AcliConfig>,
    inline: &mut InlineSecrets,
) -> Result<()> {
    inline.bearer_token = secret_value(
        cli.bearer_token.as_deref(),
        config.and_then(|config| secret_literal(&config.auth.bearer_token)),
        ENV_BEARER_TOKEN,
    );
    inline.basic_user = secret_value(
        cli.basic_user.as_deref(),
        config.and_then(|config| secret_literal(&config.auth.basic_user)),
        ENV_BASIC_USER,
    );
    inline.basic_pass = secret_value(
        cli.basic_pass.as_deref(),
        config.and_then(|config| secret_literal(&config.auth.basic_pass)),
        ENV_BASIC_PASS,
    );
    inline.api_key = secret_value(
        cli.api_key.as_deref(),
        config.and_then(|config| secret_literal(&config.auth.api_key)),
        ENV_API_KEY,
    );
    if let Some(config) = config {
        for (scheme, secret) in &config.auth.named {
            if let Some(value) = secret.literal_value() {
                inline.auth.insert(scheme.clone(), value);
            }
        }
    }
    for pair in &cli.auth {
        let (scheme, value) = parse_one_pair(pair, "auth")?;
        inline.auth.insert(scheme, value);
    }
    Ok(())
}

fn secret_value(
    cli_value: Option<&str>,
    config_value: Option<String>,
    env_key: &str,
) -> Option<String> {
    cli_value
        .map(ToOwned::to_owned)
        .or(config_value)
        .or_else(|| env::var(env_key).ok())
        .filter(|value| !value.is_empty())
}

fn secret_literal(secret: &Option<SecretConfig>) -> Option<String> {
    secret.as_ref().and_then(SecretConfig::literal_value)
}

fn secret_env_ref(secret: &Option<SecretConfig>) -> Option<String> {
    secret.as_ref().and_then(SecretConfig::env_ref)
}

fn config_has_env_secret_refs(config: Option<&AcliConfig>) -> bool {
    let Some(config) = config else {
        return false;
    };
    config
        .auth
        .bearer_token
        .as_ref()
        .and_then(SecretConfig::env_ref)
        .is_some()
        || config
            .auth
            .basic_user
            .as_ref()
            .and_then(SecretConfig::env_ref)
            .is_some()
        || config
            .auth
            .basic_pass
            .as_ref()
            .and_then(SecretConfig::env_ref)
            .is_some()
        || config
            .auth
            .api_key
            .as_ref()
            .and_then(SecretConfig::env_ref)
            .is_some()
        || config
            .auth
            .named
            .values()
            .any(|secret| secret.env_ref().is_some())
}

fn store_env_refs_from_cli(
    cli: &InstallCli,
    config: Option<&AcliConfig>,
    env_secrets: &mut EnvSecrets,
) -> Result<()> {
    env_secrets.bearer_token =
        normalize_env_ref(cli.bearer_token_env.as_deref(), "bearer-token-env")?
            .or_else(|| config.and_then(|config| secret_env_ref(&config.auth.bearer_token)));
    env_secrets.basic_user = normalize_env_ref(cli.basic_user_env.as_deref(), "basic-user-env")?
        .or_else(|| config.and_then(|config| secret_env_ref(&config.auth.basic_user)));
    env_secrets.basic_pass = normalize_env_ref(cli.basic_pass_env.as_deref(), "basic-pass-env")?
        .or_else(|| config.and_then(|config| secret_env_ref(&config.auth.basic_pass)));
    env_secrets.api_key = normalize_env_ref(cli.api_key_env.as_deref(), "api-key-env")?
        .or_else(|| config.and_then(|config| secret_env_ref(&config.auth.api_key)));
    if let Some(config) = config {
        for (scheme, secret) in &config.auth.named {
            if let Some(env_name) = secret.env_ref() {
                env_secrets.auth.insert(scheme.clone(), env_name);
            }
        }
    }
    for pair in &cli.auth_env {
        let (scheme, env_name) = parse_one_pair(pair, "auth-env")?;
        let env_name = normalize_env_ref(Some(&env_name), "auth-env")?
            .ok_or_else(|| anyhow!("auth-env expects a non-empty environment variable name"))?;
        env_secrets.auth.insert(scheme, env_name);
    }
    Ok(())
}

fn normalize_env_ref(value: Option<&str>, flag: &str) -> Result<Option<String>> {
    let Some(trimmed) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if trimmed.contains('=') {
        bail!("{flag} expects an environment variable name, not NAME=VALUE");
    }
    Ok(Some(trimmed.to_string()))
}

fn reject_env_secret_refs(cli: &InstallCli, config: Option<&AcliConfig>) -> Result<()> {
    if cli.bearer_token_env.is_some()
        || cli.basic_user_env.is_some()
        || cli.basic_pass_env.is_some()
        || cli.api_key_env.is_some()
        || !cli.auth_env.is_empty()
    {
        bail!("secret environment reference flags require --secrets env");
    }
    if let Some(config) = config {
        let has_env_ref = config
            .auth
            .bearer_token
            .as_ref()
            .and_then(SecretConfig::env_ref)
            .is_some()
            || config
                .auth
                .basic_user
                .as_ref()
                .and_then(SecretConfig::env_ref)
                .is_some()
            || config
                .auth
                .basic_pass
                .as_ref()
                .and_then(SecretConfig::env_ref)
                .is_some()
            || config
                .auth
                .api_key
                .as_ref()
                .and_then(SecretConfig::env_ref)
                .is_some()
            || config
                .auth
                .named
                .values()
                .any(|secret| secret.env_ref().is_some());
        if has_env_ref {
            bail!("auth env references require install.secrets env");
        }
    }
    Ok(())
}

fn reject_literal_secret_values(cli: &InstallCli, config: Option<&AcliConfig>) -> Result<()> {
    if cli.bearer_token.is_some()
        || cli.basic_user.is_some()
        || cli.basic_pass.is_some()
        || cli.api_key.is_some()
        || !cli.auth.is_empty()
    {
        bail!("literal secret value flags cannot be used with --secrets env; use --*-env flags instead");
    }
    if let Some(config) = config {
        let has_literal = config
            .auth
            .bearer_token
            .as_ref()
            .and_then(SecretConfig::literal_value)
            .is_some()
            || config
                .auth
                .basic_user
                .as_ref()
                .and_then(SecretConfig::literal_value)
                .is_some()
            || config
                .auth
                .basic_pass
                .as_ref()
                .and_then(SecretConfig::literal_value)
                .is_some()
            || config
                .auth
                .api_key
                .as_ref()
                .and_then(SecretConfig::literal_value)
                .is_some()
            || config
                .auth
                .named
                .values()
                .any(|secret| secret.literal_value().is_some());
        if has_literal {
            bail!("literal auth values cannot be used with install.secrets env");
        }
    }
    Ok(())
}

fn store_secrets_in_keychain(
    service: &str,
    cli: &InstallCli,
    config: Option<&AcliConfig>,
    _inline: &mut InlineSecrets,
    auth_accounts: &mut Vec<String>,
) -> Result<()> {
    if let Some(token) = secret_value(
        cli.bearer_token.as_deref(),
        config.and_then(|config| secret_literal(&config.auth.bearer_token)),
        ENV_BEARER_TOKEN,
    ) {
        keychain_set(service, ENV_BEARER_TOKEN, &token)?;
    }
    if let Some(user) = secret_value(
        cli.basic_user.as_deref(),
        config.and_then(|config| secret_literal(&config.auth.basic_user)),
        ENV_BASIC_USER,
    ) {
        keychain_set(service, ENV_BASIC_USER, &user)?;
    }
    if let Some(pass) = secret_value(
        cli.basic_pass.as_deref(),
        config.and_then(|config| secret_literal(&config.auth.basic_pass)),
        ENV_BASIC_PASS,
    ) {
        keychain_set(service, ENV_BASIC_PASS, &pass)?;
    }
    if let Some(key) = secret_value(
        cli.api_key.as_deref(),
        config.and_then(|config| secret_literal(&config.auth.api_key)),
        ENV_API_KEY,
    ) {
        keychain_set(service, ENV_API_KEY, &key)?;
    }
    if let Some(config) = config {
        for (scheme, secret) in &config.auth.named {
            if let Some(value) = secret.literal_value() {
                let account = format!("{ENV_AUTH_PREFIX}{}", sanitize_env_key(scheme));
                keychain_set(service, &account, &value)?;
                auth_accounts.push(account);
            }
        }
    }
    for pair in &cli.auth {
        let (scheme, value) = parse_one_pair(pair, "auth")?;
        let account = format!("{ENV_AUTH_PREFIX}{}", sanitize_env_key(&scheme));
        keychain_set(service, &account, &value)?;
        auth_accounts.push(account);
    }
    Ok(())
}

fn keychain_set(service: &str, account: &str, secret: &str) -> Result<()> {
    let entry = Entry::new(service, account)
        .with_context(|| format!("keychain entry '{service}' / '{account}'"))?;
    entry
        .set_password(secret)
        .with_context(|| format!("failed to store secret in keychain for '{account}'"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    struct EnvVarGuard {
        name: String,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &str, value: Option<&str>) -> Self {
            let previous = std::env::var_os(name);
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
            Self {
                name: name.to_string(),
                previous,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(&self.name, value),
                    None => std::env::remove_var(&self.name),
                }
            }
        }
    }

    fn minimal_openapi_json() -> String {
                r#"{
                    "openapi": "3.0.0",
                    "info": {"title": "My Service", "version": "1"},
                    "paths": {
                        "/pets": {
                            "get": {
                                "operationId": "listPets",
                                "responses": {
                                    "200": {"description": "ok"}
                                }
                            }
                        }
                    }
                }"#
                        .to_string()
    }

    #[test]
    fn slugify_crate_name_normalizes_title() {
        assert_eq!(slugify_crate_name("Pet Store API!"), "pet-store-api");
        assert_eq!(slugify_crate_name("___"), "api-cli");
    }

    #[test]
    fn slugify_binary_name_uses_underscores() {
        assert_eq!(slugify_binary_name("Pet Store"), "pet_store");
    }

    #[test]
    fn merge_server_vars_cli_overrides_env_json() {
        let _lock = crate::ENV_LOCK.lock().unwrap();
        let _g = EnvVarGuard::set(ENV_SERVER_VARS, Some(r#"{"a":"env"}"#));
        let _g2 = EnvVarGuard::set(ENV_DEFAULT_HEADERS, None);
        let cli = InstallCli::try_parse_from(["testprog", "--server-var", "a=cli"]).expect("parse");
        let map = merge_server_vars(&cli, None).expect("merge");
        assert_eq!(map.get("a").map(String::as_str), Some("cli"));
    }

    #[test]
    fn merge_default_headers_merges_env_and_cli() {
        let _lock = crate::ENV_LOCK.lock().unwrap();
        let _g0 = EnvVarGuard::set(ENV_SERVER_VARS, None);
        let _g = EnvVarGuard::set(ENV_DEFAULT_HEADERS, Some(r#"{"X":"1"}"#));
        let cli =
            InstallCli::try_parse_from(["testprog", "--default-header", "Y=2"]).expect("parse");
        let map = merge_default_headers(&cli, None).expect("merge");
        assert_eq!(map.get("X").map(String::as_str), Some("1"));
        assert_eq!(map.get("Y").map(String::as_str), Some("2"));
    }

    #[test]
    fn read_manifest_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spec_rel = "openapi.json";
        fs::write(dir.path().join(spec_rel), minimal_openapi_json()).unwrap();
        let manifest = LockManifest {
            version: 1,
            spec_path: spec_rel.to_string(),
            title: Some("T".into()),
            color_scheme: None,
            color: None,
            no_banner: true,
            server_url: Some("https://example.test".into()),
            server_index: 2,
            server_vars: BTreeMap::from([("host".into(), "api".into())]),
            default_headers: BTreeMap::from([("X-Foo".into(), "bar".into())]),
            operation_names: BTreeMap::from([("listPets".into(), "pets-list".into())]),
            timeout_secs: 99,
            insecure: true,
            keychain_service: None,
            keychain_auth_accounts: vec![],
            inline_secrets: InlineSecrets::default(),
            env_secrets: EnvSecrets::default(),
        };
        fs::write(
            dir.path().join(MANIFEST_FILE),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let loaded = read_manifest(dir.path()).expect("read");
        assert_eq!(loaded.title.as_deref(), Some("T"));
        assert_eq!(loaded.server_index, 2);
        assert_eq!(
            loaded.server_vars.get("host").map(String::as_str),
            Some("api")
        );
        assert_eq!(
            loaded.operation_names.get("listPets").map(String::as_str),
            Some("pets-list")
        );
    }

    #[test]
    fn apply_to_env_sets_expected_vars() {
        let _lock = crate::ENV_LOCK.lock().unwrap();
        let _g_spec = EnvVarGuard::set(ENV_SPEC, None);
        let _g_title = EnvVarGuard::set(ENV_TITLE, None);
        let _g_scheme = EnvVarGuard::set(ENV_COLOR_SCHEME, None);
        let _g_color = EnvVarGuard::set(ENV_COLOR, None);
        let _g_base = EnvVarGuard::set(ENV_BASE_URL, None);
        let _g_nb = EnvVarGuard::set(ENV_NO_BANNER, None);
        let _g_idx = EnvVarGuard::set(ENV_SERVER_INDEX, None);
        let _g_sv = EnvVarGuard::set(ENV_SERVER_VARS, None);
        let _g_dh = EnvVarGuard::set(ENV_DEFAULT_HEADERS, None);
        let _g_to = EnvVarGuard::set(ENV_TIMEOUT, None);
        let _g_insec = EnvVarGuard::set(ENV_INSECURE, None);

        let dir = tempfile::tempdir().expect("tempdir");
        let spec_rel = "openapi.json";
        fs::write(dir.path().join(spec_rel), minimal_openapi_json()).unwrap();
        let manifest = LockManifest {
            version: 1,
            spec_path: spec_rel.to_string(),
            title: Some("T".into()),
            color_scheme: None,
            color: None,
            no_banner: true,
            server_url: Some("https://example.test".into()),
            server_index: 2,
            server_vars: BTreeMap::from([("host".into(), "api".into())]),
            default_headers: BTreeMap::from([("X-Foo".into(), "bar".into())]),
            operation_names: BTreeMap::new(),
            timeout_secs: 99,
            insecure: true,
            keychain_service: None,
            keychain_auth_accounts: vec![],
            inline_secrets: InlineSecrets::default(),
            env_secrets: EnvSecrets::default(),
        };

        manifest.apply_to_env(dir.path()).expect("apply");
        let spec = std::env::var(ENV_SPEC).expect("spec set");
        assert!(spec.ends_with("openapi.json"));
        assert_eq!(std::env::var(ENV_TITLE).ok().as_deref(), Some("T"));
        assert_eq!(
            std::env::var(ENV_BASE_URL).ok().as_deref(),
            Some("https://example.test")
        );
        assert_eq!(std::env::var(ENV_SERVER_INDEX).ok().as_deref(), Some("2"));
        assert_eq!(std::env::var(ENV_TIMEOUT).ok().as_deref(), Some("99"));
        assert!(env_truthy(ENV_INSECURE));
        assert!(env_truthy(ENV_NO_BANNER));
        let sv = std::env::var(ENV_SERVER_VARS).expect("server vars");
        assert!(sv.contains("host"));
        let dh = std::env::var(ENV_DEFAULT_HEADERS).expect("headers");
        assert!(dh.contains("X-Foo"));
    }

    #[test]
    fn apply_to_env_resolves_secret_env_refs_at_runtime() {
        let _lock = crate::ENV_LOCK.lock().unwrap();
        let _g_bearer = EnvVarGuard::set(ENV_BEARER_TOKEN, None);
        let _g_basic_user = EnvVarGuard::set(ENV_BASIC_USER, None);
        let _g_basic_pass = EnvVarGuard::set(ENV_BASIC_PASS, None);
        let _g_api_key = EnvVarGuard::set(ENV_API_KEY, None);
        let _g_auth = EnvVarGuard::set("ACLI_AUTH_PARTNER", None);
        let _g_host_token = EnvVarGuard::set("HOST_BEARER_TOKEN", Some("runtime-token"));
        let _g_host_user = EnvVarGuard::set("HOST_BASIC_USER", Some("runtime-user"));
        let _g_host_pass = EnvVarGuard::set("HOST_BASIC_PASS", Some(""));
        let _g_host_api = EnvVarGuard::set("HOST_API_KEY", Some("runtime-key"));
        let _g_host_auth = EnvVarGuard::set("HOST_PARTNER_TOKEN", Some("runtime-partner"));

        let dir = tempfile::tempdir().expect("tempdir");
        let spec_rel = "openapi.json";
        fs::write(dir.path().join(spec_rel), minimal_openapi_json()).unwrap();
        let manifest = LockManifest {
            version: 1,
            spec_path: spec_rel.to_string(),
            title: None,
            color_scheme: None,
            color: None,
            no_banner: false,
            server_url: None,
            server_index: 0,
            server_vars: BTreeMap::new(),
            default_headers: BTreeMap::new(),
            operation_names: BTreeMap::new(),
            timeout_secs: 30,
            insecure: false,
            keychain_service: None,
            keychain_auth_accounts: vec![],
            inline_secrets: InlineSecrets::default(),
            env_secrets: EnvSecrets {
                bearer_token: Some("HOST_BEARER_TOKEN".into()),
                basic_user: Some("HOST_BASIC_USER".into()),
                basic_pass: Some("HOST_BASIC_PASS".into()),
                api_key: Some("HOST_API_KEY".into()),
                auth: BTreeMap::from([("partner".into(), "HOST_PARTNER_TOKEN".into())]),
            },
        };

        manifest.apply_to_env(dir.path()).expect("apply");

        assert_eq!(
            std::env::var(ENV_BEARER_TOKEN).ok().as_deref(),
            Some("runtime-token")
        );
        assert_eq!(
            std::env::var(ENV_BASIC_USER).ok().as_deref(),
            Some("runtime-user")
        );
        assert_eq!(std::env::var(ENV_BASIC_PASS).ok(), None);
        assert_eq!(
            std::env::var(ENV_API_KEY).ok().as_deref(),
            Some("runtime-key")
        );
        assert_eq!(
            std::env::var("ACLI_AUTH_PARTNER").ok().as_deref(),
            Some("runtime-partner")
        );
    }

    #[test]
    fn env_secret_refs_are_serialized_from_install_cli() {
        let _lock = crate::ENV_LOCK.lock().unwrap();
        let _g_api_key = EnvVarGuard::set(ENV_API_KEY, None);
        let dir = tempfile::tempdir().expect("tempdir");
        let spec_path = dir.path().join("source.json");
        fs::write(&spec_path, minimal_openapi_json()).unwrap();
        let out = dir.path().join("out");

        let cli = InstallCli::try_parse_from([
            "testprog",
            "--no-install",
            "--output",
            out.to_str().unwrap(),
            "--spec",
            spec_path.to_str().unwrap(),
            "--secrets",
            "env",
            "--bearer-token-env",
            "HOST_BEARER_TOKEN",
            "--api-key-env",
            "HOST_API_KEY",
            "--auth-env",
            "partner=HOST_PARTNER_TOKEN",
        ])
        .expect("parse");

        run_install_command_inner(cli).expect("install");
        let manifest = read_manifest(&out).expect("manifest");

        assert_eq!(
            manifest.env_secrets.bearer_token.as_deref(),
            Some("HOST_BEARER_TOKEN")
        );
        assert_eq!(
            manifest.env_secrets.api_key.as_deref(),
            Some("HOST_API_KEY")
        );
        assert_eq!(
            manifest.env_secrets.auth.get("partner").map(String::as_str),
            Some("HOST_PARTNER_TOKEN")
        );
        assert!(manifest.inline_secrets.is_empty());
        assert!(manifest.keychain_service.is_none());
    }

    #[test]
    fn install_config_writes_lock_manifest_and_cli_overrides_config() {
        let _lock = crate::ENV_LOCK.lock().unwrap();
        let _g_spec = EnvVarGuard::set(ENV_SPEC, None);
        let _g_headers = EnvVarGuard::set(ENV_DEFAULT_HEADERS, Some(r#"{"X-Mode":"env"}"#));
        let _g_server_vars = EnvVarGuard::set(ENV_SERVER_VARS, Some(r#"{"region":"env"}"#));
        let dir = tempfile::tempdir().expect("tempdir");
        let spec_path = dir.path().join("source.json");
        let out = dir.path().join("out");
        let config_path = dir.path().join("acli.json");
        fs::write(&spec_path, minimal_openapi_json()).unwrap();
        fs::write(
            &config_path,
            format!(
                r#"{{
                  "version": 1,
                  "spec": {spec:?},
                  "cli": {{
                    "binaryName": "config_service",
                    "title": "Config Title",
                                        "colorScheme": "ocean",
                                        "operationNames": {{"listPets": "pets-list"}}
                  }},
                  "server": {{
                    "url": "https://config.example.test",
                    "index": 4,
                    "vars": {{"region": "config"}}
                  }},
                  "http": {{
                    "timeoutSecs": 44,
                    "defaultHeaders": {{"X-Mode": "config"}}
                  }},
                  "auth": {{
                    "bearerToken": {{"env": "HOST_BEARER"}},
                    "named": {{"partner": {{"env": "HOST_PARTNER"}}}}
                  }},
                  "install": {{
                    "output": {out:?},
                    "noInstall": true
                  }}
                }}"#,
                spec = spec_path.to_str().unwrap(),
                out = out.to_str().unwrap()
            ),
        )
        .unwrap();

        let cli = InstallCli::try_parse_from([
            "testprog",
            "--config",
            config_path.to_str().unwrap(),
            "--server-var",
            "region=cli",
            "--default-header",
            "X-Mode=cli",
            "--timeout",
            "55",
        ])
        .expect("parse");

        run_install_command_inner(cli).expect("install");
        let manifest = read_manifest(&out).expect("manifest");

        assert_eq!(manifest.title.as_deref(), Some("Config Title"));
        assert_eq!(manifest.color_scheme.as_deref(), Some("ocean"));
        assert_eq!(
            manifest.server_url.as_deref(),
            Some("https://config.example.test")
        );
        assert_eq!(manifest.server_index, 4);
        assert_eq!(
            manifest.server_vars.get("region").map(String::as_str),
            Some("cli")
        );
        assert_eq!(
            manifest.default_headers.get("X-Mode").map(String::as_str),
            Some("cli")
        );
        assert_eq!(manifest.timeout_secs, 55);
        assert_eq!(
            manifest.env_secrets.bearer_token.as_deref(),
            Some("HOST_BEARER")
        );
        assert_eq!(
            manifest.env_secrets.auth.get("partner").map(String::as_str),
            Some("HOST_PARTNER")
        );
        assert_eq!(
            manifest.operation_names.get("listPets").map(String::as_str),
            Some("pets-list")
        );
    }

    fn write_test_bundle(dir: &Path) {
        fs::create_dir_all(dir).expect("bundle dir");
        fs::write(dir.join(SPEC_FILE), minimal_openapi_json()).expect("spec");
        let manifest = LockManifest {
            version: 1,
            spec_path: SPEC_FILE.to_string(),
            title: Some("My Service".into()),
            color_scheme: None,
            color: None,
            no_banner: false,
            server_url: None,
            server_index: 0,
            server_vars: BTreeMap::new(),
            default_headers: BTreeMap::new(),
            operation_names: BTreeMap::new(),
            timeout_secs: 30,
            insecure: false,
            keychain_service: None,
            keychain_auth_accounts: vec![],
            inline_secrets: InlineSecrets::default(),
            env_secrets: EnvSecrets::default(),
        };
        fs::write(
            dir.join(MANIFEST_FILE),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .expect("manifest");
    }

    #[test]
    fn install_lock_bundle_copies_runtime_and_bundle_without_cargo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = dir.path().join("bundle");
        let runtime = dir.path().join("acli-runtime");
        let install_root = dir.path().join("install-root");
        let data_dir = dir.path().join("data");
        write_test_bundle(&bundle);
        fs::write(&runtime, "runtime").expect("runtime");

        let installed = install_lock_bundle_from_runtime(
            &bundle,
            &runtime,
            "my_service",
            Some(&install_root),
            Some(&data_dir),
        )
        .expect("install");

        assert_eq!(
            installed.executable,
            install_root
                .join("bin")
                .join(installed_executable_name("my_service"))
        );
        assert_eq!(
            installed.lock_dir,
            data_dir.join("locks").join("my_service")
        );
        assert!(installed.lock_dir.join(MANIFEST_FILE).exists());
        assert!(installed.lock_dir.join(SPEC_FILE).exists());
        assert_eq!(
            fs::read_to_string(&installed.executable).unwrap(),
            "runtime"
        );
        assert_eq!(
            read_lock_dir_sidecar(&launcher_sidecar_path(&installed.executable))
                .unwrap()
                .unwrap(),
            installed.lock_dir
        );
    }

    #[test]
    fn uninstall_removes_launcher_sidecar_and_bundle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = dir.path().join("bundle");
        let runtime = dir.path().join("acli-runtime");
        let install_root = dir.path().join("install-root");
        let data_dir = dir.path().join("data");
        write_test_bundle(&bundle);
        fs::write(&runtime, "runtime").expect("runtime");

        let installed = install_lock_bundle_from_runtime(
            &bundle,
            &runtime,
            "my_service",
            Some(&install_root),
            Some(&data_dir),
        )
        .expect("install");
        let sidecar = launcher_sidecar_path(&installed.executable);

        let cli = UninstallCli::try_parse_from([
            "testprog",
            "my_service",
            "--install-root",
            install_root.to_str().unwrap(),
            "--data-dir",
            data_dir.to_str().unwrap(),
        ])
        .expect("parse");

        run_uninstall_command_inner(cli).expect("uninstall");

        assert!(!installed.executable.exists());
        assert!(!sidecar.exists());
        assert!(!installed.lock_dir.exists());
    }

    #[test]
    fn install_refuses_to_overwrite_current_runtime() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = dir.path().join("bundle");
        let install_root = dir.path().join("install-root");
        let runtime = install_root
            .join("bin")
            .join(installed_executable_name("my_service"));
        let data_dir = dir.path().join("data");
        write_test_bundle(&bundle);
        fs::create_dir_all(runtime.parent().unwrap()).expect("runtime parent");
        fs::write(&runtime, "runtime").expect("runtime");

        let err = install_lock_bundle_from_runtime(
            &bundle,
            &runtime,
            "my_service",
            Some(&install_root),
            Some(&data_dir),
        )
        .expect_err("self-overwrite should fail");

        assert!(err.to_string().contains("would overwrite the running"));
    }

    #[test]
    fn install_ignores_cargo_and_does_not_need_host_toolchain() {
        let _lock = crate::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        let spec_path = dir.path().join("source.json");
        let out = dir.path().join("out");
        let install_root = dir.path().join("install-root");
        let data_dir = dir.path().join("data");
        fs::write(&spec_path, minimal_openapi_json()).unwrap();

        let cli = InstallCli::try_parse_from([
            "testprog",
            "--output",
            out.to_str().unwrap(),
            "--spec",
            spec_path.to_str().unwrap(),
            "--secrets",
            "env",
            "--binary-name",
            "my_service",
            "--install-root",
            install_root.to_str().unwrap(),
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--cargo",
            "/definitely/not/a/cargo/toolchain",
        ])
        .expect("parse");

        run_install_command_inner(cli).expect("install");

        assert!(install_root.join("bin").join("my_service").exists());
        assert!(install_root
            .join("bin")
            .join("my_service.acli-lock")
            .exists());
        assert!(data_dir
            .join("locks")
            .join("my_service")
            .join(MANIFEST_FILE)
            .exists());
        assert!(data_dir
            .join("locks")
            .join("my_service")
            .join(SPEC_FILE)
            .exists());
    }

    #[test]
    #[cfg(not(windows))]
    fn default_install_root_is_user_local_on_unix() {
        let _lock = crate::ENV_LOCK.lock().unwrap();
        let _cargo_home = EnvVarGuard::set("CARGO_HOME", Some("/tmp/ignored-cargo-home"));
        let _install_root = EnvVarGuard::set(ENV_INSTALL_ROOT, None);

        assert_eq!(
            resolve_install_root(None).expect("install root"),
            home_dir().unwrap().join(".local")
        );
    }

    #[test]
    fn env_secret_refs_can_point_to_standard_acli_secret_env_vars() {
        let _lock = crate::ENV_LOCK.lock().unwrap();
        let _g_api_key = EnvVarGuard::set(ENV_API_KEY, Some("runtime-only"));
        let dir = tempfile::tempdir().expect("tempdir");
        let spec_path = dir.path().join("source.json");
        fs::write(&spec_path, minimal_openapi_json()).unwrap();
        let out = dir.path().join("out");

        let cli = InstallCli::try_parse_from([
            "testprog",
            "--no-install",
            "--output",
            out.to_str().unwrap(),
            "--spec",
            spec_path.to_str().unwrap(),
            "--secrets",
            "env",
            "--api-key-env",
            ENV_API_KEY,
        ])
        .expect("parse");

        run_install_command_inner(cli).expect("install");
        let manifest = read_manifest(&out).expect("manifest");

        assert_eq!(manifest.env_secrets.api_key.as_deref(), Some(ENV_API_KEY));
        assert!(manifest.inline_secrets.is_empty());
    }

    #[test]
    fn inline_secret_mode_still_reads_standard_secret_env_vars() {
        let _lock = crate::ENV_LOCK.lock().unwrap();
        let _g_api_key = EnvVarGuard::set(ENV_API_KEY, Some("inline-from-env"));
        let dir = tempfile::tempdir().expect("tempdir");
        let spec_path = dir.path().join("source.json");
        fs::write(&spec_path, minimal_openapi_json()).unwrap();
        let out = dir.path().join("out");

        let cli = InstallCli::try_parse_from([
            "testprog",
            "--no-install",
            "--output",
            out.to_str().unwrap(),
            "--spec",
            spec_path.to_str().unwrap(),
            "--secrets",
            "inline",
        ])
        .expect("parse");

        run_install_command_inner(cli).expect("install");
        let manifest = read_manifest(&out).expect("manifest");

        assert_eq!(
            manifest.inline_secrets.api_key.as_deref(),
            Some("inline-from-env")
        );
        assert!(manifest.env_secrets.is_empty());
    }
}
