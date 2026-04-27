use anyhow::{anyhow, bail, Context, Result};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcliConfig {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional JSON Schema reference used by editors.")]
    pub schema: Option<String>,
    #[schemars(description = "Configuration document version. v1 is the only supported version.")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "OpenAPI spec source: HTTPS URL, local file path, or inline JSON.")]
    pub spec: Option<String>,
    #[serde(default)]
    pub cli: CliConfig,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub http: HttpConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub install: InstallConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CliConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Installed command name for a locked CLI.")]
    pub binary_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional ASCII banner title.")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Color preset (default, mono, ocean, sunset) or a JSON theme object."
    )]
    pub color_scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Color mode for banner, help output, and tables.")]
    pub color: Option<ColorModeConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Suppress ASCII banner rendering.")]
    pub no_banner: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(description = "Map original OpenAPI operationId values to custom CLI command names.")]
    pub operation_names: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ColorModeConfig {
    Auto,
    Always,
    Never,
}

impl ColorModeConfig {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Override the server URL declared in the OpenAPI spec.")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Select a server entry from the OpenAPI spec by index.")]
    pub index: Option<usize>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(description = "OpenAPI server template variable overrides.")]
    pub vars: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "HTTP request timeout in seconds.")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Disable TLS certificate verification.")]
    pub insecure: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Headers sent with every request. Values may use {{.ENV_VAR}} templates."
    )]
    pub default_headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<SecretConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic_user: Option<SecretConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic_pass: Option<SecretConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<SecretConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Named security scheme overrides keyed by OpenAPI security scheme name."
    )]
    pub named: BTreeMap<String, SecretConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Literal secret value. Prefer env for shareable configs.")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Host environment variable to read at runtime.")]
    pub env: Option<String>,
}

impl SecretConfig {
    pub fn resolve_runtime_value(&self) -> Option<String> {
        self.env
            .as_deref()
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.is_empty())
            .or_else(|| self.value.clone().filter(|value| !value.is_empty()))
    }

    pub fn literal_value(&self) -> Option<String> {
        self.value.clone().filter(|value| !value.is_empty())
    }

    pub fn env_ref(&self) -> Option<String> {
        self.env.clone().filter(|value| !value.trim().is_empty())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Directory for the generated lock bundle.")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Install root whose bin directory receives the locked CLI launcher.")]
    pub install_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "App-owned data directory for installed lock bundles.")]
    pub data_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Write the lock bundle without installing a launcher.")]
    pub no_install: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Where to persist sensitive values for installed CLIs.")]
    pub secrets: Option<SecretsModeConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SecretsModeConfig {
    Keychain,
    Inline,
    Env,
}

impl SecretsModeConfig {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::Inline => "inline",
            Self::Env => "env",
        }
    }
}

pub fn load_config_source(source: &str) -> Result<AcliConfig> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        bail!("empty config source");
    }

    let text = if trimmed.starts_with('{') {
        trimmed.to_string()
    } else {
        fs::read_to_string(Path::new(trimmed))
            .with_context(|| format!("failed to read acli config '{trimmed}'"))?
    };

    parse_config_json(&text)
}

pub fn parse_config_json(json: &str) -> Result<AcliConfig> {
    let config: AcliConfig =
        serde_json::from_str(json).context("failed to parse acli JSON config")?;
    if config.version != 1 {
        return Err(anyhow!(
            "unsupported acli config version {} (expected 1)",
            config.version
        ));
    }
    Ok(config)
}

pub fn config_schema_json() -> Result<String> {
    let schema = schema_for!(AcliConfig);
    Ok(serde_json::to_string_pretty(&schema)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let config = parse_config_json(
            r#"{
              "version": 1,
              "spec": "https://example.test/openapi.json"
            }"#,
        )
        .expect("config");

        assert_eq!(
            config.spec.as_deref(),
            Some("https://example.test/openapi.json")
        );
    }

    #[test]
    fn parses_full_config() {
        let config = parse_config_json(
            r#"{
              "$schema": "./acli.schema.json",
              "version": 1,
              "spec": "./openapi.json",
              "cli": {
                "binaryName": "petstore_cli",
                "title": "Petstore",
                "colorScheme": "ocean",
                "color": "never",
                                "noBanner": true,
                                "operationNames": {
                                    "listPets": "pets-list"
                                }
              },
              "server": {
                "url": "https://api.example.test",
                "index": 2,
                "vars": {"region": "eu"}
              },
              "http": {
                "timeoutSecs": 45,
                "insecure": true,
                "defaultHeaders": {"X-API-Key": "{{.PETSTORE_API_KEY}}"}
              },
              "auth": {
                "bearerToken": {"env": "PETSTORE_BEARER_TOKEN"},
                "apiKey": {"value": "literal-key"},
                "named": {"partner": {"env": "PARTNER_TOKEN"}}
              },
              "install": {
                "output": "./petstore-cli",
                "installRoot": "./install-root",
                "dataDir": "./data",
                "noInstall": true,
                "secrets": "env"
              }
            }"#,
        )
        .expect("config");

        assert_eq!(config.cli.binary_name.as_deref(), Some("petstore_cli"));
        assert_eq!(
            config.cli.operation_names.get("listPets").map(String::as_str),
            Some("pets-list")
        );
        assert_eq!(config.server.index, Some(2));
        assert_eq!(config.http.timeout_secs, Some(45));
        assert_eq!(
            config
                .auth
                .bearer_token
                .as_ref()
                .and_then(SecretConfig::env_ref),
            Some("PETSTORE_BEARER_TOKEN".to_string())
        );
        assert_eq!(config.install.secrets, Some(SecretsModeConfig::Env));
    }

    #[test]
    fn rejects_invalid_version() {
        let error = parse_config_json(r#"{"version":2,"spec":"x"}"#).unwrap_err();

        assert!(error.to_string().contains("version 2"));
    }

    #[test]
    fn rejects_invalid_enum() {
        let error = parse_config_json(r#"{"version":1,"spec":"x","install":{"secrets":"vault"}}"#)
            .unwrap_err();

        assert!(error.to_string().contains("failed to parse"));
    }

    #[test]
    fn emits_schema_with_key_properties() {
        let schema = config_schema_json().expect("schema");
        let value: serde_json::Value = serde_json::from_str(&schema).expect("valid json");

        assert!(value.get("properties").is_some());
        assert!(schema.contains("\"spec\""));
        assert!(schema.contains("\"binaryName\""));
        assert!(schema.contains("\"operationNames\""));
        assert!(schema.contains("\"defaultHeaders\""));
        assert!(schema.contains("\"secrets\""));
    }
}
