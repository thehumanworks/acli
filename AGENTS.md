# AGENTS.md

## acli JSON Config Schema Docs

- When changing the `acli` JSON config shape in `src/app_config.rs`, update `README.md` and bootstrap help text in `src/config.rs`.
- `acli schema` must print the generated JSON Schema to stdout.
- `acli schema --help` must document editor setup:
  - Generate `acli.schema.json` with `acli schema > acli.schema.json`.
  - Prefer adding `"$schema": "./acli.schema.json"` to each `acli.json` or `*.acli.json` file.
  - Cursor and VS Code can use `.vscode/settings.json` with `json.schemas` mapping `"/acli.json"` and `"/*.acli.json"` to `"./acli.schema.json"`.
  - Neovim users can configure the same mapping through `jsonls` with `fileMatch = { "acli.json", "*.acli.json" }`.
