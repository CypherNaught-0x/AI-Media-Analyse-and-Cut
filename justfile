# List available recipes
default:
    @just --list

# Regenerate the README preview screenshots in dev-resources/
screenshots:
    pnpm exec playwright install chromium
    node scripts/generate-screenshots.mjs
