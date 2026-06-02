set dotenv-load := false

version:
    @awk 'BEGIN { in_section = 0 } /^\[workspace\.package\]$/ { in_section = 1; next } /^\[/ { in_section = 0 } in_section && /^version = / { gsub(/"/, "", $3); print $3; exit }' Cargo.toml

set-version version:
    #!/usr/bin/env bash
    set -euo pipefail
    version="{{version}}"
    if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]]; then
        echo "version must be SemVer, for example 0.2.0" >&2
        exit 2
    fi
    tmp="$(mktemp)"
    awk -v version="$version" '
        /^\[workspace\.package\]$/ { in_section = 1 }
        in_section && /^version = / {
            print "version = \"" version "\""
            in_section = 0
            next
        }
        /^\[/ && !/^\[workspace\.package\]$/ { in_section = 0 }
        { print }
    ' Cargo.toml > "$tmp"
    mv "$tmp" Cargo.toml
    cargo metadata --format-version 1 >/dev/null
    npm run sync-version --prefix editors/vscode-mcdpl
    echo "Set workspace and VSCode extension version to $version"

bump-version part="patch":
    #!/usr/bin/env bash
    set -euo pipefail
    part="{{part}}"
    current="$(just version)"
    if [[ ! "$current" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
        echo "can only bump plain major.minor.patch versions, found $current" >&2
        exit 2
    fi
    major="${BASH_REMATCH[1]}"
    minor="${BASH_REMATCH[2]}"
    patch="${BASH_REMATCH[3]}"
    case "$part" in
        major) major=$((major + 1)); minor=0; patch=0 ;;
        minor) minor=$((minor + 1)); patch=0 ;;
        patch) patch=$((patch + 1)) ;;
        *) echo "part must be one of: major, minor, patch" >&2; exit 2 ;;
    esac
    just set-version "$major.$minor.$patch"

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

release-check:
    cargo fmt --all --check
    just check
    just test
    just clippy
    just vsc-check
    cargo run -p mcdp-lsp -- --version
