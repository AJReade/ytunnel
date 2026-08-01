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
    // The originally-loaded zone id/name, so the save flow can detect changes
    // and trigger DNS reconciliation.
    pub original_zone_id: String,
    pub original_hostname: String,
    // Staged new zone selection. When None, no change vs. original.
    pub pending_zone: Option<crate::config::ZoneConfig>,
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
            original_zone_id: tunnel.zone_id.clone(),
            original_hostname: tunnel.hostname.clone(),
            pending_zone: None,
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
        tunnel.auto_start = self.auto_start;
        tunnel.metrics_port = self.metrics_port;

        if let Some(zone) = &self.pending_zone {
            tunnel.zone_id = zone.id.clone();
            tunnel.zone_name = zone.name.clone();
            tunnel.hostname = format!("{}.{}", tunnel.name, zone.name);
        }

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
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            format!(" Edit Tunnel: {} ", sheet.tunnel_name),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))
        .padding(ratatui::widgets::Padding::new(2, 2, 1, 1));
    let inner = block.inner(outer);
    f.render_widget(block, outer);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // tabs
            Constraint::Min(1),    // content
            Constraint::Length(2), // description / help text for the selected row
            Constraint::Length(1), // keybinding help
        ])
        .split(inner);

    let titles = vec![Line::from("Basic"), Line::from("Advanced")];
    let selected = match sheet.active_tab {
        SheetTab::Basic => 0,
        SheetTab::Advanced => 1,
    };
    let tabs = Tabs::new(titles)
        .select(selected)
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
        .divider("  ")
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    f.render_widget(tabs, chunks[0]);

    match sheet.active_tab {
        SheetTab::Basic => render_basic(f, chunks[1], sheet),
        SheetTab::Advanced => render_advanced(f, chunks[1], sheet),
    }

    // Context line: description of the currently-selected row.
    let context = match sheet.active_tab {
        SheetTab::Basic => basic_field_description(sheet.basic_selected),
        SheetTab::Advanced => sheet
            .advanced_rows
            .get(sheet.selected_row)
            .map(|row| row.spec.description)
            .unwrap_or(""),
    };
    f.render_widget(
        Paragraph::new(context)
            .style(Style::default().fg(Color::Yellow))
            .wrap(ratatui::widgets::Wrap { trim: true }),
        chunks[2],
    );

    let help = match sheet.active_tab {
        SheetTab::Basic => "↑/↓: select   Enter: edit   Tab: switch pane   Ctrl+S: save   Esc: cancel",
        SheetTab::Advanced => "↑/↓: select   Enter: edit   d: clear   Tab: switch pane   Ctrl+S: save   Esc: cancel",
    };
    f.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
        chunks[3],
    );
}

// One-line description shown below the Basic-tab content for the selected row.
fn basic_field_description(idx: usize) -> &'static str {
    match idx {
        0 => "Local URL cloudflared should forward requests to (e.g. http://localhost:3000).",
        1 => "Cloudflare zone hosting the tunnel hostname. Enter to pick a zone — DNS records are reconciled automatically on save.",
        2 => "Auto-start on login via launchd (macOS) / systemd (Linux). Enter toggles.",
        3 => "Local port for cloudflared's Prometheus metrics endpoint. Blank = auto-assigned.",
        _ => "",
    }
}

fn render_basic(f: &mut Frame, area: Rect, sheet: &EditSheetState) {
    let sel = sheet.basic_selected;
    let marker_span = |idx: usize| {
        let (glyph, color) = if idx == sel {
            ("› ", Color::Yellow)
        } else {
            ("  ", Color::DarkGray)
        };
        Span::styled(glyph, Style::default().fg(color))
    };
    let label_style = Style::default().add_modifier(Modifier::BOLD);
    let value_style = Style::default().fg(Color::Green);

    let lines = vec![
        Line::from(vec![
            marker_span(0),
            Span::styled("Target:       ", label_style),
            Span::styled(&sheet.target, value_style),
        ]),
        {
            let (zone_text, zone_style, suffix) = match &sheet.pending_zone {
                Some(z) => (
                    z.name.clone(),
                    Style::default().fg(Color::Green),
                    Some(Span::styled("  (staged — save to apply)", Style::default().fg(Color::DarkGray))),
                ),
                None => (
                    sheet.zone_name.clone(),
                    Style::default(),
                    None,
                ),
            };
            let mut zone_spans = vec![
                marker_span(1),
                Span::styled("Zone:         ", label_style),
                Span::styled(zone_text, zone_style),
            ];
            if let Some(s) = suffix {
                zone_spans.push(s);
            }
            Line::from(zone_spans)
        },
        Line::from(vec![
            marker_span(2),
            Span::styled("Auto-start:   ", label_style),
            Span::styled(
                if sheet.auto_start { "yes" } else { "no" },
                value_style,
            ),
        ]),
        Line::from(vec![
            marker_span(3),
            Span::styled("Metrics port: ", label_style),
            Span::styled(
                sheet.metrics_port.map(|p| p.to_string()).unwrap_or_else(|| "(auto)".into()),
                value_style,
            ),
        ]),
    ];

    // Summary of configured Advanced options below the four Basic fields.
    let mut all_lines = lines;
    all_lines.push(Line::from(""));
    all_lines.push(Line::from(Span::styled(
        "── Configured Advanced options ──",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));

    let mut set_rows: Vec<&AdvancedRow> = sheet
        .advanced_rows
        .iter()
        .filter(|r| r.value.is_some())
        .collect();
    // Group by scope for readability: Tunnel first, then OriginRequest.
    set_rows.sort_by_key(|r| match r.spec.scope {
        OptionScope::Tunnel => 0,
        OptionScope::OriginRequest => 1,
    });

    if set_rows.is_empty() {
        all_lines.push(Line::from(Span::styled(
            "(none — Tab to Advanced to configure cloudflared options)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let mut last_scope: Option<OptionScope> = None;
        for row in set_rows {
            if Some(row.spec.scope) != last_scope {
                let header = match row.spec.scope {
                    OptionScope::Tunnel => "  Tunnel:",
                    OptionScope::OriginRequest => "  Origin Request:",
                };
                all_lines.push(Line::from(Span::styled(
                    header,
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                )));
                last_scope = Some(row.spec.scope);
            }
            let value_display = row
                .value
                .as_ref()
                .map(value_to_display)
                .unwrap_or_default();
            all_lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(row.spec.yaml_key, label_style),
                Span::raw(" = "),
                Span::styled(value_display, value_style),
            ]));
        }
    }

    f.render_widget(Paragraph::new(all_lines), area);
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

        let is_selected = idx == sheet.selected_row;
        let selected_marker = if is_selected { "› " } else { "  " };
        let value_style = match &row.value {
            Some(_) => Style::default().fg(Color::Green),
            None => Style::default().fg(Color::DarkGray),
        };
        let mut line = Line::from(vec![
            Span::styled(
                selected_marker,
                Style::default().fg(if is_selected { Color::Yellow } else { Color::DarkGray }),
            ),
            Span::styled(
                format!("{:<24}", row.spec.display_name),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(value_display, value_style),
        ]);
        if is_selected {
            line = line.style(Style::default().bg(Color::Rgb(40, 40, 40)));
        }
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
            log_mode: crate::state::LogMode::Default,
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
    fn from_tunnel_stores_original_zone_for_change_detection() {
        let tunnel = tunnel_with_options();
        let sheet = EditSheetState::from_tunnel(&tunnel);
        assert_eq!(sheet.original_zone_id, tunnel.zone_id);
        assert_eq!(sheet.original_hostname, tunnel.hostname);
        assert!(sheet.pending_zone.is_none());
    }

    #[test]
    fn apply_writes_pending_zone_and_recomputes_hostname() {
        use crate::config::ZoneConfig;
        let tunnel = tunnel_with_options();
        let mut sheet = EditSheetState::from_tunnel(&tunnel);
        sheet.pending_zone = Some(ZoneConfig {
            id: "new-zone-id".into(),
            name: "newdomain.com".into(),
        });

        let mut applied = tunnel.clone();
        sheet.apply(&mut applied);
        assert_eq!(applied.zone_id, "new-zone-id");
        assert_eq!(applied.zone_name, "newdomain.com");
        assert_eq!(applied.hostname, "demo.newdomain.com");
    }

    #[test]
    fn apply_with_no_pending_zone_preserves_zone_fields() {
        let tunnel = tunnel_with_options();
        let sheet = EditSheetState::from_tunnel(&tunnel);
        let mut applied = tunnel.clone();
        sheet.apply(&mut applied);
        assert_eq!(applied.zone_id, tunnel.zone_id);
        assert_eq!(applied.zone_name, tunnel.zone_name);
        assert_eq!(applied.hostname, tunnel.hostname);
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
            log_mode: crate::state::LogMode::Default,
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
