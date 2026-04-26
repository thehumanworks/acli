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
- Can generate API-specific locked CLIs that resolve secret values from host environment variables at runtime
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

## Locked CLIs

`acli lock` generates an API-specific crate, embeds the pinned spec and lock manifest into the compiled binary, then runs Cargo to build and install the CLI:

```bash
cargo run -- lock \
  --output ./petstore-cli \
  --spec https://petstore3.swagger.io/api/v3/openapi.json

petstore_cli list
```

The install step uses `cargo install --path <output> --force`, so it requires a Rust toolchain with Cargo and rustc available. `acli` does not ship Cargo or rustc; install Rust with rustup or pass `--cargo <PATH>` if Cargo is not on `PATH`. To install somewhere other than Cargo's default user bin directory, pass `--install-root <DIR>`. To only generate the crate without building or installing, pass `--no-install`.

The generated crate does not use `build.rs` for installation. Cargo build scripts are designed for build-time code generation and metadata, not cross-platform installation into a user's executable path; `cargo install` is the portable Rust-native build and install mechanism.

## Locked CLI secret references

`acli lock` can generate an API-specific crate without storing secret values. Use `--secrets env` and pass the host environment variable names to resolve when the generated tool starts:

```bash
cargo run -- lock \
  --output ./petstore-cli \
  --spec https://petstore3.swagger.io/api/v3/openapi.json \
  --secrets env \
  --bearer-token-env PETSTORE_BEARER_TOKEN \
  --api-key-env PETSTORE_API_KEY \
  --auth-env partner=PETSTORE_PARTNER_TOKEN

PETSTORE_API_KEY=secret petstore_cli list
```

At runtime, non-empty host values are copied into `ACLI_BEARER_TOKEN`, `ACLI_API_KEY`, or `ACLI_AUTH_<SCHEME_NAME>` before the request is built.

Default headers also support runtime environment templates with `{{.ENV_VAR}}` placeholders:

```bash
export API_KEY=secret
export ACLI_DEFAULT_HEADERS='{"Authorization":"Bearer {{.API_KEY}}"}'
```

Missing or empty template variables are reported as configuration errors.

## Binary downloads

Pushes to `main` build Linux, macOS, and Windows binaries with GitHub Actions. Download the latest main-branch artifacts from the `Build binaries` workflow run.

Version tags that start with `v` also publish archives and SHA-256 checksum files to GitHub Releases:

- `acli-linux-x86_64.tar.gz`
- `acli-macos-x86_64.tar.gz`
- `acli-macos-aarch64.tar.gz`
- `acli-windows-x86_64.zip`

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
