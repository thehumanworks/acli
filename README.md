# acli

Application Command Line Interface: a Rust CLI that loads an OpenAPI JSON document at runtime and turns it into an ergonomic command line interface.

## What it does

- Accepts the spec from `OPENAPI_CLI_SPEC` or `--spec`
  - `https://...` URL
  - local file path
  - raw inline JSON string
- Builds one subcommand per operation
- Generates help screens from the spec metadata
- Supports path, query, header, and cookie parameters
- Supports raw bodies, file bodies, stdin bodies, forms, and multipart uploads
- Supports common OpenAPI auth styles
  - HTTP bearer
  - HTTP basic
  - apiKey in header/query/cookie
  - oauth2/openIdConnect token passthrough
- Supports shell completions
- Renders an optional ASCII-art banner from `OPENAPI_CLI_TITLE`
- Applies an optional color theme from `OPENAPI_CLI_COLOR_SCHEME`

## Environment variables

- `OPENAPI_CLI_SPEC` — required unless `--spec` is passed
- `OPENAPI_CLI_TITLE` — optional ASCII banner title
- `OPENAPI_CLI_COLOR_SCHEME` — optional preset (`default|mono|ocean|sunset`) or JSON object
- `OPENAPI_CLI_COLOR` — `auto|always|never`
- `OPENAPI_CLI_BASE_URL` — override the spec server URL
- `OPENAPI_CLI_SERVER_VARS` — JSON object for server template variables
- `OPENAPI_CLI_BEARER_TOKEN`
- `OPENAPI_CLI_BASIC_USER`
- `OPENAPI_CLI_BASIC_PASS`
- `OPENAPI_CLI_API_KEY`
- `OPENAPI_CLI_TIMEOUT_SECS`
- `OPENAPI_CLI_INSECURE`
- `OPENAPI_CLI_AUTH_<SCHEME_NAME>` — named auth override, where non-alphanumeric characters are converted to `_` and the name is uppercased

## Example theme JSON

```json
{
  "banner": "bright-cyan bold",
  "header": "bright-blue bold",
  "accent": "cyan bold",
  "muted": "bright-black",
  "success": "green bold",
  "warning": "yellow bold",
  "error": "bright-red bold"
}
```

## Example usage

```bash
export OPENAPI_CLI_SPEC='https://petstore3.swagger.io/api/v3/openapi.json'
export OPENAPI_CLI_TITLE='Petstore'
export OPENAPI_CLI_COLOR_SCHEME='ocean'

cargo run -- list
cargo run -- describe get-pet-by-id
cargo run -- get-pet-by-id --pet-id 123
cargo run -- add-pet --body-file ./pet.json
cargo run -- completions zsh > _acli
```

## Generated command shape

```text
acli list
acli describe <operation>
acli completions <shell>
acli <operation> [generated parameter flags] [--query name=value] [--body-file payload.json]
```

## Project layout

- `src/main.rs` — bootstrap, banner, and command dispatch
- `src/cli.rs` — runtime clap command construction
- `src/spec.rs` — OpenAPI loading, local `$ref` resolution, and operation extraction
- `src/execute.rs` — request assembly, auth resolution, invocation, and rendering
- `src/colors.rs` — color presets, JSON theme overrides, and clap styles
- `src/config.rs` — env var names and bootstrap parsing
