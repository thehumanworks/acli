use crate::colors::Theme;
use crate::config::{
    ENV_API_KEY, ENV_BASE_URL, ENV_BASIC_PASS, ENV_BASIC_USER, ENV_BEARER_TOKEN, ENV_COLOR,
    ENV_COLOR_SCHEME, ENV_DEFAULT_HEADERS, ENV_INSECURE, ENV_NO_BANNER, ENV_SERVER_INDEX,
    ENV_SERVER_VARS, ENV_SPEC, ENV_TIMEOUT, ENV_TITLE,
};
use crate::spec::{OpenApiSpec, OperationSpec, ParameterSpec, SecurityRequirement};
use clap::builder::PossibleValuesParser;
use clap::{value_parser, Arg, ArgAction, ArgGroup, Command};

pub fn build_command(bin_name: &str, spec: &OpenApiSpec, theme: &Theme) -> Command {
    let about = match (&spec.info.title, &spec.info.version) {
        (Some(title), Some(version)) => format!(
            "Dynamic CLI for the OpenAPI document '{}' (spec version {})",
            title, version
        ),
        (Some(title), None) => format!("Dynamic CLI for the OpenAPI document '{title}'"),
        _ => "Dynamic CLI generated from an OpenAPI JSON document".to_string(),
    };

    let mut cmd = Command::new(bin_name.to_string())
        .version(env!("CARGO_PKG_VERSION"))
        .about(about)
        .long_about(long_about(spec))
        .after_help(root_after_help(bin_name))
        .styles(theme.clap_styles())
        .arg_required_else_help(true)
        .subcommand_required(true)
        .propagate_version(true);

    for arg in global_args() {
        cmd = cmd.arg(arg);
    }

    cmd = cmd
        .subcommand(list_command())
        .subcommand(describe_command())
        .subcommand(completions_command());

    for operation in &spec.operations {
        cmd = cmd.subcommand(operation_command(operation));
    }

    cmd
}

fn global_args() -> Vec<Arg> {
    vec![
        Arg::new("spec")
            .long("spec")
            .value_name("URL|PATH|JSON")
            .env(ENV_SPEC)
            .global(true)
            .help("OpenAPI spec source; accepts an HTTPS URL, a local path, or raw JSON"),
        Arg::new("title")
            .long("title")
            .value_name("TEXT")
            .env(ENV_TITLE)
            .global(true)
            .help("Optional ASCII banner title"),
        Arg::new("color_scheme")
            .long("color-scheme")
            .value_name("PRESET|JSON")
            .env(ENV_COLOR_SCHEME)
            .global(true)
            .help("Color preset (default|mono|ocean|sunset) or JSON theme override"),
        Arg::new("color")
            .long("color")
            .value_name("MODE")
            .value_parser(["auto", "always", "never"])
            .env(ENV_COLOR)
            .global(true)
            .help("Color mode for banner, help output, and tables"),
        Arg::new("no_banner")
            .long("no-banner")
            .action(ArgAction::SetTrue)
            .env(ENV_NO_BANNER)
            .global(true)
            .help("Disable ASCII banner rendering"),
        Arg::new("server_url")
            .long("server-url")
            .value_name("URL")
            .env(ENV_BASE_URL)
            .global(true)
            .help("Override the server URL declared in the spec"),
        Arg::new("server_index")
            .long("server-index")
            .value_name("INDEX")
            .value_parser(value_parser!(usize))
            .default_value("0")
            .env(ENV_SERVER_INDEX)
            .global(true)
            .help("Select a server entry from the spec by index"),
        Arg::new("server_var")
            .long("server-var")
            .value_name("NAME=VALUE")
            .action(ArgAction::Append)
            .global(true)
            .help("Override an OpenAPI server template variable"),
        Arg::new("bearer_token")
            .long("bearer-token")
            .value_name("TOKEN")
            .env(ENV_BEARER_TOKEN)
            .global(true)
            .help("Override the bearer token used for bearer/oauth2 security schemes"),
        Arg::new("basic_user")
            .long("basic-user")
            .value_name("USER")
            .env(ENV_BASIC_USER)
            .global(true)
            .help("Override the HTTP basic auth username"),
        Arg::new("basic_pass")
            .long("basic-pass")
            .value_name("PASS")
            .env(ENV_BASIC_PASS)
            .global(true)
            .help("Override the HTTP basic auth password"),
        Arg::new("api_key")
            .long("api-key")
            .value_name("VALUE")
            .env(ENV_API_KEY)
            .global(true)
            .help("Fallback API key used for apiKey security schemes"),
        Arg::new("auth")
            .long("auth")
            .value_name("SCHEME=VALUE")
            .action(ArgAction::Append)
            .global(true)
            .help("Provide a value for a named security scheme"),
        Arg::new("timeout_secs")
            .long("timeout")
            .value_name("SECONDS")
            .value_parser(value_parser!(u64))
            .env(ENV_TIMEOUT)
            .default_value("30")
            .global(true)
            .help("HTTP request timeout in seconds"),
        Arg::new("insecure")
            .long("insecure")
            .action(ArgAction::SetTrue)
            .env(ENV_INSECURE)
            .global(true)
            .help("Disable TLS certificate verification"),
        Arg::new("verbose")
            .long("verbose")
            .short('v')
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Print request and response metadata"),
        Arg::new("raw_output")
            .long("raw")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Do not pretty-print JSON responses"),
        Arg::new("output")
            .long("output")
            .short('o')
            .value_name("FILE")
            .global(true)
            .help("Write the response body to a file instead of stdout"),
    ]
}

fn list_command() -> Command {
    Command::new("list")
        .about("List operations exposed by the loaded OpenAPI document")
        .arg(
            Arg::new("tag")
                .long("tag")
                .value_name("TAG")
                .help("Filter operations by tag"),
        )
        .arg(
            Arg::new("method")
                .long("method")
                .value_name("METHOD")
                .help("Filter operations by HTTP method"),
        )
        .arg(
            Arg::new("deprecated_only")
                .long("deprecated")
                .action(ArgAction::SetTrue)
                .help("Only show deprecated operations"),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Emit machine-readable JSON"),
        )
}

fn describe_command() -> Command {
    Command::new("describe")
        .about("Show the detailed contract for a single operation")
        .arg(
            Arg::new("operation")
                .value_name("OPERATION")
                .required(true)
                .help("Operation slug or exact operationId"),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Emit machine-readable JSON"),
        )
}

fn completions_command() -> Command {
    Command::new("completions")
        .about("Generate shell completion scripts for the live command tree")
        .arg(
            Arg::new("shell")
                .value_name("SHELL")
                .required(true)
                .value_parser(PossibleValuesParser::new([
                    "bash",
                    "elvish",
                    "fish",
                    "powershell",
                    "zsh",
                ]))
                .help("Shell type"),
        )
}

fn operation_command(operation: &OperationSpec) -> Command {
    let mut cmd = Command::new(operation.slug.clone())
        .about(
            operation
                .summary
                .clone()
                .unwrap_or_else(|| operation.title()),
        )
        .long_about(operation_long_about(operation))
        .after_help(operation_after_help(operation));

    if let Some(alias) = operation.operation_id.as_ref().filter(|operation_id| {
        is_alias_safe(operation_id.as_str()) && operation_id.as_str() != operation.slug
    }) {
        cmd = cmd.alias(alias.clone());
    }

    for parameter in &operation.parameters {
        cmd = cmd.arg(parameter_arg(parameter));
    }

    cmd = cmd
        .arg(
            Arg::new("path_pairs")
                .long("path")
                .value_name("NAME=VALUE")
                .action(ArgAction::Append)
                .help("Provide or override a path parameter by name"),
        )
        .arg(
            Arg::new("query_pairs")
                .long("query")
                .value_name("NAME=VALUE")
                .action(ArgAction::Append)
                .help("Add a query parameter without relying on generated flags"),
        )
        .arg(
            Arg::new("header_pairs")
                .long("header")
                .value_name("NAME=VALUE")
                .action(ArgAction::Append)
                .help("Add an extra request header"),
        )
        .arg(
            Arg::new("cookie_pairs")
                .long("cookie")
                .value_name("NAME=VALUE")
                .action(ArgAction::Append)
                .help("Add an extra cookie"),
        )
        .arg(
            Arg::new("body")
                .long("body")
                .value_name("STRING")
                .help("Inline request body payload"),
        )
        .arg(
            Arg::new("body_file")
                .long("body-file")
                .value_name("PATH")
                .help("Read the request body from a file"),
        )
        .arg(
            Arg::new("body_stdin")
                .long("body-stdin")
                .action(ArgAction::SetTrue)
                .help("Read the request body from stdin"),
        )
        .arg(
            Arg::new("form_pairs")
                .long("form")
                .value_name("NAME=VALUE")
                .action(ArgAction::Append)
                .help("Add a form or multipart field"),
        )
        .arg(
            Arg::new("file_pairs")
                .long("file")
                .value_name("FIELD=PATH")
                .action(ArgAction::Append)
                .help("Attach a file for multipart/form-data requests"),
        )
        .arg(
            Arg::new("content_type")
                .long("content-type")
                .value_name("MIME")
                .help("Override the request content type"),
        )
        .arg(
            Arg::new("accept")
                .long("accept")
                .value_name("MIME")
                .help("Explicit Accept header value"),
        )
        .arg(
            Arg::new("dry_run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Print the assembled request without sending it"),
        );

    if operation.request_body.is_some() {
        let body_required = operation
            .request_body
            .as_ref()
            .map(|body| body.required)
            .unwrap_or(false);
        cmd = cmd.group(
            ArgGroup::new("request_body_group")
                .args([
                    "body",
                    "body_file",
                    "body_stdin",
                    "form_pairs",
                    "file_pairs",
                ])
                .required(body_required),
        );
    }

    cmd
}

fn parameter_arg(parameter: &ParameterSpec) -> Arg {
    let help = parameter_help(parameter);
    let value_name = parameter_value_name(parameter);

    Arg::new(parameter.arg_id.clone())
        .long(parameter.flag_name.clone())
        .value_name(value_name)
        .required(parameter.required && parameter.location != "header")
        .help(help)
}

fn parameter_help(parameter: &ParameterSpec) -> String {
    let mut pieces = Vec::new();

    if let Some(description) = &parameter.description {
        pieces.push(description.clone());
    }
    pieces.push(format!("location: {}", parameter.location));
    if parameter.required {
        pieces.push("required".to_string());
    }
    if parameter.deprecated {
        pieces.push("deprecated".to_string());
    }
    if let Some(schema) = &parameter.schema {
        if let Some(type_name) = &schema.type_name {
            pieces.push(format!("type: {type_name}"));
        }
        if let Some(format) = &schema.format {
            pieces.push(format!("format: {format}"));
        }
        if !schema.enum_values.is_empty() {
            pieces.push(format!("enum: {}", schema.enum_values.join(", ")));
        }
        if let Some(default) = &schema.default {
            pieces.push(format!("default: {}", default));
        }
    }
    if let Some(style) = &parameter.style {
        pieces.push(format!("style: {style}"));
    }

    pieces.join(" | ")
}

fn parameter_value_name(parameter: &ParameterSpec) -> &'static str {
    match parameter
        .schema
        .as_ref()
        .and_then(|schema| schema.type_name.as_deref())
    {
        Some("integer") => "INTEGER",
        Some("number") => "NUMBER",
        Some("boolean") => "BOOL",
        Some("array") => "JSON",
        Some("object") => "JSON",
        _ => "VALUE",
    }
}

fn operation_long_about(operation: &OperationSpec) -> String {
    let mut lines = vec![format!("{} {}", operation.method, operation.path)];

    if let Some(operation_id) = &operation.operation_id {
        lines.push(format!("operationId: {operation_id}"));
    }
    if let Some(summary) = &operation.summary {
        lines.push(format!("summary: {summary}"));
    }
    if let Some(description) = &operation.description {
        lines.push(String::new());
        lines.push(description.clone());
    }
    if !operation.tags.is_empty() {
        lines.push(String::new());
        lines.push(format!("tags: {}", operation.tags.join(", ")));
    }
    if operation.deprecated {
        lines.push("deprecated: true".to_string());
    }
    if !operation.parameters.is_empty() {
        lines.push(String::new());
        lines.push("parameters:".to_string());
        for parameter in &operation.parameters {
            lines.push(format!(
                "  --{} ({}, {})",
                parameter.flag_name,
                parameter.location,
                if parameter.required {
                    "required"
                } else {
                    "optional"
                }
            ));
        }
    }
    if let Some(body) = &operation.request_body {
        lines.push(String::new());
        lines.push(format!(
            "request body: {}",
            if body.required {
                "required"
            } else {
                "optional"
            }
        ));
        for media_type in &body.content {
            lines.push(format!("  - {}", media_type.content_type));
        }
    }
    if !operation.responses.is_empty() {
        lines.push(String::new());
        lines.push("responses:".to_string());
        for response in &operation.responses {
            if response.content_types.is_empty() {
                lines.push(format!("  - {}", response.status));
            } else {
                lines.push(format!(
                    "  - {} ({})",
                    response.status,
                    response.content_types.join(", ")
                ));
            }
        }
    }
    if let Some(security) = &operation.security {
        lines.push(String::new());
        lines.push(format!(
            "security: {}",
            render_security_requirements(security)
        ));
    }

    lines.join("\n")
}

fn operation_after_help(operation: &OperationSpec) -> String {
    let mut examples = Vec::new();
    if let Some(path_param) = operation.path_parameters().next() {
        examples.push(format!(
            "  {0} --{1} 123",
            operation.slug, path_param.flag_name
        ));
    } else {
        examples.push(format!("  {}", operation.slug));
    }
    if operation.request_body.is_some() {
        examples.push(format!("  {} --body-file ./payload.json", operation.slug));
    }
    examples.push(format!("  describe {}", operation.slug));

    format!(
        "Examples:\n{}\n\nUse --query/--header/--cookie/--path for parameters that are awkward to express as generated flags.",
        examples.join("\n")
    )
}

fn render_security_requirements(requirements: &[SecurityRequirement]) -> String {
    if requirements.is_empty() {
        return "none".to_string();
    }

    requirements
        .iter()
        .map(|requirement| {
            let mut parts = requirement.keys().cloned().collect::<Vec<_>>();
            parts.sort();
            parts.join(" + ")
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn long_about(spec: &OpenApiSpec) -> String {
    let mut sections = Vec::new();
    sections.push(format!(
        "Loaded OpenAPI document: {}",
        spec.info
            .title
            .clone()
            .unwrap_or_else(|| "(untitled)".to_string())
    ));
    sections.push(format!("OpenAPI version: {}", spec.info.openapi_version));
    sections.push(format!("Operations: {}", spec.operations.len()));

    if !spec.root_servers.is_empty() {
        sections.push(String::new());
        sections.push(
            "Servers: ".to_string()
                + &spec
                    .root_servers
                    .iter()
                    .map(|server| server.url.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
        );
    }

    if !spec.security_schemes.is_empty() {
        sections.push(String::new());
        sections.push(
            "Security schemes: ".to_string()
                + &spec
                    .security_schemes
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
        );
    }

    sections.join("\n")
}

fn root_after_help(bin_name: &str) -> String {
    format!(
        "Environment:\n  {ENV_SPEC}              OpenAPI spec source (URL, path, or inline JSON)\n  {ENV_TITLE}             Optional ASCII banner title\n  {ENV_COLOR_SCHEME}      Color preset or JSON theme config\n  {ENV_COLOR}             auto|always|never\n  {ENV_NO_BANNER}         Set to true/1 to suppress the banner\n  {ENV_BASE_URL}          Server URL override\n  {ENV_SERVER_INDEX}      Default server index (integer)\n  {ENV_SERVER_VARS}       JSON object for server variable overrides\n  {ENV_DEFAULT_HEADERS}  JSON object of headers sent with every request; values may use {{{{.ENV_VAR}}}} templates\n  {ENV_BEARER_TOKEN}      Default bearer token\n  {ENV_BASIC_USER}        Default basic auth username\n  {ENV_BASIC_PASS}        Default basic auth password\n  {ENV_API_KEY}           Default api key fallback\n\nLock workflow (pin spec + emit API-specific crate):\n  {bin_name} lock --output ./my-api-cli --spec <URL|PATH> [--secrets keychain|inline|env]\n  {bin_name} lock --output ./my-api-cli --spec <URL|PATH> --secrets env --api-key-env HOST_API_KEY\n  cargo build --release --manifest-path ./my-api-cli/Cargo.toml\n\nCommon workflow:\n  {bin_name} list\n  {bin_name} describe <operation>\n  {bin_name} <operation> [generated flags] [--query name=value] [--body-file payload.json]"
    )
}

fn is_alias_safe(value: &str) -> bool {
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    fn required_parameter(location: &str) -> ParameterSpec {
        ParameterSpec {
            name: "API-Key".to_string(),
            location: location.to_string(),
            flag_name: format!("{location}-api-key"),
            arg_id: format!("param__{location}__api_key"),
            required: true,
            deprecated: false,
            description: None,
            style: None,
            explode: None,
            schema: None,
            content_types: Vec::new(),
        }
    }

    #[test]
    fn required_header_parameters_are_not_clap_required() {
        let parameter = required_parameter("header");
        let command = Command::new("operation").arg(parameter_arg(&parameter));
        let matches = command.try_get_matches_from(["operation"]).unwrap();

        assert!(matches
            .get_one::<String>("param__header__api_key")
            .is_none());
    }

    #[test]
    fn required_query_parameters_are_still_clap_required() {
        let parameter = required_parameter("query");
        let error = Command::new("operation")
            .arg(parameter_arg(&parameter))
            .try_get_matches_from(["operation"])
            .unwrap_err();

        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }
}
