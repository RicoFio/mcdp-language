set dotenv-load := false

fmt:
    cargo fmt --all

check:
    cargo check --workspace --all-targets

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace --all-targets

build:
    cargo build --workspace --all-targets

vsc-install:
    npm install --prefix editors/vscode-mcdpl

vsc-check:
    npm run check --prefix editors/vscode-mcdpl
