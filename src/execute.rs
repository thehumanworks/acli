use crate::colors::Theme;
use crate::config::{ENV_INSECURE, ENV_SERVER_VARS};
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
        let mut server_vars =
            parse_json_string_map(std::env::var(ENV_SERVER_VARS).ok().as_deref())?;
        for (key, value) in parse_pairs(matches.get_many::<String>("server_var"))? {
            server_vars.insert(key, value);
        }

        let mut auth_overrides = BTreeMap::new();
        for (key, value) in parse_pairs(matches.get_many::<String>("auth"))? {
            auth_overrides.insert(key, value);
        }

        let insecure = matches.get_flag("insecure") || env_truthy(ENV_INSECURE);

        Ok(Self {
            server_url: matches.get_one::<String>("server_url").cloned(),
            server_index: *matches.get_one::<usize>("server_index").unwrap_or(&0),
            server_vars,
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
    _runtime: &RuntimeOptions,
    invocation: &InvocationInput,
    auth: &ResolvedAuth,
) -> Result<RequestPlan> {
    let mut query_pairs = invocation.query_values.clone();
    query_pairs.extend(auth.query_pairs.clone());

    let url = build_url(operation, server_url, &invocation.path_values, &query_pairs)?;
    let mut headers = HeaderMap::new();

    for (name, value) in &invocation.header_values {
        append_header(&mut headers, name, value)?;
    }
    for (name, value) in &auth.header_pairs {
        append_header(&mut headers, name, value)?;
    }
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
            "operation '{}' does not declare any servers; pass --server-url or set OPENAPI_CLI_BASE_URL",
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
        bail!("server URL is empty; pass --server-url or set OPENAPI_CLI_BASE_URL");
    }

    if Url::parse(trimmed).is_ok() {
        return Ok(trimmed.to_string());
    }

    let base = base.ok_or_else(|| {
        anyhow!(
            "server URL '{trimmed}' is relative but the spec was not loaded from an HTTP(S) URL; \
             pass --server-url or set OPENAPI_CLI_BASE_URL to an absolute URL"
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

fn parse_json_string_map(input: Option<&str>) -> Result<BTreeMap<String, String>> {
    let Some(input) = input.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(BTreeMap::new());
    };

    let value: Value = serde_json::from_str(input)
        .with_context(|| format!("failed to parse {ENV_SERVER_VARS} as a JSON object"))?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("{ENV_SERVER_VARS} must be a JSON object"))?;
    let mut out = BTreeMap::new();
    for (key, value) in object {
        out.insert(key.clone(), value_to_string(value));
    }
    Ok(out)
}

fn append_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<()> {
    let header_name = HeaderName::from_bytes(name.as_bytes())?;
    let header_value = HeaderValue::from_str(value)?;
    headers.append(header_name, header_value);
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
    let key = format!("OPENAPI_CLI_AUTH_{}", sanitize_env_key(scheme_name));
    std::env::var(key).ok()
}

fn sanitize_env_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
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
