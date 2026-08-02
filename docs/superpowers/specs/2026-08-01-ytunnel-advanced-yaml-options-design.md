# ytunnel Advanced Options (YAML-based)

**Date:** 2026-08-01
**Status:** Design approved, ready for implementation planning

## Motivation

ytunnel currently generates a minimal per-tunnel cloudflared YAML with only `tunnel`, `credentials-file`, and a single ingress rule. Users who need cloudflared knobs — Host-header rewriting for local dev (`google-spike.localhost`), `noTLSVerify`, `protocol: http2`, `loglevel: debug`, `retries`, `edge-ip-version`, etc. — currently cannot set them from ytunnel at all.

We want to expose the full set of realistic cloudflared configuration options through a new **Advanced** pane in the TUI edit sheet, storing user selections in `tunnels.toml` and serializing them into the generated per-tunnel YAML.

## Goals

- Users can set any of ~30 commonly-tuned cloudflared options per-tunnel from the TUI.
- Options are stored durably in `tunnels.toml` and reapplied on every tunnel start.
- The generated YAML remains user-inspectable at `~/.config/ytunnel/tunnel-configs/{name}.yml`.
- Backward compatible: existing tunnels.toml files load without migration.
- Zero changes to daemon plists / systemd units — the daemon already re-reads the YAML on restart.

## Non-Goals

- Free-form YAML editing / escape hatches. The curated pane is the interface. If a user needs a knob we don't expose, they file an issue and we add it to the curated list. This keeps everything typed, documented, and safe.
- Automatic parsing of `cloudflared --help` at runtime. The option table is a hardcoded Rust `const` slice; when cloudflared adds new options, someone updates the table in a future release.
- Exposing CLI-only cloudflared flags that have no YAML equivalent (`--credentials-contents`, `--token-file`, etc.) — out of scope for a config-file-centric design.
- Editing existing Basic fields (target, zone, name) in Advanced. Those stay in Basic.

## Architecture

### Data model (`src/state.rs`)

Add two new fields to `PersistentTunnel`, both with `#[serde(default)]` for backward compatibility:

```rust
pub struct PersistentTunnel {
    // ... existing fields ...

    /// Top-level cloudflared YAML options (loglevel, protocol, retries, ...)
    #[serde(default)]
    pub tunnel_options: BTreeMap<String, TunnelOptionValue>,

    /// Per-ingress-rule originRequest options (httpHostHeader, noTLSVerify, ...)
    #[serde(default)]
    pub origin_request: BTreeMap<String, TunnelOptionValue>,
}

/// Type-preserving YAML value. Restricted to what our curated options need.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TunnelOptionValue {
    Bool(bool),
    Int(i64),
    String(String),
    List(Vec<String>),
}
```

`BTreeMap` gives deterministic ordering in the generated YAML (helpful for diffs and inspection). Missing map = empty map on deserialize.

### Curated option registry (`src/cloudflared_options.rs`, new file)

A static registry describing every option ytunnel knows how to render:

```rust
pub struct OptionSpec {
    pub yaml_key: &'static str,        // "loglevel" or "httpHostHeader"
    pub scope: OptionScope,             // Tunnel | OriginRequest
    pub display_name: &'static str,     // "Log Level"
    pub description: &'static str,      // one-line help
    pub kind: OptionKind,
}

pub enum OptionScope { Tunnel, OriginRequest }

pub enum OptionKind {
    Bool { default: bool },
    Int { default: Option<i64>, min: Option<i64>, max: Option<i64> },
    String { default: Option<&'static str>, placeholder: &'static str },
    Duration { default: Option<&'static str> },   // "30s", "1m", validated
    Enum { default: Option<&'static str>, choices: &'static [&'static str] },
    List { placeholder: &'static str },           // comma or newline separated
}

pub const OPTIONS: &[OptionSpec] = &[ /* ~30 entries */ ];
```

**Initial option set (approximate — final list confirmed during implementation):**

*Tunnel scope (top-level YAML keys — all flat, no nested structures in v1):*
`loglevel`, `transport-loglevel`, `protocol`, `retries`, `grace-period`,
`edge-ip-version`, `region`, `metrics-update-freq`, `compression-quality`,
`post-quantum`, `label`, `features` (list), `bastion`, `pidfile`, `logfile`,
`log-directory`

Nested-key options like `warp-routing.enabled` are deferred until v2. All v1 options serialize to a single top-level YAML key.

*OriginRequest scope (per-ingress-rule):*
`httpHostHeader`, `originServerName`, `caPool`, `noTLSVerify`, `noHappyEyeballs`,
`connectTimeout`, `tlsTimeout`, `tcpKeepAlive`, `keepAliveConnections`,
`keepAliveTimeout`, `http2Origin`, `disableChunkedEncoding`, `proxyAddress`,
`proxyPort`, `proxyType`

### YAML generation (`src/state.rs::generate_tunnel_config`)

Rewrite to build the YAML programmatically (via `serde_yaml`) rather than the current `format!` string:

1. Start with the base doc: `tunnel`, `credentials-file`.
2. For each `(key, value)` in `tunnel.tunnel_options`, insert at the top level, cross-referencing `OPTIONS` to convert the `TunnelOptionValue` into the correct YAML shape (duration string, int, bool, list of strings).
3. Build the ingress list: one rule for the tunnel's hostname + service; if `tunnel.origin_request` is non-empty, attach it as an `originRequest:` map on that rule.
4. Append the terminal `service: http_status:404` catch-all rule.
5. Serialize with `serde_yaml::to_string()`.

Rationale for switching to `serde_yaml`: string interpolation doesn't compose when you have a dozen optional nested keys. It also gets us proper escaping for values with special characters.

### Ephemeral tunnels (`src/cloudflare.rs`)

The ephemeral `ytunnel run` path builds its own temp YAML at `src/cloudflare.rs:55-62`. Extract the YAML-building logic into a shared helper so ephemeral and persistent tunnels emit identical YAML shapes when given the same `tunnel_options` / `origin_request`. Ephemeral tunnels get the same Advanced options via CLI flags on `ytunnel run` (out of scope for this spec — persistent-only for v1; ephemeral is a follow-up).

### TUI (`src/tui/app.rs`, `src/tui/ui.rs`)

**New pattern: two-tab edit sheet.**

Replaces the current chained-modal edit flow (`EditTarget` → `EditZone`) with a single overlay dialog that has two tabs:

- **Basic** — the existing editable fields (target URL, zone), plus (moved from separate keybindings if any) `auto_start` and `metrics_port`.
- **Advanced** — a scrollable two-section list:
  - Section header: "Tunnel"
  - One row per `OptionSpec` with `scope == Tunnel`: `flag_name  [current value or "default"]  <one-line description>`
  - Section header: "Origin Request"
  - One row per `OptionSpec` with `scope == OriginRequest`, same shape.

Keybindings:
- `Tab` / `Shift+Tab` — switch between Basic and Advanced tabs.
- `↑/↓` — navigate rows within the active tab.
- `Enter` — open the type-appropriate editor for the focused row:
  - `Bool` → toggle immediately, no sub-dialog.
  - `Enum` → picker (like the existing `EditZone` picker).
  - `Int`, `String`, `Duration`, `List` → text input (like `EditTarget`), with type validation on Enter.
- `d` on a set option — clear it (reverts to cloudflared default; row shows "default" again).
- `Ctrl+S` — save all pending edits, regenerate YAML, reinstall the daemon (see below).
- `Esc` — discard all pending edits (with confirm prompt if the sheet is dirty).

The Advanced tab pulls its content from `OPTIONS` — no per-option UI code. Adding a new option later means one entry in the registry.

### Input mode additions

New `InputMode` variants: `EditSheetBasic`, `EditSheetAdvanced`, `EditSheetPicker(String)`, `EditSheetInput(String)`, `EditSheetConfirmDiscard`. The old `EditTarget` / `EditZone` variants are removed and their handling folded into the sheet.

### Daemon reload

After `Ctrl+S` in the edit sheet, if any of `tunnel_options`, `origin_request`, `target`, or `zone` changed:

1. Regenerate the YAML with the new `generate_tunnel_config()`.
2. If the tunnel is currently installed as a daemon:
   - macOS: `launchctl unload <plist> && launchctl load <plist>` (or `kickstart -k`).
   - Linux: `systemctl daemon-reload && systemctl restart cloudflared-tunnel-<name>.service`.
3. If the tunnel is running but not installed as a daemon (unlikely for persistent tunnels), leave it — user will restart manually.

This is a behavior change from today: `EditTarget` currently regenerates the YAML but relies on the daemon to eventually pick it up. Making reinstall explicit means edits take effect immediately, matching user expectations for a "save" action.

## Data Flow

1. User opens edit sheet (`e` on a tunnel row in the list).
2. Loads the current `PersistentTunnel` into the sheet's edit buffer.
3. User navigates Basic ↔ Advanced, edits fields.
4. `Ctrl+S`:
   - Validate all pending inputs (duration parsing, int ranges, enum membership).
   - If invalid, show error, stay in sheet.
   - Apply edits to the in-memory `PersistentTunnel`.
   - `TunnelState::save()` — writes updated `tunnels.toml`.
   - `state::write_tunnel_config()` — regenerates per-tunnel YAML.
   - `daemon::reload_if_installed(tunnel)` — restarts the daemon if needed.
   - Close sheet, return to list.
5. `Esc`: prompt confirm if dirty, discard, close sheet.

## Error Handling

- **Validation errors** (bad duration, out-of-range int, unknown enum value): shown inline in the row's editor, sheet stays open.
- **YAML write failure**: bubble up as an error toast in the TUI; edits stay in the in-memory buffer so user can retry.
- **Daemon reload failure**: warn but do not roll back the config write — the new YAML is on disk, user can restart the daemon manually. Show a toast: "Config saved but daemon reload failed: {err}. Restart the tunnel manually."

## Testing

- **Unit tests in `src/state.rs`:** given a `PersistentTunnel` with various `tunnel_options` and `origin_request` combinations, assert the generated YAML matches expected output. Cover empty maps, mixed types, deprecated originRequest fields with typos, etc.
- **Unit tests in `src/cloudflared_options.rs`:** every `OptionSpec` in `OPTIONS` is valid — non-empty key, description, kind matches YAML type expectation, enums have non-empty choices.
- **Roundtrip test:** write a `PersistentTunnel` with populated maps to TOML, load it back, assert equality. Ensures serde handles the untagged enum correctly.
- **TUI:** manual verification only for v1 — ratatui doesn't have snapshot testing in this codebase. Follow-up work could add `insta` snapshots of the rendered sheet.

## Backward Compatibility

- `#[serde(default)]` on both new fields means existing `tunnels.toml` files without them deserialize to empty maps. No migration code needed.
- Existing YAML output is preserved when both maps are empty — the generator produces byte-identical output to today's format string for tunnels with no advanced options set. This is worth an explicit test: `test_yaml_matches_legacy_format_when_no_options_set`.

## Rollout / Migration

- No schema migration.
- On first run of the new version, all existing tunnels have empty `tunnel_options` and `origin_request` — Advanced pane shows every row as "default".
- On first save from the new edit sheet, `tunnels.toml` gains the new fields for that tunnel only.

## Open Questions

None outstanding — all design decisions closed during brainstorming.

## Files Changed

| File | Change |
|---|---|
| `src/state.rs` | Add `tunnel_options`, `origin_request` fields; rewrite `generate_tunnel_config` using `serde_yaml`. |
| `src/cloudflared_options.rs` (new) | Curated `OPTIONS` registry with ~30 entries. |
| `src/tui/app.rs` | Replace `EditTarget`/`EditZone` `InputMode`s with edit-sheet variants; add sheet state (active tab, selected row, per-row edit buffer, dirty flag). |
| `src/tui/ui.rs` | Add `render_edit_sheet`, replace `render_edit_dialog` and `render_edit_zone_dialog`. |
| `src/daemon.rs` | Add `reload_if_installed(tunnel)` that wraps launchctl/systemctl reload. |
| `src/cloudflare.rs` | Route ephemeral YAML generation through the shared helper (persistent-parity; wiring Advanced through the ephemeral CLI is a follow-up). |
| `Cargo.toml` | Add `serde_yaml` dependency. |
