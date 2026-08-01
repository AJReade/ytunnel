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

// Index of an editable field on the Basic tab.
// #[repr(u8)] allows `as u8` casts in tests to verify render order.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasicField {
    Target = 0,
    Zone = 1,
    AutoStart = 2,
    MetricsPort = 3,
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
    // Index of the currently-selected Basic tab field (0-3, maps to BasicField).
    pub basic_selected: usize,
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
            basic_selected: 0,
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

    // Move Basic tab selection down (bounded at last field, index 3).
    pub fn select_basic_next(&mut self) {
        if self.basic_selected < 3 {
            self.basic_selected += 1;
        }
    }

    // Move Basic tab selection up (bounded at 0).
    pub fn select_basic_prev(&mut self) {
        if self.basic_selected > 0 {
            self.basic_selected -= 1;
        }
    }
}

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs},
    Frame,
};

use crate::cloudflared_options::OptionKind;

// Render the edit sheet as a centered overlay.
pub fn render(f: &mut Frame, area: Rect, sheet: &EditSheetState) {
    let outer = centered_rect(70, 80, area);
    f.render_widget(Clear, outer);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Edit tunnel: {} ", sheet.tunnel_name));
    let inner = block.inner(outer);
    f.render_widget(block, outer);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

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

    match sheet.active_tab {
        SheetTab::Basic => render_basic(f, chunks[1], sheet),
        SheetTab::Advanced => render_advanced(f, chunks[1], sheet),
    }

    let help = match sheet.active_tab {
        SheetTab::Basic => "↑/↓: select   Enter: edit   Tab: switch pane   Ctrl+S: save   Esc: cancel",
        SheetTab::Advanced => "↑/↓: select   Enter: edit   d: clear   Tab: switch pane   Ctrl+S: save   Esc: cancel",
    };
    f.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn render_basic(f: &mut Frame, area: Rect, sheet: &EditSheetState) {
    let sel = sheet.basic_selected;
    let marker = |idx: usize| if idx == sel { "› " } else { "  " };

    let lines = vec![
        Line::from(vec![
            Span::raw(marker(0)),
            Span::styled("Target:       ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&sheet.target),
        ]),
        Line::from(vec![
            Span::raw(marker(1)),
            Span::styled("Zone:         ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{} (read-only)", sheet.zone_name),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::raw(marker(2)),
            Span::styled("Auto-start:   ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(if sheet.auto_start { "yes" } else { "no" }),
        ]),
        Line::from(vec![
            Span::raw(marker(3)),
            Span::styled("Metrics port: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(
                sheet.metrics_port.map(|p| p.to_string()).unwrap_or_else(|| "(auto)".into()),
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

    f.render_widget(List::new(items).block(Block::default().borders(Borders::NONE)), area);
}

fn value_to_display(v: &crate::state::TunnelOptionValue) -> String {
    use crate::state::TunnelOptionValue;
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
    fn select_basic_navigates_between_fields() {
        let mut s = EditSheetState::from_tunnel(&tunnel_with_options());
        assert_eq!(s.basic_selected, 0);
        s.select_basic_next();
        assert_eq!(s.basic_selected, 1);
        s.select_basic_next();
        s.select_basic_next();
        assert_eq!(s.basic_selected, 3);
        // Bounded at 3 (last field: MetricsPort).
        s.select_basic_next();
        assert_eq!(s.basic_selected, 3);
        s.select_basic_prev();
        assert_eq!(s.basic_selected, 2);
        // Bounded at 0.
        s.select_basic_prev();
        s.select_basic_prev();
        s.select_basic_prev();
        assert_eq!(s.basic_selected, 0);
    }

    #[test]
    fn basic_field_index_maps_to_enum() {
        // Sanity: the field order in the enum matches the render order.
        // If you reorder, the tests here and render_basic must both be updated.
        assert_eq!(BasicField::Target as u8, 0);
        assert_eq!(BasicField::Zone as u8, 1);
        assert_eq!(BasicField::AutoStart as u8, 2);
        assert_eq!(BasicField::MetricsPort as u8, 3);
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
