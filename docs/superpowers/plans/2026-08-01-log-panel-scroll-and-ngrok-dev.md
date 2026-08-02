# Log Panel: Scrollable + ngrok-dev Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Make the log panel scrollable (byte-offset tail + ring buffer + keybindings) and add a new `Log mode` picker on the Basic tab (`Default | Debug | ngrok-dev`) where ngrok-dev parses cloudflared debug lines into compact per-request entries.

**Architecture:** New modules `src/tui/log_tail.rs` (bounded ring buffer, appends new bytes only) and `src/tui/log_filter.rs` (regex-lite parser for cloudflared debug lines → compact request rows). New `LogMode` enum on `PersistentTunnel` (with `#[serde(default)]` for backward compat + one-shot migration from `tunnel_options["loglevel"]`). No new dependencies.

**Design spec:** `docs/superpowers/specs/2026-08-01-log-panel-scroll-and-ngrok-dev-design.md`

**Working directory:** `/Users/ar/ytunnel`, branch `advanced-tunnel-options`.

**Conventions:** `//` comments only. No `Co-Authored-By: Claude`. After code changes: `cargo build --release && cp target/release/ytunnel /opt/homebrew/bin/ytunnel`.

---

## File Structure

**Files created:**
- `src/tui/log_tail.rs` — bounded ring buffer with byte-offset tail.
- `src/tui/log_filter.rs` — `parse_line` + `format_ngrok` for compact request formatting.

**Files modified:**
- `src/tui/mod.rs` — register two new modules.
- `src/state.rs` — `LogMode` enum + `PersistentTunnel::log_mode` field + migration in `TunnelState::load_and_migrate` + YAML gen hook in `build_tunnel_yaml`.
- `src/tui/app.rs` — retire `App::logs`; add `log_tails: HashMap<String, LogTail>`, `log_scroll`, `log_follow`; PgUp/PgDn/Home/End/Ctrl+U/Ctrl+D keybindings in Normal mode; new `InputMode::EditSheetLogModePicker`; extend `BasicField`.
- `src/tui/ui.rs` — `render_logs` reads from LogTail + optional NgrokDev filter + honors scroll offset; help-bar strings.
- `src/tui/edit_sheet.rs` — extend `EditSheetState` with `pending_log_mode`; extend `render_basic` (5th row); update `basic_field_description`; `apply` sets `tunnel.log_mode`.
- `src/cloudflared_options.rs` — remove `loglevel` entry from `OPTIONS`.
- `CHANGELOG.md` — document new features.

---

## Task 1: LogTail byte-offset ring buffer

**Files:**
- Create: `src/tui/log_tail.rs`
- Modify: `src/tui/mod.rs` (add `pub mod log_tail;`)

- [ ] **Step 1: Write the failing tests**

Create `src/tui/log_tail.rs` with:

```rust
// Byte-offset ring-buffer tail of a log file.
//
// Reads only newly-appended bytes on each poll, keeping a bounded VecDeque
// of the last N complete lines. Handles file truncation/rotation (restarts
// from offset 0). Handles partial lines (bytes without a trailing newline)
// by stashing them and prepending on the next poll.

use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use anyhow::{Context, Result};

pub struct LogTail {
    path: PathBuf,
    offset: u64,
    buffer: VecDeque<String>,
    partial: String,
    pub max_lines: usize,
}

impl LogTail {
    pub fn new(path: PathBuf, max_lines: usize) -> Self {
        Self {
            path,
            offset: 0,
            buffer: VecDeque::with_capacity(max_lines),
            partial: String::new(),
            max_lines,
        }
    }

    // Seed the buffer with the LAST `max_lines` lines currently on disk.
    // Sets `offset` to end-of-file so `poll` only picks up new bytes after.
    pub fn seed(&mut self) -> Result<()> {
        self.buffer.clear();
        self.partial.clear();

        if !self.path.exists() {
            self.offset = 0;
            return Ok(());
        }

        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("Failed to read {}", self.path.display()))?;
        let all_lines: Vec<&str> = content.lines().collect();
        let start = all_lines.len().saturating_sub(self.max_lines);
        for line in &all_lines[start..] {
            self.buffer.push_back((*line).to_string());
        }
        self.offset = fs::metadata(&self.path)?.len();
        Ok(())
    }

    // Read newly-appended bytes since last poll. Returns count of NEW complete
    // lines appended to the buffer.
    pub fn poll(&mut self) -> Result<usize> {
        if !self.path.exists() {
            return Ok(0);
        }
        let len = fs::metadata(&self.path)?.len();

        // File shrank (truncation / rotation). Re-seed from scratch.
        if len < self.offset {
            self.seed()?;
            return Ok(0);
        }

        if len == self.offset {
            return Ok(0);
        }

        let mut file = fs::File::open(&self.path)
            .with_context(|| format!("Failed to open {}", self.path.display()))?;
        file.seek(SeekFrom::Start(self.offset))?;

        let mut bytes = Vec::with_capacity((len - self.offset) as usize);
        file.read_to_end(&mut bytes)?;
        self.offset = len;

        let text = String::from_utf8_lossy(&bytes);
        let mut new_count = 0;

        // Prepend any partial line from the previous poll.
        let combined = format!("{}{}", self.partial, text);
        self.partial.clear();

        let mut iter = combined.split_inclusive('\n').peekable();
        while let Some(chunk) = iter.next() {
            if let Some(stripped) = chunk.strip_suffix('\n') {
                let line = stripped.strip_suffix('\r').unwrap_or(stripped);
                self.push_line(line.to_string());
                new_count += 1;
            } else {
                // Final chunk with no trailing newline — save for next poll.
                self.partial = chunk.to_string();
            }
        }

        Ok(new_count)
    }

    fn push_line(&mut self, line: String) {
        if self.buffer.len() == self.max_lines {
            self.buffer.pop_front();
        }
        self.buffer.push_back(line);
    }

    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.buffer.iter().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.log");
        (dir, path)
    }

    #[test]
    fn seed_reads_last_max_lines_from_file() {
        let (_dir, path) = tmp_file();
        let mut f = fs::File::create(&path).unwrap();
        for i in 0..10 {
            writeln!(f, "line {}", i).unwrap();
        }
        drop(f);

        let mut tail = LogTail::new(path.clone(), 3);
        tail.seed().unwrap();
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines, vec!["line 7", "line 8", "line 9"]);
        assert_eq!(tail.offset, fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn poll_appends_new_lines_only() {
        let (_dir, path) = tmp_file();
        {
            let mut f = fs::File::create(&path).unwrap();
            writeln!(f, "one").unwrap();
        }
        let mut tail = LogTail::new(path.clone(), 100);
        tail.seed().unwrap();
        assert_eq!(tail.len(), 1);

        {
            let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, "two").unwrap();
            writeln!(f, "three").unwrap();
        }
        let n = tail.poll().unwrap();
        assert_eq!(n, 2);
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines, vec!["one", "two", "three"]);
    }

    #[test]
    fn poll_handles_partial_line_across_two_polls() {
        let (_dir, path) = tmp_file();
        fs::write(&path, "").unwrap();
        let mut tail = LogTail::new(path.clone(), 100);
        tail.seed().unwrap();

        // Append a partial line (no trailing newline).
        {
            let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
            write!(f, "partial-").unwrap();
        }
        let n = tail.poll().unwrap();
        assert_eq!(n, 0);
        assert!(tail.is_empty());

        // Complete the line in a second write.
        {
            let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, "line").unwrap();
        }
        let n = tail.poll().unwrap();
        assert_eq!(n, 1);
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines, vec!["partial-line"]);
    }

    #[test]
    fn poll_handles_truncation() {
        let (_dir, path) = tmp_file();
        {
            let mut f = fs::File::create(&path).unwrap();
            for i in 0..5 {
                writeln!(f, "old {}", i).unwrap();
            }
        }
        let mut tail = LogTail::new(path.clone(), 100);
        tail.seed().unwrap();
        assert_eq!(tail.len(), 5);

        // Truncate the file (simulates rotation).
        fs::write(&path, "fresh\n").unwrap();
        let n = tail.poll().unwrap();
        assert_eq!(n, 0);  // Re-seed doesn't count as newly-appended.
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines, vec!["fresh"]);
    }

    #[test]
    fn ring_buffer_evicts_oldest_when_over_capacity() {
        let (_dir, path) = tmp_file();
        {
            let mut f = fs::File::create(&path).unwrap();
            for i in 0..5 {
                writeln!(f, "line {}", i).unwrap();
            }
        }
        let mut tail = LogTail::new(path.clone(), 3);
        tail.seed().unwrap();
        assert_eq!(tail.len(), 3);
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines, vec!["line 2", "line 3", "line 4"]);

        {
            let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, "line 5").unwrap();
            writeln!(f, "line 6").unwrap();
        }
        tail.poll().unwrap();
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines, vec!["line 4", "line 5", "line 6"]);
    }
}
```

Add `tempfile = "3"` to `[dev-dependencies]` in `Cargo.toml` — needed by the tests. This is a dev-dep only, no impact on the release binary. If `[dev-dependencies]` section doesn't exist, create it under `[dependencies]`.

- [ ] **Step 2: Register the module**

In `src/tui/mod.rs`, add `pub mod log_tail;` after the existing declarations.

- [ ] **Step 3: Run tests**

`cargo test --bin ytunnel log_tail` → expect 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/tui/log_tail.rs src/tui/mod.rs Cargo.toml Cargo.lock && git commit -m "tui: add LogTail byte-offset ring buffer with tests"
```

---

## Task 2: NgrokDev log parser

**Files:**
- Create: `src/tui/log_filter.rs`
- Modify: `src/tui/mod.rs` (add `pub mod log_filter;`)

- [ ] **Step 1: Create module with tests-first**

Create `src/tui/log_filter.rs`:

```rust
// Parses cloudflared log lines into compact per-request rows for ngrok-dev mode.
//
// At DBG level cloudflared emits:
//   2026-08-01T22:42:33Z DBG GET https://host/path?... HTTP/1.1 connIndex=1 ... path=/live/longpoll
// At any level, failed requests emit:
//   2026-08-01T22:42:33Z ERR Request failed error="..." connIndex=0 dest=https://... type=http
//
// parse_line extracts the interesting bits; format_ngrok renders a fixed-width row.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRequest {
    // HH:MM:SS extracted from ISO-8601 timestamp.
    pub time: String,
    pub method: String,
    pub path: String,
    pub conn_index: Option<u32>,
    pub is_error: bool,
    pub error: Option<String>,
}

pub fn parse_line(line: &str) -> Option<ParsedRequest> {
    let (ts, rest) = line.split_once(' ')?;
    let time = extract_hhmmss(ts)?;
    let (level, rest) = rest.split_once(' ')?;
    match level {
        "DBG" => parse_dbg_request(&time, rest),
        "ERR" if rest.starts_with("Request failed") => parse_err_request(&time, rest),
        _ => None,
    }
}

fn extract_hhmmss(ts: &str) -> Option<String> {
    // ISO-8601 "2026-08-01T22:42:33Z" — extract 22:42:33.
    let t_pos = ts.find('T')?;
    let after_t = &ts[t_pos + 1..];
    let end = after_t.find(|c: char| !c.is_ascii_digit() && c != ':')?;
    let hhmmss = &after_t[..end];
    if hhmmss.len() != 8 { return None; }
    Some(hhmmss.to_string())
}

fn parse_dbg_request(time: &str, rest: &str) -> Option<ParsedRequest> {
    let (method, rest) = rest.split_once(' ')?;
    if !is_http_method(method) { return None; }
    // Skip URL and HTTP version.
    let (_url, rest) = rest.split_once(' ')?;
    let (_httpver, kv_str) = rest.split_once(' ')?;
    let path = extract_kv(kv_str, "path")?;
    let conn_index = extract_kv(kv_str, "connIndex").and_then(|s| s.parse().ok());
    Some(ParsedRequest {
        time: time.to_string(),
        method: method.to_string(),
        path,
        conn_index,
        is_error: false,
        error: None,
    })
}

fn parse_err_request(time: &str, rest: &str) -> Option<ParsedRequest> {
    // "Request failed error=\"...\" ... dest=https://host/path ..."
    let error = extract_kv(rest, "error");
    let dest = extract_kv(rest, "dest")?;
    let conn_index = extract_kv(rest, "connIndex").and_then(|s| s.parse().ok());
    // Extract path from URL: skip scheme://host, take the rest.
    let path = url_to_path(&dest)?;
    Some(ParsedRequest {
        time: time.to_string(),
        method: "???".to_string(),
        path,
        conn_index,
        is_error: true,
        error,
    })
}

fn is_http_method(s: &str) -> bool {
    matches!(s, "GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "OPTIONS" | "PATCH" | "CONNECT" | "TRACE")
}

// Extract the value of `key=<value>` where value is either quoted ("...") or bare
// non-whitespace. Handles values containing '=' or JSON braces by grabbing
// balanced quoted strings.
fn extract_kv(s: &str, key: &str) -> Option<String> {
    // Search for " key=" or start-of-string " key=" pattern.
    let needle = format!(" {}=", key);
    let start = if s.starts_with(&format!("{}=", key)) {
        key.len() + 1
    } else {
        s.find(&needle)? + needle.len()
    };
    let value_str = &s[start..];
    if value_str.starts_with('"') {
        // Quoted: scan for closing quote, honoring backslash escapes.
        let mut chars = value_str.char_indices();
        chars.next(); // skip opening "
        let mut escaped = false;
        for (i, c) in chars {
            if escaped { escaped = false; continue; }
            match c {
                '\\' => escaped = true,
                '"' => return Some(value_str[1..i].to_string()),
                _ => {}
            }
        }
        None
    } else {
        // Bare: read up to next whitespace.
        let end = value_str.find(char::is_whitespace).unwrap_or(value_str.len());
        Some(value_str[..end].to_string())
    }
}

fn url_to_path(url: &str) -> Option<String> {
    // http[s]://host/path?query...
    let scheme_end = url.find("://")?;
    let rest = &url[scheme_end + 3..];
    let path_start = rest.find('/').unwrap_or(rest.len());
    Some(rest[path_start..].to_string())
}

pub fn format_ngrok(req: &ParsedRequest) -> String {
    let method = format!("{:<6}", req.method);
    // Truncate very long paths so the row stays scannable.
    let path = truncate(&req.path, 60);
    let mut out = format!("{}  {}  {}", req.time, method, path);
    if req.is_error {
        if let Some(err) = &req.error {
            out.push_str(&format!("  ERROR: {}", truncate(err, 40)));
        } else {
            out.push_str("  ERROR");
        }
    }
    if let Some(idx) = req.conn_index {
        out.push_str(&format!("  conn={}", idx));
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_hhmmss_from_iso() {
        assert_eq!(extract_hhmmss("2026-08-01T22:42:33Z").unwrap(), "22:42:33");
        assert_eq!(extract_hhmmss("2026-08-01T00:00:00Z").unwrap(), "00:00:00");
        assert_eq!(extract_hhmmss("garbage"), None);
    }

    #[test]
    fn extract_kv_handles_quoted_and_bare() {
        let s = r#"foo=bar baz="quoted value" num=42"#;
        assert_eq!(extract_kv(s, "foo"), Some("bar".to_string()));
        assert_eq!(extract_kv(s, "baz"), Some("quoted value".to_string()));
        assert_eq!(extract_kv(s, "num"), Some("42".to_string()));
        assert_eq!(extract_kv(s, "missing"), None);
    }

    #[test]
    fn extract_kv_ignores_substring_matches() {
        // "xfoo=bar" should not match key "foo".
        let s = "xfoo=nope foo=yes";
        assert_eq!(extract_kv(s, "foo"), Some("yes".to_string()));
    }

    #[test]
    fn parses_dbg_get_request() {
        let line = r#"2026-08-01T22:42:33Z DBG GET https://google-spike.alexreade.me/live/longpoll HTTP/1.1 connIndex=1 content-length=0 event=1 path=/live/longpoll"#;
        let req = parse_line(line).unwrap();
        assert_eq!(req.time, "22:42:33");
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/live/longpoll");
        assert_eq!(req.conn_index, Some(1));
        assert!(!req.is_error);
    }

    #[test]
    fn parses_dbg_post_request_with_headers_blob() {
        // Real-world line with a huge headers={...} blob before path=.
        let line = r#"2026-08-01T22:42:33Z DBG POST https://host/api HTTP/1.1 connIndex=2 content-length=1299 event=1 headers={"a":"b"} host=host ingressRule=0 originService=http://local path=/api"#;
        let req = parse_line(line).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/api");
        assert_eq!(req.conn_index, Some(2));
    }

    #[test]
    fn parses_err_request_failed() {
        let line = r#"2026-08-01T19:00:10Z ERR Request failed error="Incoming request ended abruptly: context canceled" connIndex=0 dest=https://google-spike.alexreade.me/live/longpoll event=0 ip=2606:4700:a0::5 type=http"#;
        let req = parse_line(line).unwrap();
        assert_eq!(req.time, "19:00:10");
        assert!(req.is_error);
        assert_eq!(req.path, "/live/longpoll");
        assert_eq!(req.conn_index, Some(0));
        assert_eq!(req.error.as_deref(), Some("Incoming request ended abruptly: context canceled"));
    }

    #[test]
    fn rejects_non_request_lines() {
        for noise in [
            "2026-08-01T22:42:31Z INF Starting tunnel tunnelID=abc",
            "2026-08-01T22:42:31Z DBG edge discovery: looking up edge SRV record domain=x event=0",
            "2026-08-01T22:42:31Z DBG Tunnel connection options connIndex=0 event=0",
            "2026-08-01T22:42:31Z WRN some warning",
            "2026-08-01T22:42:31Z ERR failed to accept incoming stream requests connIndex=0",
            "garbage that doesn't match anything",
        ] {
            assert!(parse_line(noise).is_none(), "should reject: {noise}");
        }
    }

    #[test]
    fn format_ngrok_renders_compact_row() {
        let req = ParsedRequest {
            time: "22:42:33".into(),
            method: "GET".into(),
            path: "/live/longpoll".into(),
            conn_index: Some(1),
            is_error: false,
            error: None,
        };
        let out = format_ngrok(&req);
        assert!(out.starts_with("22:42:33  GET   "));
        assert!(out.contains("/live/longpoll"));
        assert!(out.contains("conn=1"));
    }

    #[test]
    fn format_ngrok_marks_errors() {
        let req = ParsedRequest {
            time: "19:00:10".into(),
            method: "???".into(),
            path: "/live/longpoll".into(),
            conn_index: Some(0),
            is_error: true,
            error: Some("context canceled".into()),
        };
        let out = format_ngrok(&req);
        assert!(out.contains("ERROR: context canceled"));
    }

    #[test]
    fn format_ngrok_truncates_long_paths() {
        let req = ParsedRequest {
            time: "22:42:33".into(),
            method: "GET".into(),
            path: "/very/long/path/".to_string() + &"x".repeat(200),
            conn_index: None,
            is_error: false,
            error: None,
        };
        let out = format_ngrok(&req);
        assert!(out.contains('…'));
    }
}
```

- [ ] **Step 2: Register the module**

In `src/tui/mod.rs`, add `pub mod log_filter;`.

- [ ] **Step 3: Run tests**

`cargo test --bin ytunnel log_filter` → expect 9 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/tui/log_filter.rs src/tui/mod.rs && git commit -m "tui: add log_filter parser for ngrok-dev mode"
```

---

## Task 3: LogMode enum on PersistentTunnel + migration + YAML gen

**Files:**
- Modify: `src/state.rs`

- [ ] **Step 1: Write failing tests**

Add to `src/state.rs::mod tests`:

```rust
    #[test]
    fn log_mode_defaults_to_default() {
        let tunnel: PersistentTunnel = toml::from_str(r#"
            name = "demo"
            account_name = "acct"
            target = "http://localhost:3000"
            zone_id = "z"
            zone_name = "example.com"
            hostname = "demo.example.com"
            tunnel_id = "uuid"
            enabled = true
        "#).unwrap();
        assert_eq!(tunnel.log_mode, LogMode::Default);
    }

    #[test]
    fn log_mode_debug_emits_loglevel_in_yaml() {
        let mut tunnel = sample_tunnel();
        tunnel.log_mode = LogMode::Debug;
        let yaml = generate_tunnel_config(&tunnel).unwrap();
        assert!(yaml.contains("\nloglevel: \"debug\"\n"), "expected loglevel line:\n{yaml}");
    }

    #[test]
    fn log_mode_ngrok_dev_emits_loglevel_in_yaml() {
        let mut tunnel = sample_tunnel();
        tunnel.log_mode = LogMode::NgrokDev;
        let yaml = generate_tunnel_config(&tunnel).unwrap();
        assert!(yaml.contains("\nloglevel: \"debug\"\n"));
    }

    #[test]
    fn log_mode_default_does_not_emit_loglevel() {
        let tunnel = sample_tunnel();
        let yaml = generate_tunnel_config(&tunnel).unwrap();
        assert!(!yaml.contains("loglevel"));
    }

    #[test]
    fn migration_moves_legacy_loglevel_to_log_mode() {
        let toml = r#"
            [[tunnels]]
            name = "demo"
            account_name = "acct"
            target = "http://localhost:3000"
            zone_id = "z"
            zone_name = "example.com"
            hostname = "demo.example.com"
            tunnel_id = "uuid"
            enabled = true

            [tunnels.tunnel_options]
            loglevel = "debug"
        "#;
        let mut state: TunnelState = toml::from_str(toml).unwrap();
        migrate_loglevel_to_log_mode(&mut state);
        assert_eq!(state.tunnels[0].log_mode, LogMode::Debug);
        assert!(!state.tunnels[0].tunnel_options.contains_key("loglevel"));
    }
```

Also update `sample_tunnel()` to include `log_mode: LogMode::Default` in the struct literal.

- [ ] **Step 2: Run tests, verify failure**

`cargo test --bin ytunnel state::tests` — expect the log_mode tests to fail (field doesn't exist).

- [ ] **Step 3: Add LogMode enum and field**

At the top of `src/state.rs` after the `TunnelOptionValue` enum, add:

```rust
// User-selected log mode: cloudflared verbosity + ytunnel display filter.
// Default: cloudflared at info, raw display. Debug: cloudflared at debug, raw
// display (verbose, includes headers). NgrokDev: cloudflared at debug, parsed
// display (compact per-request rows via crate::tui::log_filter).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LogMode {
    #[default]
    Default,
    Debug,
    NgrokDev,
}

impl LogMode {
    pub fn is_default(&self) -> bool {
        matches!(self, LogMode::Default)
    }
    // The cloudflared loglevel value this mode implies. None => omit from YAML.
    pub fn to_cloudflared_loglevel(&self) -> Option<&'static str> {
        match self {
            LogMode::Default => None,
            LogMode::Debug | LogMode::NgrokDev => Some("debug"),
        }
    }
}
```

Add to `PersistentTunnel` (after `origin_request`):

```rust
    // Log verbosity + display filter. Default = cloudflared info, raw display.
    // See LogMode for details.
    #[serde(default, skip_serializing_if = "LogMode::is_default")]
    pub log_mode: LogMode,
```

Add migration function above `TunnelState::load_and_migrate`:

```rust
// If a tunnel has tunnel_options["loglevel"] set (from before log_mode
// existed), promote it to the first-class log_mode field and remove the
// map entry. Preserves the user's intent across the schema change.
pub fn migrate_loglevel_to_log_mode(state: &mut TunnelState) {
    for tunnel in &mut state.tunnels {
        if let Some(TunnelOptionValue::String(s)) = tunnel.tunnel_options.get("loglevel") {
            if s == "debug" && tunnel.log_mode.is_default() {
                tunnel.log_mode = LogMode::Debug;
            }
            tunnel.tunnel_options.remove("loglevel");
        }
    }
}
```

Extend `load_and_migrate` (or add a new call site) to invoke `migrate_loglevel_to_log_mode(&mut state)` and re-save if any migration happened. Follow the pattern of the existing account_name migration.

Update `build_tunnel_yaml` to accept `log_mode` (or read it via `PersistentTunnel`). Simplest: extend `generate_tunnel_config`'s wrapper to fold log_mode into `tunnel_options` at build time:

```rust
pub fn generate_tunnel_config(tunnel: &PersistentTunnel) -> Result<String> {
    let credentials_path = tunnel.credentials_path()?;
    // Merge log_mode's implied loglevel into tunnel_options for YAML emission.
    let mut effective_options = tunnel.tunnel_options.clone();
    if let Some(level) = tunnel.log_mode.to_cloudflared_loglevel() {
        effective_options.insert(
            "loglevel".to_string(),
            TunnelOptionValue::String(level.to_string()),
        );
    }
    build_tunnel_yaml(
        &tunnel.tunnel_id,
        &credentials_path,
        &tunnel.hostname,
        &tunnel.target,
        &effective_options,
        &tunnel.origin_request,
    )
}
```

- [ ] **Step 4: Fix any construction sites**

`cargo check` — struct literals for `PersistentTunnel` in `src/main.rs`, `src/tui/app.rs`, `src/tui/edit_sheet.rs` tests need `log_mode: LogMode::Default,`. Add.

- [ ] **Step 5: Run all tests**

`cargo test --bin ytunnel` — expect all pass, including new log_mode tests.

- [ ] **Step 6: Commit**

```bash
git add src/ && git commit -m "state: add LogMode enum + PersistentTunnel::log_mode + migration"
```

---

## Task 4: Remove `loglevel` from OPTIONS registry

**Files:**
- Modify: `src/cloudflared_options.rs`

- [ ] **Step 1: Remove entry**

Delete the entire `OptionSpec { yaml_key: "loglevel", ... }` entry (should be the first entry under `// --- Tunnel scope ---`).

Keep `transport-loglevel` — that's a distinct knob for protocol-level debugging.

- [ ] **Step 2: Verify tests still pass**

`cargo test --bin ytunnel cloudflared_options` — expect 7 pass (all_yaml_keys_unique should be unaffected).

- [ ] **Step 3: Commit**

```bash
git add src/cloudflared_options.rs && git commit -m "cloudflared_options: remove loglevel from Advanced (now first-class on Basic)"
```

---

## Task 5: Basic-tab "Log mode" field + picker

**Files:**
- Modify: `src/tui/edit_sheet.rs`
- Modify: `src/tui/app.rs`
- Modify: `src/tui/ui.rs`

- [ ] **Step 1: Extend EditSheetState**

In `src/tui/edit_sheet.rs`:

Extend `BasicField`:
```rust
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasicField {
    Target = 0,
    Zone = 1,
    AutoStart = 2,
    MetricsPort = 3,
    LogMode = 4,
}
```

Extend `EditSheetState`:
```rust
    // Log mode value staged for save. Defaults to the tunnel's current value.
    pub log_mode: crate::state::LogMode,
```

In `from_tunnel`: `log_mode: tunnel.log_mode`.

In `apply`: `tunnel.log_mode = self.log_mode`.

In `select_basic_next`: bound is now 4 not 3.

In `basic_field_description`: add case for `4`:
```rust
        4 => "Cloudflared log verbosity + ytunnel display filter. 'ngrok-dev' shows a compact per-request table (requires debug internally).",
```

In `render_basic`: add a 5th line with the log mode:
```rust
    lines.push(Line::from(vec![
        marker_span(4),
        Span::styled("Log mode:     ", label_style),
        Span::styled(log_mode_label(sheet.log_mode), value_style),
    ]));
```

Add helper `fn log_mode_label(m: LogMode) -> &'static str { match m { Default => "Default", Debug => "Debug", NgrokDev => "ngrok-dev" } }`.

- [ ] **Step 2: New InputMode + key handling**

In `src/tui/app.rs`, add to `InputMode`:
```rust
    EditSheetLogModePicker { cursor: usize },
```

In the `InputMode::EditSheet` Enter handler's Basic-tab dispatch, add case for `basic_selected == 4`:
```rust
        4 => {
            app.input_mode = InputMode::EditSheetLogModePicker { cursor: log_mode_to_index(app.edit_sheet.as_ref().map(|s| s.log_mode).unwrap_or_default()) };
        }
```

Add helpers `log_mode_to_index` and `log_mode_from_index`:
```rust
fn log_mode_to_index(m: crate::state::LogMode) -> usize {
    match m { LogMode::Default => 0, LogMode::Debug => 1, LogMode::NgrokDev => 2 }
}
fn log_mode_from_index(i: usize) -> crate::state::LogMode {
    match i { 1 => LogMode::Debug, 2 => LogMode::NgrokDev, _ => LogMode::Default }
}
```

Add key handler for `EditSheetLogModePicker` (mirror `EditSheetZonePicker` shape). On Enter:
```rust
        KeyCode::Enter => {
            if let InputMode::EditSheetLogModePicker { cursor } = &app.input_mode {
                let mode = log_mode_from_index(*cursor);
                if let Some(s) = app.edit_sheet.as_mut() {
                    if s.log_mode != mode {
                        s.log_mode = mode;
                        s.dirty = true;
                    }
                }
            }
            app.input_mode = InputMode::EditSheet;
        }
```

- [ ] **Step 3: Render the picker**

In `src/tui/ui.rs::render_sheet_modal_overlays`, add case for `EditSheetLogModePicker` (mirror the Zone picker), with items:
```rust
let choices = ["Default", "Debug", "ngrok-dev"];
```

Also add `EditSheetLogModePicker { .. }` to the outer render dispatch pattern (around line 105) and help-bar string.

- [ ] **Step 4: Build + tests**

`cargo check && cargo test --bin ytunnel edit_sheet` — expect clean + all edit_sheet tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/tui/ && git commit -m "tui: add Log mode field to Basic tab with picker"
```

---

## Task 6: LogTail wiring + scrollable logs

**Files:**
- Modify: `src/tui/app.rs`
- Modify: `src/tui/ui.rs`

- [ ] **Step 1: Retire App::logs, add LogTail machinery**

In `src/tui/app.rs`, remove `pub logs: Vec<String>` (search all usages first — should mainly be in `refresh_logs` and `render_logs`).

Add:
```rust
    pub log_tails: std::collections::HashMap<String, crate::tui::log_tail::LogTail>,
    pub log_scroll: u16,
    pub log_follow: bool,
```

Initialize in constructors: `log_tails: HashMap::new(), log_scroll: 0, log_follow: true,`.

Replace `refresh_logs` body with:

```rust
    pub fn refresh_logs(&mut self) {
        let Some(entry) = self.tunnels.get(self.selected) else { return };
        let name = entry.tunnel.name.clone();
        let path = match entry.tunnel.log_path() {
            Ok(p) => p,
            Err(_) => return,
        };
        let tail = self.log_tails.entry(name).or_insert_with(|| {
            crate::tui::log_tail::LogTail::new(path.clone(), 5000)
        });
        // Newly-created LogTails need seeding.
        if tail.is_empty() {
            let _ = tail.seed();
        } else {
            let _ = tail.poll();
        }
    }
```

- [ ] **Step 2: Add scroll keybindings**

In the `InputMode::Normal` match block, add:
```rust
        KeyCode::PageUp => app.scroll_logs_up(10),
        KeyCode::PageDown => app.scroll_logs_down(10),
        KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_logs_top(),
        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_logs_bottom(),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_logs_up(5),
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_logs_down(5),
```

Add helper methods on `App`:
```rust
    pub fn scroll_logs_up(&mut self, n: u16) {
        self.log_scroll = self.log_scroll.saturating_add(n);
        self.log_follow = false;
    }
    pub fn scroll_logs_down(&mut self, n: u16) {
        self.log_scroll = self.log_scroll.saturating_sub(n);
        if self.log_scroll == 0 { self.log_follow = true; }
    }
    pub fn scroll_logs_bottom(&mut self) {
        self.log_scroll = 0;
        self.log_follow = true;
    }
    pub fn scroll_logs_top(&mut self) {
        // Set to a large sentinel; render clamps to actual buffer size.
        self.log_scroll = u16::MAX;
        self.log_follow = false;
    }
```

On tunnel-selection change (in whatever function handles `Down`/`Up` on the tunnel list), reset scroll:
```rust
        self.log_scroll = 0;
        self.log_follow = true;
```

- [ ] **Step 3: Update render_logs**

In `src/tui/ui.rs::render_logs`, replace the body. Read from the selected tunnel's LogTail:
```rust
fn render_logs(f: &mut Frame, app: &App, area: Rect) {
    let name = app.tunnels.get(app.selected).map(|e| e.tunnel.name.clone());

    let empty_lines: Vec<String> = vec!["No tunnel selected".to_string()];
    let all_lines: Vec<String> = match name.as_deref().and_then(|n| app.log_tails.get(n)) {
        Some(tail) if !tail.is_empty() => tail.lines().map(String::from).collect(),
        Some(_) => vec!["No logs yet".to_string()],
        None => empty_lines,
    };

    // Determine visible slice based on scroll offset.
    let visible_height = area.height.saturating_sub(2) as usize; // account for borders
    let total = all_lines.len();
    let scroll = (app.log_scroll as usize).min(total.saturating_sub(1));
    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(visible_height);
    let visible = &all_lines[start..end];

    let log_lines: Vec<Line> = visible.iter().map(|l| Line::from(l.as_str())).collect();

    let title = if app.log_follow {
        format!(" Logs ({}) ", total)
    } else {
        format!(" Logs ({} — scrolled, End to follow) ", total)
    };
    let logs = Paragraph::new(log_lines)
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(logs, area);
}
```

Remove the old `render_logs` code that referenced `app.logs`.

- [ ] **Step 4: Fix stragglers**

`cargo check` — anywhere else `app.logs` is used (search: `grep -n "app.logs\|self.logs" src/`), replace by reading from LogTail equivalent OR delete (many uses were `self.logs = vec![...]` messaging — those can just call `self.log_tails.clear()` or be dropped).

- [ ] **Step 5: Verify manual TUI**

`cargo build --release && cp target/release/ytunnel /opt/homebrew/bin/ytunnel`

Manual checks:
- Select a running tunnel — logs appear.
- Wait a few seconds — new log lines appear live.
- PgUp — scroll back; title says "scrolled".
- PgDn — scroll forward.
- End (with Ctrl on macOS if needed) — snap to bottom; follow re-enabled; title clean.
- Switch between tunnels — logs update to selected tunnel's file.

- [ ] **Step 6: Commit**

```bash
git add src/tui/ && git commit -m "tui: scrollable logs via LogTail (byte-offset tail + PgUp/PgDn keybindings)"
```

---

## Task 7: Wire ngrok-dev filter into log render

**Files:**
- Modify: `src/tui/ui.rs`

- [ ] **Step 1: Filter in render_logs when log_mode is NgrokDev**

At the top of `render_logs`, check the selected tunnel's log_mode:

```rust
    let log_mode = app.tunnels.get(app.selected)
        .map(|e| e.tunnel.log_mode)
        .unwrap_or_default();
```

After collecting `all_lines`, if `log_mode == crate::state::LogMode::NgrokDev`:

```rust
    let all_lines: Vec<String> = if log_mode == crate::state::LogMode::NgrokDev {
        all_lines.iter()
            .filter_map(|l| crate::tui::log_filter::parse_line(l))
            .map(|req| crate::tui::log_filter::format_ngrok(&req))
            .collect()
    } else {
        all_lines
    };
```

If NgrokDev is active and `all_lines` is empty *after* filtering (but the raw tail wasn't empty), show a helpful hint:

```rust
    let all_lines = if log_mode == crate::state::LogMode::NgrokDev && all_lines.is_empty() {
        vec!["(ngrok-dev: no HTTP requests yet — hit a URL on your tunnel to see them appear)".to_string()]
    } else {
        all_lines
    };
```

Update the title to reflect mode:
```rust
    let mode_tag = match log_mode {
        LogMode::Default => "",
        LogMode::Debug => " [debug]",
        LogMode::NgrokDev => " [ngrok-dev]",
    };
    let title = format!(" Logs{} ({}) ", mode_tag, total);
```

- [ ] **Step 2: Manual TUI verification**

Build + install. Set a tunnel's Log mode to `ngrok-dev` via the Basic tab, save. Hit the tunnel URL from a browser. See a clean compact request line show up per hit.

- [ ] **Step 3: Commit**

```bash
git add src/tui/ui.rs && git commit -m "tui: wire ngrok-dev filter into log render"
```

---

## Task 8: CHANGELOG

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add entries**

Under `### Unreleased`, append:

```markdown
- **Scrollable log panel** — PgUp/PgDn to page, Ctrl+U/Ctrl+D for half-page, Ctrl+Home/Ctrl+End for top/bottom. Buffer holds the last 5000 lines. New lines auto-scroll to the bottom; scrolling up pauses auto-scroll until you press Ctrl+End.
- **Live log tail** — the log panel now polls the file every second and reads only newly-appended bytes, so cloudflared events appear live instead of only on tunnel actions.
- **Log mode picker on Basic tab** — pick `Default` (cloudflared info level), `Debug` (verbose, includes headers), or `ngrok-dev` (compact per-request table like ngrok). `loglevel` is no longer in the Advanced options list — set it via Log mode instead. Existing tunnels with `loglevel: debug` in `tunnels.toml` are auto-migrated to `Log mode = Debug` on next load.
- **ngrok-dev mode** — parses cloudflared's debug log stream into one line per HTTP request: `HH:MM:SS  METHOD  path  conn=N`. Skips edge-discovery, connection registration, and other non-request noise. Note: cloudflared doesn't log response status or duration at any level, so those fields are absent.
```

- [ ] **Step 2: Commit**

```bash
git add CHANGELOG.md && git commit -m "changelog: scrollable logs + Log mode picker + ngrok-dev"
```

---

## Self-Review Notes

**Spec coverage:**
- Scrollable byte-offset tail → Task 1 + Task 6
- ngrok-dev parser → Task 2 + Task 7
- LogMode enum + migration + YAML gen → Task 3
- Remove loglevel from Advanced → Task 4
- Basic-tab Log mode field + picker → Task 5

**Placeholder scan:** none. All code snippets are complete.

**Deferred / follow-ups:**
- Log search (`/`) inside the panel.
- Log rotation on disk.
- Response status / duration in ngrok-dev (cloudflared limitation).
