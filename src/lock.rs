//! Pin an OpenAPI JSON document with optional runtime config, store secrets in the OS keychain by default, and emit a small Rust crate that compiles to an API-specific CLI.

use crate::config::{
    env_truthy, sanitize_env_key, ENV_API_KEY, ENV_AUTH_PREFIX, ENV_BASE_URL, ENV_BASIC_PASS,
    ENV_BASIC_USER, ENV_BEARER_TOKEN, ENV_COLOR, ENV_COLOR_SCHEME, ENV_DEFAULT_HEADERS,
    ENV_INSECURE, ENV_NO_BANNER, ENV_SERVER_INDEX, ENV_SERVER_VARS, ENV_SPEC, ENV_TIMEOUT,
    ENV_TITLE,
};
use crate::spec::load_spec_text;
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use keyring::Entry;
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

#[derive(Debug, Parser)]
#[command(
    name = "acli lock",
    override_usage = "acli [global options] lock [OPTIONS]",
    about = "Pin an OpenAPI JSON spec and config, store secrets in the keychain by default, and emit a compilable API-specific CLI crate"
)]
pub struct LockCli {
    /// Directory for the generated crate (`Cargo.toml`, `openapi.json`, `acli.lock.json`, `src/main.rs`)
    #[arg(long, value_name = "DIR", default_value = ".")]
    pub output: PathBuf,

    /// Path to the `acli` package root from the generated crate (used in `Cargo.toml`)
    #[arg(long, value_name = "PATH", default_value = "..")]
    pub acli_crate_path: String,

    /// Cargo package name (default: derived from the API title in the spec)
    #[arg(long, value_name = "NAME")]
    pub crate_name: Option<String>,

    /// Built binary name (default: derived from the API title)
    #[arg(long, value_name = "NAME")]
    pub binary_name: Option<String>,

    /// Where to persist sensitive values: `keychain` (default) or `inline` in the manifest (not recommended)
    #[arg(long, value_parser = ["keychain", "inline"], default_value = "keychain")]
    pub secrets: String,

    /// OpenAPI spec source (URL, path, or JSON); defaults to `ACLI_SPEC`
    #[arg(long, value_name = "URL|PATH|JSON", env = ENV_SPEC)]
    pub spec: Option<String>,

    #[arg(long, env = ENV_TITLE)]
    pub title: Option<String>,

    #[arg(long, env = ENV_COLOR_SCHEME)]
    pub color_scheme: Option<String>,

    #[arg(long, env = ENV_COLOR)]
    pub color: Option<String>,

    #[arg(long, action = clap::ArgAction::SetTrue, env = ENV_NO_BANNER)]
    pub no_banner: bool,

    #[arg(long, env = ENV_BASE_URL)]
    pub server_url: Option<String>,

    #[arg(long, default_value_t = 0usize, env = ENV_SERVER_INDEX)]
    pub server_index: usize,

    #[arg(long, value_name = "NAME=VALUE", action = clap::ArgAction::Append)]
    pub server_var: Vec<String>,

    #[arg(long, value_name = "NAME=VALUE", action = clap::ArgAction::Append)]
    pub default_header: Vec<String>,

    #[arg(long, env = ENV_TIMEOUT)]
    pub timeout: Option<u64>,

    #[arg(long, action = clap::ArgAction::SetTrue, env = ENV_INSECURE)]
    pub insecure: bool,

    #[arg(long, env = ENV_BEARER_TOKEN)]
    pub bearer_token: Option<String>,

    #[arg(long, env = ENV_BASIC_USER)]
    pub basic_user: Option<String>,

    #[arg(long, env = ENV_BASIC_PASS)]
    pub basic_pass: Option<String>,

    #[arg(long, env = ENV_API_KEY)]
    pub api_key: Option<String>,

    #[arg(long, value_name = "SCHEME=VALUE", action = clap::ArgAction::Append)]
    pub auth: Vec<String>,
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

impl LockManifest {
    pub fn apply_to_env(&self, lock_dir: &Path) -> Result<()> {
        let spec_abs = lock_dir.join(&self.spec_path);
        let spec_str = spec_abs
            .to_str()
            .ok_or_else(|| anyhow!("lock directory or spec path is not valid UTF-8"))?;
        unsafe {
            std::env::set_var(ENV_SPEC, spec_str);
        }

        set_opt_env(ENV_TITLE, self.title.as_deref());
        set_opt_env(ENV_COLOR_SCHEME, self.color_scheme.as_deref());
        set_opt_env(ENV_COLOR, self.color.as_deref());
        set_opt_env(ENV_BASE_URL, self.server_url.as_deref());

        if self.no_banner {
            unsafe {
                std::env::set_var(ENV_NO_BANNER, "1");
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
                std::env::set_var(ENV_INSECURE, "1");
            }
        }

        if let Some(service) = &self.keychain_service {
            apply_keychain_secrets(service, &self.keychain_auth_accounts)?;
        }

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

pub fn run_lock_command(cli: LockCli) -> Result<()> {
    run_lock_command_inner(cli)
}

fn run_lock_command_inner(cli: LockCli) -> Result<()> {
    let no_banner = cli.no_banner || env_truthy(ENV_NO_BANNER);

    let spec_source = cli
        .spec
        .clone()
        .or_else(|| env::var(ENV_SPEC).ok())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("missing spec; pass --spec or set {ENV_SPEC}"))?;

    let spec_text = load_spec_text(&spec_source)
        .with_context(|| format!("failed to load OpenAPI spec from '{spec_source}'"))?;
    let api_title = api_title_from_json(&spec_text)?;

    let crate_name = cli
        .crate_name
        .clone()
        .unwrap_or_else(|| slugify_crate_name(&api_title));
    let binary_name = cli
        .binary_name
        .clone()
        .unwrap_or_else(|| slugify_binary_name(&api_title));

    fs::create_dir_all(cli.output.join("src"))
        .with_context(|| format!("failed to create {}", cli.output.display()))?;

    fs::write(cli.output.join(SPEC_FILE), spec_text.as_bytes())
        .with_context(|| format!("failed to write {SPEC_FILE}"))?;

    let server_vars = merge_server_vars(&cli)?;
    let default_headers = merge_default_headers(&cli)?;

    let mut keychain_auth_accounts = Vec::new();
    let mut inline = InlineSecrets::default();
    let keychain_service = if cli.secrets == "inline" {
        store_inline_from_cli(&cli, &mut inline)?;
        None
    } else {
        let service = format!(
            "{}-{}-{}",
            KEYCHAIN_SERVICE_PREFIX,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        store_secrets_in_keychain(&service, &cli, &mut inline, &mut keychain_auth_accounts)?;
        Some(service)
    };

    let manifest = LockManifest {
        version: 1,
        spec_path: SPEC_FILE.to_string(),
        title: cli.title.clone(),
        color_scheme: cli.color_scheme.clone(),
        color: cli.color.clone(),
        no_banner,
        server_url: cli.server_url.clone(),
        server_index: cli.server_index,
        server_vars,
        default_headers,
        timeout_secs: cli.timeout.unwrap_or(30),
        insecure: cli.insecure,
        keychain_service,
        keychain_auth_accounts,
        inline_secrets: inline,
    };

    let manifest_path = cli.output.join(MANIFEST_FILE);
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest)?.as_bytes(),
    )
    .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    write_generated_crate(&cli.output, &cli.acli_crate_path, &crate_name, &binary_name)?;

    eprintln!(
        "Wrote API-specific crate under {}:\n  - Cargo.toml\n  - {}\n  - {}\n  - src/main.rs\n\nBuild:\n  cargo build --release --manifest-path {}/Cargo.toml",
        cli.output.display(),
        SPEC_FILE,
        MANIFEST_FILE,
        cli.output.display(),
    );

    Ok(())
}

fn write_generated_crate(
    output: &Path,
    acli_path: &str,
    crate_name: &str,
    binary_name: &str,
) -> Result<()> {
    let cargo_toml = format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2021"

[dependencies]
acli = {{ path = {acli_path:?} }}

[[bin]]
name = "{binary_name}"
path = "src/main.rs"
"#
    );
    fs::write(output.join("Cargo.toml"), cargo_toml)?;

    let main_rs = r#"fn main() {
    let lock_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Err(e) = acli::run_locked(&lock_dir) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
"#;
    fs::write(output.join("src").join("main.rs"), main_rs)?;

    Ok(())
}

fn api_title_from_json(json: &str) -> Result<String> {
    let v: Value = serde_json::from_str(json).context("failed to parse spec as JSON")?;
    Ok(v.get("info")
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("api")
        .to_string())
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

fn merge_server_vars(cli: &LockCli) -> Result<BTreeMap<String, String>> {
    let mut map =
        parse_json_object_string(env::var(ENV_SERVER_VARS).ok().as_deref(), ENV_SERVER_VARS)?;
    for pair in &cli.server_var {
        let (k, v) = parse_one_pair(pair, "server-var")?;
        map.insert(k, v);
    }
    Ok(map)
}

fn merge_default_headers(cli: &LockCli) -> Result<BTreeMap<String, String>> {
    let mut map = parse_json_object_string(
        env::var(ENV_DEFAULT_HEADERS).ok().as_deref(),
        ENV_DEFAULT_HEADERS,
    )?;
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

fn store_inline_from_cli(cli: &LockCli, inline: &mut InlineSecrets) -> Result<()> {
    inline.bearer_token = cli.bearer_token.clone();
    inline.basic_user = cli.basic_user.clone();
    inline.basic_pass = cli.basic_pass.clone();
    inline.api_key = cli.api_key.clone();
    for pair in &cli.auth {
        let (scheme, value) = parse_one_pair(pair, "auth")?;
        inline.auth.insert(scheme, value);
    }
    Ok(())
}

fn store_secrets_in_keychain(
    service: &str,
    cli: &LockCli,
    _inline: &mut InlineSecrets,
    auth_accounts: &mut Vec<String>,
) -> Result<()> {
    if let Some(token) = &cli.bearer_token {
        keychain_set(service, ENV_BEARER_TOKEN, token)?;
    }
    if let Some(user) = &cli.basic_user {
        keychain_set(service, ENV_BASIC_USER, user)?;
    }
    if let Some(pass) = &cli.basic_pass {
        keychain_set(service, ENV_BASIC_PASS, pass)?;
    }
    if let Some(key) = &cli.api_key {
        keychain_set(service, ENV_API_KEY, key)?;
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        r#"{"openapi":"3.0.0","info":{"title":"My Service","version":"1"},"paths":{}}"#.to_string()
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
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvVarGuard::set(ENV_SERVER_VARS, Some(r#"{"a":"env"}"#));
        let _g2 = EnvVarGuard::set(ENV_DEFAULT_HEADERS, None);
        let cli = LockCli::try_parse_from(["testprog", "--server-var", "a=cli"]).expect("parse");
        let map = merge_server_vars(&cli).expect("merge");
        assert_eq!(map.get("a").map(String::as_str), Some("cli"));
    }

    #[test]
    fn merge_default_headers_merges_env_and_cli() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g0 = EnvVarGuard::set(ENV_SERVER_VARS, None);
        let _g = EnvVarGuard::set(ENV_DEFAULT_HEADERS, Some(r#"{"X":"1"}"#));
        let cli = LockCli::try_parse_from(["testprog", "--default-header", "Y=2"]).expect("parse");
        let map = merge_default_headers(&cli).expect("merge");
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
            timeout_secs: 99,
            insecure: true,
            keychain_service: None,
            keychain_auth_accounts: vec![],
            inline_secrets: InlineSecrets::default(),
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
    }

    #[test]
    fn apply_to_env_sets_expected_vars() {
        let _lock = ENV_LOCK.lock().unwrap();
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
            timeout_secs: 99,
            insecure: true,
            keychain_service: None,
            keychain_auth_accounts: vec![],
            inline_secrets: InlineSecrets::default(),
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
}
