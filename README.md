# MCDPL Language Tooling

Standalone workspace for the open-source MCDPL language front end, stdio
language server, and editor clients.

## Layout

- `crates/mcdp-language`: shared syntax, diagnostics, source spans, semantic
  helper types, and unit/front-end APIs.
- `crates/mcdp-lsp`: Rust `tower-lsp` server.
- `editors/vscode-mcdpl`: VSCode development client.

## Local Development

Run the workspace checks from this repository root:

```sh
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

For VSCode testing, install the extension dependency once:

```sh
cd editors/vscode-mcdpl
npm install
```

Then open `editors/vscode-mcdpl` in VSCode and run the extension development host.
