// State + rendering for the two-tab tunnel edit sheet.
//
// Basic tab: existing per-tunnel fields (target, zone, auto_start, metrics_port).
// Advanced tab: curated cloudflared options from crate::cloudflared_options::OPTIONS.

use std::collections::BTreeMap;

use crate::cloudflared_options::{OptionScope, OptionSpec, OPTIONS};
use crate::state::{PersistentTunnel, TunnelOptionValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetTab {
    Basic,
    Advanced,
}

// A single editable row in the Advanced tab.
// One row per entry in OPTIONS, in registry order.
#[derive(Debug, Clone)]
pub struct AdvancedRow {
    pub spec: &'static OptionSpec,
    // Current value if set; None means "use cloudflared default".
    pub value: Option<TunnelOptionValue>,
}

// Full state of the edit sheet.
#[derive(Debug, Clone)]
pub struct EditSheetState {
    pub tunnel_name: String,
    pub active_tab: SheetTab,
    pub target: String,
    pub zone_name: String,
    pub auto_start: bool,
    pub metrics_port: Option<u16>,
    pub advanced_rows: Vec<AdvancedRow>,
    pub selected_row: usize,
    // True when any field diverges from the original tunnel.
    pub dirty: bool,
}

impl EditSheetState {
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

    // Apply sheet state back onto a PersistentTunnel.
    //
    // Registry-authoritative semantics: both `tunnel_options` and
    // `origin_request` are rebuilt from `advanced_rows`. Any pre-existing keys
    // that don't correspond to an entry in `OPTIONS` are dropped. This is
    // intentional per the design spec (no free-form YAML editing) — options
    // must be added to the registry to be preserved through an edit cycle.
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
        tunnel.tunnel_options.insert("loglevel".into(), TunnelOptionValue::String("debug".into()));
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

        let loglevel_row = sheet.advanced_rows.iter().find(|r| r.spec.yaml_key == "loglevel").unwrap();
        assert!(matches!(&loglevel_row.value, Some(TunnelOptionValue::String(s)) if s == "debug"));

        let host_row = sheet.advanced_rows.iter().find(|r| r.spec.yaml_key == "httpHostHeader").unwrap();
        assert!(matches!(&host_row.value, Some(TunnelOptionValue::String(s)) if s == "foo.local"));

        let retries_row = sheet.advanced_rows.iter().find(|r| r.spec.yaml_key == "retries").unwrap();
        assert!(retries_row.value.is_none());
    }

    #[test]
    fn apply_rebuilds_option_maps() {
        let tunnel = tunnel_with_options();
        let mut sheet = EditSheetState::from_tunnel(&tunnel);

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
        assert!(matches!(applied.tunnel_options.get("retries"), Some(TunnelOptionValue::Int(10))));
        assert!(applied.origin_request.contains_key("httpHostHeader"));
    }

    #[test]
    fn clear_selected_removes_value_and_marks_dirty() {
        let tunnel = tunnel_with_options();
        let mut sheet = EditSheetState::from_tunnel(&tunnel);
        let idx = sheet.advanced_rows.iter().position(|r| r.spec.yaml_key == "loglevel").unwrap();
        sheet.selected_row = idx;

        sheet.clear_selected();
        assert!(sheet.advanced_rows[idx].value.is_none());
        assert!(sheet.dirty);
    }

    #[test]
    fn toggle_tab_alternates() {
        let mut s = EditSheetState::from_tunnel(&tunnel_with_options());
        assert_eq!(s.active_tab, SheetTab::Basic);
        s.toggle_tab();
        assert_eq!(s.active_tab, SheetTab::Advanced);
        s.toggle_tab();
        assert_eq!(s.active_tab, SheetTab::Basic);
    }

    #[test]
    fn apply_discards_keys_not_in_registry() {
        // A tunnel that has a `tunnel_options`/`origin_request` key our registry
        // doesn't know about (e.g. hand-edited tunnels.toml, or an older ytunnel
        // version that supported an option we've since removed).
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
        tunnel.tunnel_options.insert(
            "some-future-flag".into(),
            TunnelOptionValue::Bool(true),
        );
        tunnel.origin_request.insert(
            "unknownOriginKnob".into(),
            TunnelOptionValue::String("x".into()),
        );

        // Sheet built from the tunnel does NOT contain the unknown keys as rows
        // (rows come from OPTIONS, not from the tunnel's maps).
        let sheet = EditSheetState::from_tunnel(&tunnel);
        assert!(sheet.advanced_rows.iter().all(|r| r.spec.yaml_key != "some-future-flag"));
        assert!(sheet.advanced_rows.iter().all(|r| r.spec.yaml_key != "unknownOriginKnob"));

        // Applying the sheet back onto the tunnel drops those unknown keys.
        sheet.apply(&mut tunnel);
        assert!(!tunnel.tunnel_options.contains_key("some-future-flag"));
        assert!(!tunnel.origin_request.contains_key("unknownOriginKnob"));
    }
}
