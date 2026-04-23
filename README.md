# acli

Application Command Line Interface: a Rust CLI that loads an OpenAPI JSON document at runtime and turns it into an ergonomic command line interface.

## What it does

- Accepts the spec from `ACLI_SPEC` or `--spec`
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
- Renders an optional ASCII-art banner from `ACLI_TITLE`
- Applies an optional color theme from `ACLI_COLOR_SCHEME`

## Environment variables

- `ACLI_SPEC` — required unless `--spec` is passed
- `ACLI_TITLE` — optional ASCII banner title
- `ACLI_COLOR_SCHEME` — optional preset (`default|mono|ocean|sunset`) or JSON object
- `ACLI_COLOR` — `auto|always|never`
- `ACLI_BASE_URL` — override the spec server URL
- `ACLI_SERVER_VARS` — JSON object for server template variables
- `ACLI_DEFAULT_HEADERS` — JSON object of headers to send with every API request; these can satisfy required header parameters generated from the spec
- `ACLI_BEARER_TOKEN`
- `ACLI_BASIC_USER`
- `ACLI_BASIC_PASS`
- `ACLI_API_KEY`
- `ACLI_TIMEOUT_SECS`
- `ACLI_INSECURE`
- `ACLI_AUTH_<SCHEME_NAME>` — named auth override, where non-alphanumeric characters are converted to `_` and the name is uppercased

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
export ACLI_SPEC='https://petstore3.swagger.io/api/v3/openapi.json'
export ACLI_TITLE='Petstore'
export ACLI_COLOR_SCHEME='ocean'
export ACLI_DEFAULT_HEADERS='{"X-API-Key":"secret"}'

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
