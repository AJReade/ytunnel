# ytunnel Advanced Options Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an "Advanced" tab to ytunnel's tunnel edit sheet that lets users configure ~30 cloudflared options (loglevel, protocol, retries, httpHostHeader, noTLSVerify, etc.), stored in `tunnels.toml` and serialized into the per-tunnel cloudflared YAML.

**Architecture:** Extend `PersistentTunnel` with two `BTreeMap<String, TunnelOptionValue>` fields (`tunnel_options` for top-level YAML keys, `origin_request` for per-ingress-rule `originRequest` keys). Rewrite `generate_tunnel_config` using `serde_yaml`. Replace the current chained-modal edit flow (`EditTarget` → `EditZone`) with a two-tab overlay (`Basic` / `Advanced`). No changes to launchd plists or systemd units — cloudflared re-reads the YAML on restart.

**Tech Stack:** Rust 2021, ratatui 0.30, crossterm 0.29, serde 1, toml 0.8. New dep: `serde_yaml` 0.9.

**Design Spec:** `docs/superpowers/specs/2026-08-01-ytunnel-advanced-yaml-options-design.md`

---

## File Structure

**Files created:**
- `src/cloudflared_options.rs` — Static `OPTIONS: &[OptionSpec]` registry describing every cloudflared option ytunnel knows how to render (name, scope, kind, description).
- `src/tui/edit_sheet.rs` — State + rendering for the new two-tab edit sheet (Basic / Advanced).

**Files modified:**
- `Cargo.toml` — add `serde_yaml = "0.9"`.
- `src/state.rs` — add `tunnel_options` + `origin_request` fields; add `TunnelOptionValue` enum; rewrite `generate_tunnel_config` using `serde_yaml`.
- `src/tunnel.rs` — replace inline YAML formatting with a shared helper that takes a `PersistentTunnel`-shaped input.
- `src/tui/app.rs` — remove `InputMode::EditTarget` / `InputMode::EditZone`; add sheet state; wire keybindings.
- `src/tui/ui.rs` — remove `render_edit_dialog` / `render_edit_zone_dialog`; delegate to the new module.
- `src/tui/mod.rs` — declare `mod edit_sheet`.
- `src/daemon.rs` — add `reload_if_installed(tunnel)` that wraps `launchctl kickstart -k` / `systemctl restart`.

**Files unchanged (verified):**
- `src/daemon.rs::generate_plist` and `::generate_service` — the argv doesn't grow; only the YAML the argv points at gains new keys.
- All CF API code in `src/cloudflare.rs`.

---

## Task 1: Add serde_yaml dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add dependency**

Edit `Cargo.toml`, in the `[dependencies]` section, after the `toml = "0.8"` line, add:

```toml
serde_yaml = "0.9"
```

- [ ] **Step 2: Verify it resolves**

Run: `cd /tmp/ytunnel && cargo check`
Expected: successful compile (no code changes yet, just dep resolution).

- [ ] **Step 3: Commit**

```bash
cd /tmp/ytunnel && git add Cargo.toml Cargo.lock && git commit -m "deps: add serde_yaml for structured YAML generation"
```

---

## Task 2: Add TunnelOptionValue enum and new fields to PersistentTunnel

**Files:**
- Modify: `src/state.rs:1-45` (imports + struct)

- [ ] **Step 1: Write the failing test**

At the bottom of `src/state.rs`, add (or extend if a `#[cfg(test)] mod tests` block already exists):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_tunnel_deserializes_without_new_fields() {
        // Represents a tunnels.toml entry written by an older ytunnel version.
        let legacy_toml = r#"
            name = "demo"
            account_name = "acct"
            target = "http://localhost:3000"
            zone_id = "zone123"
            zone_name = "example.com"
            hostname = "demo.example.com"
            tunnel_id = "uuid-1"
            enabled = true
        "#;

        let tunnel: PersistentTunnel = toml::from_str(legacy_toml).expect("should parse");
        assert_eq!(tunnel.name, "demo");
        assert!(tunnel.tunnel_options.is_empty());
        assert!(tunnel.origin_request.is_empty());
    }

    #[test]
    fn tunnel_option_value_roundtrips_all_variants() {
        use std::collections::BTreeMap;

        let mut opts: BTreeMap<String, TunnelOptionValue> = BTreeMap::new();
        opts.insert("loglevel".into(), TunnelOptionValue::String("debug".into()));
        opts.insert("retries".into(), TunnelOptionValue::Int(5));
        opts.insert("no-autoupdate".into(), TunnelOptionValue::Bool(true));
        opts.insert(
            "features".into(),
            TunnelOptionValue::List(vec!["a".into(), "b".into()]),
        );

        let serialized = toml::to_string(&opts).unwrap();
        let deserialized: BTreeMap<String, TunnelOptionValue> = toml::from_str(&serialized).unwrap();
        assert_eq!(opts.len(), deserialized.len());
        assert!(matches!(deserialized.get("loglevel"), Some(TunnelOptionValue::String(s)) if s == "debug"));
        assert!(matches!(deserialized.get("retries"), Some(TunnelOptionValue::Int(5))));
        assert!(matches!(deserialized.get("no-autoupdate"), Some(TunnelOptionValue::Bool(true))));
        assert!(matches!(deserialized.get("features"), Some(TunnelOptionValue::List(v)) if v.len() == 2));
    }
}
```

- [ ] **Step 2: Run tests, verify failure**

Run: `cd /tmp/ytunnel && cargo test --lib state::tests -- --nocapture`
Expected: compile error — `TunnelOptionValue` not found, `tunnel_options` / `origin_request` not fields.

- [ ] **Step 3: Add the enum and fields**

At the top of `src/state.rs`, add to imports (line 1-4):

```rust
use std::collections::BTreeMap;
```

After the imports and before `pub enum TunnelStatus` (around line 8), add:

```rust
/// A YAML-serializable value for a cloudflared option.
/// Untagged enum: TOML/YAML value shape determines the variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TunnelOptionValue {
    Bool(bool),
    Int(i64),
    String(String),
    List(Vec<String>),
}
```

Then modify the `PersistentTunnel` struct (lines 27-45). After the `metrics_port` field (line 44), add two new fields:

```rust
    /// Top-level cloudflared YAML options (loglevel, protocol, retries, ...).
    /// Serialized as top-level keys in the generated per-tunnel YAML.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tunnel_options: BTreeMap<String, TunnelOptionValue>,

    /// Per-ingress-rule originRequest options (httpHostHeader, noTLSVerify, ...).
    /// Attached to the ingress rule as `originRequest:` in the generated YAML.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub origin_request: BTreeMap<String, TunnelOptionValue>,
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cd /tmp/ytunnel && cargo test --lib state::tests -- --nocapture`
Expected: both tests pass.

- [ ] **Step 5: Verify existing state.rs consumers still compile**

Run: `cd /tmp/ytunnel && cargo check`
Expected: clean compile (`PersistentTunnel` construction sites in other modules may need `..Default::default()` if they use struct-literal syntax — if any errors surface, they'll show file:line; add the two new fields to each construction site as `tunnel_options: BTreeMap::new(), origin_request: BTreeMap::new()`).

- [ ] **Step 6: Commit**

```bash
cd /tmp/ytunnel && git add src/state.rs && git commit -m "state: add TunnelOptionValue and advanced-options fields to PersistentTunnel"
```

---

## Task 3: Rewrite generate_tunnel_config using serde_yaml (empty-options parity)

**Files:**
- Modify: `src/state.rs:238-264`

This task rewrites the YAML generator but confirms it produces the same output as today when both new maps are empty. Later tasks extend it to serialize the maps.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/state.rs`:

```rust
    fn sample_tunnel() -> PersistentTunnel {
        PersistentTunnel {
            name: "demo".into(),
            account_name: "acct".into(),
            target: "http://localhost:3000".into(),
            zone_id: "zone123".into(),
            zone_name: "example.com".into(),
            hostname: "demo.example.com".into(),
            tunnel_id: "uuid-1".into(),
            enabled: true,
            auto_start: false,
            metrics_port: Some(21000),
            tunnel_options: BTreeMap::new(),
            origin_request: BTreeMap::new(),
        }
    }

    #[test]
    fn generate_yaml_matches_legacy_shape_when_no_options() {
        let tunnel = sample_tunnel();
        let yaml = generate_tunnel_config(&tunnel).expect("generate");

        // Parse via serde_yaml to compare structurally (whitespace-tolerant).
        let doc: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(doc["tunnel"].as_str().unwrap(), "uuid-1");
        assert!(doc["credentials-file"].as_str().unwrap().ends_with("uuid-1.json"));

        let ingress = doc["ingress"].as_sequence().unwrap();
        assert_eq!(ingress.len(), 2);
        assert_eq!(ingress[0]["hostname"].as_str().unwrap(), "demo.example.com");
        assert_eq!(ingress[0]["service"].as_str().unwrap(), "http://localhost:3000");
        assert!(ingress[0].get("originRequest").is_none(), "empty origin_request must not emit key");
        assert_eq!(ingress[1]["service"].as_str().unwrap(), "http_status:404");

        // Top-level options must be absent when tunnel_options is empty.
        for key in ["loglevel", "protocol", "retries"] {
            assert!(doc.get(key).is_none(), "unexpected top-level key: {key}");
        }
    }
```

- [ ] **Step 2: Run test, verify failure**

Run: `cd /tmp/ytunnel && cargo test --lib state::tests::generate_yaml_matches_legacy_shape_when_no_options -- --nocapture`
Expected: FAIL — either compile error (if imports missing) or assertion failure. Confirm the failure mode is *what you'd expect*, not something unrelated.

- [ ] **Step 3: Rewrite the generator**

Replace `generate_tunnel_config` (lines 238-264) with:

```rust
// Generate the cloudflared config YAML content for a tunnel
pub fn generate_tunnel_config(tunnel: &PersistentTunnel) -> Result<String> {
    let credentials_path = tunnel.credentials_path()?;

    // Normalize target URL (preserve existing behavior).
    let target_url =
        if tunnel.target.starts_with("http://") || tunnel.target.starts_with("https://") {
            tunnel.target.clone()
        } else {
            format!("http://{}", tunnel.target)
        };

    let mut doc = serde_yaml::Mapping::new();
    doc.insert(
        serde_yaml::Value::from("tunnel"),
        serde_yaml::Value::from(tunnel.tunnel_id.clone()),
    );
    doc.insert(
        serde_yaml::Value::from("credentials-file"),
        serde_yaml::Value::from(credentials_path.display().to_string()),
    );

    // Ingress rules: one for the tunnel's hostname, one catch-all.
    let mut rule = serde_yaml::Mapping::new();
    rule.insert(
        serde_yaml::Value::from("hostname"),
        serde_yaml::Value::from(tunnel.hostname.clone()),
    );
    rule.insert(
        serde_yaml::Value::from("service"),
        serde_yaml::Value::from(target_url),
    );

    let mut catch_all = serde_yaml::Mapping::new();
    catch_all.insert(
        serde_yaml::Value::from("service"),
        serde_yaml::Value::from("http_status:404"),
    );

    let ingress = serde_yaml::Value::Sequence(vec![
        serde_yaml::Value::Mapping(rule),
        serde_yaml::Value::Mapping(catch_all),
    ]);
    doc.insert(serde_yaml::Value::from("ingress"), ingress);

    serde_yaml::to_string(&serde_yaml::Value::Mapping(doc))
        .context("Failed to serialize tunnel YAML")
}
```

- [ ] **Step 4: Run test, verify pass**

Run: `cd /tmp/ytunnel && cargo test --lib state::tests -- --nocapture`
Expected: all state::tests pass.

- [ ] **Step 5: Full build check**

Run: `cd /tmp/ytunnel && cargo check`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd /tmp/ytunnel && git add src/state.rs && git commit -m "state: rewrite generate_tunnel_config using serde_yaml (behavior unchanged)"
```

---

## Task 4: Serialize tunnel_options into top-level YAML keys

**Files:**
- Modify: `src/state.rs::generate_tunnel_config`

- [ ] **Step 1: Write the failing test**

Add to `mod tests`:

```rust
    #[test]
    fn tunnel_options_render_as_top_level_yaml_keys() {
        let mut tunnel = sample_tunnel();
        tunnel.tunnel_options.insert(
            "loglevel".into(),
            TunnelOptionValue::String("debug".into()),
        );
        tunnel.tunnel_options.insert("retries".into(), TunnelOptionValue::Int(5));
        tunnel.tunnel_options.insert(
            "no-autoupdate".into(),
            TunnelOptionValue::Bool(true),
        );
        tunnel.tunnel_options.insert(
            "features".into(),
            TunnelOptionValue::List(vec!["diag-http".into(), "diag-mem".into()]),
        );

        let yaml = generate_tunnel_config(&tunnel).unwrap();
        let doc: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(doc["loglevel"].as_str().unwrap(), "debug");
        assert_eq!(doc["retries"].as_i64().unwrap(), 5);
        assert_eq!(doc["no-autoupdate"].as_bool().unwrap(), true);
        let features = doc["features"].as_sequence().unwrap();
        assert_eq!(features.len(), 2);
        assert_eq!(features[0].as_str().unwrap(), "diag-http");
    }
```

- [ ] **Step 2: Run test, verify failure**

Run: `cd /tmp/ytunnel && cargo test --lib state::tests::tunnel_options_render_as_top_level_yaml_keys -- --nocapture`
Expected: FAIL — options are silently ignored today.

- [ ] **Step 3: Extend the generator**

In `generate_tunnel_config`, add a helper for converting `TunnelOptionValue` into `serde_yaml::Value`. At the bottom of `src/state.rs` (outside `mod tests`), add:

```rust
fn option_value_to_yaml(v: &TunnelOptionValue) -> serde_yaml::Value {
    match v {
        TunnelOptionValue::Bool(b) => serde_yaml::Value::Bool(*b),
        TunnelOptionValue::Int(i) => serde_yaml::Value::Number((*i).into()),
        TunnelOptionValue::String(s) => serde_yaml::Value::String(s.clone()),
        TunnelOptionValue::List(items) => serde_yaml::Value::Sequence(
            items
                .iter()
                .map(|s| serde_yaml::Value::String(s.clone()))
                .collect(),
        ),
    }
}
```

Then inside `generate_tunnel_config`, after the `credentials-file` insert and *before* the ingress construction, insert:

```rust
    // Top-level tunnel options (loglevel, protocol, retries, features, ...).
    for (key, value) in &tunnel.tunnel_options {
        doc.insert(
            serde_yaml::Value::from(key.clone()),
            option_value_to_yaml(value),
        );
    }
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cd /tmp/ytunnel && cargo test --lib state::tests -- --nocapture`
Expected: all pass, including the new one *and* the "empty options" test from Task 3.

- [ ] **Step 5: Commit**

```bash
cd /tmp/ytunnel && git add src/state.rs && git commit -m "state: render tunnel_options as top-level YAML keys"
```

---

## Task 5: Serialize origin_request as originRequest on the ingress rule

**Files:**
- Modify: `src/state.rs::generate_tunnel_config`

- [ ] **Step 1: Write the failing test**

Add to `mod tests`:

```rust
    #[test]
    fn origin_request_attaches_to_ingress_rule() {
        let mut tunnel = sample_tunnel();
        tunnel.origin_request.insert(
            "httpHostHeader".into(),
            TunnelOptionValue::String("google-spike.localhost".into()),
        );
        tunnel
            .origin_request
            .insert("noTLSVerify".into(), TunnelOptionValue::Bool(true));
        tunnel.origin_request.insert(
            "connectTimeout".into(),
            TunnelOptionValue::String("30s".into()),
        );

        let yaml = generate_tunnel_config(&tunnel).unwrap();
        let doc: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let ingress = doc["ingress"].as_sequence().unwrap();

        // Origin request is on the hostname rule, not the catch-all.
        let rule = &ingress[0];
        assert_eq!(rule["hostname"].as_str().unwrap(), "demo.example.com");
        let origin = &rule["originRequest"];
        assert_eq!(
            origin["httpHostHeader"].as_str().unwrap(),
            "google-spike.localhost"
        );
        assert_eq!(origin["noTLSVerify"].as_bool().unwrap(), true);
        assert_eq!(origin["connectTimeout"].as_str().unwrap(), "30s");

        // Catch-all rule must NOT have originRequest.
        assert!(ingress[1].get("originRequest").is_none());
    }
```

- [ ] **Step 2: Run test, verify failure**

Run: `cd /tmp/ytunnel && cargo test --lib state::tests::origin_request_attaches_to_ingress_rule -- --nocapture`
Expected: FAIL — `origin_request` map is silently ignored.

- [ ] **Step 3: Extend the generator**

In `generate_tunnel_config`, modify the `rule` construction (the section that inserts `hostname` and `service`). *After* the two inserts and *before* `catch_all` construction, add:

```rust
    if !tunnel.origin_request.is_empty() {
        let mut origin = serde_yaml::Mapping::new();
        for (key, value) in &tunnel.origin_request {
            origin.insert(
                serde_yaml::Value::from(key.clone()),
                option_value_to_yaml(value),
            );
        }
        rule.insert(
            serde_yaml::Value::from("originRequest"),
            serde_yaml::Value::Mapping(origin),
        );
    }
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cd /tmp/ytunnel && cargo test --lib state::tests -- --nocapture`
Expected: all pass. The Task 3 "no originRequest key when empty" assertion protects the `if !is_empty` guard from regression.

- [ ] **Step 5: Commit**

```bash
cd /tmp/ytunnel && git add src/state.rs && git commit -m "state: attach origin_request as originRequest on ingress rule"
```

---

## Task 6: Create cloudflared options registry

**Files:**
- Create: `src/cloudflared_options.rs`
- Modify: `src/main.rs` (add `mod cloudflared_options;`)

- [ ] **Step 1: Write the failing test**

Create `src/cloudflared_options.rs` with:

```rust
// Static registry of cloudflared configuration options ytunnel can render in the
// Advanced edit pane. Each entry describes one option's YAML key, scope, type,
// default (if any), and a one-line human description.

use crate::state::TunnelOptionValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionScope {
    /// Top-level YAML key on the tunnel config document.
    Tunnel,
    /// Nested under the ingress rule's `originRequest:` block.
    OriginRequest,
}

#[derive(Debug, Clone)]
pub enum OptionKind {
    Bool {
        default: bool,
    },
    Int {
        default: Option<i64>,
        min: Option<i64>,
        max: Option<i64>,
    },
    /// Free-text string. `placeholder` shown in the empty edit field.
    String {
        default: Option<&'static str>,
        placeholder: &'static str,
    },
    /// Duration in cloudflared syntax (e.g. "30s", "1m30s"). Validated on save.
    Duration {
        default: Option<&'static str>,
    },
    /// Fixed set of allowed values.
    Enum {
        default: Option<&'static str>,
        choices: &'static [&'static str],
    },
    /// Newline-separated list in the UI; serialized as a YAML sequence.
    List {
        placeholder: &'static str,
    },
}

#[derive(Debug, Clone)]
pub struct OptionSpec {
    pub yaml_key: &'static str,
    pub scope: OptionScope,
    pub display_name: &'static str,
    pub description: &'static str,
    pub kind: OptionKind,
}

pub const OPTIONS: &[OptionSpec] = &[
    // --- Tunnel scope ---
    OptionSpec {
        yaml_key: "loglevel",
        scope: OptionScope::Tunnel,
        display_name: "Log level",
        description: "Application logging verbosity.",
        kind: OptionKind::Enum {
            default: Some("info"),
            choices: &["debug", "info", "warn", "error", "fatal"],
        },
    },
    OptionSpec {
        yaml_key: "transport-loglevel",
        scope: OptionScope::Tunnel,
        display_name: "Transport log level",
        description: "Transport-layer logging verbosity.",
        kind: OptionKind::Enum {
            default: Some("info"),
            choices: &["debug", "info", "warn", "error", "fatal"],
        },
    },
    OptionSpec {
        yaml_key: "protocol",
        scope: OptionScope::Tunnel,
        display_name: "Protocol",
        description: "Transport protocol to Cloudflare edge.",
        kind: OptionKind::Enum {
            default: Some("auto"),
            choices: &["auto", "http2", "quic"],
        },
    },
    OptionSpec {
        yaml_key: "retries",
        scope: OptionScope::Tunnel,
        display_name: "Retries",
        description: "Max connection retry attempts on error.",
        kind: OptionKind::Int {
            default: Some(5),
            min: Some(0),
            max: Some(100),
        },
    },
    OptionSpec {
        yaml_key: "grace-period",
        scope: OptionScope::Tunnel,
        display_name: "Grace period",
        description: "Shutdown grace period after SIGTERM.",
        kind: OptionKind::Duration {
            default: Some("30s"),
        },
    },
    OptionSpec {
        yaml_key: "edge-ip-version",
        scope: OptionScope::Tunnel,
        display_name: "Edge IP version",
        description: "IP version for connections to Cloudflare edge.",
        kind: OptionKind::Enum {
            default: Some("auto"),
            choices: &["4", "6", "auto"],
        },
    },
    OptionSpec {
        yaml_key: "region",
        scope: OptionScope::Tunnel,
        display_name: "Region",
        description: "Cloudflare edge region. Empty = global.",
        kind: OptionKind::String {
            default: None,
            placeholder: "us / eu / (empty for global)",
        },
    },
    OptionSpec {
        yaml_key: "metrics-update-freq",
        scope: OptionScope::Tunnel,
        display_name: "Metrics update frequency",
        description: "How often to refresh internal metrics.",
        kind: OptionKind::Duration {
            default: Some("5s"),
        },
    },
    OptionSpec {
        yaml_key: "compression-quality",
        scope: OptionScope::Tunnel,
        display_name: "Compression quality",
        description: "Cross-stream compression level. 0=off, 3+=high.",
        kind: OptionKind::Int {
            default: Some(0),
            min: Some(0),
            max: Some(9),
        },
    },
    OptionSpec {
        yaml_key: "post-quantum",
        scope: OptionScope::Tunnel,
        display_name: "Post-quantum",
        description: "Enable experimental post-quantum secure tunnel.",
        kind: OptionKind::Bool { default: false },
    },
    OptionSpec {
        yaml_key: "label",
        scope: OptionScope::Tunnel,
        display_name: "Connector label",
        description: "Human-readable label for this connector instance.",
        kind: OptionKind::String {
            default: None,
            placeholder: "e.g. laptop-office",
        },
    },
    OptionSpec {
        yaml_key: "features",
        scope: OptionScope::Tunnel,
        display_name: "Features",
        description: "Opt into experimental features (one per line).",
        kind: OptionKind::List {
            placeholder: "diag-http\ndiag-mem",
        },
    },
    OptionSpec {
        yaml_key: "bastion",
        scope: OptionScope::Tunnel,
        display_name: "Bastion",
        description: "Run as jump host.",
        kind: OptionKind::Bool { default: false },
    },
    OptionSpec {
        yaml_key: "pidfile",
        scope: OptionScope::Tunnel,
        display_name: "PID file",
        description: "Path to write cloudflared PID.",
        kind: OptionKind::String {
            default: None,
            placeholder: "/var/run/cloudflared.pid",
        },
    },
    OptionSpec {
        yaml_key: "logfile",
        scope: OptionScope::Tunnel,
        display_name: "Log file",
        description: "Path to write cloudflared logs.",
        kind: OptionKind::String {
            default: None,
            placeholder: "/var/log/cloudflared.log",
        },
    },
    OptionSpec {
        yaml_key: "log-directory",
        scope: OptionScope::Tunnel,
        display_name: "Log directory",
        description: "Directory to write cloudflared logs.",
        kind: OptionKind::String {
            default: None,
            placeholder: "/var/log/cloudflared",
        },
    },
    // --- Origin request scope ---
    OptionSpec {
        yaml_key: "httpHostHeader",
        scope: OptionScope::OriginRequest,
        display_name: "HTTP Host header",
        description: "Override the Host header sent to origin.",
        kind: OptionKind::String {
            default: None,
            placeholder: "e.g. google-spike.localhost",
        },
    },
    OptionSpec {
        yaml_key: "originServerName",
        scope: OptionScope::OriginRequest,
        display_name: "Origin server name",
        description: "SNI hostname when connecting to origin over TLS.",
        kind: OptionKind::String {
            default: None,
            placeholder: "e.g. api.internal",
        },
    },
    OptionSpec {
        yaml_key: "caPool",
        scope: OptionScope::OriginRequest,
        display_name: "Origin CA pool",
        description: "Path to CA bundle for validating origin cert.",
        kind: OptionKind::String {
            default: None,
            placeholder: "/etc/ssl/certs/ca.pem",
        },
    },
    OptionSpec {
        yaml_key: "noTLSVerify",
        scope: OptionScope::OriginRequest,
        display_name: "No TLS verify",
        description: "Skip TLS verification of the origin cert (unsafe).",
        kind: OptionKind::Bool { default: false },
    },
    OptionSpec {
        yaml_key: "noHappyEyeballs",
        scope: OptionScope::OriginRequest,
        display_name: "No happy eyeballs",
        description: "Disable IPv4/v6 happy-eyeballs fallback.",
        kind: OptionKind::Bool { default: false },
    },
    OptionSpec {
        yaml_key: "connectTimeout",
        scope: OptionScope::OriginRequest,
        display_name: "Connect timeout",
        description: "Origin connection establishment timeout.",
        kind: OptionKind::Duration {
            default: Some("30s"),
        },
    },
    OptionSpec {
        yaml_key: "tlsTimeout",
        scope: OptionScope::OriginRequest,
        display_name: "TLS timeout",
        description: "Origin TLS handshake timeout.",
        kind: OptionKind::Duration {
            default: Some("10s"),
        },
    },
    OptionSpec {
        yaml_key: "tcpKeepAlive",
        scope: OptionScope::OriginRequest,
        display_name: "TCP keepalive",
        description: "TCP keepalive interval for origin connection.",
        kind: OptionKind::Duration {
            default: Some("30s"),
        },
    },
    OptionSpec {
        yaml_key: "keepAliveConnections",
        scope: OptionScope::OriginRequest,
        display_name: "Keepalive pool size",
        description: "Max idle origin connections to keep alive.",
        kind: OptionKind::Int {
            default: Some(100),
            min: Some(0),
            max: Some(10_000),
        },
    },
    OptionSpec {
        yaml_key: "keepAliveTimeout",
        scope: OptionScope::OriginRequest,
        display_name: "Keepalive timeout",
        description: "Idle timeout before closing a keepalive conn.",
        kind: OptionKind::Duration {
            default: Some("1m30s"),
        },
    },
    OptionSpec {
        yaml_key: "http2Origin",
        scope: OptionScope::OriginRequest,
        display_name: "HTTP/2 origin",
        description: "Speak HTTP/2 to the origin server.",
        kind: OptionKind::Bool { default: false },
    },
    OptionSpec {
        yaml_key: "disableChunkedEncoding",
        scope: OptionScope::OriginRequest,
        display_name: "Disable chunked encoding",
        description: "Disable transfer-encoding: chunked (WSGI hack).",
        kind: OptionKind::Bool { default: false },
    },
    OptionSpec {
        yaml_key: "proxyAddress",
        scope: OptionScope::OriginRequest,
        display_name: "Proxy listen address",
        description: "cloudflared's local proxy listen address.",
        kind: OptionKind::String {
            default: Some("127.0.0.1"),
            placeholder: "127.0.0.1",
        },
    },
    OptionSpec {
        yaml_key: "proxyPort",
        scope: OptionScope::OriginRequest,
        display_name: "Proxy listen port",
        description: "cloudflared's local proxy listen port.",
        kind: OptionKind::Int {
            default: Some(0),
            min: Some(0),
            max: Some(65_535),
        },
    },
    OptionSpec {
        yaml_key: "proxyType",
        scope: OptionScope::OriginRequest,
        display_name: "Proxy type",
        description: "Proxy protocol (empty = HTTP).",
        kind: OptionKind::Enum {
            default: None,
            choices: &["", "socks"],
        },
    },
];

/// Validate a `TunnelOptionValue` against its `OptionSpec`. Used before save.
pub fn validate_value(spec: &OptionSpec, value: &TunnelOptionValue) -> Result<(), String> {
    match (&spec.kind, value) {
        (OptionKind::Bool { .. }, TunnelOptionValue::Bool(_)) => Ok(()),
        (OptionKind::Int { min, max, .. }, TunnelOptionValue::Int(n)) => {
            if let Some(lo) = min {
                if n < lo {
                    return Err(format!("{} < min {}", n, lo));
                }
            }
            if let Some(hi) = max {
                if n > hi {
                    return Err(format!("{} > max {}", n, hi));
                }
            }
            Ok(())
        }
        (OptionKind::String { .. }, TunnelOptionValue::String(_)) => Ok(()),
        (OptionKind::Duration { .. }, TunnelOptionValue::String(s)) => parse_duration(s)
            .map(|_| ())
            .map_err(|e| format!("invalid duration: {}", e)),
        (OptionKind::Enum { choices, .. }, TunnelOptionValue::String(s)) => {
            if choices.contains(&s.as_str()) {
                Ok(())
            } else {
                Err(format!("must be one of: {}", choices.join(", ")))
            }
        }
        (OptionKind::List { .. }, TunnelOptionValue::List(_)) => Ok(()),
        _ => Err("type mismatch between spec and value".into()),
    }
}

/// Parse a cloudflared-style duration ("30s", "1m30s", "500ms").
/// Minimal parser: accepts sequences of `<int><unit>` where unit ∈ {ms, s, m, h}.
pub fn parse_duration(s: &str) -> Result<std::time::Duration, String> {
    if s.is_empty() {
        return Err("empty".into());
    }
    let mut total = std::time::Duration::ZERO;
    let mut buf = String::new();
    let mut iter = s.chars().peekable();
    while let Some(c) = iter.next() {
        if c.is_ascii_digit() {
            buf.push(c);
            continue;
        }
        let n: u64 = buf.parse().map_err(|e| format!("bad number: {}", e))?;
        buf.clear();
        // Unit: 1 or 2 chars.
        let unit = if c == 'm' && iter.peek() == Some(&'s') {
            iter.next();
            "ms"
        } else {
            match c {
                'h' => "h",
                'm' => "m",
                's' => "s",
                other => return Err(format!("bad unit: {}", other)),
            }
        };
        total += match unit {
            "ms" => std::time::Duration::from_millis(n),
            "s" => std::time::Duration::from_secs(n),
            "m" => std::time::Duration::from_secs(n * 60),
            "h" => std::time::Duration::from_secs(n * 3600),
            _ => unreachable!(),
        };
    }
    if !buf.is_empty() {
        return Err(format!("trailing digits without unit: {}", buf));
    }
    Ok(total)
}

pub fn find(yaml_key: &str) -> Option<&'static OptionSpec> {
    OPTIONS.iter().find(|o| o.yaml_key == yaml_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_yaml_keys_unique() {
        let mut seen = std::collections::HashSet::new();
        for spec in OPTIONS {
            assert!(
                seen.insert(spec.yaml_key),
                "duplicate yaml_key: {}",
                spec.yaml_key
            );
        }
    }

    #[test]
    fn all_specs_have_non_empty_fields() {
        for spec in OPTIONS {
            assert!(!spec.yaml_key.is_empty());
            assert!(!spec.display_name.is_empty());
            assert!(!spec.description.is_empty());
            if let OptionKind::Enum { choices, .. } = &spec.kind {
                assert!(!choices.is_empty(), "enum {} has no choices", spec.yaml_key);
            }
        }
    }

    #[test]
    fn parse_duration_accepts_common_formats() {
        assert_eq!(parse_duration("30s").unwrap().as_secs(), 30);
        assert_eq!(parse_duration("1m").unwrap().as_secs(), 60);
        assert_eq!(parse_duration("1m30s").unwrap().as_secs(), 90);
        assert_eq!(parse_duration("2h").unwrap().as_secs(), 7200);
        assert_eq!(parse_duration("500ms").unwrap().as_millis(), 500);
    }

    #[test]
    fn parse_duration_rejects_bad_input() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("30").is_err());
        assert!(parse_duration("30x").is_err());
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn validate_int_range() {
        let spec = find("retries").unwrap();
        assert!(validate_value(spec, &TunnelOptionValue::Int(5)).is_ok());
        assert!(validate_value(spec, &TunnelOptionValue::Int(-1)).is_err());
        assert!(validate_value(spec, &TunnelOptionValue::Int(101)).is_err());
    }

    #[test]
    fn validate_enum_membership() {
        let spec = find("loglevel").unwrap();
        assert!(validate_value(spec, &TunnelOptionValue::String("debug".into())).is_ok());
        assert!(validate_value(spec, &TunnelOptionValue::String("verbose".into())).is_err());
    }
}
```

- [ ] **Step 2: Register the module**

In `src/main.rs`, find the `mod` declarations near the top (should be near `mod config;`, `mod state;`, etc.) and add:

```rust
mod cloudflared_options;
```

- [ ] **Step 3: Run tests, verify pass**

Run: `cd /tmp/ytunnel && cargo test --lib cloudflared_options -- --nocapture`
Expected: all 5 tests pass.

- [ ] **Step 4: Full build check**

Run: `cd /tmp/ytunnel && cargo check`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
cd /tmp/ytunnel && git add src/cloudflared_options.rs src/main.rs && git commit -m "cloudflared_options: add curated registry of ~30 tunable options"
```

---

## Task 7: Refactor ephemeral tunnel YAML to share the persistent generator

**Files:**
- Modify: `src/tunnel.rs:20-52`
- Modify: `src/state.rs` (expose a helper suitable for ephemeral use)

Ephemeral tunnels build their own YAML string in `src/tunnel.rs`. To keep the two paths from drifting, extract the ingress-rule building into a shared function that takes just the primitives (tunnel_id, credentials_path, hostname, target_url, plus optional option maps).

- [ ] **Step 1: Write the failing test**

Add to `src/state.rs::mod tests`:

```rust
    #[test]
    fn build_yaml_from_parts_matches_persistent_shape() {
        use std::path::PathBuf;

        let yaml = build_tunnel_yaml(
            "uuid-1",
            &PathBuf::from("/tmp/creds.json"),
            "demo.example.com",
            "http://localhost:3000",
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();

        let doc: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(doc["tunnel"].as_str().unwrap(), "uuid-1");
        assert_eq!(
            doc["credentials-file"].as_str().unwrap(),
            "/tmp/creds.json"
        );
        assert_eq!(
            doc["ingress"][0]["service"].as_str().unwrap(),
            "http://localhost:3000"
        );
    }
```

- [ ] **Step 2: Run test, verify failure**

Run: `cd /tmp/ytunnel && cargo test --lib state::tests::build_yaml_from_parts -- --nocapture`
Expected: FAIL — `build_tunnel_yaml` doesn't exist.

- [ ] **Step 3: Extract the shared function**

In `src/state.rs`, add a new function above `generate_tunnel_config`:

```rust
/// Build the cloudflared YAML from raw parts. Shared between persistent tunnels
/// (via `generate_tunnel_config`) and ephemeral tunnels (via `tunnel::run_tunnel`).
pub fn build_tunnel_yaml(
    tunnel_id: &str,
    credentials_path: &std::path::Path,
    hostname: &str,
    target: &str,
    tunnel_options: &BTreeMap<String, TunnelOptionValue>,
    origin_request: &BTreeMap<String, TunnelOptionValue>,
) -> Result<String> {
    let target_url = if target.starts_with("http://") || target.starts_with("https://") {
        target.to_string()
    } else {
        format!("http://{}", target)
    };

    let mut doc = serde_yaml::Mapping::new();
    doc.insert(
        serde_yaml::Value::from("tunnel"),
        serde_yaml::Value::from(tunnel_id.to_string()),
    );
    doc.insert(
        serde_yaml::Value::from("credentials-file"),
        serde_yaml::Value::from(credentials_path.display().to_string()),
    );

    for (key, value) in tunnel_options {
        doc.insert(
            serde_yaml::Value::from(key.clone()),
            option_value_to_yaml(value),
        );
    }

    let mut rule = serde_yaml::Mapping::new();
    rule.insert(
        serde_yaml::Value::from("hostname"),
        serde_yaml::Value::from(hostname.to_string()),
    );
    rule.insert(
        serde_yaml::Value::from("service"),
        serde_yaml::Value::from(target_url),
    );
    if !origin_request.is_empty() {
        let mut origin = serde_yaml::Mapping::new();
        for (key, value) in origin_request {
            origin.insert(
                serde_yaml::Value::from(key.clone()),
                option_value_to_yaml(value),
            );
        }
        rule.insert(
            serde_yaml::Value::from("originRequest"),
            serde_yaml::Value::Mapping(origin),
        );
    }

    let mut catch_all = serde_yaml::Mapping::new();
    catch_all.insert(
        serde_yaml::Value::from("service"),
        serde_yaml::Value::from("http_status:404"),
    );

    doc.insert(
        serde_yaml::Value::from("ingress"),
        serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::Mapping(rule),
            serde_yaml::Value::Mapping(catch_all),
        ]),
    );

    serde_yaml::to_string(&serde_yaml::Value::Mapping(doc))
        .context("Failed to serialize tunnel YAML")
}
```

Then replace `generate_tunnel_config` body with a thin wrapper:

```rust
pub fn generate_tunnel_config(tunnel: &PersistentTunnel) -> Result<String> {
    build_tunnel_yaml(
        &tunnel.tunnel_id,
        &tunnel.credentials_path()?,
        &tunnel.hostname,
        &tunnel.target,
        &tunnel.tunnel_options,
        &tunnel.origin_request,
    )
}
```

- [ ] **Step 4: Run state tests, verify pass**

Run: `cd /tmp/ytunnel && cargo test --lib state::tests -- --nocapture`
Expected: all pass (including all four prior tests + the new build_yaml_from_parts test).

- [ ] **Step 5: Update ephemeral tunnel to use the shared helper**

In `src/tunnel.rs`, replace lines 20-52 (the `run_tunnel` body up through `fs::write`) with:

```rust
pub async fn run_tunnel(
    tunnel_id: &str,
    credentials_path: &std::path::Path,
    hostname: &str,
    target: &str,
) -> Result<()> {
    use std::collections::BTreeMap;

    let config_dir = config::config_dir()?;
    let config_path = config_dir.join(format!("tunnel-{}.yml", tunnel_id));

    let config_content = crate::state::build_tunnel_yaml(
        tunnel_id,
        credentials_path,
        hostname,
        target,
        &BTreeMap::new(),
        &BTreeMap::new(),
    )?;

    fs::write(&config_path, &config_content)
        .with_context(|| format!("Failed to write tunnel config to {}", config_path.display()))?;
```

Leave the rest of the function (`Command::new("cloudflared")...` onward) unchanged.

- [ ] **Step 6: Full build + tests**

Run: `cd /tmp/ytunnel && cargo test --lib -- --nocapture && cargo check`
Expected: all pass, clean check.

- [ ] **Step 7: Commit**

```bash
cd /tmp/ytunnel && git add src/state.rs src/tunnel.rs && git commit -m "state: extract build_tunnel_yaml; reuse from ephemeral run_tunnel"
```

---

## Task 8: Add daemon reload_if_installed helper

**Files:**
- Modify: `src/daemon.rs`

This gives the TUI a single call to make edits take effect immediately without re-implementing the launchctl/systemctl commands in the UI code.

- [ ] **Step 1: Add the macOS implementation**

In `src/daemon.rs`, inside the `#[cfg(target_os = "macos")]` block (after `uninstall_daemon`, around line 260), add:

```rust
#[cfg(target_os = "macos")]
pub async fn reload_if_installed(tunnel: &PersistentTunnel) -> Result<()> {
    let path = plist_path(&tunnel.account_name, &tunnel.name)?;
    if !path.exists() {
        return Ok(());
    }
    let label = launchd_label(&tunnel.account_name, &tunnel.name);
    // `kickstart -k` restarts the service, picking up any changes to the
    // cloudflared YAML that the plist points at. It's a no-op if the service
    // isn't currently loaded, so we `load` first for good measure.
    let _ = Command::new("launchctl")
        .args(["load", "-w"])
        .arg(&path)
        .output()
        .await;
    let output = Command::new("launchctl")
        .args(["kickstart", "-k"])
        .arg(format!("gui/{}/{}", unsafe { libc::getuid() }, label))
        .output()
        .await
        .context("Failed to run launchctl kickstart")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("launchctl kickstart failed: {}", err.trim());
    }
    Ok(())
}
```

- [ ] **Step 2: Add the Linux implementation**

In the `#[cfg(target_os = "linux")]` block (after the Linux `uninstall_daemon` around line 460), add:

```rust
#[cfg(target_os = "linux")]
pub async fn reload_if_installed(tunnel: &PersistentTunnel) -> Result<()> {
    let path = service_path(&tunnel.account_name, &tunnel.name)?;
    if !path.exists() {
        return Ok(());
    }
    let service_name = systemd_service_name(&tunnel.account_name, &tunnel.name);
    let output = Command::new("systemctl")
        .args(["--user", "restart", &service_name])
        .output()
        .await
        .context("Failed to run systemctl restart")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("systemctl restart failed: {}", err.trim());
    }
    Ok(())
}
```

If `service_path` or `systemd_service_name` don't exist as separate functions, check the existing `install_daemon` / `uninstall_daemon` code above and reuse whatever path/name helpers they use.

- [ ] **Step 3: Add the fallback stub**

In the fallback platform block (around line 630, after the other fallback stubs), add:

```rust
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub async fn reload_if_installed(_tunnel: &PersistentTunnel) -> Result<()> {
    Ok(())
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cd /tmp/ytunnel && cargo check`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
cd /tmp/ytunnel && git add src/daemon.rs && git commit -m "daemon: add reload_if_installed for post-edit daemon restart"
```

---

## Task 9: Create the edit sheet module (state + skeleton renderer)

**Files:**
- Create: `src/tui/edit_sheet.rs`
- Modify: `src/tui/mod.rs` (add `pub mod edit_sheet;`)

This lays the foundation for the two-tab UI. Later tasks fill in rendering and input handling.

- [ ] **Step 1: Locate the tui module declaration**

Read `src/tui/mod.rs` to see the existing structure. It should have `pub mod app;` and `pub mod ui;`. Add `pub mod edit_sheet;` after them.

- [ ] **Step 2: Create the module**

Create `src/tui/edit_sheet.rs`:

```rust
// State + rendering for the two-tab tunnel edit sheet.
//
// Basic tab: existing per-tunnel fields (target, zone, auto_start, metrics_port).
// Advanced tab: curated cloudflared options from `crate::cloudflared_options::OPTIONS`.

use std::collections::BTreeMap;

use crate::cloudflared_options::{OptionScope, OptionSpec, OPTIONS};
use crate::state::{PersistentTunnel, TunnelOptionValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetTab {
    Basic,
    Advanced,
}

/// A single editable row in the Advanced tab.
/// One row per entry in `OPTIONS`, in scope-then-registry-order.
#[derive(Debug, Clone)]
pub struct AdvancedRow {
    pub spec: &'static OptionSpec,
    /// Current value if set; None means "use cloudflared default".
    pub value: Option<TunnelOptionValue>,
}

/// Full state of the edit sheet.
#[derive(Debug, Clone)]
pub struct EditSheetState {
    /// Which tunnel is being edited (name; used to find the record on save).
    pub tunnel_name: String,
    pub active_tab: SheetTab,

    /// Basic tab draft fields.
    pub target: String,
    pub zone_name: String,
    pub auto_start: bool,
    pub metrics_port: Option<u16>,

    /// Advanced tab rows, in display order.
    pub advanced_rows: Vec<AdvancedRow>,
    pub selected_row: usize,

    /// Set to true when any field diverges from the original tunnel.
    pub dirty: bool,
}

impl EditSheetState {
    /// Build from an existing tunnel. All fields populated from `tunnel`.
    pub fn from_tunnel(tunnel: &PersistentTunnel) -> Self {
        let advanced_rows = OPTIONS
            .iter()
            .map(|spec| {
                let map = match spec.scope {
                    OptionScope::Tunnel => &tunnel.tunnel_options,
                    OptionScope::OriginRequest => &tunnel.origin_request,
                };
                AdvancedRow {
                    spec,
                    value: map.get(spec.yaml_key).cloned(),
                }
            })
            .collect();

        Self {
            tunnel_name: tunnel.name.clone(),
            active_tab: SheetTab::Basic,
            target: tunnel.target.clone(),
            zone_name: tunnel.zone_name.clone(),
            auto_start: tunnel.auto_start,
            metrics_port: tunnel.metrics_port,
            advanced_rows,
            selected_row: 0,
            dirty: false,
        }
    }

    /// Apply sheet state back onto a `PersistentTunnel`.
    /// Rebuilds `tunnel_options` and `origin_request` from `advanced_rows`.
    pub fn apply(&self, tunnel: &mut PersistentTunnel) {
        tunnel.target = self.target.clone();
        tunnel.zone_name = self.zone_name.clone();
        tunnel.auto_start = self.auto_start;
        tunnel.metrics_port = self.metrics_port;

        let mut tunnel_opts: BTreeMap<String, TunnelOptionValue> = BTreeMap::new();
        let mut origin_req: BTreeMap<String, TunnelOptionValue> = BTreeMap::new();

        for row in &self.advanced_rows {
            let Some(value) = &row.value else { continue };
            match row.spec.scope {
                OptionScope::Tunnel => {
                    tunnel_opts.insert(row.spec.yaml_key.to_string(), value.clone());
                }
                OptionScope::OriginRequest => {
                    origin_req.insert(row.spec.yaml_key.to_string(), value.clone());
                }
            }
        }
        tunnel.tunnel_options = tunnel_opts;
        tunnel.origin_request = origin_req;
    }

    pub fn toggle_tab(&mut self) {
        self.active_tab = match self.active_tab {
            SheetTab::Basic => SheetTab::Advanced,
            SheetTab::Advanced => SheetTab::Basic,
        };
    }

    pub fn select_next(&mut self) {
        if self.selected_row + 1 < self.advanced_rows.len() {
            self.selected_row += 1;
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected_row > 0 {
            self.selected_row -= 1;
        }
    }

    /// Clear the value on the currently-selected advanced row.
    pub fn clear_selected(&mut self) {
        if let Some(row) = self.advanced_rows.get_mut(self.selected_row) {
            if row.value.is_some() {
                row.value = None;
                self.dirty = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tunnel_with_options() -> PersistentTunnel {
        let mut tunnel = PersistentTunnel {
            name: "demo".into(),
            account_name: "acct".into(),
            target: "http://localhost:3000".into(),
            zone_id: "zone123".into(),
            zone_name: "example.com".into(),
            hostname: "demo.example.com".into(),
            tunnel_id: "uuid-1".into(),
            enabled: true,
            auto_start: false,
            metrics_port: None,
            tunnel_options: BTreeMap::new(),
            origin_request: BTreeMap::new(),
        };
        tunnel
            .tunnel_options
            .insert("loglevel".into(), TunnelOptionValue::String("debug".into()));
        tunnel.origin_request.insert(
            "httpHostHeader".into(),
            TunnelOptionValue::String("foo.local".into()),
        );
        tunnel
    }

    #[test]
    fn from_tunnel_populates_advanced_rows_with_current_values() {
        let tunnel = tunnel_with_options();
        let sheet = EditSheetState::from_tunnel(&tunnel);

        let loglevel_row = sheet
            .advanced_rows
            .iter()
            .find(|r| r.spec.yaml_key == "loglevel")
            .unwrap();
        assert!(matches!(
            &loglevel_row.value,
            Some(TunnelOptionValue::String(s)) if s == "debug"
        ));

        let host_row = sheet
            .advanced_rows
            .iter()
            .find(|r| r.spec.yaml_key == "httpHostHeader")
            .unwrap();
        assert!(matches!(
            &host_row.value,
            Some(TunnelOptionValue::String(s)) if s == "foo.local"
        ));

        // Unset options are None.
        let retries_row = sheet
            .advanced_rows
            .iter()
            .find(|r| r.spec.yaml_key == "retries")
            .unwrap();
        assert!(retries_row.value.is_none());
    }

    #[test]
    fn apply_rebuilds_option_maps() {
        let tunnel = tunnel_with_options();
        let mut sheet = EditSheetState::from_tunnel(&tunnel);

        // Clear loglevel, set retries.
        for row in &mut sheet.advanced_rows {
            match row.spec.yaml_key {
                "loglevel" => row.value = None,
                "retries" => row.value = Some(TunnelOptionValue::Int(10)),
                _ => {}
            }
        }

        let mut applied = tunnel.clone();
        sheet.apply(&mut applied);

        assert!(!applied.tunnel_options.contains_key("loglevel"));
        assert!(matches!(
            applied.tunnel_options.get("retries"),
            Some(TunnelOptionValue::Int(10))
        ));
        // origin_request preserved.
        assert!(applied.origin_request.contains_key("httpHostHeader"));
    }

    #[test]
    fn clear_selected_removes_value_and_marks_dirty() {
        let tunnel = tunnel_with_options();
        let mut sheet = EditSheetState::from_tunnel(&tunnel);
        let idx = sheet
            .advanced_rows
            .iter()
            .position(|r| r.spec.yaml_key == "loglevel")
            .unwrap();
        sheet.selected_row = idx;

        sheet.clear_selected();
        assert!(sheet.advanced_rows[idx].value.is_none());
        assert!(sheet.dirty);
    }

    #[test]
    fn toggle_tab_alternates() {
        let sheet = EditSheetState::from_tunnel(&tunnel_with_options());
        let mut s = sheet;
        assert_eq!(s.active_tab, SheetTab::Basic);
        s.toggle_tab();
        assert_eq!(s.active_tab, SheetTab::Advanced);
        s.toggle_tab();
        assert_eq!(s.active_tab, SheetTab::Basic);
    }
}
```

- [ ] **Step 3: Register the module**

Edit `src/tui/mod.rs`. If it currently reads e.g. `pub mod app;\npub mod ui;`, add `pub mod edit_sheet;` after those lines.

- [ ] **Step 4: Run tests, verify pass**

Run: `cd /tmp/ytunnel && cargo test --lib tui::edit_sheet -- --nocapture`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
cd /tmp/ytunnel && git add src/tui/mod.rs src/tui/edit_sheet.rs && git commit -m "tui: add edit_sheet module with state model and tests"
```

---

## Task 10: Wire the edit sheet into the TUI (rendering + input)

**Files:**
- Modify: `src/tui/app.rs` (input modes, key handling, action)
- Modify: `src/tui/ui.rs` (rendering)
- Modify: `src/tui/edit_sheet.rs` (render function)

This is the biggest UI change. It replaces the current `EditTarget` → `EditZone` chained modals with the two-tab sheet.

- [ ] **Step 1: Add render function to edit_sheet.rs**

Append to `src/tui/edit_sheet.rs`:

```rust
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs},
    Frame,
};

use crate::cloudflared_options::OptionKind;

/// Render the edit sheet as a centered overlay. Caller is responsible for
/// clearing the underlying area (via `Clear` widget) if needed.
pub fn render(f: &mut Frame, area: Rect, sheet: &EditSheetState) {
    // Centered 70%x80% overlay.
    let outer = centered_rect(70, 80, area);
    f.render_widget(Clear, outer);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Edit tunnel: {} ", sheet.tunnel_name));
    let inner = block.inner(outer);
    f.render_widget(block, outer);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // tabs
            Constraint::Min(1),    // body
            Constraint::Length(2), // help
        ])
        .split(inner);

    // Tabs.
    let titles = vec![Line::from("Basic"), Line::from("Advanced")];
    let selected = match sheet.active_tab {
        SheetTab::Basic => 0,
        SheetTab::Advanced => 1,
    };
    let tabs = Tabs::new(titles)
        .select(selected)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(tabs, chunks[0]);

    // Body.
    match sheet.active_tab {
        SheetTab::Basic => render_basic(f, chunks[1], sheet),
        SheetTab::Advanced => render_advanced(f, chunks[1], sheet),
    }

    // Help line.
    let help = match sheet.active_tab {
        SheetTab::Basic => "Tab: switch pane   Ctrl+S: save   Esc: cancel",
        SheetTab::Advanced => "↑/↓: select   Enter: edit   d: clear   Tab: switch pane   Ctrl+S: save   Esc: cancel",
    };
    f.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn render_basic(f: &mut Frame, area: Rect, sheet: &EditSheetState) {
    let lines = vec![
        Line::from(vec![
            Span::styled("Target: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&sheet.target),
        ]),
        Line::from(vec![
            Span::styled("Zone:   ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&sheet.zone_name),
        ]),
        Line::from(vec![
            Span::styled(
                "Auto-start: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(if sheet.auto_start { "yes" } else { "no" }),
        ]),
        Line::from(vec![
            Span::styled(
                "Metrics port: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(
                sheet
                    .metrics_port
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "(auto)".into()),
            ),
        ]),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

fn render_advanced(f: &mut Frame, area: Rect, sheet: &EditSheetState) {
    let mut items: Vec<ListItem> = Vec::new();
    let mut last_scope: Option<OptionScope> = None;

    for (idx, row) in sheet.advanced_rows.iter().enumerate() {
        if Some(row.spec.scope) != last_scope {
            let header = match row.spec.scope {
                OptionScope::Tunnel => "── Tunnel ──",
                OptionScope::OriginRequest => "── Origin Request ──",
            };
            items.push(ListItem::new(Line::from(Span::styled(
                header,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))));
            last_scope = Some(row.spec.scope);
        }

        let value_display = match &row.value {
            Some(v) => value_to_display(v),
            None => match &row.spec.kind {
                OptionKind::Bool { default } => format!("(default: {})", default),
                OptionKind::Int { default: Some(n), .. } => format!("(default: {})", n),
                OptionKind::String { default: Some(s), .. } => format!("(default: {})", s),
                OptionKind::Duration { default: Some(s) } => format!("(default: {})", s),
                OptionKind::Enum { default: Some(s), .. } => format!("(default: {})", s),
                _ => "(unset)".to_string(),
            },
        };

        let selected_marker = if idx == sheet.selected_row { "› " } else { "  " };
        let line = Line::from(vec![
            Span::raw(selected_marker),
            Span::styled(
                format!("{:<24}", row.spec.display_name),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(value_display),
        ]);
        items.push(ListItem::new(line));
    }

    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::NONE)),
        area,
    );
}

fn value_to_display(v: &TunnelOptionValue) -> String {
    match v {
        TunnelOptionValue::Bool(b) => b.to_string(),
        TunnelOptionValue::Int(n) => n.to_string(),
        TunnelOptionValue::String(s) => s.clone(),
        TunnelOptionValue::List(items) => items.join(", "),
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
```

- [ ] **Step 2: Replace old InputMode variants in app.rs**

Open `src/tui/app.rs`. Find `pub enum InputMode` (around line 387) and:

- Remove `EditTarget` and `EditZone` variants.
- Add these new variants:
  ```rust
      EditSheet,
      EditSheetInput { yaml_key: String, buffer: String },
      EditSheetPicker { yaml_key: String, cursor: usize },
      EditSheetConfirmDiscard,
  ```

Then find `pub struct App` (around line 538) and add these fields:

```rust
    pub edit_sheet: Option<crate::tui::edit_sheet::EditSheetState>,
```

Initialize in the `App::new` (or whatever the constructor is) as `edit_sheet: None`.

- [ ] **Step 3: Replace start_edit and remove old edit handlers**

Find `fn start_edit` (around line 1470). Replace its body with:

```rust
    pub fn start_edit(&mut self) {
        let Some(name) = self.selected_tunnel_name() else { return };
        let Some(tunnel) = self.tunnel_state.find(&name).cloned() else { return };
        self.edit_sheet = Some(crate::tui::edit_sheet::EditSheetState::from_tunnel(&tunnel));
        self.input_mode = InputMode::EditSheet;
    }
```

`selected_tunnel_name()` should already exist; if it doesn't, use whatever the existing `start_edit` used to identify the tunnel (`self.editing_tunnel_name` field, `self.tunnels.selected()`, etc.). Preserve that pattern.

Delete the old `finish_edit_target` / `finish_edit_zone` / `cancel_edit` functions (whatever their names are — trace them from the `EditTarget` / `EditZone` match arms).

- [ ] **Step 4: Add sheet key handling in the event loop**

Find the input mode match block (around line 1926, `InputMode::Normal => match key.code {`). Delete the `InputMode::EditTarget` and `InputMode::EditZone` arms. Add before them:

```rust
                    InputMode::EditSheet => match key.code {
                        KeyCode::Esc => {
                            if app.edit_sheet.as_ref().is_some_and(|s| s.dirty) {
                                app.input_mode = InputMode::EditSheetConfirmDiscard;
                            } else {
                                app.edit_sheet = None;
                                app.input_mode = InputMode::Normal;
                            }
                        }
                        KeyCode::Tab | KeyCode::BackTab => {
                            if let Some(s) = app.edit_sheet.as_mut() {
                                s.toggle_tab();
                            }
                        }
                        KeyCode::Up => {
                            if let Some(s) = app.edit_sheet.as_mut() {
                                if s.active_tab == crate::tui::edit_sheet::SheetTab::Advanced {
                                    s.select_prev();
                                }
                            }
                        }
                        KeyCode::Down => {
                            if let Some(s) = app.edit_sheet.as_mut() {
                                if s.active_tab == crate::tui::edit_sheet::SheetTab::Advanced {
                                    s.select_next();
                                }
                            }
                        }
                        KeyCode::Char('d')
                            if app.edit_sheet.as_ref().is_some_and(|s| {
                                s.active_tab == crate::tui::edit_sheet::SheetTab::Advanced
                            }) =>
                        {
                            if let Some(s) = app.edit_sheet.as_mut() {
                                s.clear_selected();
                            }
                        }
                        KeyCode::Enter => {
                            // Open editor for selected row.
                            if let Some(s) = app.edit_sheet.as_ref() {
                                if s.active_tab == crate::tui::edit_sheet::SheetTab::Advanced {
                                    let row = &s.advanced_rows[s.selected_row];
                                    let yaml_key = row.spec.yaml_key.to_string();
                                    use crate::cloudflared_options::OptionKind;
                                    match &row.spec.kind {
                                        OptionKind::Bool { default } => {
                                            // Toggle in-place, no sub-editor.
                                            let current = match &row.value {
                                                Some(crate::state::TunnelOptionValue::Bool(b)) => *b,
                                                _ => *default,
                                            };
                                            if let Some(sm) = app.edit_sheet.as_mut() {
                                                sm.advanced_rows[sm.selected_row].value =
                                                    Some(crate::state::TunnelOptionValue::Bool(!current));
                                                sm.dirty = true;
                                            }
                                        }
                                        OptionKind::Enum { .. } => {
                                            app.input_mode = InputMode::EditSheetPicker {
                                                yaml_key,
                                                cursor: 0,
                                            };
                                        }
                                        _ => {
                                            let buffer = row
                                                .value
                                                .as_ref()
                                                .map(|v| match v {
                                                    crate::state::TunnelOptionValue::String(s) => s.clone(),
                                                    crate::state::TunnelOptionValue::Int(n) => n.to_string(),
                                                    crate::state::TunnelOptionValue::List(items) => items.join("\n"),
                                                    crate::state::TunnelOptionValue::Bool(b) => b.to_string(),
                                                })
                                                .unwrap_or_default();
                                            app.input_mode =
                                                InputMode::EditSheetInput { yaml_key, buffer };
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            save_edit_sheet(app).await;
                        }
                        _ => {}
                    },
                    InputMode::EditSheetInput { .. } => match key.code {
                        KeyCode::Esc => app.input_mode = InputMode::EditSheet,
                        KeyCode::Enter => {
                            commit_input_edit(app);
                            app.input_mode = InputMode::EditSheet;
                        }
                        KeyCode::Char(c) => {
                            if let InputMode::EditSheetInput { buffer, .. } = &mut app.input_mode {
                                buffer.push(c);
                            }
                        }
                        KeyCode::Backspace => {
                            if let InputMode::EditSheetInput { buffer, .. } = &mut app.input_mode {
                                buffer.pop();
                            }
                        }
                        _ => {}
                    },
                    InputMode::EditSheetPicker { .. } => match key.code {
                        KeyCode::Esc => app.input_mode = InputMode::EditSheet,
                        KeyCode::Up => {
                            if let InputMode::EditSheetPicker { cursor, .. } = &mut app.input_mode {
                                *cursor = cursor.saturating_sub(1);
                            }
                        }
                        KeyCode::Down => {
                            if let InputMode::EditSheetPicker { yaml_key, cursor } = &mut app.input_mode {
                                if let Some(spec) = crate::cloudflared_options::find(yaml_key) {
                                    if let crate::cloudflared_options::OptionKind::Enum {
                                        choices, ..
                                    } = &spec.kind
                                    {
                                        if *cursor + 1 < choices.len() {
                                            *cursor += 1;
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Enter => {
                            commit_picker_edit(app);
                            app.input_mode = InputMode::EditSheet;
                        }
                        _ => {}
                    },
                    InputMode::EditSheetConfirmDiscard => match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            app.edit_sheet = None;
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                            app.input_mode = InputMode::EditSheet;
                        }
                        _ => {}
                    },
```

Add the helpers `save_edit_sheet`, `commit_input_edit`, and `commit_picker_edit` as free functions (or methods on `App`) near the top of `app.rs`:

```rust
async fn save_edit_sheet(app: &mut App) {
    let Some(sheet) = app.edit_sheet.clone() else { return };
    let Some(tunnel) = app.tunnel_state.find_mut(&sheet.tunnel_name) else { return };
    sheet.apply(tunnel);
    let cloned = tunnel.clone();
    if let Err(e) = app.tunnel_state.save() {
        app.set_error(format!("Failed to save tunnels.toml: {e}"));
        return;
    }
    if let Err(e) = crate::state::write_tunnel_config(&cloned) {
        app.set_error(format!("Failed to write YAML: {e}"));
        return;
    }
    if let Err(e) = crate::daemon::reload_if_installed(&cloned).await {
        app.set_error(format!("Config saved; daemon reload failed: {e}"));
    }
    app.edit_sheet = None;
    app.input_mode = InputMode::Normal;
}

fn commit_input_edit(app: &mut App) {
    let InputMode::EditSheetInput { yaml_key, buffer } = &app.input_mode else { return };
    let yaml_key = yaml_key.clone();
    let buffer = buffer.clone();
    let Some(spec) = crate::cloudflared_options::find(&yaml_key) else { return };
    use crate::cloudflared_options::OptionKind;
    use crate::state::TunnelOptionValue;

    let parsed = match &spec.kind {
        OptionKind::Int { .. } => match buffer.trim().parse::<i64>() {
            Ok(n) => Some(TunnelOptionValue::Int(n)),
            Err(_) => {
                app.set_error(format!("Invalid integer: {}", buffer));
                return;
            }
        },
        OptionKind::String { .. } | OptionKind::Duration { .. } => {
            if buffer.trim().is_empty() {
                None
            } else {
                Some(TunnelOptionValue::String(buffer.trim().to_string()))
            }
        }
        OptionKind::List { .. } => {
            let items: Vec<String> = buffer
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if items.is_empty() {
                None
            } else {
                Some(TunnelOptionValue::List(items))
            }
        }
        _ => None,
    };

    // Validate.
    if let Some(v) = &parsed {
        if let Err(e) = crate::cloudflared_options::validate_value(spec, v) {
            app.set_error(format!("Validation failed: {e}"));
            return;
        }
    }

    if let Some(sheet) = app.edit_sheet.as_mut() {
        if let Some(row) = sheet.advanced_rows.iter_mut().find(|r| r.spec.yaml_key == yaml_key) {
            row.value = parsed;
            sheet.dirty = true;
        }
    }
}

fn commit_picker_edit(app: &mut App) {
    let InputMode::EditSheetPicker { yaml_key, cursor } = &app.input_mode else { return };
    let yaml_key = yaml_key.clone();
    let cursor = *cursor;
    let Some(spec) = crate::cloudflared_options::find(&yaml_key) else { return };
    let crate::cloudflared_options::OptionKind::Enum { choices, .. } = &spec.kind else { return };
    let choice = choices[cursor].to_string();

    if let Some(sheet) = app.edit_sheet.as_mut() {
        if let Some(row) = sheet.advanced_rows.iter_mut().find(|r| r.spec.yaml_key == yaml_key) {
            row.value = if choice.is_empty() {
                None
            } else {
                Some(crate::state::TunnelOptionValue::String(choice))
            };
            sheet.dirty = true;
        }
    }
}
```

If `App::set_error` doesn't exist, use whatever error-display mechanism the app has (a status line field, a toast queue, etc.). Trace it from existing error paths.

- [ ] **Step 5: Wire rendering in ui.rs**

Open `src/tui/ui.rs`. Find lines 101-102 (the `InputMode::EditTarget` and `InputMode::EditZone` match arms in the main render dispatch). Replace them with:

```rust
        InputMode::EditSheet
        | InputMode::EditSheetInput { .. }
        | InputMode::EditSheetPicker { .. }
        | InputMode::EditSheetConfirmDiscard => {
            if let Some(sheet) = app.edit_sheet.as_ref() {
                crate::tui::edit_sheet::render(f, f.area(), sheet);
            }
            render_sheet_modal_overlays(f, app);
        }
```

Then add near the bottom of `ui.rs`:

```rust
fn render_sheet_modal_overlays(f: &mut Frame, app: &App) {
    match &app.input_mode {
        InputMode::EditSheetInput { yaml_key, buffer } => {
            let area = centered_rect_ui(50, 20, f.area());
            f.render_widget(ratatui::widgets::Clear, area);
            let title = format!(" Edit: {} ", yaml_key);
            let block = ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .title(title);
            let inner = block.inner(area);
            f.render_widget(block, area);
            let p = ratatui::widgets::Paragraph::new(buffer.as_str())
                .wrap(ratatui::widgets::Wrap { trim: false });
            f.render_widget(p, inner);
        }
        InputMode::EditSheetPicker { yaml_key, cursor } => {
            let Some(spec) = crate::cloudflared_options::find(yaml_key) else { return };
            let crate::cloudflared_options::OptionKind::Enum { choices, .. } = &spec.kind else { return };
            let area = centered_rect_ui(30, 40, f.area());
            f.render_widget(ratatui::widgets::Clear, area);
            let block = ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .title(format!(" Choose: {} ", yaml_key));
            let inner = block.inner(area);
            f.render_widget(block, area);
            let items: Vec<ratatui::widgets::ListItem> = choices
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let display = if c.is_empty() { "(unset)" } else { c };
                    let marker = if i == *cursor { "› " } else { "  " };
                    ratatui::widgets::ListItem::new(format!("{marker}{display}"))
                })
                .collect();
            f.render_widget(ratatui::widgets::List::new(items), inner);
        }
        InputMode::EditSheetConfirmDiscard => {
            let area = centered_rect_ui(40, 15, f.area());
            f.render_widget(ratatui::widgets::Clear, area);
            let block = ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .title(" Discard changes? ");
            let inner = block.inner(area);
            f.render_widget(block, area);
            f.render_widget(
                ratatui::widgets::Paragraph::new("You have unsaved edits.\n\n[y]es  [n]o / Esc"),
                inner,
            );
        }
        _ => {}
    }
}

fn centered_rect_ui(percent_x: u16, percent_y: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
    use ratatui::layout::{Constraint, Direction, Layout};
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup[1])[1]
}
```

Also delete the now-unused `render_edit_dialog` and `render_edit_zone_dialog` functions (around lines 722 and 765). Their `EditTarget` / `EditZone` callers no longer exist.

- [ ] **Step 6: Update the status-bar help lines**

In `src/tui/ui.rs`, find lines 590-591 (the `InputMode::EditTarget` / `InputMode::EditZone` help strings). Delete them and add:

```rust
        InputMode::EditSheet => " Tab: switch pane   Ctrl+S: save   Esc: cancel".to_string(),
        InputMode::EditSheetInput { .. } => " Type value   Enter: confirm   Esc: cancel".to_string(),
        InputMode::EditSheetPicker { .. } => " ↑/↓: select   Enter: confirm   Esc: cancel".to_string(),
        InputMode::EditSheetConfirmDiscard => " y: discard   n/Esc: keep editing".to_string(),
```

- [ ] **Step 7: Build and fix any straggler references**

Run: `cd /tmp/ytunnel && cargo check`
Expected: may surface uses of the removed `EditTarget` / `EditZone` variants (like in help screens or match blocks). Delete or replace each. Keep iterating until clean.

- [ ] **Step 8: Run all tests**

Run: `cd /tmp/ytunnel && cargo test --lib -- --nocapture`
Expected: all pass.

- [ ] **Step 9: Manual TUI verification**

Run: `cd /tmp/ytunnel && cargo build --release && ./target/release/ytunnel` in a terminal.

Verify:
- Select a tunnel, press `e` — the two-tab sheet opens on Basic.
- Press `Tab` — switches to Advanced. See sections "Tunnel" and "Origin Request".
- Arrow down to "HTTP Host header", press Enter — text input appears.
- Type `google-spike.localhost`, press Enter — value shown on row.
- Ctrl+S — sheet closes. Inspect `~/.config/ytunnel/tunnel-configs/<tunnel>.yml`; confirm it contains `originRequest: { httpHostHeader: google-spike.localhost }` on the ingress rule.
- Re-open edit — the value is preserved.
- Select the row, press `d` — value cleared, YAML on next save no longer has the key.
- Press `Esc` after making changes — get discard-confirm dialog. `n` returns to sheet, `y` cancels.

*Manual verification only — ratatui doesn't have snapshot infrastructure in this project.*

- [ ] **Step 10: Commit**

```bash
cd /tmp/ytunnel && git add src/tui/ src/state.rs && git commit -m "tui: replace chained edit modals with two-tab sheet (Basic/Advanced)"
```

---

## Task 11: Update CHANGELOG

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add entry**

Open `CHANGELOG.md`. Under a new `## [Unreleased]` section (or the topmost version block, whichever the project uses), add:

```markdown
### Added
- **Advanced tunnel options**: new two-tab edit sheet (Basic / Advanced) lets you configure ~30 cloudflared options per tunnel, including `httpHostHeader`, `noTLSVerify`, `protocol`, `loglevel`, `retries`, `edge-ip-version`, and origin-request tuning. Settings persist in `tunnels.toml` and are written into the generated cloudflared YAML.
- Options take effect immediately: saving the edit sheet reloads the running daemon (via `launchctl kickstart -k` / `systemctl --user restart`).

### Changed
- Tunnel edit UI: replaces the previous target → zone modal chain with a single overlay sheet you can tab between.
- YAML generation now uses `serde_yaml` for structured output. Empty-options case produces the same shape as before.
```

- [ ] **Step 2: Commit**

```bash
cd /tmp/ytunnel && git add CHANGELOG.md && git commit -m "changelog: advanced tunnel options + edit sheet refactor"
```

---

## Self-Review Notes

**Spec coverage:** every section of the spec maps to a task —
- Data model → Task 2
- YAML generation → Tasks 3, 4, 5
- Curated registry → Task 6
- Ephemeral parity → Task 7
- Daemon reload → Task 8
- TUI (edit sheet) → Tasks 9, 10
- Backward compat → Task 2 test (`persistent_tunnel_deserializes_without_new_fields`)
- Legacy YAML shape unchanged → Task 3 test (`generate_yaml_matches_legacy_shape_when_no_options`)
- Testing approach → covered per-task

**Placeholder scan:** no TBDs, TODOs, or "add appropriate X" phrases. Every step has real code.

**Type consistency:** `TunnelOptionValue` used identically across state.rs, cloudflared_options.rs, edit_sheet.rs, and app.rs. `OptionScope`, `OptionKind`, `OptionSpec`, `EditSheetState`, `SheetTab`, `AdvancedRow` all defined once and referenced consistently.

**Deferred / follow-ups (documented in spec, not in this plan):**
- Ephemeral tunnels expose Advanced options via CLI flags (currently they get YAML parity via the shared helper but no CLI surface).
- Nested-key options like `warp-routing.enabled` (v2).
- Snapshot testing for TUI (needs `insta` infrastructure).
