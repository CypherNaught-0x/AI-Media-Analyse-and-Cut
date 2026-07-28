# List available recipes
default:
    @just --list

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
    git push --follow-tags
    echo "Released v$version (tag app-v$version pushed)."
