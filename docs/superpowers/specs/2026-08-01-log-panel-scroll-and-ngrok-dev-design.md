# Log Panel: Scrollable Buffer + ngrok-dev Mode

**Date:** 2026-08-01
**Status:** Design approved, ready for implementation

## Motivation

Two related pain points in the current log panel:

1. **Not scrollable**: the panel shows only the last 100 lines of the tunnel's log file, re-read wholesale every 1 second. Users can't page back to see earlier events; the whole file is re-read every tick regardless of size.
2. **Cloudflared logs at `info` show nothing per-request; at `debug` they show ~1000 chars/line dumping all headers.** Neither is useful for "what URLs am I serving right now?" ngrok solves this with a compact request table. ytunnel should offer the same shape.

## Goals

- Log panel becomes scrollable with keyboard (PgUp/PgDn/Home/End). Follow-mode auto-scrolls to bottom on new lines; manually scrolling up pauses follow.
- LogTail reads only newly-appended bytes on each tick (byte-offset tracking), not the whole file. Buffer is a bounded ring (5000 lines default).
- New `Log mode` field on the Basic tab: `Default | Debug | ngrok-dev`.
- `ngrok-dev` mode:
  - Forces cloudflared to `debug` under the hood.
  - Filters the log stream to emit one compact line per request, dropping non-request noise (edge discovery, connection registration, etc.).
- `loglevel` moves out of the Advanced-options registry (it's now first-class in Basic).

## Non-Goals

- Log search (`/` grep) — natural follow-up, not in this spec.
- Log rotation — cloudflared doesn't rotate and this spec doesn't add it. Note as known limitation.
- Response status codes or per-request duration in ngrok-dev — cloudflared doesn't log these at any level. ngrok-dev output shows method + path + origin, not status/timing.
- Ephemeral tunnel logs (`ytunnel run`) — this spec is for persistent-tunnel log panel only.

## Cloudflared Debug Log Format (empirical)

Confirmed by inspecting `~/Library/Application Support/ytunnel/logs/google-spike.log`.

### Successful request (DBG level, present only when `loglevel: debug`)
```
2026-08-01T22:42:33Z DBG GET https://google-spike.alexreade.me/live/longpoll?... HTTP/1.1 connIndex=1 content-length=0 event=1 headers={...huge JSON...} host=google-spike.alexreade.me ingressRule=0 originService=http://google-spike.localhost path=/live/longpoll
```

Parseable fields (via regex + `key=value` scan):
- Timestamp (ISO-8601, up to `Z `)
- Level (`DBG`)
- Method (`GET`/`POST`/`HEAD`/`PUT`/`DELETE`/etc. — the first bare word after the level)
- URL (full https URL following the method)
- `path=<value>` (already-parsed request path)
- `host=<value>`, `originService=<value>`, `connIndex=<n>`, `ingressRule=<n>`

Not present anywhere in the log stream: response status, response bytes, request duration.

### Failed request (ERR level, present at all levels)
```
2026-08-01T22:42:33Z ERR Request failed error="Incoming request ended abruptly: context canceled" connIndex=0 dest=https://google-spike.alexreade.me/live/longpoll?... event=0 ip=... type=http
```
Parseable via same key=value scanner; `dest=` carries the URL.

### Other log lines (should be filtered out in ngrok-dev mode)
- `INF Starting tunnel ...`
- `INF Registered tunnel connection ...`
- `DBG edge discovery: ...`
- `DBG Tunnel connection options ...`
- `DBG QUIC MTU updated ...`
- etc.

## Architecture

### Feature 1: Scrollable byte-offset tail

**New module `src/tui/log_tail.rs`:**

```rust
pub struct LogTail {
    path: PathBuf,
    offset: u64,           // Byte offset of next unread byte
    buffer: VecDeque<String>,
    partial: String,       // Trailing partial line (no \n yet) — carried between polls
    pub max_lines: usize,  // Ring capacity (default 5000)
}

impl LogTail {
    pub fn new(path: PathBuf, max_lines: usize) -> Self;
    /// Seed the buffer by reading the LAST `max_lines` from disk. Sets offset to EOF.
    /// Called on tunnel-selection change.
    pub fn seed(&mut self) -> Result<()>;
    /// Read newly-appended bytes since last poll, split into lines, push to buffer.
    /// Evicts oldest lines when over `max_lines`. Called every 1s.
    pub fn poll(&mut self) -> Result<usize>;  // Returns count of newly-appended lines
    pub fn buffer(&self) -> impl Iterator<Item = &str>;
}
```

**Handles:**
- File truncation / rotation (if new `metadata().len()` < `self.offset`, reset offset to 0 and re-seed).
- Partial lines (trailing bytes without a newline) via `self.partial`.
- Non-UTF-8 bytes (use `String::from_utf8_lossy`).

### Feature 2: App-side scroll state

Add to `App`:

```rust
// One LogTail per loaded tunnel; keyed by tunnel name.
pub log_tails: HashMap<String, LogTail>,

// UI state (for the currently-selected tunnel).
pub log_scroll: u16,      // Lines from bottom. 0 = bottom (follow-mode).
pub log_follow: bool,     // True while log_scroll == 0.
```

The existing `App::logs: Vec<String>` is retired; renderer reads directly from the selected tunnel's `LogTail`.

### Feature 3: LogMode

Add to `PersistentTunnel`:

```rust
#[serde(default, skip_serializing_if = "LogMode::is_default")]
pub log_mode: LogMode,

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LogMode {
    #[default] Default,   // cloudflared at info; raw display
    Debug,                // cloudflared at debug; raw display
    NgrokDev,             // cloudflared at debug; parsed display
}
```

**YAML generation:** in `build_tunnel_yaml`, if `log_mode == Debug || NgrokDev`, ensure `loglevel: debug` gets emitted. If a user has *both* a manual `tunnel_options["loglevel"]` and `log_mode == Debug`, the log_mode wins.

**Migration on load:** in `TunnelState::load` (or an extended `load_and_migrate`), if any tunnel has `tunnel_options["loglevel"] == "debug"`:
1. Set `log_mode: Debug` on that tunnel (if `log_mode == Default`).
2. Remove `loglevel` from `tunnel_options`.
3. Save.

Existing `load_and_migrate(default_account)` pattern is the model — extend or add a sibling.

### Feature 4: Remove `loglevel` from OPTIONS registry

Delete the `loglevel` entry from `src/cloudflared_options.rs::OPTIONS`. Keep `transport-loglevel` (still an Advanced-only knob; distinct from the request-log-level concept).

### Feature 5: Basic-tab "Log mode" field

Extend `BasicField` enum to include `LogMode`. `basic_selected` bound becomes 4 instead of 3. Layout in `render_basic`:

```
› Target:       http://localhost:3000
  Zone:         alexreade.me
  Auto-start:   yes
  Metrics port: (auto)
  Log mode:     Default    ← new, picker on Enter (like Zone)
```

New `InputMode::EditSheetLogModePicker { cursor: usize }` (mirrors `EditSheetZonePicker`).

`basic_field_description(4)` returns something like `"Cloudflared log verbosity + ytunnel display filter. 'ngrok-dev' shows a compact per-request table."`.

### Feature 6: ngrok-dev parser

**New module `src/tui/log_filter.rs`:**

```rust
pub struct ParsedRequest {
    pub timestamp: String,   // Just HH:MM:SS extracted from ISO-8601
    pub method: String,
    pub path: String,
    pub conn_index: Option<u32>,
    pub is_error: bool,      // True if from an ERR "Request failed" line
    pub error: Option<String>,
}

/// Parses one raw cloudflared log line into a ParsedRequest, or returns None
/// if the line isn't request-related.
pub fn parse_line(line: &str) -> Option<ParsedRequest>;

/// Formats a ParsedRequest as an ngrok-style compact line.
pub fn format_ngrok(req: &ParsedRequest) -> String;
// Example output: "22:42:33  GET  /live/longpoll                            connIndex=1"
```

**Detection heuristics** (based on empirical log format):

- **Request line (DBG):** `<ts> DBG <METHOD> https://<host><path> HTTP/1.1 connIndex=<n> ... path=<path>`
  - `<METHOD>` ∈ {GET, POST, PUT, DELETE, HEAD, OPTIONS, PATCH}
  - Extract `path=<value>` directly (already parsed by cloudflared).
- **Error line (ERR):** `<ts> ERR Request failed error="<msg>" ... dest=<url> ...`
  - Parse URL from `dest=`, extract path via URL split.
  - Extract `error=` value.

Regex/scanner in `parse_line`:
```rust
// Rough sketch:
let (ts, rest) = split_once_ws(line)?;
let (level, rest) = split_once_ws(rest)?;
match level {
    "DBG" => {
        let (method, rest) = split_once_ws(rest)?;
        if !is_http_method(method) { return None; }
        // Continue parsing: URL, HTTP/x.x, then key=value pairs.
        let (_url, kv_str) = split_once_ws(rest)?;
        let (_httpver, kv_str) = split_once_ws(kv_str)?;
        let path = extract_kv(kv_str, "path")?;
        let conn_index = extract_kv(kv_str, "connIndex").and_then(|s| s.parse().ok());
        Some(ParsedRequest { ... })
    }
    "ERR" if rest.starts_with("Request failed") => {
        // Similar, using dest= for URL
    }
    _ => None,
}
```

`extract_kv` scans for `<key>=<value>` where value is either quoted (`key="..."`) or a bare non-whitespace token. Skips over other key/value pairs (headers, host, ingressRule, etc.) that we don't want.

### Rendering wiring

In `src/tui/ui.rs::render_logs`, if the selected tunnel has `log_mode == NgrokDev`:
1. Iterate the LogTail buffer.
2. Pass each raw line through `log_filter::parse_line`.
3. Emit `log_filter::format_ngrok` for the ones that return `Some`; drop the rest.
4. Apply the same scroll offset logic to the filtered list.

Otherwise (Default / Debug), render the raw LogTail buffer as today.

## Data Flow

1. Tunnel selected → `App` calls `get_or_create_log_tail(name)` → `LogTail::seed()` reads last 5000 lines from disk, sets offset to EOF.
2. Every 1s tick → for each selected tunnel, `log_tail.poll()` reads newly-appended bytes, splits lines, appends to ring buffer.
3. `render_logs` reads from `log_tail.buffer()`, optionally filters via `parse_line + format_ngrok`, honors `log_scroll` offset, renders.
4. User presses PgUp → `log_scroll += page_height; log_follow = false`.
5. User presses End → `log_scroll = 0; log_follow = true`.
6. If `log_follow` and new lines arrived → `log_scroll` stays at 0 (auto-scroll to bottom).

## Error Handling

- **File missing / not-yet-written**: `LogTail::poll` returns `Ok(0)`; buffer displays empty state (message like "No logs yet").
- **File truncated / rotated**: `poll` detects `metadata.len() < offset`, resets offset to 0 and re-seeds. Buffer cleared before re-seed.
- **Non-UTF-8 bytes**: `String::from_utf8_lossy` replaces with U+FFFD. Non-fatal.
- **Parser returns None** for a line in NgrokDev mode: line dropped silently. If EVERY line is dropped (buffer produces no output), renderer shows a hint: `"(ngrok-dev: waiting for HTTP requests — set loglevel: debug should be automatic; try hitting a URL)"`.

## Testing

- **`LogTail`** unit tests: seed from small file, poll appended bytes, handle partial lines, truncation reset, buffer eviction beyond max_lines.
- **`parse_line`** unit tests: fed real log lines captured from the reference file. Success cases (GET/POST/HEAD requests parse to correct method+path), rejection cases (edge discovery, connection registered → None), error case (`ERR Request failed` → parsed with `is_error: true`).
- **`format_ngrok`** unit tests: rendered compact line matches expected fixed-width columns.
- **`log_mode` migration**: existing `tunnels.toml` with `tunnel_options["loglevel"] = "debug"` loads → after migration, `log_mode == Debug`, `loglevel` absent from map.
- **YAML gen**: `log_mode == NgrokDev` → generated YAML contains `loglevel: debug`; `log_mode == Default` → generated YAML has no `loglevel`.
- **TUI**: manual verification.

## Backward Compatibility

- Existing `PersistentTunnel` records without `log_mode` default to `LogMode::Default` via `#[serde(default)]`.
- Existing `tunnel_options["loglevel"]` entries migrated to `log_mode` on first load post-upgrade (see Migration above).
- Generated YAML: tunnels with `LogMode::Default` and no other advanced options produce byte-identical legacy output.

## Files Changed

| File | Change |
|---|---|
| `src/tui/log_tail.rs` (new) | `LogTail` byte-offset ring-buffer reader. |
| `src/tui/log_filter.rs` (new) | `parse_line` + `format_ngrok` for ngrok-dev mode. |
| `src/tui/mod.rs` | Declare the two new modules. |
| `src/state.rs` | `LogMode` enum + `PersistentTunnel::log_mode` field + YAML generation hook. |
| `src/tui/app.rs` | Replace `App::logs` with `HashMap<String, LogTail>`; add `log_scroll`/`log_follow`; new keybindings; new `InputMode::EditSheetLogModePicker`; extend `BasicField`. |
| `src/tui/ui.rs` | `render_logs` uses LogTail + optional NgrokDev filter; new modal overlay; help-bar updates. |
| `src/tui/edit_sheet.rs` | `render_basic` gains 5th row (Log mode); `basic_field_description` extended; `apply` handles log_mode; `EditSheetState` carries pending log_mode. |
| `src/cloudflared_options.rs` | Delete `loglevel` entry from `OPTIONS`. |
| `CHANGELOG.md` | Document scrollable logs + ngrok-dev + Log mode field. |

## Known Limitations (documented for CHANGELOG)

- ngrok-dev shows method + path + origin only. cloudflared doesn't log response status or per-request duration at any level, so those fields can't appear.
- Log files aren't rotated; long-lived tunnels accumulate. LogTail buffer is bounded (5000 lines default) so ytunnel's memory is fine, but the on-disk file grows forever. Rotation is a future concern.
