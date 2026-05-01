use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::Path;
use url::Url;

pub type SecurityRequirement = BTreeMap<String, Vec<String>>;

#[derive(Debug, Clone, Serialize)]
pub struct ApiInfo {
    pub openapi_version: String,
    pub title: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SchemaSummary {
    pub type_name: Option<String>,
    pub format: Option<String>,
    pub enum_values: Vec<String>,
    pub default: Option<Value>,
    pub example: Option<Value>,
    pub items: Option<Box<SchemaSummary>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BodyFieldSpec {
    pub name: String,
    pub flag_name: String,
    pub arg_id: String,
    pub required: bool,
    pub deprecated: bool,
    pub description: Option<String>,
    pub schema: Option<SchemaSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParameterSpec {
    pub name: String,
    pub location: String,
    pub flag_name: String,
    pub arg_id: String,
    pub required: bool,
    pub deprecated: bool,
    pub description: Option<String>,
    pub style: Option<String>,
    pub explode: Option<bool>,
    pub schema: Option<SchemaSummary>,
    pub content_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaTypeSpec {
    pub content_type: String,
    pub schema: Option<SchemaSummary>,
    pub example: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestBodySpec {
    pub required: bool,
    pub description: Option<String>,
    pub content: Vec<MediaTypeSpec>,
    pub fields: Vec<BodyFieldSpec>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseSpec {
    pub status: String,
    pub description: Option<String>,
    pub content_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerVariableSpec {
    pub default: String,
    pub description: Option<String>,
    pub enum_values: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerSpec {
    pub url: String,
    pub description: Option<String>,
    pub variables: BTreeMap<String, ServerVariableSpec>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SecuritySchemeSpec {
    pub key: String,
    pub kind: String,
    pub scheme: Option<String>,
    pub parameter_name: Option<String>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub bearer_format: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationSpec {
    pub slug: String,
    pub operation_id: Option<String>,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub deprecated: bool,
    pub parameters: Vec<ParameterSpec>,
    pub request_body: Option<RequestBodySpec>,
    pub responses: Vec<ResponseSpec>,
    pub servers: Vec<ServerSpec>,
    pub security: Option<Vec<SecurityRequirement>>,
}

impl OperationSpec {
    pub fn title(&self) -> String {
        format!("{} {}", self.method, self.path)
    }

    pub fn path_parameters(&self) -> impl Iterator<Item = &ParameterSpec> {
        self.parameters
            .iter()
            .filter(|parameter| parameter.location == "path")
    }

    pub fn request_content_types(&self) -> Vec<String> {
        self.request_body
            .as_ref()
            .map(|body| {
                body.content
                    .iter()
                    .map(|item| item.content_type.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct OpenApiSpec {
    pub info: ApiInfo,
    pub operations: Vec<OperationSpec>,
    pub root_servers: Vec<ServerSpec>,
    pub security_schemes: BTreeMap<String, SecuritySchemeSpec>,
    pub root_security: Option<Vec<SecurityRequirement>>,
    pub source_url: Option<Url>,
    raw: Value,
}

pub fn load_spec_text(source: &str) -> Result<String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("empty spec source"));
    }

    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Ok(trimmed.to_string());
    }

    if let Ok(url) = Url::parse(trimmed) {
        match url.scheme() {
            "http" | "https" => {
                let client = Client::builder().build()?;
                let response = client.get(trimmed).send()?.error_for_status()?;
                return Ok(response.text()?);
            }
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|_| anyhow!("invalid file:// URL for spec source"))?;
                return Ok(fs::read_to_string(path)?);
            }
            _ => {}
        }
    }

    let path = Path::new(trimmed);
    if path.exists() {
        return Ok(fs::read_to_string(path)?);
    }

    Err(anyhow!(
        "could not resolve spec source as inline JSON, URL, or local file path"
    ))
}

impl OpenApiSpec {
    pub fn from_json_with_source(json: &str, source: Option<&str>) -> Result<Self> {
        let raw: Value = serde_json::from_str(json).context("failed to parse OpenAPI JSON")?;
        let openapi_version = raw
            .get("openapi")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();

        let info = ApiInfo {
            openapi_version,
            title: raw
                .get("info")
                .and_then(|info| info.get("title"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            version: raw
                .get("info")
                .and_then(|info| info.get("version"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            description: raw
                .get("info")
                .and_then(|info| info.get("description"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        };

        let source_url = source
            .and_then(|value| Url::parse(value.trim()).ok())
            .filter(|url| matches!(url.scheme(), "http" | "https"));

        let mut spec = Self {
            info,
            operations: Vec::new(),
            root_servers: Vec::new(),
            security_schemes: BTreeMap::new(),
            root_security: None,
            source_url,
            raw,
        };

        spec.root_servers = spec.parse_servers(spec.raw.get("servers"))?;
        spec.root_security = spec.parse_security_requirements(spec.raw.get("security"))?;
        spec.security_schemes = spec.parse_security_schemes()?;
        spec.operations = spec.parse_operations()?;

        Ok(spec)
    }

    pub fn find_operation(&self, name: &str) -> Option<&OperationSpec> {
        let normalized = slugify(name);
        self.operations.iter().find(|operation| {
            operation.slug == name
                || operation.slug == normalized
                || operation.operation_id.as_deref() == Some(name)
        })
    }

    pub fn apply_operation_name_overrides(
        &mut self,
        overrides: &BTreeMap<String, String>,
    ) -> Result<()> {
        if overrides.is_empty() {
            return Ok(());
        }

        let known_operation_ids = self
            .operations
            .iter()
            .filter_map(|operation| operation.operation_id.as_ref())
            .cloned()
            .collect::<BTreeSet<_>>();
        let unknown = overrides
            .keys()
            .filter(|operation_id| !known_operation_ids.contains(*operation_id))
            .cloned()
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            bail!(
                "cli.operationNames references unknown operationId values: {}",
                unknown.join(", ")
            );
        }

        let mut slug_to_operation = BTreeMap::<String, String>::new();
        let mut final_slugs = Vec::with_capacity(self.operations.len());

        for operation in &self.operations {
            let remapped_slug = match operation
                .operation_id
                .as_ref()
                .and_then(|operation_id| overrides.get(operation_id))
            {
                Some(name) => {
                    let slug = slugify(name);
                    if RESERVED_ROOT_COMMANDS.contains(&slug.as_str()) {
                        bail!(
                            "cli.operationNames maps operationId '{}' to reserved CLI command '{}'",
                            operation.operation_id.as_deref().unwrap_or("<unknown>"),
                            slug
                        );
                    }
                    slug
                }
                None => operation.slug.clone(),
            };

            let operation_label = operation
                .operation_id
                .clone()
                .unwrap_or_else(|| operation.slug.clone());
            if let Some(existing) =
                slug_to_operation.insert(remapped_slug.clone(), operation_label.clone())
            {
                bail!(
                    "cli.operationNames produces duplicate CLI command '{}' for operations '{}' and '{}'",
                    remapped_slug,
                    existing,
                    operation_label
                );
            }

            final_slugs.push(remapped_slug);
        }

        for (operation, slug) in self.operations.iter_mut().zip(final_slugs) {
            operation.slug = slug;
        }
        self.operations
            .sort_by(|left, right| left.slug.cmp(&right.slug));

        Ok(())
    }

    fn parse_operations(&self) -> Result<Vec<OperationSpec>> {
        let mut operations = Vec::new();
        let mut used_slugs = HashSet::new();

        let paths = self
            .raw
            .get("paths")
            .and_then(Value::as_object)
            .context("OpenAPI document is missing a 'paths' object")?;

        for (path, path_item_raw) in paths {
            let path_item = self.resolve_value(path_item_raw)?;
            let path_parameters = self.collect_parameters(path_item.get("parameters"))?;
            let path_servers = self.parse_servers(path_item.get("servers"))?;

            for method in [
                "get", "put", "post", "delete", "options", "head", "patch", "trace",
            ] {
                if let Some(operation_raw) = path_item.get(method) {
                    if operation_raw.is_null() {
                        continue;
                    }

                    let operation_value = self.resolve_value(operation_raw)?;
                    let mut parameters = path_parameters.clone();
                    parameters = merge_parameters(
                        parameters,
                        self.collect_parameters(operation_value.get("parameters"))?,
                    );
                    sort_parameters(&mut parameters);

                    let operation_servers = {
                        let own = self.parse_servers(operation_value.get("servers"))?;
                        if !own.is_empty() {
                            own
                        } else if !path_servers.is_empty() {
                            path_servers.clone()
                        } else {
                            self.root_servers.clone()
                        }
                    };

                    let operation_id = operation_value
                        .get("operationId")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    let summary = operation_value
                        .get("summary")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    let description = operation_value
                        .get("description")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    let tags = operation_value
                        .get("tags")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(Value::as_str)
                                .map(ToOwned::to_owned)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let deprecated = operation_value
                        .get("deprecated")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let request_body =
                        self.parse_request_body(operation_value.get("requestBody"))?;
                    let responses = self.parse_responses(operation_value.get("responses"))?;
                    let security = match operation_value.get("security") {
                        Some(value) => self.parse_security_requirements(Some(value))?,
                        None => self.root_security.clone(),
                    };

                    let base_name = operation_id
                        .clone()
                        .unwrap_or_else(|| derive_operation_name(method, path));
                    let slug = unique_slug(&base_name, &mut used_slugs);

                    operations.push(OperationSpec {
                        slug,
                        operation_id,
                        method: method.to_ascii_uppercase(),
                        path: path.to_string(),
                        summary,
                        description,
                        tags,
                        deprecated,
                        parameters,
                        request_body,
                        responses,
                        servers: operation_servers,
                        security,
                    });
                }
            }
        }

        operations.sort_by(|left, right| left.slug.cmp(&right.slug));
        Ok(operations)
    }

    fn parse_security_schemes(&self) -> Result<BTreeMap<String, SecuritySchemeSpec>> {
        let Some(components) = self.raw.get("components") else {
            return Ok(BTreeMap::new());
        };
        let Some(security_schemes) = components.get("securitySchemes").and_then(Value::as_object)
        else {
            return Ok(BTreeMap::new());
        };

        let mut schemes = BTreeMap::new();
        for (key, value) in security_schemes {
            let resolved = self.resolve_value(value)?;
            let scheme = SecuritySchemeSpec {
                key: key.clone(),
                kind: resolved
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                scheme: resolved
                    .get("scheme")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                parameter_name: resolved
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                location: resolved
                    .get("in")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                description: resolved
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                bearer_format: resolved
                    .get("bearerFormat")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            };
            schemes.insert(key.clone(), scheme);
        }

        Ok(schemes)
    }

    fn parse_security_requirements(
        &self,
        security_value: Option<&Value>,
    ) -> Result<Option<Vec<SecurityRequirement>>> {
        let Some(value) = security_value else {
            return Ok(None);
        };
        let Some(items) = value.as_array() else {
            return Err(anyhow!("security field must be an array"));
        };

        let mut requirements = Vec::new();
        for item in items {
            let Some(obj) = item.as_object() else {
                return Err(anyhow!("security requirement item must be an object"));
            };
            let mut requirement = SecurityRequirement::new();
            for (scheme, scopes) in obj {
                let scope_items = scopes
                    .as_array()
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(ToOwned::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                requirement.insert(scheme.clone(), scope_items);
            }
            requirements.push(requirement);
        }

        Ok(Some(requirements))
    }

    fn parse_servers(&self, value: Option<&Value>) -> Result<Vec<ServerSpec>> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        let Some(items) = value.as_array() else {
            return Err(anyhow!("servers field must be an array"));
        };

        let mut servers = Vec::new();
        for item in items {
            let resolved = self.resolve_value(item)?;
            let url = resolved
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("server is missing a 'url' field"))?
                .to_string();
            let description = resolved
                .get("description")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let mut variables = BTreeMap::new();
            if let Some(object) = resolved.get("variables").and_then(Value::as_object) {
                for (key, value) in object {
                    let variable = self.resolve_value(value)?;
                    let default = variable
                        .get("default")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let description = variable
                        .get("description")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    let enum_values = variable
                        .get("enum")
                        .and_then(Value::as_array)
                        .map(|values| values.iter().map(value_to_string).collect())
                        .unwrap_or_default();
                    variables.insert(
                        key.clone(),
                        ServerVariableSpec {
                            default,
                            description,
                            enum_values,
                        },
                    );
                }
            }
            servers.push(ServerSpec {
                url,
                description,
                variables,
            });
        }

        Ok(servers)
    }

    fn collect_parameters(&self, value: Option<&Value>) -> Result<Vec<ParameterSpec>> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        let Some(items) = value.as_array() else {
            return Err(anyhow!("parameters field must be an array"));
        };

        let mut parameters = Vec::new();
        for item in items {
            let resolved = self.resolve_value(item)?;
            parameters.push(self.parse_parameter(&resolved)?);
        }

        Ok(parameters)
    }

    fn parse_parameter(&self, value: &Value) -> Result<ParameterSpec> {
        let Some(obj) = value.as_object() else {
            return Err(anyhow!("parameter must be an object"));
        };

        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("parameter is missing 'name'"))?
            .to_string();
        let location = obj
            .get("in")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("parameter '{name}' is missing 'in'"))?
            .to_string();
        let required = if location == "path" {
            true
        } else {
            obj.get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        let deprecated = obj
            .get("deprecated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let description = obj
            .get("description")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let style = obj
            .get("style")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let explode = obj.get("explode").and_then(Value::as_bool);
        let schema = match obj.get("schema") {
            Some(schema) => self.parse_schema_summary(schema)?,
            None => None,
        };
        let content_types = obj
            .get("content")
            .and_then(Value::as_object)
            .map(|content| content.keys().cloned().collect())
            .unwrap_or_default();
        let flag_name = build_parameter_flag_name(&name, &location);
        let arg_id = format!("param__{}__{}", location, sanitize_identifier(&name));

        Ok(ParameterSpec {
            name,
            location,
            flag_name,
            arg_id,
            required,
            deprecated,
            description,
            style,
            explode,
            schema,
            content_types,
        })
    }

    fn parse_request_body(&self, value: Option<&Value>) -> Result<Option<RequestBodySpec>> {
        let Some(value) = value else {
            return Ok(None);
        };
        let resolved = self.resolve_value(value)?;
        let Some(obj) = resolved.as_object() else {
            return Err(anyhow!("requestBody must be an object"));
        };
        let required = obj
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let description = obj
            .get("description")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);

        let mut content = Vec::new();
        let mut field_candidates = Vec::<(bool, String, Vec<BodyFieldSpec>)>::new();
        if let Some(items) = obj.get("content").and_then(Value::as_object) {
            for (content_type, value) in items {
                let media_type = self.resolve_value(value)?;
                let schema_value = media_type.get("schema");
                let schema = match schema_value {
                    Some(schema) => self.parse_schema_summary(schema)?,
                    None => None,
                };
                if let Some(schema_value) = schema_value {
                    let fields = self.parse_body_fields(schema_value)?;
                    let is_json = is_json_content_type(content_type);
                    if !fields.is_empty() && is_json {
                        field_candidates.push((is_json, content_type.clone(), fields));
                    }
                }
                let example = media_type
                    .get("example")
                    .cloned()
                    .or_else(|| {
                        media_type
                            .get("examples")
                            .and_then(Value::as_object)
                            .and_then(|examples| {
                                examples.values().find_map(|example| {
                                    let resolved = self.resolve_value(example).ok()?;
                                    resolved.get("value").cloned()
                                })
                            })
                    })
                    .or_else(|| schema.as_ref().and_then(|schema| schema.example.clone()));

                content.push(MediaTypeSpec {
                    content_type: content_type.clone(),
                    schema,
                    example,
                });
            }
        }
        content.sort_by(|left, right| left.content_type.cmp(&right.content_type));
        field_candidates
            .sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        let fields = field_candidates
            .into_iter()
            .next()
            .map(|(_, _, fields)| fields)
            .unwrap_or_default();

        Ok(Some(RequestBodySpec {
            required,
            description,
            content,
            fields,
        }))
    }

    fn parse_responses(&self, value: Option<&Value>) -> Result<Vec<ResponseSpec>> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        let Some(items) = value.as_object() else {
            return Err(anyhow!("responses field must be an object"));
        };

        let mut responses = Vec::new();
        for (status, value) in items {
            let resolved = self.resolve_value(value)?;
            let description = resolved
                .get("description")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let mut content_types = resolved
                .get("content")
                .and_then(Value::as_object)
                .map(|items| items.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            content_types.sort();
            responses.push(ResponseSpec {
                status: status.clone(),
                description,
                content_types,
            });
        }

        responses.sort_by(|left, right| left.status.cmp(&right.status));
        Ok(responses)
    }

    fn parse_schema_summary(&self, value: &Value) -> Result<Option<SchemaSummary>> {
        let resolved = self.resolve_value(value)?;
        let summary = match resolved {
            Value::Bool(allowed) => SchemaSummary {
                type_name: Some(if allowed { "any" } else { "never" }.to_string()),
                format: None,
                enum_values: Vec::new(),
                default: None,
                example: None,
                items: None,
            },
            Value::Object(ref obj) => self.schema_summary_from_object(obj)?,
            _ => return Ok(None),
        };

        Ok(Some(summary))
    }

    fn schema_summary_from_object(&self, obj: &Map<String, Value>) -> Result<SchemaSummary> {
        if let Some(composition) = obj
            .get("anyOf")
            .or_else(|| obj.get("oneOf"))
            .and_then(Value::as_array)
        {
            let mut type_names = Vec::new();
            let mut enum_values = Vec::new();
            let mut item_summary = None;
            for item in composition {
                if let Some(summary) = self.parse_schema_summary(item)? {
                    if let Some(type_name) = summary.type_name {
                        for part in type_name.split('|') {
                            let part = part.trim();
                            if !part.is_empty() && !type_names.iter().any(|seen| seen == part) {
                                type_names.push(part.to_string());
                            }
                        }
                    }
                    enum_values.extend(summary.enum_values);
                    if item_summary.is_none() {
                        item_summary = summary.items;
                    }
                }
            }

            return Ok(SchemaSummary {
                type_name: (!type_names.is_empty()).then(|| type_names.join("|")),
                format: obj
                    .get("format")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                enum_values,
                default: obj.get("default").cloned(),
                example: obj.get("example").cloned(),
                items: item_summary,
            });
        }

        if let Some(items) = obj.get("allOf").and_then(Value::as_array) {
            let mut summaries = Vec::new();
            for item in items {
                if let Some(summary) = self.parse_schema_summary(item)? {
                    summaries.push(summary);
                }
            }
            if let Some(mut summary) = summaries.into_iter().next() {
                summary.default = obj.get("default").cloned().or(summary.default);
                summary.example = obj.get("example").cloned().or(summary.example);
                return Ok(summary);
            }
        }

        Ok(SchemaSummary {
            type_name: schema_type_name(obj).or_else(|| infer_schema_type(obj)),
            format: obj
                .get("format")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            enum_values: obj
                .get("enum")
                .and_then(Value::as_array)
                .map(|values| values.iter().map(value_to_string).collect())
                .unwrap_or_default(),
            default: obj.get("default").cloned(),
            example: obj.get("example").cloned(),
            items: match obj.get("items") {
                Some(items) => self.parse_schema_summary(items)?.map(Box::new),
                None => None,
            },
        })
    }

    fn parse_body_fields(&self, value: &Value) -> Result<Vec<BodyFieldSpec>> {
        let resolved = self.resolve_value(value)?;
        let Some(schema_obj) = self.object_schema(&resolved)? else {
            return Ok(Vec::new());
        };

        let required = schema_obj
            .get("required")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let Some(properties) = schema_obj.get("properties").and_then(Value::as_object) else {
            return Ok(Vec::new());
        };

        let mut fields = Vec::new();
        let mut used_flags = HashSet::new();
        let mut used_ids = HashSet::new();
        for (name, raw_property) in properties {
            let property = self.resolve_value(raw_property)?;
            let property_obj = property.as_object();
            let deprecated = property_obj
                .and_then(|obj| obj.get("deprecated"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let description = property_obj.and_then(|obj| {
                obj.get("description")
                    .and_then(Value::as_str)
                    .or_else(|| obj.get("title").and_then(Value::as_str))
                    .map(ToOwned::to_owned)
            });
            let schema = self.parse_schema_summary(&property)?;
            let flag_name = unique_name(format!("body-{}", slugify(name)), &mut used_flags, '-');
            let arg_id = unique_name(
                format!("body__{}", sanitize_identifier(name)),
                &mut used_ids,
                '_',
            );

            fields.push(BodyFieldSpec {
                name: name.clone(),
                flag_name,
                arg_id,
                required: required.contains(name),
                deprecated,
                description,
                schema,
            });
        }

        Ok(fields)
    }

    fn object_schema(&self, value: &Value) -> Result<Option<Map<String, Value>>> {
        let Some(obj) = value.as_object() else {
            return Ok(None);
        };
        if obj.contains_key("properties") {
            return Ok(Some(obj.clone()));
        }

        for key in ["anyOf", "oneOf", "allOf"] {
            let Some(items) = obj.get(key).and_then(Value::as_array) else {
                continue;
            };
            for item in items {
                let resolved = self.resolve_value(item)?;
                if let Value::Object(candidate) = resolved {
                    if candidate.contains_key("properties") {
                        return Ok(Some(candidate));
                    }
                }
            }
        }

        Ok(None)
    }

    fn resolve_value(&self, value: &Value) -> Result<Value> {
        self.resolve_value_inner(value, 0)
    }

    fn resolve_value_inner(&self, value: &Value, depth: usize) -> Result<Value> {
        if depth > 32 {
            return Err(anyhow!("reference resolution exceeded max depth"));
        }

        let Some(obj) = value.as_object() else {
            return Ok(value.clone());
        };

        let Some(reference) = obj.get("$ref").and_then(Value::as_str) else {
            return Ok(value.clone());
        };

        if !reference.starts_with("#/") {
            return Err(anyhow!(
                "only local JSON pointer refs are supported, found '{reference}'"
            ));
        }

        let pointer = &reference[1..];
        let target = self
            .raw
            .pointer(pointer)
            .ok_or_else(|| anyhow!("could not resolve ref '{reference}'"))?;
        let mut resolved = self.resolve_value_inner(target, depth + 1)?;

        if obj.len() > 1 {
            if let Value::Object(ref mut resolved_object) = resolved {
                for (key, value) in obj {
                    if key == "$ref" {
                        continue;
                    }
                    resolved_object.insert(key.clone(), value.clone());
                }
            }
        }

        Ok(resolved)
    }
}

const RESERVED_ROOT_COMMANDS: &[&str] = &[
    "completions",
    "describe",
    "help",
    "install",
    "list",
    "schema",
    "uninstall",
];

fn schema_type_name(obj: &Map<String, Value>) -> Option<String> {
    match obj.get("type") {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Array(values)) => {
            let parts = values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("|"))
        }
        _ => None,
    }
}

fn infer_schema_type(obj: &Map<String, Value>) -> Option<String> {
    if obj.contains_key("properties") {
        Some("object".to_string())
    } else if obj.contains_key("items") {
        Some("array".to_string())
    } else if obj.contains_key("enum") {
        Some("string".to_string())
    } else {
        None
    }
}

fn build_parameter_flag_name(name: &str, location: &str) -> String {
    let slug = slugify(name);
    match location {
        "path" => slug,
        "query" => format!("query-{slug}"),
        "header" => format!("header-{slug}"),
        "cookie" => format!("cookie-{slug}"),
        other => format!("{other}-{slug}"),
    }
}

fn merge_parameters(
    mut base: Vec<ParameterSpec>,
    overrides: Vec<ParameterSpec>,
) -> Vec<ParameterSpec> {
    let mut seen = BTreeSet::new();
    for parameter in &overrides {
        seen.insert((parameter.location.clone(), parameter.name.clone()));
    }
    base.retain(|parameter| !seen.contains(&(parameter.location.clone(), parameter.name.clone())));
    base.extend(overrides);
    base
}

fn sort_parameters(parameters: &mut [ParameterSpec]) {
    parameters.sort_by(|left, right| {
        location_rank(&left.location)
            .cmp(&location_rank(&right.location))
            .then_with(|| left.required.cmp(&right.required).reverse())
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn location_rank(location: &str) -> usize {
    match location {
        "path" => 0,
        "query" => 1,
        "header" => 2,
        "cookie" => 3,
        _ => 4,
    }
}

fn derive_operation_name(method: &str, path: &str) -> String {
    let path_slug = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.trim_matches('{').trim_matches('}'))
        .collect::<Vec<_>>()
        .join("-");

    if path_slug.is_empty() {
        method.to_string()
    } else {
        format!("{method}-{path_slug}")
    }
}

fn unique_slug(base: &str, used: &mut HashSet<String>) -> String {
    unique_name(slugify(base), used, '-')
}

fn unique_name(base: String, used: &mut HashSet<String>, separator: char) -> String {
    if used.insert(base.clone()) {
        return base;
    }

    let mut index = 2usize;
    loop {
        let candidate = format!("{base}{separator}{index}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

fn is_json_content_type(content_type: &str) -> bool {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    media_type == "application/json" || media_type.ends_with("+json")
}

pub fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;
    let chars = input.chars().collect::<Vec<_>>();

    for (index, ch) in chars.iter().copied().enumerate() {
        let prev = chars.get(index.saturating_sub(1)).copied();
        let next = chars.get(index + 1).copied();
        let is_upper = ch.is_ascii_uppercase();
        let is_alnum = ch.is_ascii_alphanumeric();

        if is_alnum {
            let insert_dash = is_upper
                && !out.is_empty()
                && !last_was_dash
                && prev.is_some_and(|prev| prev.is_ascii_lowercase() || prev.is_ascii_digit())
                || (is_upper
                    && !out.is_empty()
                    && !last_was_dash
                    && prev.is_some_and(|prev| prev.is_ascii_uppercase())
                    && next.is_some_and(|next| next.is_ascii_lowercase()));

            if insert_dash {
                out.push('-');
            }

            out.push(if is_upper {
                ch.to_ascii_lowercase()
            } else {
                ch
            });
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }

    while out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "operation".to_string()
    } else {
        out
    }
}

fn sanitize_identifier(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' => out.push(ch.to_ascii_lowercase()),
            _ => out.push('_'),
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn operation_id_openapi_json() -> &'static str {
        r##"{
                    "openapi": "3.0.3",
                    "info": {"title": "x", "version": "1"},
                    "paths": {
                        "/pets": {
                            "get": {
                                "operationId": "listPets",
                                "responses": {"200": {"description": "ok"}}
                            },
                            "post": {
                                "operationId": "createPet",
                                "responses": {"201": {"description": "created"}}
                            }
                        }
                    }
                }"##
    }

    #[test]
    fn slugify_is_stable() {
        assert_eq!(slugify("ListPets"), "list-pets");
        assert_eq!(slugify("list pets"), "list-pets");
        assert_eq!(slugify("GET /pets/{id}"), "get-pets-id");
    }

    #[test]
    fn applies_operation_name_overrides() {
        let mut spec =
            OpenApiSpec::from_json_with_source(operation_id_openapi_json(), None).unwrap();
        let overrides = BTreeMap::from([("listPets".to_string(), "pets list".to_string())]);

        spec.apply_operation_name_overrides(&overrides).unwrap();

        let operation = spec.find_operation("pets-list").expect("remapped op");
        assert_eq!(operation.operation_id.as_deref(), Some("listPets"));
        assert_eq!(operation.slug, "pets-list");
        assert_eq!(spec.find_operation("listPets").unwrap().slug, "pets-list");
    }

    #[test]
    fn rejects_unknown_operation_name_override() {
        let mut spec =
            OpenApiSpec::from_json_with_source(operation_id_openapi_json(), None).unwrap();
        let overrides = BTreeMap::from([("missingOperation".to_string(), "pets-list".to_string())]);

        let error = spec.apply_operation_name_overrides(&overrides).unwrap_err();

        assert!(error.to_string().contains("unknown operationId"));
    }

    #[test]
    fn rejects_reserved_operation_name_override() {
        let mut spec =
            OpenApiSpec::from_json_with_source(operation_id_openapi_json(), None).unwrap();
        let overrides = BTreeMap::from([("listPets".to_string(), "list".to_string())]);

        let error = spec.apply_operation_name_overrides(&overrides).unwrap_err();

        assert!(error.to_string().contains("reserved CLI command"));
    }

    #[test]
    fn rejects_operation_name_override_that_collides_with_existing_slug() {
        let mut spec =
            OpenApiSpec::from_json_with_source(operation_id_openapi_json(), None).unwrap();
        let overrides = BTreeMap::from([("listPets".to_string(), "create pet".to_string())]);

        let error = spec.apply_operation_name_overrides(&overrides).unwrap_err();

        assert!(error
            .to_string()
            .contains("duplicate CLI command 'create-pet'"));
    }

    #[test]
    fn resolves_local_parameter_refs() {
        let json = r##"{
          "openapi": "3.0.3",
          "info": {"title": "x", "version": "1"},
          "components": {
            "parameters": {
              "PetId": {
                "name": "petId",
                "in": "path",
                "required": true,
                "schema": {"type": "string"}
              }
            }
          },
          "paths": {
            "/pets/{petId}": {
              "get": {
                "parameters": [{"$ref": "#/components/parameters/PetId"}],
                "responses": {"200": {"description": "ok"}}
              }
            }
          }
        }"##;

        let spec = OpenApiSpec::from_json_with_source(json, None).unwrap();
        let operation = spec.find_operation("get-pets-pet-id").unwrap();
        assert_eq!(operation.parameters.len(), 1);
        assert_eq!(operation.parameters[0].name, "petId");
        assert_eq!(operation.parameters[0].location, "path");
    }

    #[test]
    fn extracts_json_request_body_fields_from_component_schema() {
        let json = r##"{
          "openapi": "3.1.0",
          "info": {"title": "x", "version": "1"},
          "paths": {
            "/exec": {
              "post": {
                "operationId": "exec",
                "requestBody": {
                  "required": true,
                  "content": {
                    "application/json": {
                      "schema": {"$ref": "#/components/schemas/ExecRequest"}
                    }
                  }
                },
                "responses": {"200": {"description": "ok"}}
              }
            }
          },
          "components": {
            "schemas": {
              "ExecRequest": {
                "type": "object",
                "required": ["command"],
                "properties": {
                  "command": {"type": "array", "items": {"type": "string"}, "description": "Command argv"},
                  "timeout": {"anyOf": [{"type": "integer"}, {"type": "null"}], "title": "Timeout"},
                  "pty": {"type": "boolean", "default": false}
                }
              }
            }
          }
        }"##;

        let spec = OpenApiSpec::from_json_with_source(json, None).unwrap();
        let operation = spec.find_operation("exec").unwrap();
        let body = operation.request_body.as_ref().unwrap();

        assert_eq!(body.fields.len(), 3);
        let command = body
            .fields
            .iter()
            .find(|field| field.name == "command")
            .unwrap();
        assert_eq!(command.flag_name, "body-command");
        assert!(command.required);
        assert_eq!(
            command.schema.as_ref().unwrap().type_name.as_deref(),
            Some("array")
        );
        assert_eq!(
            command
                .schema
                .as_ref()
                .unwrap()
                .items
                .as_ref()
                .unwrap()
                .type_name
                .as_deref(),
            Some("string")
        );
        let timeout = body
            .fields
            .iter()
            .find(|field| field.name == "timeout")
            .unwrap();
        assert_eq!(
            timeout.schema.as_ref().unwrap().type_name.as_deref(),
            Some("integer|null")
        );
    }
}
