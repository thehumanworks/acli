use crate::colors::Theme;
use crate::config::{
    sanitize_env_key, ENV_AUTH_PREFIX, ENV_BASE_URL, ENV_DEFAULT_HEADERS, ENV_INSECURE,
    ENV_SERVER_VARS,
};
use crate::spec::{
    OpenApiSpec, OperationSpec, SecurityRequirement, SecuritySchemeSpec, ServerSpec,
};
use anyhow::{anyhow, bail, Context, Result};
use clap::{ArgMatches, Command};
use clap_complete::aot::{generate, Shell};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::blocking::multipart::{Form, Part};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE, COOKIE};
use reqwest::{Method, Url};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

pub fn run(
    bin_name: &str,
    spec: &OpenApiSpec,
    theme: &Theme,
    matches: &ArgMatches,
    mut command: Command,
) -> Result<()> {
    match matches.subcommand() {
        Some(("list", sub_matches)) => list_operations(spec, theme, sub_matches),
        Some(("describe", sub_matches)) => describe_operation(spec, theme, sub_matches),
        Some(("completions", sub_matches)) => emit_completions(bin_name, sub_matches, &mut command),
        Some((operation_name, sub_matches)) => {
            let operation = spec
                .find_operation(operation_name)
                .ok_or_else(|| anyhow!("unknown operation '{operation_name}'"))?;
            invoke_operation(spec, operation, theme, matches, sub_matches)
        }
        None => Ok(()),
    }
}

fn list_operations(spec: &OpenApiSpec, theme: &Theme, matches: &ArgMatches) -> Result<()> {
    let tag_filter = matches
        .get_one::<String>("tag")
        .map(|value| value.to_ascii_lowercase());
    let method_filter = matches
        .get_one::<String>("method")
        .map(|value| value.to_ascii_uppercase());
    let json_output = matches.get_flag("json");
    let deprecated_only = matches.get_flag("deprecated_only");

    let operations = spec
        .operations
        .iter()
        .filter(|operation| !deprecated_only || operation.deprecated)
        .filter(|operation| {
            tag_filter.as_ref().is_none_or(|tag| {
                operation
                    .tags
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(tag))
            })
        })
        .filter(|operation| {
            method_filter
                .as_ref()
                .is_none_or(|method| &operation.method == method)
        })
        .collect::<Vec<_>>();

    if json_output {
        #[derive(Serialize)]
        struct Item<'a> {
            slug: &'a str,
            operation_id: Option<&'a str>,
            method: &'a str,
            path: &'a str,
            summary: Option<&'a str>,
            tags: &'a [String],
            deprecated: bool,
        }

        let payload = operations
            .iter()
            .map(|operation| Item {
                slug: &operation.slug,
                operation_id: operation.operation_id.as_deref(),
                method: &operation.method,
                path: &operation.path,
                summary: operation.summary.as_deref(),
                tags: &operation.tags,
                deprecated: operation.deprecated,
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    if operations.is_empty() {
        println!(
            "{}",
            theme.muted("no operations matched the requested filters")
        );
        return Ok(());
    }

    let method_width = operations
        .iter()
        .map(|operation| operation.method.len())
        .max()
        .unwrap_or(6)
        .max("METHOD".len());
    let slug_width = operations
        .iter()
        .map(|operation| operation.slug.len())
        .max()
        .unwrap_or(4)
        .max("OPERATION".len());

    println!(
        "{}",
        theme.header(format!(
            "{:<method_width$}  {:<slug_width$}  PATH  SUMMARY",
            "METHOD",
            "OPERATION",
            method_width = method_width,
            slug_width = slug_width
        ))
    );

    for operation in operations {
        let summary = operation
            .summary
            .as_deref()
            .or(operation.description.as_deref())
            .unwrap_or("");
        let deprecated_suffix = if operation.deprecated {
            format!(" {}", theme.warning("[deprecated]"))
        } else {
            String::new()
        };
        println!(
            "{:<method_width$}  {:<slug_width$}  {}  {}{}",
            theme.accent(&operation.method),
            operation.slug,
            operation.path,
            summary,
            deprecated_suffix,
            method_width = method_width,
            slug_width = slug_width
        );
    }

    Ok(())
}

fn describe_operation(spec: &OpenApiSpec, theme: &Theme, matches: &ArgMatches) -> Result<()> {
    let name = matches
        .get_one::<String>("operation")
        .ok_or_else(|| anyhow!("missing operation name"))?;
    let operation = spec
        .find_operation(name)
        .ok_or_else(|| anyhow!("unknown operation '{name}'"))?;

    if matches.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(operation)?);
        return Ok(());
    }

    println!("{}", theme.header(operation.title()));
    if let Some(operation_id) = &operation.operation_id {
        println!("{} {}", theme.accent("operationId:"), operation_id);
    }
    println!("{} {}", theme.accent("slug:"), operation.slug);
    if !operation.tags.is_empty() {
        println!("{} {}", theme.accent("tags:"), operation.tags.join(", "));
    }
    if operation.deprecated {
        println!("{} true", theme.warning("deprecated:"));
    }
    if let Some(summary) = &operation.summary {
        println!("{} {}", theme.accent("summary:"), summary);
    }
    if let Some(description) = &operation.description {
        println!();
        println!("{}", description);
    }

    if !operation.parameters.is_empty() {
        println!();
        println!("{}", theme.header("Parameters"));
        for parameter in &operation.parameters {
            let schema = parameter
                .schema
                .as_ref()
                .and_then(|schema| schema.type_name.as_deref())
                .unwrap_or("value");
            println!(
                "  {} --{} ({}, {}, type={})",
                if parameter.required {
                    theme.success("*")
                } else {
                    theme.muted("-")
                },
                parameter.flag_name,
                parameter.location,
                if parameter.required {
                    "required"
                } else {
                    "optional"
                },
                schema
            );
            if let Some(description) = &parameter.description {
                println!("      {}", description);
            }
            if !parameter.content_types.is_empty() {
                println!("      content: {}", parameter.content_types.join(", "));
            }
        }
    }

    if let Some(body) = &operation.request_body {
        println!();
        println!("{}", theme.header("Request body"));
        println!(
            "  {}",
            if body.required {
                "required"
            } else {
                "optional"
            }
        );
        for media_type in &body.content {
            println!("  - {}", media_type.content_type);
        }
    }

    if !operation.responses.is_empty() {
        println!();
        println!("{}", theme.header("Responses"));
        for response in &operation.responses {
            if response.content_types.is_empty() {
                println!("  {}", response.status);
            } else {
                println!(
                    "  {} -> {}",
                    response.status,
                    response.content_types.join(", ")
                );
            }
            if let Some(description) = &response.description {
                println!("      {}", description);
            }
        }
    }

    if !operation.servers.is_empty() {
        println!();
        println!("{}", theme.header("Servers"));
        for (index, server) in operation.servers.iter().enumerate() {
            println!("  [{}] {}", index, server.url);
        }
    }

    if let Some(security) = &operation.security {
        println!();
        println!("{}", theme.header("Security"));
        if security.is_empty() {
            println!("  none");
        } else {
            for requirement in security {
                let names = requirement.keys().cloned().collect::<Vec<_>>().join(" + ");
                println!("  {}", names);
            }
        }
    }

    Ok(())
}

fn emit_completions(bin_name: &str, matches: &ArgMatches, command: &mut Command) -> Result<()> {
    let shell = matches
        .get_one::<String>("shell")
        .ok_or_else(|| anyhow!("missing shell"))?;

    match shell.as_str() {
        "bash" => generate(Shell::Bash, command, bin_name, &mut io::stdout()),
        "elvish" => generate(Shell::Elvish, command, bin_name, &mut io::stdout()),
        "fish" => generate(Shell::Fish, command, bin_name, &mut io::stdout()),
        "powershell" => generate(Shell::PowerShell, command, bin_name, &mut io::stdout()),
        "zsh" => generate(Shell::Zsh, command, bin_name, &mut io::stdout()),
        _ => bail!(
            "unsupported shell '{}': expected bash|elvish|fish|powershell|zsh",
            shell
        ),
    }

    Ok(())
}

fn invoke_operation(
    spec: &OpenApiSpec,
    operation: &OperationSpec,
    theme: &Theme,
    root_matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<()> {
    let runtime = RuntimeOptions::from_matches(root_matches)?;
    let invocation = InvocationInput::from_matches(operation, sub_matches)?;

    let server_url = resolve_server_url(spec, operation, &runtime)?;
    let auth = resolve_security(spec, operation, &runtime)?;
    let request_plan = build_request_plan(operation, &server_url, &runtime, &invocation, &auth)?;

    if runtime.verbose || invocation.dry_run {
        print_request_plan(theme, &request_plan)?;
    }
    if invocation.dry_run {
        return Ok(());
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(runtime.timeout_secs))
        .danger_accept_invalid_certs(runtime.insecure)
        .build()?;

    let mut builder = client.request(request_plan.method.clone(), request_plan.url.clone());
    if !request_plan.headers.is_empty() {
        builder = builder.headers(request_plan.headers.clone());
    }
    builder = attach_body(
        builder,
        &request_plan.body,
        request_plan.content_type.as_deref(),
    )?;

    let response = builder.send()?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let bytes = response.bytes()?;

    if runtime.verbose {
        eprintln!("{} {}", theme.accent("status:"), status);
        for (name, value) in &response_headers {
            eprintln!(
                "{} {}",
                theme.muted(format!("{}:", name.as_str())),
                value.to_str().unwrap_or("<binary>")
            );
        }
    }

    if let Some(path) = &runtime.output {
        fs::write(path, &bytes)?;
        println!("{} {}", theme.success("wrote response to"), path);
    } else {
        print_response_body(&bytes, &response_headers, runtime.raw_output)?;
    }

    if !status.is_success() {
        bail!("request failed with HTTP status {}", status);
    }

    Ok(())
}

#[derive(Debug)]
struct RuntimeOptions {
    server_url: Option<String>,
    server_index: usize,
    server_vars: BTreeMap<String, String>,
    default_headers: BTreeMap<String, String>,
    bearer_token: Option<String>,
    basic_user: Option<String>,
    basic_pass: Option<String>,
    api_key: Option<String>,
    auth_overrides: BTreeMap<String, String>,
    timeout_secs: u64,
    insecure: bool,
    verbose: bool,
    raw_output: bool,
    output: Option<String>,
}

impl RuntimeOptions {
    fn from_matches(matches: &ArgMatches) -> Result<Self> {
        let mut server_vars = parse_json_string_map(
            std::env::var(ENV_SERVER_VARS).ok().as_deref(),
            ENV_SERVER_VARS,
        )?;
        for (key, value) in parse_pairs(matches.get_many::<String>("server_var"))? {
            server_vars.insert(key, value);
        }
        let default_headers = parse_json_string_map(
            std::env::var(ENV_DEFAULT_HEADERS).ok().as_deref(),
            ENV_DEFAULT_HEADERS,
        )?;

        let mut auth_overrides = BTreeMap::new();
        for (key, value) in parse_pairs(matches.get_many::<String>("auth"))? {
            auth_overrides.insert(key, value);
        }

        let insecure = matches.get_flag("insecure") || env_truthy(ENV_INSECURE);

        Ok(Self {
            server_url: matches.get_one::<String>("server_url").cloned(),
            server_index: *matches.get_one::<usize>("server_index").unwrap_or(&0),
            server_vars,
            default_headers,
            bearer_token: matches.get_one::<String>("bearer_token").cloned(),
            basic_user: matches.get_one::<String>("basic_user").cloned(),
            basic_pass: matches.get_one::<String>("basic_pass").cloned(),
            api_key: matches.get_one::<String>("api_key").cloned(),
            auth_overrides,
            timeout_secs: *matches.get_one::<u64>("timeout_secs").unwrap_or(&30),
            insecure,
            verbose: matches.get_flag("verbose"),
            raw_output: matches.get_flag("raw_output"),
            output: matches.get_one::<String>("output").cloned(),
        })
    }
}

#[derive(Debug)]
struct InvocationInput {
    path_values: BTreeMap<String, String>,
    query_values: Vec<(String, String)>,
    header_values: Vec<(String, String)>,
    cookie_values: Vec<(String, String)>,
    body: BodyInput,
    content_type: Option<String>,
    accept: Option<String>,
    dry_run: bool,
}

impl InvocationInput {
    fn from_matches(operation: &OperationSpec, matches: &ArgMatches) -> Result<Self> {
        let mut path_values = BTreeMap::new();
        let mut query_values = Vec::new();
        let mut header_values = Vec::new();
        let mut cookie_values = Vec::new();

        for parameter in &operation.parameters {
            if let Some(value) = matches.get_one::<String>(&parameter.arg_id) {
                match parameter.location.as_str() {
                    "path" => {
                        path_values.insert(parameter.name.clone(), value.clone());
                    }
                    "query" => query_values.push((parameter.name.clone(), value.clone())),
                    "header" => header_values.push((parameter.name.clone(), value.clone())),
                    "cookie" => cookie_values.push((parameter.name.clone(), value.clone())),
                    _ => {}
                }
            }
        }

        for (key, value) in parse_pairs(matches.get_many::<String>("path_pairs"))? {
            path_values.insert(key, value);
        }
        query_values.extend(parse_pairs(matches.get_many::<String>("query_pairs"))?);
        header_values.extend(parse_pairs(matches.get_many::<String>("header_pairs"))?);
        cookie_values.extend(parse_pairs(matches.get_many::<String>("cookie_pairs"))?);

        let fields = parse_pairs(matches.get_many::<String>("form_pairs"))?;
        let files = parse_pairs(matches.get_many::<String>("file_pairs"))?
            .into_iter()
            .map(|(field, path)| (field, PathBuf::from(path)))
            .collect::<Vec<_>>();

        let body = if !files.is_empty() || !fields.is_empty() {
            BodyInput::Form { fields, files }
        } else if let Some(body) = matches.get_one::<String>("body") {
            BodyInput::Raw(body.as_bytes().to_vec())
        } else if let Some(path) = matches.get_one::<String>("body_file") {
            BodyInput::Raw(
                fs::read(path).with_context(|| format!("failed to read body file '{path}'"))?,
            )
        } else if matches.get_flag("body_stdin") {
            let mut buffer = Vec::new();
            io::stdin().read_to_end(&mut buffer)?;
            BodyInput::Raw(buffer)
        } else {
            BodyInput::None
        };

        Ok(Self {
            path_values,
            query_values,
            header_values,
            cookie_values,
            body,
            content_type: matches.get_one::<String>("content_type").cloned(),
            accept: matches.get_one::<String>("accept").cloned(),
            dry_run: matches.get_flag("dry_run"),
        })
    }
}

#[derive(Debug)]
enum BodyInput {
    None,
    Raw(Vec<u8>),
    Form {
        fields: Vec<(String, String)>,
        files: Vec<(String, PathBuf)>,
    },
}

#[derive(Debug, Default)]
struct ResolvedAuth {
    bearer_token: Option<String>,
    basic_credentials: Option<(String, String)>,
    query_pairs: Vec<(String, String)>,
    header_pairs: Vec<(String, String)>,
    cookie_pairs: Vec<(String, String)>,
}

#[derive(Debug)]
struct RequestPlan {
    method: Method,
    url: Url,
    headers: HeaderMap,
    body: BodyInput,
    content_type: Option<String>,
}

fn build_request_plan(
    operation: &OperationSpec,
    server_url: &str,
    runtime: &RuntimeOptions,
    invocation: &InvocationInput,
    auth: &ResolvedAuth,
) -> Result<RequestPlan> {
    let mut query_pairs = invocation.query_values.clone();
    query_pairs.extend(auth.query_pairs.clone());

    let url = build_url(operation, server_url, &invocation.path_values, &query_pairs)?;
    let mut headers = HeaderMap::new();
    let mut default_header_names = Vec::new();

    for (name, value) in &runtime.default_headers {
        default_header_names.push(insert_header(&mut headers, name, value)?);
    }
    append_header_pairs_replacing_defaults(
        &mut headers,
        &invocation.header_values,
        &mut default_header_names,
    )?;
    append_header_pairs_replacing_defaults(
        &mut headers,
        &auth.header_pairs,
        &mut default_header_names,
    )?;
    if let Some(accept) = &invocation.accept {
        headers.insert(ACCEPT, HeaderValue::from_str(accept)?);
    }
    let cookie_pairs = invocation
        .cookie_values
        .iter()
        .cloned()
        .chain(auth.cookie_pairs.clone())
        .collect::<Vec<_>>();
    if !cookie_pairs.is_empty() {
        let cookie_header = cookie_pairs
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        headers.insert(COOKIE, HeaderValue::from_str(&cookie_header)?);
    }

    let content_type = choose_content_type(operation, invocation);
    if matches!(invocation.body, BodyInput::Raw(_)) {
        if let Some(content_type) = content_type.as_deref() {
            headers.insert(CONTENT_TYPE, HeaderValue::from_str(content_type)?);
        }
    }

    if let Some(token) = &auth.bearer_token {
        let value = format!("Bearer {token}");
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&value)?,
        );
    }

    if let Some((user, pass)) = &auth.basic_credentials {
        let token = base64_encode(format!("{user}:{pass}"));
        let value = format!("Basic {token}");
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&value)?,
        );
    }

    validate_required_headers(operation, &headers)?;

    Ok(RequestPlan {
        method: Method::from_bytes(operation.method.as_bytes())?,
        url,
        headers,
        body: clone_body(&invocation.body),
        content_type,
    })
}

fn choose_content_type(operation: &OperationSpec, invocation: &InvocationInput) -> Option<String> {
    if let Some(explicit) = &invocation.content_type {
        return Some(explicit.clone());
    }

    let available = operation.request_content_types();
    match &invocation.body {
        BodyInput::Form { fields: _, files } if !files.is_empty() => {
            return Some("multipart/form-data".to_string())
        }
        BodyInput::Form { .. } => {
            if available
                .iter()
                .any(|value| value == "application/x-www-form-urlencoded")
            {
                return Some("application/x-www-form-urlencoded".to_string());
            }
            return Some("multipart/form-data".to_string());
        }
        BodyInput::Raw(_) => {
            if available.iter().any(|value| value == "application/json") {
                return Some("application/json".to_string());
            }
            if available.len() == 1 {
                return available.first().cloned();
            }
        }
        BodyInput::None => {}
    }

    if available.len() == 1 {
        available.first().cloned()
    } else {
        None
    }
}

fn build_url(
    operation: &OperationSpec,
    server_url: &str,
    path_values: &BTreeMap<String, String>,
    query_values: &[(String, String)],
) -> Result<Url> {
    let mut path = operation.path.clone();
    for parameter in operation.path_parameters() {
        let value = path_values.get(&parameter.name).ok_or_else(|| {
            anyhow!(
                "missing required path parameter '{}' for operation '{}'",
                parameter.name,
                operation.slug
            )
        })?;
        let encoded = utf8_percent_encode(value, NON_ALPHANUMERIC).to_string();
        path = path.replace(&format!("{{{}}}", parameter.name), &encoded);
    }

    if path.contains('{') {
        bail!("unresolved path template remains after parameter substitution: {path}");
    }

    let full_url = format!(
        "{}{}",
        server_url.trim_end_matches('/'),
        if path.starts_with('/') {
            path
        } else {
            format!("/{path}")
        }
    );
    let mut url =
        Url::parse(&full_url).with_context(|| format!("invalid request URL '{full_url}'"))?;
    {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in query_values {
            pairs.append_pair(name, value);
        }
    }
    Ok(url)
}

fn resolve_server_url(
    spec: &OpenApiSpec,
    operation: &OperationSpec,
    runtime: &RuntimeOptions,
) -> Result<String> {
    if let Some(url) = &runtime.server_url {
        return absolutize_server_url(url, spec.source_url.as_ref());
    }

    if operation.servers.is_empty() {
        bail!(
            "operation '{}' does not declare any servers; pass --server-url or set {ENV_BASE_URL}",
            operation.slug
        );
    }

    let server = operation.servers.get(runtime.server_index).ok_or_else(|| {
        anyhow!(
            "server index {} is out of range for operation '{}'",
            runtime.server_index,
            operation.slug
        )
    })?;

    let rendered = render_server_url(server, &runtime.server_vars)?;
    absolutize_server_url(&rendered, spec.source_url.as_ref())
}

fn absolutize_server_url(candidate: &str, base: Option<&Url>) -> Result<String> {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        bail!("server URL is empty; pass --server-url or set {ENV_BASE_URL}");
    }

    if Url::parse(trimmed).is_ok() {
        return Ok(trimmed.to_string());
    }

    let base = base.ok_or_else(|| {
        anyhow!(
            "server URL '{trimmed}' is relative but the spec was not loaded from an HTTP(S) URL; \
             pass --server-url or set {ENV_BASE_URL} to an absolute URL"
        )
    })?;

    let resolved = base.join(trimmed).with_context(|| {
        format!("failed to resolve relative server URL '{trimmed}' against '{base}'")
    })?;
    Ok(resolved.to_string())
}

fn render_server_url(server: &ServerSpec, overrides: &BTreeMap<String, String>) -> Result<String> {
    let mut url = server.url.clone();
    for (name, variable) in &server.variables {
        let value = overrides
            .get(name)
            .cloned()
            .unwrap_or_else(|| variable.default.clone());
        url = url.replace(&format!("{{{name}}}"), &value);
    }
    if url.contains('{') {
        bail!("server URL contains unresolved variables: {url}");
    }
    Ok(url)
}

fn resolve_security(
    spec: &OpenApiSpec,
    operation: &OperationSpec,
    runtime: &RuntimeOptions,
) -> Result<ResolvedAuth> {
    let Some(requirements) = &operation.security else {
        return Ok(ResolvedAuth::default());
    };

    if requirements.is_empty() {
        return Ok(ResolvedAuth::default());
    }

    let mut failures = Vec::new();
    for requirement in requirements {
        match resolve_security_requirement(spec, requirement, runtime) {
            Ok(auth) => return Ok(auth),
            Err(error) => failures.push(error.to_string()),
        }
    }

    bail!(
        "could not satisfy any security requirement for '{}': {}",
        operation.slug,
        failures.join(" | ")
    )
}

fn resolve_security_requirement(
    spec: &OpenApiSpec,
    requirement: &SecurityRequirement,
    runtime: &RuntimeOptions,
) -> Result<ResolvedAuth> {
    let mut auth = ResolvedAuth::default();

    for scheme_name in requirement.keys() {
        let scheme = spec.security_schemes.get(scheme_name).ok_or_else(|| {
            anyhow!(
                "security scheme '{}' is not declared in components",
                scheme_name
            )
        })?;
        apply_security_scheme(scheme_name, scheme, runtime, &mut auth)?;
    }

    Ok(auth)
}

fn apply_security_scheme(
    scheme_name: &str,
    scheme: &SecuritySchemeSpec,
    runtime: &RuntimeOptions,
    auth: &mut ResolvedAuth,
) -> Result<()> {
    match scheme.kind.as_str() {
        "http" => match scheme
            .scheme
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "bearer" => {
                let token = runtime
                    .auth_overrides
                    .get(scheme_name)
                    .cloned()
                    .or_else(|| env_auth_override(scheme_name))
                    .or_else(|| runtime.bearer_token.clone())
                    .ok_or_else(|| {
                        anyhow!("missing bearer token for security scheme '{scheme_name}'")
                    })?;
                auth.bearer_token = Some(token);
            }
            "basic" => {
                let credentials = runtime
                    .auth_overrides
                    .get(scheme_name)
                    .cloned()
                    .or_else(|| env_auth_override(scheme_name))
                    .and_then(|raw| {
                        raw.split_once(':')
                            .map(|(u, p)| (u.to_string(), p.to_string()))
                    })
                    .or_else(|| Some((runtime.basic_user.clone()?, runtime.basic_pass.clone()?)))
                    .ok_or_else(|| {
                        anyhow!("missing basic credentials for security scheme '{scheme_name}'")
                    })?;
                auth.basic_credentials = Some(credentials);
            }
            other => bail!("unsupported HTTP auth scheme '{other}'"),
        },
        "oauth2" | "openIdConnect" => {
            let token = runtime
                .auth_overrides
                .get(scheme_name)
                .cloned()
                .or_else(|| env_auth_override(scheme_name))
                .or_else(|| runtime.bearer_token.clone())
                .ok_or_else(|| anyhow!("missing token for security scheme '{scheme_name}'"))?;
            auth.bearer_token = Some(token);
        }
        "apiKey" => {
            let key_value = runtime
                .auth_overrides
                .get(scheme_name)
                .cloned()
                .or_else(|| env_auth_override(scheme_name))
                .or_else(|| runtime.api_key.clone())
                .ok_or_else(|| anyhow!("missing API key for security scheme '{scheme_name}'"))?;
            let parameter_name = scheme
                .parameter_name
                .clone()
                .ok_or_else(|| anyhow!("apiKey scheme '{scheme_name}' is missing 'name'"))?;
            match scheme.location.as_deref().unwrap_or("") {
                "query" => auth.query_pairs.push((parameter_name, key_value)),
                "header" => auth.header_pairs.push((parameter_name, key_value)),
                "cookie" => auth.cookie_pairs.push((parameter_name, key_value)),
                other => bail!("unsupported apiKey location '{other}'"),
            }
        }
        other => bail!("unsupported security scheme type '{other}'"),
    }

    Ok(())
}

fn attach_body(
    mut builder: RequestBuilder,
    body: &BodyInput,
    content_type: Option<&str>,
) -> Result<RequestBuilder> {
    match body {
        BodyInput::None => Ok(builder),
        BodyInput::Raw(bytes) => Ok(builder.body(bytes.clone())),
        BodyInput::Form { fields, files } => {
            if !files.is_empty() || matches!(content_type, Some("multipart/form-data")) {
                let mut form = Form::new();
                for (name, value) in fields {
                    form = form.text(name.clone(), value.clone());
                }
                for (field, path) in files {
                    let bytes = fs::read(path).with_context(|| {
                        format!("failed to read multipart file '{}'", path.display())
                    })?;
                    let part = Part::bytes(bytes).file_name(
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("upload.bin")
                            .to_string(),
                    );
                    form = form.part(field.clone(), part);
                }
                builder = builder.multipart(form);
            } else {
                builder = builder.form(fields);
            }
            Ok(builder)
        }
    }
}

fn print_request_plan(theme: &Theme, plan: &RequestPlan) -> Result<()> {
    eprintln!("{} {} {}", theme.header("request:"), plan.method, plan.url);
    for (name, value) in &plan.headers {
        eprintln!(
            "{} {}",
            theme.muted(format!("{}:", name.as_str())),
            value.to_str().unwrap_or("<binary>")
        );
    }
    match &plan.body {
        BodyInput::None => {}
        BodyInput::Raw(bytes) => {
            eprintln!("{} {} bytes", theme.accent("body:"), bytes.len());
            if let Ok(text) = std::str::from_utf8(bytes) {
                if let Ok(json) = serde_json::from_str::<Value>(text) {
                    eprintln!("{}", serde_json::to_string_pretty(&json)?);
                } else {
                    eprintln!("{}", text);
                }
            }
        }
        BodyInput::Form { fields, files } => {
            eprintln!(
                "{} form fields={:?} files={:?}",
                theme.accent("body:"),
                fields,
                files
            );
        }
    }
    Ok(())
}

fn print_response_body(bytes: &[u8], headers: &HeaderMap, raw_output: bool) -> Result<()> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");

    if !raw_output && content_type.contains("json") {
        if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        print!("{}", text);
        if !text.ends_with('\n') {
            println!();
        }
    } else {
        io::stdout().write_all(bytes)?;
    }

    Ok(())
}

fn parse_pairs<'a, I>(values: Option<I>) -> Result<Vec<(String, String)>>
where
    I: IntoIterator<Item = &'a String>,
{
    let mut pairs = Vec::new();
    if let Some(values) = values {
        for value in values {
            let (key, value) = value
                .split_once('=')
                .ok_or_else(|| anyhow!("expected NAME=VALUE, got '{value}'"))?;
            pairs.push((key.to_string(), value.to_string()));
        }
    }
    Ok(pairs)
}

fn parse_json_string_map(input: Option<&str>, env_name: &str) -> Result<BTreeMap<String, String>> {
    let Some(input) = input.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(BTreeMap::new());
    };

    let value: Value = serde_json::from_str(input)
        .with_context(|| format!("failed to parse {env_name} as a JSON object"))?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("{env_name} must be a JSON object"))?;
    let mut out = BTreeMap::new();
    for (key, value) in object {
        let value = value_to_string(value);
        out.insert(key.clone(), expand_env_templates(&value, env_name)?);
    }
    Ok(out)
}

fn expand_env_templates(input: &str, env_name: &str) -> Result<String> {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("{{.") {
        output.push_str(&rest[..start]);
        let after_open = &rest[start + 3..];
        let Some(end) = after_open.find("}}") else {
            bail!("{env_name} contains an unterminated environment template");
        };
        let var_name = after_open[..end].trim();
        if var_name.is_empty() {
            bail!("{env_name} contains an empty environment template");
        }
        let value = std::env::var(var_name).with_context(|| {
            format!("{env_name} references missing environment variable {var_name}")
        })?;
        if value.is_empty() {
            bail!("{env_name} references empty environment variable {var_name}");
        }
        output.push_str(&value);
        rest = &after_open[end + 2..];
    }

    output.push_str(rest);
    Ok(output)
}

fn insert_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<HeaderName> {
    let header_name = parse_header_name(name)?;
    let header_value = parse_header_value(name, value)?;
    headers.insert(header_name.clone(), header_value);
    Ok(header_name)
}

fn append_header_pairs_replacing_defaults(
    headers: &mut HeaderMap,
    pairs: &[(String, String)],
    default_header_names: &mut Vec<HeaderName>,
) -> Result<()> {
    let mut replaced_names = Vec::new();
    for (name, value) in pairs {
        let header_name = parse_header_name(name)?;
        if !replaced_names.contains(&header_name) {
            if let Some(index) = default_header_names
                .iter()
                .position(|default_name| default_name == header_name)
            {
                headers.remove(&header_name);
                default_header_names.remove(index);
            }
            replaced_names.push(header_name.clone());
        }
        let header_value = parse_header_value(name, value)?;
        headers.append(header_name, header_value);
    }
    Ok(())
}

fn parse_header_name(name: &str) -> Result<HeaderName> {
    HeaderName::from_bytes(name.as_bytes())
        .with_context(|| format!("invalid request header name '{name}'"))
}

fn parse_header_value(name: &str, value: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(value)
        .with_context(|| format!("invalid value for request header '{name}'"))
}

fn validate_required_headers(operation: &OperationSpec, headers: &HeaderMap) -> Result<()> {
    for parameter in &operation.parameters {
        if parameter.location != "header" || !parameter.required {
            continue;
        }
        let header_name = parse_header_name(&parameter.name)?;
        if !headers.contains_key(&header_name) {
            bail!(
                "missing required header parameter '{}' for operation '{}'",
                parameter.name,
                operation.slug
            );
        }
    }

    Ok(())
}

fn clone_body(body: &BodyInput) -> BodyInput {
    match body {
        BodyInput::None => BodyInput::None,
        BodyInput::Raw(bytes) => BodyInput::Raw(bytes.clone()),
        BodyInput::Form { fields, files } => BodyInput::Form {
            fields: fields.clone(),
            files: files.clone(),
        },
    }
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn env_auth_override(scheme_name: &str) -> Option<String> {
    let key = format!("{ENV_AUTH_PREFIX}{}", sanitize_env_key(scheme_name));
    std::env::var(key).ok()
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn base64_encode(input: String) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut out = String::new();
    let mut index = 0usize;
    while index < bytes.len() {
        let b0 = bytes[index];
        let b1 = *bytes.get(index + 1).unwrap_or(&0);
        let b2 = *bytes.get(index + 2).unwrap_or(&0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | b2 as u32;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if index + 1 < bytes.len() {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if index + 2 < bytes.len() {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        index += 3;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::ParameterSpec;
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

    fn minimal_operation() -> OperationSpec {
        OperationSpec {
            slug: "list-widgets".to_string(),
            operation_id: Some("listWidgets".to_string()),
            method: "GET".to_string(),
            path: "/widgets".to_string(),
            summary: None,
            description: None,
            tags: Vec::new(),
            deprecated: false,
            parameters: Vec::new(),
            request_body: None,
            responses: Vec::new(),
            servers: Vec::new(),
            security: None,
        }
    }

    fn minimal_runtime() -> RuntimeOptions {
        RuntimeOptions {
            server_url: None,
            server_index: 0,
            server_vars: BTreeMap::new(),
            bearer_token: None,
            basic_user: None,
            basic_pass: None,
            api_key: None,
            auth_overrides: BTreeMap::new(),
            default_headers: BTreeMap::new(),
            timeout_secs: 30,
            insecure: false,
            verbose: false,
            raw_output: false,
            output: None,
        }
    }

    fn minimal_invocation() -> InvocationInput {
        InvocationInput {
            path_values: BTreeMap::new(),
            query_values: Vec::new(),
            header_values: Vec::new(),
            cookie_values: Vec::new(),
            body: BodyInput::None,
            content_type: None,
            accept: None,
            dry_run: false,
        }
    }

    fn required_header_parameter(name: &str) -> ParameterSpec {
        ParameterSpec {
            name: name.to_string(),
            location: "header".to_string(),
            flag_name: format!("header-{}", name.to_ascii_lowercase()),
            arg_id: format!("param__header__{}", name.to_ascii_lowercase()),
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
    fn forwards_default_headers_to_request_plan() {
        let operation = minimal_operation();
        let mut runtime = minimal_runtime();
        runtime
            .default_headers
            .insert("X-API-Key".to_string(), "secret".to_string());
        let invocation = minimal_invocation();
        let auth = ResolvedAuth::default();

        let plan = build_request_plan(
            &operation,
            "https://api.example.com",
            &runtime,
            &invocation,
            &auth,
        )
        .unwrap();

        assert_eq!(
            plan.headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("secret")
        );
    }

    #[test]
    fn explicit_headers_replace_matching_default_headers() {
        let operation = minimal_operation();
        let mut runtime = minimal_runtime();
        runtime
            .default_headers
            .insert("X-API-Key".to_string(), "default".to_string());
        let mut invocation = minimal_invocation();
        invocation
            .header_values
            .push(("X-API-Key".to_string(), "explicit".to_string()));
        let auth = ResolvedAuth::default();

        let plan = build_request_plan(
            &operation,
            "https://api.example.com",
            &runtime,
            &invocation,
            &auth,
        )
        .unwrap();
        let values = plan
            .headers
            .get_all("x-api-key")
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .collect::<Vec<_>>();

        assert_eq!(values, vec!["explicit"]);
    }

    #[test]
    fn auth_headers_replace_matching_default_headers() {
        let operation = minimal_operation();
        let mut runtime = minimal_runtime();
        runtime
            .default_headers
            .insert("X-API-Key".to_string(), "default".to_string());
        let invocation = minimal_invocation();
        let mut auth = ResolvedAuth::default();
        auth.header_pairs
            .push(("X-API-Key".to_string(), "auth".to_string()));

        let plan = build_request_plan(
            &operation,
            "https://api.example.com",
            &runtime,
            &invocation,
            &auth,
        )
        .unwrap();
        let values = plan
            .headers
            .get_all("x-api-key")
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .collect::<Vec<_>>();

        assert_eq!(values, vec!["auth"]);
    }

    #[test]
    fn auth_headers_preserve_non_default_explicit_headers() {
        let operation = minimal_operation();
        let runtime = minimal_runtime();
        let mut invocation = minimal_invocation();
        invocation
            .header_values
            .push(("X-API-Key".to_string(), "explicit".to_string()));
        let mut auth = ResolvedAuth::default();
        auth.header_pairs
            .push(("X-API-Key".to_string(), "auth".to_string()));

        let plan = build_request_plan(
            &operation,
            "https://api.example.com",
            &runtime,
            &invocation,
            &auth,
        )
        .unwrap();
        let values = plan
            .headers
            .get_all("x-api-key")
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .collect::<Vec<_>>();

        assert_eq!(values, vec!["explicit", "auth"]);
    }

    #[test]
    fn required_header_parameter_is_satisfied_by_default_header() {
        let mut operation = minimal_operation();
        operation
            .parameters
            .push(required_header_parameter("API-Key"));
        let mut runtime = minimal_runtime();
        runtime
            .default_headers
            .insert("API-Key".to_string(), "secret".to_string());
        let invocation = minimal_invocation();
        let auth = ResolvedAuth::default();

        let plan = build_request_plan(
            &operation,
            "https://api.example.com",
            &runtime,
            &invocation,
            &auth,
        )
        .unwrap();

        assert_eq!(
            plan.headers
                .get("api-key")
                .and_then(|value| value.to_str().ok()),
            Some("secret")
        );
    }

    #[test]
    fn missing_required_header_parameter_is_rejected_at_runtime() {
        let mut operation = minimal_operation();
        operation
            .parameters
            .push(required_header_parameter("API-Key"));
        let runtime = minimal_runtime();
        let invocation = minimal_invocation();
        let auth = ResolvedAuth::default();

        let error = build_request_plan(
            &operation,
            "https://api.example.com",
            &runtime,
            &invocation,
            &auth,
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("API-Key"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn parses_json_string_map_errors_with_requested_env_name() {
        let error = parse_json_string_map(Some("[]"), "ACLI_DEFAULT_HEADERS").unwrap_err();

        assert!(
            error.to_string().contains("ACLI_DEFAULT_HEADERS"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn parse_json_string_map_expands_env_templates() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _api_key = EnvVarGuard::set("API_KEY", Some("runtime-secret"));

        let headers = parse_json_string_map(
            Some(r#"{"Authorization":"Bearer {{.API_KEY}}","X-Trace":"id-{{.API_KEY}}"}"#),
            "ACLI_DEFAULT_HEADERS",
        )
        .expect("parse");

        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer runtime-secret")
        );
        assert_eq!(
            headers.get("X-Trace").map(String::as_str),
            Some("id-runtime-secret")
        );
    }

    #[test]
    fn parse_json_string_map_errors_for_missing_env_templates() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _api_key = EnvVarGuard::set("API_KEY", None);

        let error = parse_json_string_map(
            Some(r#"{"Authorization":"Bearer {{.API_KEY}}"}"#),
            "ACLI_DEFAULT_HEADERS",
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("API_KEY"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn named_auth_override_uses_acli_prefix() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _new_prefix = EnvVarGuard::set("ACLI_AUTH_CUSTOM_HEADER", Some("secret"));

        assert_eq!(
            env_auth_override("custom-header").as_deref(),
            Some("secret")
        );
    }
}
