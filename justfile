# List available recipes
default:
    @just --list

# Run the app in development mode
dev:
    pnpm tauri dev

# Run every check CI runs: lint, types, formatting, clippy and all tests
check: lint format-check typecheck fmt-check clippy deny test

# Lint the frontend
lint:
    pnpm lint

# Type-check the app and the specs
typecheck:
    pnpm typecheck

# Run frontend and backend tests
test:
    pnpm test -- --run
    cd src-tauri && cargo test

# Format the frontend (Prettier)
format:
    pnpm format

# Check frontend formatting without changing files
format-check:
    pnpm format:check

# Format the Rust code
fmt:
    cd src-tauri && cargo fmt

# Check Rust formatting without changing files
fmt-check:
    cd src-tauri && cargo fmt --check

# Lint the Rust code (warnings are errors, as in CI)
clippy:
    cd src-tauri && cargo clippy --all-targets -- -D warnings

# Check Rust dependency licences, advisories and sources (needs cargo-deny)
deny:
    cd src-tauri && cargo deny check

# Regenerate src/bindings.ts from the Tauri commands (tauri-specta)
bindings:
    cd src-tauri && UPDATE_IPC_BINDINGS=1 cargo test --lib ipc_bindings_are_up_to_date

# Run the transcript-merge benchmark
bench:
    cd src-tauri && cargo bench --bench transcript_merge

# Regenerate the README preview screenshots in dev-resources/
screenshots:
    pnpm exec playwright install chromium
    node scripts/generate-screenshots.mjs

# Bump the version, commit, tag app-v<version>, and push (triggers the CI release).
# LEVEL is patch (default), minor, or major.
release level="patch":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{level}}" in
      patch) choice=1 ;;
      minor) choice=2 ;;
      major) choice=3 ;;
      *) echo "Usage: just release [patch|minor|major]" >&2; exit 1 ;;
    esac
    if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
      echo "Working tree has uncommitted changes; commit or stash them first." >&2
      exit 1
    fi
    printf '%s\n' "$choice" | node scripts/bump-version.js
    version="$(node -p "require('./package.json').version")"
    git commit -am "chore: release v$version"
    git tag "app-v$version"
    # Push the branch and the tag explicitly. `--follow-tags` only pushes
    # annotated tags, so the (lightweight) release tag must be pushed by name
    # or the tag-triggered CI release never fires.
    git push
    git push origin "app-v$version"
    echo "Released v$version (tag app-v$version pushed)."
