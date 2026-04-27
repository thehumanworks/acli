# acli

Application Command Line Interface: a Rust CLI that loads an OpenAPI JSON document at runtime and turns it into an ergonomic command line interface.

## What it does

- Accepts the spec from `ACLI_SPEC` or `--spec`
  - `https://...` URL
  - local file path
  - raw inline JSON string
- Accepts an acli JSON config from `ACLI_CONFIG` or `--config`
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
- `ACLI_CONFIG` — optional acli JSON config path or inline JSON object
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
- `ACLI_DATA_DIR` — app-owned data directory for installed lock bundles
- `ACLI_INSTALL_ROOT` — install root whose `bin` directory receives locked CLI launchers
- `ACLI_AUTH_<SCHEME_NAME>` — named auth override, where non-alphanumeric characters are converted to `_` and the name is uppercased

## JSON config

Use `acli schema` to generate a JSON Schema for editor completion and validation:

```bash
cargo run -- schema > acli.schema.json
```

Then point your config at the schema:

```json
{
  "$schema": "./acli.schema.json",
  "version": 1,
  "spec": "https://petstore3.swagger.io/api/v3/openapi.json",
  "cli": {
    "binaryName": "petstore_cli",
    "title": "Petstore",
    "colorScheme": "ocean",
    "operationNames": {
      "listPets": "pets-list"
    }
  },
  "http": {
    "defaultHeaders": {
      "X-API-Key": "{{.PETSTORE_API_KEY}}"
    },
    "timeoutSecs": 30
  },
  "install": {
    "output": "./petstore-cli",
    "secrets": "env"
  }
}
```

Config values sit between environment variables and explicit flags: flags win over JSON config, and JSON config wins over environment variables for non-secret runtime and install options.

```bash
cargo run -- --config ./acli.json list
cargo run -- install --config ./acli.json
```

### Operation command remapping

Use `cli.operationNames` to map OpenAPI `operationId` values to clearer command names:

```json
{
  "version": 1,
  "spec": "./openapi.json",
  "cli": {
    "operationNames": {
      "createInternalBillingCustomer": "create-customer",
      "listAllCustomerInvoices": "list-invoices"
    }
  }
}
```

The map keys must match the original `operationId` values exactly. The configured command names are slugified into single CLI command tokens, must stay unique across the API, and cannot shadow built-in commands such as `list` or `describe`. Installed locked CLIs persist the same remap in `acli.lock.json`, so the renamed commands still work after installation.

### Editor schema discovery

The most portable setup is to keep `acli.schema.json` next to `acli.json` and include `$schema` in the config file. This works in Cursor, VS Code, and Neovim setups that use a JSON language server.

For repo-wide discovery in Cursor and VS Code, add `.vscode/settings.json`:

```json
{
  "json.schemas": [
    {
      "fileMatch": [
        "/acli.json",
        "/*.acli.json"
      ],
      "url": "./acli.schema.json"
    }
  ]
}
```

For Neovim with `jsonls`, map the same schema in LSP settings:

```lua
require("lspconfig").jsonls.setup({
  settings = {
    json = {
      schemas = {
        {
          fileMatch = { "acli.json", "*.acli.json" },
          url = "./acli.schema.json",
        },
      },
      validate = { enable = true },
    },
  },
})
```

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

`acli install` creates an API-specific lock bundle and installs a launcher named after the API. The launcher is a copy of the current `acli` runtime, so installing or uninstalling a locked CLI does not require Cargo, rustc, or a host Rust toolchain.

```bash
cargo run -- install \
  --output ./petstore-cli \
  --spec https://petstore3.swagger.io/api/v3/openapi.json

petstore_cli list
```

The generated lock bundle contains `openapi.json` and `acli.lock.json`. During install, `acli` copies that bundle into an app-owned data directory under `locks/<binary-name>` and installs a launcher into `<install-root>/bin`.

Defaults:

- Data directory: `ACLI_DATA_DIR` when set; otherwise the platform app-data location (`~/Library/Application Support/acli` on macOS, `$XDG_DATA_HOME/acli` or `~/.local/share/acli` on Linux, `%LOCALAPPDATA%\acli` on Windows)
- Install root: `ACLI_INSTALL_ROOT` when set; otherwise `~/.local` on macOS/Linux and `%LOCALAPPDATA%\acli` on Windows

Use `--data-dir <DIR>` or `--install-root <DIR>` to override those locations. By default, launchers are installed into a user-writable bin directory such as `~/.local/bin`. To only write the lock bundle without installing the launcher, pass `--no-install`.

To uninstall a locked CLI and its app-owned lock bundle, no Rust toolchain is needed:

```bash
cargo run -- uninstall petstore_cli
```

## Locked CLI secret references

`acli install` can install an API-specific CLI without storing secret values. Use `--secrets env` and pass the host environment variable names to resolve when the generated tool starts:

```bash
cargo run -- install \
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

Pushes to `main` publish a tagged GitHub prerelease with downloadable binaries and SHA-256 checksum files. The generated tag format is `main-v<package-version>-<12-char-commit>`, so each main-branch build has a stable release page and immutable assets.

Version tags that start with `v` still publish stable release archives and checksum files.

- `acli-linux-x86_64.tar.gz`
- `acli-linux-arm64.tar.gz`
- `acli-macos-x86_64.tar.gz`
- `acli-macos-arm64.tar.gz`
- `acli-windows-x86_64.zip`
- `acli-windows-arm64.zip`

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
- `src/app_config.rs` — typed JSON config parsing and JSON Schema generation
