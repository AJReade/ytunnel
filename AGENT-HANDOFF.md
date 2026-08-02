# Agent Handoff: ytunnel Fork Work

**Written:** 2026-08-02
**Working directory:** `/Users/ar/Documents/the-garage/ytunnel`
**Fork:** https://github.com/AJReade/ytunnel (owner: AJReade)
**Upstream:** https://github.com/yetidevworks/ytunnel
**Main feature branch:** `advanced-tunnel-options`
**Second branch (bug fix):** `fix-log-tail-live`

---

## 0. Read This First

If you're the next agent picking this up:

1. Read this whole doc before touching code.
2. Also read `/Users/ar/Documents/the-garage/ytunnel/CLAUDE.md` — it has non-negotiable project conventions.
3. Read the design specs in `docs/superpowers/specs/` and plans in `docs/superpowers/plans/` — the current work grew out of those.
4. Check the state of the two PRs on the fork (PR #1 and #2, both AJReade/ytunnel).
5. Confirm the branch state locally matches the fork (`git log --oneline advanced-tunnel-options...upstream/master`).

**User is testing manually via `ytunnel` (installed to `/opt/homebrew/bin/ytunnel`).** Any UI change: rebuild release, `cp` the binary over, tell them to quit and relaunch. Don't push tests to CI expecting the user to review; they iterate live.

---

## 1. What This Fork Is

ytunnel is a TUI-first CLI for managing Cloudflare Tunnels (persistent tunnels with your own domain — like ngrok but self-hosted-DNS). The upstream repo lives at `yetidevworks/ytunnel`, homebrew-installed by the user.

This fork adds:

- **Advanced tunnel options**: curated registry of ~30 cloudflared config knobs (loglevel, protocol, httpHostHeader, noTLSVerify, retries, etc.) exposed via a new **Advanced** tab in the edit sheet. Settings persist in `tunnels.toml` and serialize into the per-tunnel cloudflared YAML.
- **Edit sheet UI overhaul**: replaces the previous chained modal (target → zone) with a two-tab overlay (Basic / Advanced). Yellow theming, inline field descriptions, `Ctrl+S` to save, Esc-with-confirm.
- **Scrollable log panel**: byte-offset tailing with a bounded 5000-line ring buffer; PgUp/PgDn/Home/End/Ctrl+U/Ctrl+D nav; auto-follow bottom until user scrolls up.
- **ngrok-dev log mode**: parses cloudflared debug output into compact per-request rows: `HH:MM:SS  METHOD  path  STATUS  bytes`. Third value of the new `Log mode` picker on Basic tab.
- **Live log tail**: 1-second poll (was: only on tunnel actions). Split off as its own PR because it's a bug fix.
- **Mouse-capture Tab-gating**: Focus toggle via Tab. When Logs are focused, mouse wheel scrolls the panel. When Tunnels focused, mouse capture is disabled so native text selection works.

The user is `ajreade406@gmail.com` (git identity), authenticated to GitHub as `AJReade`.

---

## 2. Current State

### Branches

| Branch | Purpose | Fork status | PR |
|---|---|---|---|
| `master` | Tracks upstream `yetidevworks/ytunnel:master` (b93fd26) | Clean | — |
| `advanced-tunnel-options` | Main feature work (25 commits ahead of master) | Pushed | [PR #1](https://github.com/AJReade/ytunnel/pull/1) |
| `fix-log-tail-live` | Just the live-tail fix (1 commit ahead of master) | Pushed | [PR #2](https://github.com/AJReade/ytunnel/pull/2) |

Both PRs target the **fork's own master**, NOT upstream. The user wanted to iterate on the fork first before deciding what to send upstream.

**KNOWN**: the fix-log-tail-live commit (`2658849`) is *also* still on `advanced-tunnel-options` (at the tip of PR #1's history) because a force-push attempt was blocked twice by the sandbox even after user approved with "A". User said they'd run it manually via `!` prefix but I don't have visibility into whether they did. If not, PR #1's diff includes the log-tail commit that PR #2 also carries. To fix, need to force-push `advanced-tunnel-options` to `4605d2e` (the commit before `2658849`).

### Test suite

`cargo test --bin ytunnel` — 57 tests, all passing as of last commit `78cf41e`. Breakdown:

- `state::tests` — TOML/YAML round-trip, backward compat, LogMode, migrations (~10)
- `cloudflared_options::tests` — registry validation, parse_duration, validate_value (~7)
- `tui::edit_sheet::tests` — state model, from_tunnel/apply, navigation bounds, zone-change staging (~12)
- `tui::log_tail::tests` — byte-offset tail, partial-line handling, truncation reset, ring eviction (~5)
- `tui::log_filter::tests` — parse_line, extract_kv, request/response pairing (~15)
- Plus pre-existing tests in other modules

### Warnings

7 warnings in the last release build. Most are dead-code on:

- `BasicField::AutoStart` and `BasicField::MetricsPort` (the enum variants are used by `InputMode::EditSheetBasicInput { field, ... }` when opened from the Advanced tab, but the compiler doesn't see the construction sites for these two — worth investigating with `#[allow(dead_code)]` on those variants, but not blocking)
- `log_filter::parse_line`, `format_ngrok`, `ParsedRequest` — superseded by `NgrokDevFilter`. Safe to delete; only kept to avoid churn during the parser refactor.

---

## 3. Chronological Work Log

This is roughly what happened, in order. Each bullet references the commit SHA.

### Phase 1: Advanced options + edit sheet rewrite (PR #1)

Original ask: add support for cloudflared config flags (specifically `--http-host-header google-spike.localhost` for local Phoenix dev with a Host-rewrite). Expanded into a full "Advanced tab with ~30 curated options" feature after brainstorming.

- `2b88b99` design spec — settled on: YAML-only (no CLI-arg passthrough), curated registry (no free-form editing), two-tab sheet (Basic / Advanced), no new dependencies.
- `9fe7f35` initial implementation plan
- `c476339` **plan v2**: revised after user pushed back on adding `serde_yaml` dependency. Switched to hand-written `format!`-based YAML generation. Rationale: ytunnel had 13 deps at the time, adding a YAML crate for a ~15-line YAML doc was disproportionate. Also `serde_yaml` itself is archived/unmaintained since 2024.
- `34c9916` `TunnelOptionValue` enum + `tunnel_options`/`origin_request` fields on `PersistentTunnel` with `#[serde(default)]` for backward compat
- `26e434e` render `tunnel_options` as top-level YAML keys
- `7c025cf` attach `origin_request` as `originRequest` on the ingress rule (once, never on catch-all)
- `1de3575` curated `cloudflared_options` registry (~30 entries)
- `b0f429c` `parse_duration` overflow hardening (`checked_mul` + `checked_add`)
- `6d4d402` extract `build_tunnel_yaml`; ephemeral tunnels now share the same generator
- `525bbbe` daemon `reload_if_installed` (launchctl kickstart / systemctl restart)
- `55230dc` new `edit_sheet` module with state model + tests
- `3f00e45` document `apply`'s registry-authoritative semantics + regression test
- `cd66a18` **big TUI wiring**: replace chained EditTarget/EditZone modals with two-tab sheet
- `6caedcb` Basic-tab editing (target / auto-start / metrics-port)
- `94a4512` fixes from code review: zone read-only, status_message clearing, skip no-op saves
- `14d46ba` CHANGELOG
- `74e6c47` polish: placeholder ghost-text in edit input + README keybindings
- `104de41` **restore zone editing** with DNS reconciliation (delete old CNAME, create new)
- `35d9c25` README reflects zone-picker restoration
- `c9ebd86` design spec + plan for log panel scroll + ngrok-dev (Phase 2)

### Phase 2: Log panel scroll + ngrok-dev (still on PR #1)

Grew out of user asking "how do the logs work" and being confused why cloudflared didn't show requests like ngrok does.

- `2203edc` `LogTail` byte-offset ring buffer + 5 tests
- `5c3754f` `log_filter` parser (initial version: request-only; response support came later)
- `18ae4b2` `LogMode` enum + `PersistentTunnel::log_mode` field + migration from `tunnel_options["loglevel"]`
- `974ba42` remove `loglevel` from Advanced OPTIONS registry
- `c502921` Log mode picker on Basic tab
- `c6c3d14` scrollable logs (retire `App::logs`, wire `LogTail`, PgUp/PgDn/Home/End/Ctrl+U/Ctrl+D)
- `282d8e7` wire ngrok-dev filter into render
- `022c2c9` CHANGELOG
- `6c902b4` **response pairing + focus mode**: extended NgrokDevFilter to pair request/response on connIndex (I had been wrong that cloudflared doesn't log responses — see mistakes below), added Focus::Tunnels / Focus::Logs with Tab toggle
- `5a370a6` restore log level colors (regressed during LogTail refactor) + tunnel name in title + wrap long lines
- `7f4c5ca` brighten DBG lines to white
- `d04e012` HTTP status coloring in raw logs + mouse click-to-focus + scroll routing
- `e519d34` **drop [pending] rows in ngrok-dev**; compact byte units (247B / 5.1K / 4.2M); tighter columns
- `a7838d3` **mouse capture Tab-gated**: capture only on when Logs focused, preserving native text selection when Tunnels focused (user rejected the click-to-focus model in favor of Tab-only)
- `0487378` **move Auto-start + Metrics port from Basic to Advanced** under a new `── ytunnel ──` section
- `78cf41e` `d` key resets Auto-start / Metrics port to defaults in Advanced

### Bug fix (PR #2)

- `aa6d78d` `tui: tail logs live (every 1s) instead of only on tunnel actions` — cherry-picked from `2658849` on the feature branch onto a fresh `fix-log-tail-live` branch off master

---

## 4. Key Design Decisions (and rationale)

### 4.1 No new runtime dependencies

**Decision:** hand-written YAML formatter, hand-written duration parser, hand-written log parser. Only added `tempfile` as a dev-dep for `LogTail` tests.

**Why:** ytunnel prides itself on being lean (13 runtime deps). Adding a YAML crate (~50KB + transitive `yaml-rust`) for a ~15-line YAML document is disproportionate. `serde_yaml` specifically is archived/unmaintained since 2024. The user explicitly pushed back on adding it when the plan initially proposed it.

**Cost:** more code to maintain in `state.rs`; slightly less structural test assertions (use `contains()` + `matches().count()` instead of `serde_yaml::from_str`).

### 4.2 Registry-authoritative apply

**Decision:** `EditSheetState::apply(&mut PersistentTunnel)` fully REBUILDS `tunnel.tunnel_options` and `tunnel.origin_request` from the sheet's `advanced_rows`. Any pre-existing map keys not present in `OPTIONS` are silently dropped.

**Why:** The design spec says "no free-form YAML editing / escape hatches — the curated pane is the interface." If a user hand-edits `tunnels.toml` to add an unknown key, it should be treated as user error, not preserved. Adding a new option means adding an OptionSpec entry.

**Cost:** users can't sneak in options not in the registry. If they need one we haven't exposed, they should ask (or the maintainer adds it). This is documented via a test (`apply_discards_keys_not_in_registry`) + code comment.

**Migration exception:** `migrate_loglevel_to_log_mode` in `state.rs` is the one place we DO promote legacy `tunnel_options["loglevel"]` → `log_mode: Debug`. This runs on load, preserving user intent across the schema change.

### 4.3 LogMode as a single first-class field

**Decision:** unified cloudflared `loglevel` + ngrok-style display filter into one user-facing `Log mode` picker with three values: `Default`, `Debug`, `ngrok-dev`.

**Why:** Debug and NgrokDev both require cloudflared at `debug` level (otherwise per-request lines aren't emitted). They differ only in how ytunnel displays the stream. Splitting these into two knobs (loglevel + display) would confuse users. One picker with 3 clear options is friendlier.

**Storage:** `PersistentTunnel::log_mode` is the source of truth. YAML generation derives `loglevel: debug` from it. `loglevel` was removed from the Advanced OPTIONS registry so users can't set it there and cause a conflict.

### 4.4 Zone read-only initially, then restored with DNS pipeline

**Decision timeline:**
- v1: Zone was read-only in Basic. Rationale: full DNS reconciliation is complex (delete old CNAME, refresh zone_id, regenerate hostname, create new CNAME) and originally out of scope.
- v2 (`104de41`): restored full zone editing with the DNS pipeline wired into `save_edit_sheet`. Reused logic from the old `edit_tunnel_op` function (which was `#[allow(dead_code)]`'d during the sheet rewrite).

**Why the reversal:** user's Basic tab was previously a full edit UI. Making it partially broken is a regression. Doing DNS reconciliation right is only ~30 lines and reuses existing `cloudflare::Client::ensure_dns_record`.

### 4.5 Advanced tab has a "ytunnel" section

**Decision timeline:**
- Original design: Basic tab had Target / Zone / Auto-start / Metrics port. Advanced tab was purely for cloudflared options.
- User feedback (late): "Metrics port and Auto-start shouldn't be on Basic — they aren't cloudflared options, and the old app didn't have them there." Moved them to a new `── ytunnel ──` section at the top of Advanced.
- After the move (`0487378`): Basic = Target / Zone / Log mode. Advanced starts with ytunnel section (Auto-start, Metrics port), then Tunnel section, then Origin Request section.

**Why:** cognitive separation. Basic = "the identity of the tunnel." Advanced = "how it behaves." Auto-start and Metrics port are behavior. Also matches the original app's mental model (Auto-start had its own `A` keybinding, Metrics port was never user-editable in the TUI).

### 4.6 Mouse capture Tab-gated, not always-on

**Decision timeline:**
- v1: enable mouse capture at startup, handle click-to-focus + scroll. Preserved wheel functionality on both panes.
- User pushback: "changing panes should only be done using tab not the mouse." Also broke native text selection.
- v2 (`a7838d3`): mouse capture starts OFF. Tab toggles focus AND toggles mouse capture on transition. When Focus::Tunnels → capture OFF (native text selection works). When Focus::Logs → capture ON (wheel scrolls logs).

**Why:** user was right — click-to-focus is fiddly and breaks the "click-drag to select text" mental model that terminal apps preserve. Tab-toggle is explicit and consistent. Text selection now works whenever focus is on Tunnels (which is the default), matching the original app's v0.4.0 "removed mouse capture for text selection" decision.

### 4.7 ngrok-dev drops [pending] entries

**Decision timeline:**
- v1: `NgrokDevFilter::flush_pending()` emitted `[pending]` rows for requests that had no matching response.
- User feedback: Phoenix live_reload / long-polling apps produce dozens of never-responded requests. The panel flooded with `[pending]`.
- v2 (`e519d34`): `flush_pending()` now silently drops. Users see only completed request→response pairs, matching ngrok.

**Why:** cloudflared doesn't log a "close" event for long-polling / WebSocket / SSE. Showing every unmatched request as pending is noise, not signal. If users want to see raw request lines, switch to `Debug` mode.

### 4.8 ngrok-dev correlation is FIFO-per-connIndex

**Decision:** `NgrokDevFilter` maps `connIndex → VecDeque<PendingRequest>` and pops-front on each response line.

**Why:** cloudflared's log format includes `connIndex` on both request and response lines. Assuming responses come back in the same order as requests on the same connection is *usually* correct for HTTP/1.1. For HTTP/2 or QUIC with multiplexing, this could pair the wrong request with the wrong response.

**Known limitation:** the parser DOESN'T look at HTTP/2 stream IDs (cloudflared doesn't log them clearly). Correlation may be wrong for heavy multiplexed workloads. In practice for typical dev-tunnel usage (single Phoenix app, small number of concurrent requests), it looks right.

### 4.9 File layout

- **`src/state.rs`** (~700 lines): persistence, YAML generation, migrations. Grew significantly in this fork — was ~275 lines originally.
- **`src/cloudflared_options.rs`** (~500 lines, new): the curated registry.
- **`src/tui/edit_sheet.rs`** (~600 lines, new): sheet state + rendering.
- **`src/tui/log_tail.rs`** (~200 lines, new): byte-offset ring buffer.
- **`src/tui/log_filter.rs`** (~500 lines, new): parser + NgrokDevFilter.
- **`src/tui/app.rs`** (~2900 lines): main app state + event loop. Grew significantly — was ~2600. Natural extraction of edit_sheet input handlers into a separate module was flagged as follow-up but not done.
- **`src/tui/ui.rs`** (~1000 lines): render dispatch + individual pane renders.
- **`src/daemon.rs`** (~700 lines): launchd/systemd plumbing. Added `reload_if_installed`.

---

## 5. Testing

Repo is a **binary crate** (no `[lib]`). Use `cargo test --bin ytunnel` or just `cargo test` (the `--lib` flag from the plan snippets errors "no library targets" — use `--bin ytunnel` instead if you need to be explicit).

Test locations:
- `src/state.rs::mod tests`
- `src/cloudflared_options.rs::mod tests`
- `src/tui/edit_sheet.rs::mod tests`
- `src/tui/log_tail.rs::mod tests` (uses `tempfile` dev-dep)
- `src/tui/log_filter.rs::mod tests`

**No TUI snapshot testing.** Rendering was verified manually by the user. If you want snapshot tests, `insta` is the standard crate — would add another dev-dep though.

**Testing style used**: TDD for state / parser code (write failing test → verify failure → implement → verify pass). Less strict for UI wiring (build & manual verify).

---

## 6. Build & Install

**Toolchain**: Rust stable (>= 1.85 because a transitive dep `line-clipping 0.3.5` requires edition2024). The repo's `Cargo.toml` pins to 1.84.1 but this doesn't work; `rustup override set stable` was applied to the repo directory. If you re-clone, redo the override.

**Build**: `cargo build --release`

**Install**: `cp target/release/ytunnel /opt/homebrew/bin/ytunnel`

The user has ytunnel installed via Homebrew. `/opt/homebrew/bin/ytunnel` was originally a symlink to the Cellar-installed 0.8.0 binary; my first install replaced the symlink with a plain file copy. If the user runs `brew reinstall ytunnel`, brew will restore its own symlink and undo our install.

**CLAUDE.md says**: run `cargo install --path .` after code changes. This puts the binary in `~/.cargo/bin/ytunnel` (not `/opt/homebrew/bin/ytunnel`). Either works — the user tested via the Homebrew path, so we've been cp-ing there. If you switch to `cargo install`, ensure `~/.cargo/bin` comes before `/opt/homebrew/bin` in PATH, or the wrong binary will run.

**Testing UI changes**: the user must quit ytunnel (`q`) and relaunch to pick up new binaries. Don't tell them "restart" without being explicit about this.

**Config location on macOS**: `~/Library/Application Support/ytunnel/`, NOT `~/.config/ytunnel/` (the `dirs` crate returns the platform-appropriate config dir; on macOS this is Application Support). Contents:
- `tunnels.toml` — all persistent tunnels
- `tunnel-configs/<name>.yml` — generated cloudflared configs
- `logs/<name>.log` — cloudflared stdout/stderr per tunnel
- `<tunnel-id>.json` — cloudflared credentials per tunnel

---

## 7. Conventions (from CLAUDE.md)

Non-negotiable, per the project's CLAUDE.md at repo root:

- Use `//` comments, NOT `///` (except for documenting genuinely public API surfaces — but this is a binary crate, so basically never)
- Do NOT include `Co-Authored-By: Claude` (or any similar co-author trailer) in git commit messages
- After code changes, `cargo install --path .` (we've been using `cp` instead but same intent)

---

## 8. Known Issues / Deferred Work

### High priority (should fix before merging upstream)

- **PR #1 still contains the log-tail commit** that should be exclusively on PR #2. To fix: force-push `advanced-tunnel-options` to `4605d2e` (the commit before `2658849`). Sandbox blocks this; user needs to run it manually via `!` prefix.
- **Dead-code warnings** on `log_filter::parse_line`, `format_ngrok`, `ParsedRequest` — these are the pre-refactor implementations superseded by `NgrokDevFilter`. Safe to delete outright.
- **`BasicField::AutoStart` and `BasicField::MetricsPort` dead-code warnings** — the variants are still used at input-modal construction sites in `app.rs` (opened from Advanced now), but the compiler flags them because it doesn't see the construction path clearly. Consider `#[allow(dead_code)]` on the enum, or refactor.

### Medium priority

- **`src/tui/app.rs` is ~2900 lines.** Natural extraction of edit_sheet input handlers (`save_edit_sheet`, `commit_input_edit`, `commit_picker_edit`, `commit_basic_input_edit`, `handle_mouse`) to a new `src/tui/edit_handlers.rs` would save ~200 lines and make app.rs easier to reason about. Not blocking; flagged as follow-up during code review.
- **QUIC/HTTP2 multiplexed request/response correlation** in `NgrokDevFilter` — currently FIFO per `connIndex`. Muxed streams could pair wrong request with wrong response. Not observed in practice for typical tunnels, but could bite heavy multi-request apps.
- **Log rotation**: cloudflared doesn't rotate logs; the file at `~/Library/Application Support/ytunnel/logs/<name>.log` grows unbounded. `LogTail` buffer is bounded in memory (5000 lines), but disk grows forever. Long-running tunnels could accumulate GB-scale log files.

### Lower priority / follow-ups

- **Ephemeral tunnels** (`ytunnel run`) share the YAML generator via `build_tunnel_yaml` but don't expose Advanced options via CLI flags. To add: extend `Cli` in `src/cli.rs` with `--log-mode`, `--http-host-header`, etc.
- **Snapshot testing for TUI**: no snapshot infra in the repo. Adding `insta` (dev-dep) would let us assert on rendered layouts. Not strictly needed but would catch UI regressions.
- **Nested-key YAML options** (e.g. `warp-routing.enabled`, `originRequest.access.required`) not supported by the registry — the current YAML generator emits flat top-level keys + a single-level `originRequest` block. Adding nested support means changing `OptionSpec::yaml_key` to accept dotted paths and updating `write_yaml_option` to build nested maps.
- **Log search** (`/`) inside the panel — natural next feature. Would need scrolling to found matches, highlight.
- **Filter/grep in raw log mode** — pipe raw lines through a user-supplied regex before display.

---

## 9. Mistakes I Made (so you don't repeat them)

### `/tmp/ytunnel` got wiped mid-session

**What happened:** initially cloned to `/tmp/ytunnel`. The macOS periodic cleaner (or similar) wiped `/tmp` during a long session, losing the plan v2 doc which hadn't been pushed yet. Had to reclone from the fork.

**Lesson:** use durable paths (`~/`). Never work in `/tmp` for anything longer than a few minutes. The user finally moved the repo to `~/Documents/the-garage/ytunnel` — respect this.

### Wrong-branch commit

**What happened:** while cleaning up after the reclone, I committed the log-panel spec + plan on `fix-log-tail-live` instead of `advanced-tunnel-options`. Had to cherry-pick to the correct branch and reset the wrong one.

**Lesson:** always `git branch --show-current` before committing multi-file changes. Especially after `git checkout` operations.

### Force-push denials

**What happened:** the sandbox blocked `--force-with-lease` twice, even after the user approved via "A". The action was flagged as "rewrites remote history on a branch with an open PR". Had to hand it back to the user to run manually.

**Lesson:** if force-pushing to a branch with an open PR, warn the user in advance and have them run the push themselves. Sandbox permission rules are strict here.

### Assumed cloudflared didn't log response status

**What happened:** looked at an early log sample that only had request lines. Concluded cloudflared debug doesn't log response status. Designed the ngrok-dev parser to only handle requests. User later hit a real workload and their logs clearly showed `DBG 200 OK content-length=246 ...` response lines. Had to rewrite the parser to handle request+response pairing.

**Lesson:** don't generalize from a small log sample. If unsure, ask the user to run at debug for a minute and produce a richer sample.

### `[pending]` rows flooded the panel

**What happened:** initial NgrokDevFilter emitted `[pending]` for any request without a matching response, sorted at end of buffer. Looked reasonable on paper. In practice, the user's Phoenix live_reload app produced dozens of long-polling requests, all showing as pending. Panel became unusable.

**Lesson:** think about what happens with realistic workloads before shipping UI. Ngrok solved this by having stateful per-request rows updated over time; we don't have that model, so we chose to drop unmatched entirely.

### Log level colors dropped during refactor

**What happened:** the LogTail refactor (`c6c3d14`) rewrote `render_logs` and lost the ERR/WRN/INF color styling that was there before. User noticed immediately (`Now it's just white`). Had to restore.

**Lesson:** when refactoring rendering code, diff against the pre-change version and audit for lost styling / behavior. Or just keep the styling logic in a helper function that gets carried along.

### Mouse capture default choice

**What happened:** initial mouse implementation enabled capture at startup for click-to-focus + scroll. This broke text selection. User rejected the design ("changing panes should only be done using tab") and wanted native text selection preserved. Had to redesign to Tab-gated capture.

**Lesson:** if the project explicitly disabled a feature previously (v0.4.0 removed mouse capture for text selection), understand WHY before re-enabling it. Preserve the invariant that motivated the removal.

### Basic tab placement of auto-start/metrics-port

**What happened:** initial design put auto_start and metrics_port on Basic. User later asked why they were there ("wasn't in the original app"). Rationale had seemed reasonable ("everything editable in one place") but user disagreed and asked to move them to Advanced.

**Lesson:** for placement decisions in polished apps, defer more to the existing UX than to "logical" reorganization. The original app had auto_start on a keybinding and metrics_port not user-editable — respect that as a signal.

### Tests referring to removed `loglevel` option

**What happened:** removed `loglevel` from the OPTIONS registry (`974ba42`) but the test `validate_enum_membership` used `find("loglevel").unwrap()`. Only caught it on the next full `cargo test` run.

**Lesson:** when removing registry entries, `grep -n "loglevel"` across the whole codebase before pushing. Same for any other removed identifier.

---

## 10. Nice-to-haves (Future Ideas That Came Up)

These aren't blocking anything, but the user or I noted them at various points:

- **Search inside log panel** (`/` keybinding, with `n` / `N` for next/prev)
- **Log rotation** (LogTail could truncate the on-disk file when it hits N MB, or ytunnel could rotate on startup)
- **Response status/duration correlation improvement** for HTTP/2 (needs stream ID from cloudflared logs — may not be available)
- **Extract edit_sheet input handlers to a separate module** — app.rs is bloated
- **Snapshot tests for TUI** via `insta` dev-dep
- **Ephemeral CLI exposure** — `ytunnel run` should accept the Advanced options via flags
- **Config file editor** — keybinding to open `tunnels.toml` in `$EDITOR` from the TUI (careful: our `apply` is registry-authoritative, so hand-edits get clobbered on next save)
- **Add more cloudflared options** to the registry — we skipped some (`credentials-file`, `warp-routing.enabled`, `token`, `token-file`, ICMP source addrs, DNS resolver overrides). Some don't apply to normal use; others might be requested.
- **Toggle-mouse-capture keybinding** (`M`?) as an on-demand escape hatch when user needs to select text while focus is on Logs
- **Show a "modified" indicator** in the tunnel list when a tunnel has unsaved edits open in the sheet
- **Diff view before Ctrl+S** — show what YAML lines will change before saving. Would help with confidence.

---

## 11. User Preferences (patterns observed)

- **Prefers concise responses.** No unnecessary preamble ("Great!", "Certainly!"). State what changed / what happened, get out.
- **Prefers action over meta-discussion.** Auto mode is active. If a decision is small, make it and note it. Only escalate genuine forks (like "add serde_yaml or not").
- **Doesn't like unnecessary dependencies.** Push back on your own instinct to add crates. Every dep is a trust surface.
- **Wants honest reporting.** If something wasn't tested manually, say so. Don't oversell.
- **Iterates via manual testing.** They quit and relaunch ytunnel to see changes. Don't rely on `cargo test` alone.
- **Notices regressions.** They'll flag things like "this used to be there" — respect that. The original codebase had specific choices.
- **Uses voice-to-text.** Expect typos, contractions, word substitutions. Recent examples: "grarage" → garage, "M Grupp" → ngrok, "Angro" → ngrok, "wide-tunnel" → ytunnel, "cloud-fledged" → cloudflared. Interpret charitably.
- **Splits concerns.** When asked, they'll insist a bug fix be its own PR rather than tangled with a feature branch. Watch for this signal.
- **Doesn't like inflated changelogs.** Match the concise style of the existing CHANGELOG.md — 1-line per feature, plain English, not marketing.
- **Prefers Tab-based navigation over mouse in TUIs.** Any mouse feature should be an addition, not the primary path.

---

## 12. Environment Cheatsheet

- macOS Darwin 25.4.0, homebrew installed, `/opt/homebrew/bin` in PATH
- Rust: stable (rustup override set on the repo dir)
- `cargo`: source `~/.cargo/env` before running if PATH is minimal
- git identity in this repo: `ar <ajreade406@gmail.com>` (set locally on the repo)
- GitHub: authed as `AJReade` via `gh auth`
- Fork remote: `origin` → `git@github.com:AJReade/ytunnel.git`
- Upstream remote: `upstream` → `git@github.com:yetidevworks/ytunnel.git`
- cloudflared version at time of writing: 2026.7.3

---

## 13. First Actions for the Next Agent

If you're picking this up cold:

1. **Verify build**: `cd /Users/ar/Documents/the-garage/ytunnel && source ~/.cargo/env && cargo test` — should be 57 tests passing.
2. **Verify PR state**: `gh pr list --repo AJReade/ytunnel` — should show PR #1 and #2.
3. **Verify installed binary**: `ytunnel --version` — should print `ytunnel 0.8.0` (Cargo.toml hasn't been version-bumped despite all the new features).
4. **Verify config is intact**: `ls ~/Library/Application\ Support/ytunnel/` — user has `google-spike` and `lockscreen-shopify-dev` tunnels.
5. **Decide what's next.** Options from the user's known interests:
   - Get PR #1 into a clean state (force-push to drop the log-tail commit if the user hasn't done it)
   - Version-bump `Cargo.toml` (currently still 0.8.0)
   - Address deferred items above (dead-code cleanup, module extraction)
   - Wait for user direction — they've been driving each iteration based on manual testing feedback

---

## 14. Files You'll Care About Most

Read in this order to understand the system:

1. `README.md` — user-facing intro
2. `CLAUDE.md` — non-negotiable conventions
3. `docs/superpowers/specs/2026-08-01-ytunnel-advanced-yaml-options-design.md` — spec for Phase 1
4. `docs/superpowers/specs/2026-08-01-log-panel-scroll-and-ngrok-dev-design.md` — spec for Phase 2
5. `src/state.rs` — data model + YAML gen
6. `src/cloudflared_options.rs` — the registry
7. `src/tui/edit_sheet.rs` — sheet state + rendering
8. `src/tui/log_tail.rs` + `src/tui/log_filter.rs` — log pipeline
9. `src/tui/app.rs` — main loop (biggest, most tangled)
10. `src/tui/ui.rs` — render dispatch
11. `CHANGELOG.md` — user-facing summary of the fork's additions

Good luck.
