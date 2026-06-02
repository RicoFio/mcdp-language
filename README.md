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

## Versioning and Releases

Versioning for the language crate, the `mcdp-lsp` binary, and editor packages are controlled through the Rust workspace version in the `Cargo.toml`. To prepare a new release, bump the root `[workspace.package]` version with `just`:

```sh
just bump-version patch   # 0.1.0 -> 0.1.1
just bump-version minor   # 0.1.0 -> 0.2.0
just bump-version major   # 0.1.0 -> 1.0.0
```

To set an exact version instead, run:

```sh
just set-version 0.2.0
```

Both recipes update `Cargo.toml`, refresh Cargo metadata, and synchronize
`editors/vscode-mcdpl/package.json` plus the root package entries in
`editors/vscode-mcdpl/package-lock.json`.

The LSP reports the same version in the LSP initialize handshake and on the
command line:

```sh
cargo run -p mcdp-lsp -- --version
```

Before tagging, run the release check:

```sh
just release-check
```

Commit the Cargo and editor version updates, then create and push a matching
tag:

```sh
version="$(just version)"
git tag "v${version}"
git push origin main "v${version}"
```

Pull requests and pushes to `main` run validation. Pushing a `vX.Y.Z` tag also
starts the VSCode publish job. GitHub Actions builds the release `mcdp-lsp`
binary for each supported platform, bundles it into the corresponding VSIX,
verifies the bundled server's `--version` output matches the Cargo version, and
publishes the extension version to the marketplace.

## Importing mcdp-language as crate

To use MCDPL language primitives in another Rust codebase, depend on the
package as `mcdp-language` in `Cargo.toml`. In Rust source code the crate is
imported as `mcdp_language`.

For a workspace, put the dependency in the root `Cargo.toml`:

```toml
[workspace.dependencies]
mcdp-language = { git = "https://github.com/RicoFio/mcdp-language", package = "mcdp-language" }
```

Then opt into it from each member crate that needs the front-end APIs:

```toml
[dependencies]
mcdp-language.workspace = true
```

While developing this repository and a downstream workspace side by side, add a
patch in the downstream workspace root so Cargo uses the local checkout instead
of the locked Git revision:

```toml
[patch."https://github.com/RicoFio/mcdp-language"]
mcdp-language = { path = "../mcdp-language/crates/mcdp-language" }
```

This is the pattern used from the `codesign` workspace: the dependency remains a
Git dependency for normal builds, while local development resolves to
`../mcdp-language/crates/mcdp-language`. The `Cargo.lock` entry still appears as
`mcdp-language 0.1.0`; the patch changes where Cargo gets that package from.

For a single non-workspace crate, put the Git dependency directly under
`[dependencies]`:

```toml
[dependencies]
mcdp-language = { git = "https://github.com/RicoFio/mcdp-language", package = "mcdp-language" }
```

## Rust API Usage

The crate exposes the shared MCDPL language front end used by compiler and
editor tooling:

- `lex` and `parse_document` for tokenization and document-shape recovery.
- `lower_document` for source-preserving semantic declarations.
- `graph_from_semantic` for a lightweight `DesignGraph` shell.
- `parse_expression_text`, `parse_expression_list_text`, and
  `parse_unit_expression_text` for standalone expression/unit parsing.
- `canonical_unit_label`, `canonical_unit_option`, `normalize_unit_text`, and
  `units_equivalent` for unit-label normalization and equivalence checks.
- Shared data types such as `SourceId`, `TextRange`, `TextSpan`, `Diagnostic`,
  `SemanticModel`, `PortDecl`, `ConstraintDecl`, `DesignGraph`, `PosetRef`, and
  `PortDirection`.

Typical parsing and lowering looks like this:

```rust
use mcdp_language::{
    CheckReport, SourceId, graph_from_semantic, lower_document, parse_document,
};

fn main() {
    let source = r#"
mcdp {
  provides speed [m/s]
  requires power [W]
  total_power = 10 W
}
"#;

    let source_id = SourceId::new("rover.mcdp");
    let parsed = parse_document(source_id.clone(), source);
    let (model, lowering_diagnostics) = lower_document(source_id, &parsed);

    let mut report = CheckReport::new();
    report.extend(parsed.diagnostics.clone());
    report.extend(lowering_diagnostics);

    if report.has_errors() {
        for diagnostic in report.diagnostics {
            eprintln!(
                "[{:?}] {}: {}",
                diagnostic.severity, diagnostic.code, diagnostic.message
            );
        }
        return;
    }

    let model = model.expect("a valid document lowers to a semantic model");
    let graph = graph_from_semantic(Some("rover".to_owned()), &model);

    println!(
        "{} ports, {} instances, {} constraints",
        graph.ports.len(),
        graph.nodes.len(),
        graph.constraints.len()
    );
}
```

For editor-style features, you can use the syntax layer without semantic
lowering:

```rust
use mcdp_language::{TokenKind, lex};

let tokens = lex("mcdp { provides speed [m/s] }");
let identifiers: Vec<_> = tokens
    .iter()
    .filter(|token| token.kind == TokenKind::Ident)
    .map(|token| token.text.as_str())
    .collect();
```

For compiler or solver integration, use the expression and unit helpers when
adapting source text into downstream types:

```rust
use mcdp_language::{
    canonical_unit_label, parse_expression_text, parse_unit_expression_text, units_equivalent,
};

let expression = parse_expression_text("10 W + 5 W");
let unit = parse_unit_expression_text("m/s^2");

assert_eq!(canonical_unit_label("dimensionless"), None);
assert!(units_equivalent(Some("m / s^2"), Some("m/s^2")));
```
