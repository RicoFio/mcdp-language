# MCDPL - Co-Design Language Tools

Development VSCode client for the `mcdp-lsp` stdio language server.

## Local Use

From this directory, install the extension dependency once:

```sh
npm install
```

Open this extension folder in VSCode and run the `Run MCDPL Extension` debug
target, or install it as an unpacked extension from the VSCode extension
development host.

By default the client auto-detects the Rust workspace root, launches
`target/debug/mcdp-lsp` when that binary exists, and otherwise falls back to:

```sh
cargo run -p mcdp-lsp
```

The client registers the `mcdpl` language id for:

- `*.mcdp`
- `*.mcdp_interface`
- `*.mcdp_poset`
- `*.mcdp_template`

## Settings

- `mcdpl.server.command`: optional command or binary path for the server.
- `mcdpl.server.args`: extra arguments appended to the server command.
- `mcdpl.server.cwd`: working directory for the server command.
