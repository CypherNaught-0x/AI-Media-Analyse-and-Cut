---
name: webkit-profiles
description: Dev builds and the installed app keep separate localStorage (different WebKit data dirs); check the right one
metadata:
  type: project
---

On macOS the webview's localStorage (settings `llm-settings`, autosaved session `home-edit-session-v1`) lives in two different places:

- **Installed app** (bundle id `itemis.ai-media-cutter`): `~/Library/WebKit/itemis.ai-media-cutter/WebsiteData/Default/*/*/LocalStorage/localstorage.sqlite3`
- **`pnpm tauri dev`** (unbundled binary): `~/Library/WebKit/ai-media-cutter/WebsiteData/...`

**Why:** on 2026-10-05 I diagnosed a "media file missing" banner and a keychain migration from the installed app's DB while the user was running a dev build. That DB was months old, so both checks read the wrong profile. WebKit also only flushes localStorage to disk lazily (often on quit), so a fresh change may not be visible in the sqlite file yet.

**How to apply:** when inspecting stored settings/sessions, pick the profile matching how the user runs the app, check the file's mtime, and read it read-only (`file:...?mode=ro`). Values are UTF-16LE JSON in `ItemTable`. Never print secrets; the API key now lives in the keychain (service `itemis.ai-media-cutter`, account `llm-api-key`).
