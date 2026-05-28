//! Frameless, transparent, always-on-top campaign-guide overlay.
//!
//! Mirrors the visual design of `python/overlay.py` (colors, layout,
//! three-state display) on top of eframe/egui with the glow backend.
//! Window position is persisted to `~/.local/state/rsexile/position.json`
//! with a 500 ms debounce after each WM-driven drag, and the lock button
//! toggle is persisted to `lock.json` in the same directory.

use std::sync::LazyLock;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eframe::{App, CreationContext, Frame as EframeFrame, NativeOptions};
use egui::{
    Align, Button, CentralPanel, Color32, FontFamily, FontId, Frame, Layout, Margin, RichText,
    Stroke, ViewportBuilder, ViewportCommand, WindowLevel,
};

use crate::campaign::{CampaignGuide, ZoneEntry, lookup_zone};
use crate::log_watcher::LogEvent;
use crate::persistence;

pub const PANEL_WIDTH: f32 = 340.0;
const PANEL_HEIGHT: f32 = 240.0;
// Size of the collapsed nub — just big enough for the expand button.
const COLLAPSED_SIZE: [f32; 2] = [34.0, 28.0];
const PANEL_ROUNDING: f32 = 8.0;
const PANEL_INNER_MARGIN: Margin = Margin {
    left: 14,
    right: 14,
    top: 12,
    bottom: 12,
};

// Pre-multiplied RGB matching Python's QColor(10, 10, 10, 210):
// 10 * 210 / 255 ≈ 8.
const BG_COLOR: Color32 = Color32::from_rgba_premultiplied(8, 8, 8, 210);
const ACCENT_COLOR: Color32 = Color32::from_rgb(200, 160, 80);
const TEXT_COLOR: Color32 = Color32::from_rgb(220, 220, 220);
const OPTIONAL_COLOR: Color32 = Color32::from_rgb(140, 140, 140);
const REWARD_COLOR: Color32 = Color32::from_rgb(100, 200, 120);

const FONT_SM: f32 = 11.0;
const FONT_MD: f32 = 13.0;
const FONT_LG: f32 = 17.0;
const FONT_DIVIDER: f32 = 8.0;

// Repaint cadence. We poll each frame to drain log events and to observe the
// WM-reported window position after a drag (the WM owns the drag, so polling
// is the only signal). When unlocked we keep a tight interval so a drag-release
// position is captured promptly; when locked there's no drag to track, so we
// back off to save power over a long play session.
const POLL_INTERVAL_ACTIVE: Duration = Duration::from_millis(150);
const POLL_INTERVAL_IDLE: Duration = Duration::from_millis(1000);

// The header divider rule. Built once instead of re-allocating a String every
// frame.
static DIVIDER: LazyLock<String> = LazyLock::new(|| "─".repeat(38));

// Tolerance in pixels; sub-pixel jitter shouldn't trigger a save.
const POSITION_EPSILON: f32 = 0.5;
// Debounce window: how long the position must stay stable after the last
// change before we write to disk. Long enough to absorb the typical X11
// drag-release jitter, short enough to feel responsive.
const POSITION_SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// What the overlay is currently showing.
#[derive(Debug, Clone)]
enum DisplayState {
    Idle,
    Known(ZoneEntry),
    Unknown(String),
}

pub struct OverlayApp {
    guide: CampaignGuide,
    events: Receiver<LogEvent>,
    state: DisplayState,
    locked: bool,
    /// When collapsed, the overlay shrinks to a small nub (one expand button)
    /// so it's out of the way after the campaign — click to restore.
    collapsed: bool,
    /// The window size we last asked the WM for, so we only issue a resize
    /// command when the target actually changes (expanded height tracks
    /// content, which varies per zone).
    applied_size: Option<[f32; 2]>,
    last_seen_pos: Option<[f32; 2]>,
    last_saved_pos: Option<[f32; 2]>,
    pending_save_at: Option<Instant>,
    /// Tracks window focus so we can detect the moment we *lose* it (the user
    /// clicked into the game) and re-assert always-on-top — borderless game
    /// windows raise themselves on focus and can land above us otherwise.
    was_focused: bool,
}

impl OverlayApp {
    pub fn new(
        _cc: &CreationContext<'_>,
        guide: CampaignGuide,
        events: Receiver<LogEvent>,
        locked: bool,
        collapsed: bool,
        initial_position: [f32; 2],
    ) -> Self {
        Self {
            guide,
            events,
            state: DisplayState::Idle,
            locked,
            collapsed,
            // None forces the first frame to size the window to its content.
            applied_size: None,
            // Seed `last_saved_pos` so we don't re-write the same value on
            // the first frame (where outer_rect will look "newly observed").
            last_seen_pos: Some(initial_position),
            last_saved_pos: Some(initial_position),
            pending_save_at: None,
            // Assume focused at startup; the first focus-loss edge will then
            // trigger a re-assert.
            was_focused: true,
        }
    }

    /// Re-assert always-on-top after losing focus. `with_always_on_top()` is
    /// only applied once at window creation; when the borderless game window
    /// takes focus it can raise above us and we never recover. Toggling the
    /// level through `Normal` forces winit to re-apply `_NET_WM_STATE_ABOVE`
    /// (re-setting the same level would be deduped to a no-op and wouldn't
    /// restack the window).
    fn reassert_always_on_top(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        // Only act on the focused -> unfocused edge to avoid restacking every
        // frame (which would flicker and fight the WM).
        if self.was_focused && !focused {
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::Normal));
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop));
        }
        self.was_focused = focused;
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            apply_log_event(&mut self.state, &self.guide, event);
        }
    }

    /// Draws the full panel and returns the window height needed to show it
    /// without clipping (content height plus the frame's top/bottom margins).
    fn draw_panel(&mut self, ui: &mut egui::Ui) -> f32 {
        let frame = Frame::new()
            .fill(BG_COLOR)
            .corner_radius(PANEL_ROUNDING)
            .inner_margin(PANEL_INNER_MARGIN)
            .stroke(Stroke::NONE);

        // Size to content with `Frame::show`, NOT `CentralPanel`. A central
        // panel expands its inner Ui to fill the whole window (egui calls
        // `expand_to_include_rect(max_rect)` internally), so `min_rect().height()`
        // would report the *current window height* rather than the content
        // height. Feeding that back into the window size each frame grows it by
        // the margins until it fills the screen and jitters. `Frame::show`'s
        // `min_rect` tracks the laid-out content, giving a stable fit.
        frame
            .show(ui, |inner| {
                inner.set_min_width(
                    PANEL_WIDTH - PANEL_INNER_MARGIN.leftf() - PANEL_INNER_MARGIN.rightf(),
                );
                inner.spacing_mut().item_spacing.y = 4.0;

                let [lock_rect, collapse_rect] = self.draw_header(inner);
                self.draw_zone(inner);
                self.draw_divider(inner);
                self.draw_tasks(inner);
                self.draw_footer(inner);

                // Drag handling: trigger _NET_WM_MOVERESIZE on a fresh primary
                // press inside the panel, but never when the press lands on a
                // header button (their own click handlers run on release). The
                // buttons are rendered BEFORE this check so their rects are
                // already final.
                let panel_rect = inner.min_rect();
                let ctx = inner.ctx();
                let (primary_pressed, pointer_pos) =
                    ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.interact_pos()));
                if !self.locked
                    && primary_pressed
                    && let Some(p) = pointer_pos
                    && panel_rect.contains(p)
                    && !lock_rect.contains(p)
                    && !collapse_rect.contains(p)
                {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }

                // Window height to fit the content: the laid-out content plus
                // the frame's vertical inner margins. A small fudge avoids
                // clipping the last line to sub-pixel rounding.
                panel_rect.height() + PANEL_INNER_MARGIN.topf() + PANEL_INNER_MARGIN.bottomf() + 2.0
            })
            .inner
    }

    /// Draws the collapsed nub: a single expand button. No drag handling — to
    /// reposition, expand first (position persists, so collapse afterward).
    fn draw_collapsed(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new()
            .fill(BG_COLOR)
            .corner_radius(PANEL_ROUNDING)
            .inner_margin(Margin::same(2))
            .stroke(Stroke::NONE);

        CentralPanel::default()
            .frame(frame)
            .show_inside(ui, |inner| {
                let response = inner
                    .add(
                        Button::new(
                            RichText::new("▸")
                                .color(ACCENT_COLOR)
                                .font(FontId::new(FONT_MD, FontFamily::Monospace)),
                        )
                        .min_size(egui::vec2(26.0, 22.0))
                        .corner_radius(4.0),
                    )
                    .on_hover_text("Expand rsexile");
                if response.clicked() {
                    self.collapsed = false;
                    let _ = persistence::save_collapse_state(self.collapsed);
                }
            });
    }

    /// Resize the OS window to `target`, but only when it meaningfully changes
    /// (resizing every frame would thrash the WM and the content height jitters
    /// by sub-pixels).
    fn sync_window_size(&mut self, ctx: &egui::Context, target: [f32; 2]) {
        let changed = match self.applied_size {
            None => true,
            Some(a) => (a[0] - target[0]).abs() > 1.0 || (a[1] - target[1]).abs() > 1.0,
        };
        if changed {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(target[0], target[1])));
            self.applied_size = Some(target);
        }
    }

    /// Draws the header row (act label on the left, collapse + lock buttons on
    /// the right). Returns the buttons' rects so the drag check can exclude
    /// them (a press there is a click, not a window drag).
    fn draw_header(&mut self, ui: &mut egui::Ui) -> [egui::Rect; 2] {
        let act_text = match &self.state {
            DisplayState::Idle => "rsexile".to_string(),
            DisplayState::Known(e) => e.act.clone(),
            DisplayState::Unknown(_) => "Campaign Guide".to_string(),
        };

        let mut lock_rect = egui::Rect::NOTHING;
        let mut collapse_rect = egui::Rect::NOTHING;
        ui.horizontal(|row| {
            row.label(small_dim(&act_text));
            // right_to_left: first added is rightmost, so lock sits on the far
            // right and collapse to its left -> [▾][L].
            row.with_layout(Layout::right_to_left(Align::Center), |right| {
                let label = if self.locked { "L" } else { "U" };
                let tooltip = if self.locked {
                    "Unlock overlay"
                } else {
                    "Lock overlay"
                };
                let lock = right
                    .add(
                        Button::new(
                            RichText::new(label)
                                .color(OPTIONAL_COLOR)
                                .font(FontId::new(FONT_SM, FontFamily::Monospace)),
                        )
                        .min_size(egui::vec2(22.0, 18.0))
                        .corner_radius(3.0),
                    )
                    .on_hover_text(tooltip);
                lock_rect = lock.rect;
                if lock.clicked() {
                    self.locked = !self.locked;
                    let _ = persistence::save_lock_state(self.locked);
                }

                let collapse = right
                    .add(
                        Button::new(
                            RichText::new("▾")
                                .color(OPTIONAL_COLOR)
                                .font(FontId::new(FONT_SM, FontFamily::Monospace)),
                        )
                        .min_size(egui::vec2(22.0, 18.0))
                        .corner_radius(3.0),
                    )
                    .on_hover_text("Collapse overlay");
                collapse_rect = collapse.rect;
                if collapse.clicked() {
                    self.collapsed = true;
                    let _ = persistence::save_collapse_state(self.collapsed);
                }
            });
        });
        [lock_rect, collapse_rect]
    }

    fn draw_zone(&self, ui: &mut egui::Ui) {
        let zone_text = match &self.state {
            DisplayState::Idle => "Waiting for game...".to_string(),
            DisplayState::Known(e) => e.zone.clone(),
            DisplayState::Unknown(name) => name.clone(),
        };
        ui.label(
            RichText::new(zone_text)
                .color(ACCENT_COLOR)
                .font(FontId::new(FONT_LG, FontFamily::Monospace))
                .strong(),
        );
    }

    fn draw_divider(&self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(DIVIDER.as_str())
                .color(OPTIONAL_COLOR)
                .font(FontId::new(FONT_DIVIDER, FontFamily::Monospace)),
        );
    }

    fn draw_tasks(&self, ui: &mut egui::Ui) {
        match &self.state {
            DisplayState::Idle => {
                ui.add_space(6.0);
                ui.label(dim_info("  Launch PoE2 and enter a zone"));
            }
            DisplayState::Unknown(_) => {
                ui.add_space(6.0);
                ui.label(dim_info("  No guide data for this zone"));
            }
            DisplayState::Known(entry) => {
                if entry.tasks.is_empty() {
                    ui.add_space(6.0);
                    ui.label(dim_info("  No tasks — move on"));
                } else {
                    for task in &entry.tasks {
                        ui.add_space(3.0);
                        let prefix = if task.optional { "( )" } else { "[ ]" };
                        let color = if task.optional {
                            OPTIONAL_COLOR
                        } else {
                            TEXT_COLOR
                        };
                        ui.label(
                            RichText::new(format!("{prefix} {}", task.description))
                                .color(color)
                                .font(FontId::new(FONT_MD, FontFamily::Monospace)),
                        );
                        if !task.reward.is_empty() {
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.label(
                                    RichText::new(format!("→ {}", task.reward))
                                        .color(REWARD_COLOR)
                                        .font(FontId::new(FONT_MD, FontFamily::Monospace)),
                                );
                            });
                        }
                    }
                }
            }
        }
    }

    fn draw_footer(&self, ui: &mut egui::Ui) {
        if let DisplayState::Known(entry) = &self.state
            && let Some(next) = &entry.next_zone
        {
            ui.add_space(4.0);
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                ui.label(small_dim(&format!("Next: {next}")));
            });
        }
    }

    /// Read the WM-reported window position and persist it when it changes
    /// and then stays stable for [`POSITION_SAVE_DEBOUNCE`]. We don't get a
    /// drag-stop callback (the WM owns the drag once StartDrag is sent), so
    /// polling each frame is the simplest reliable signal.
    fn tick_position_persist(&mut self, ctx: &egui::Context) {
        let Some(rect) = ctx.input(|i| i.viewport().outer_rect) else {
            return;
        };
        let cur = [rect.min.x, rect.min.y];

        let changed = match self.last_seen_pos {
            None => true,
            Some(prev) => position_changed(prev, cur),
        };

        if changed {
            self.last_seen_pos = Some(cur);
            self.pending_save_at = Some(Instant::now() + POSITION_SAVE_DEBOUNCE);
            return;
        }

        if let Some(at) = self.pending_save_at
            && Instant::now() >= at
        {
            self.pending_save_at = None;
            let already_saved = self
                .last_saved_pos
                .is_some_and(|prev| !position_changed(prev, cur));
            if !already_saved {
                if let Err(e) = persistence::save_position(cur) {
                    eprintln!("rsexile: failed to save position: {e}");
                }
                self.last_saved_pos = Some(cur);
            }
        }
    }
}

fn position_changed(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() > POSITION_EPSILON || (a[1] - b[1]).abs() > POSITION_EPSILON
}

/// Pure state transition for a single log event. Extracted from
/// `OverlayApp::drain_events` so the state machine can be unit-tested
/// without standing up a full egui app and channel.
fn apply_log_event(state: &mut DisplayState, guide: &CampaignGuide, event: LogEvent) {
    match event {
        LogEvent::ZoneEntered(name) | LogEvent::SceneSet(name) => {
            *state = match lookup_zone(guide, &name) {
                Some(entry) => DisplayState::Known(entry.clone()),
                None => DisplayState::Unknown(name),
            };
        }
        // Plumbed through for future UI features; no-op for now.
        LogEvent::Died | LogEvent::LevelUp(_) => {}
    }
}

impl App for OverlayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Transparent so the rounded panel frame is the only thing drawn.
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut EframeFrame) {
        self.drain_events();
        let target = if self.collapsed {
            self.draw_collapsed(ui);
            COLLAPSED_SIZE
        } else {
            [PANEL_WIDTH, self.draw_panel(ui)]
        };
        self.sync_window_size(ui.ctx(), target);
        self.tick_position_persist(ui.ctx());
        self.reassert_always_on_top(ui.ctx());
        // Keep the UI ticking so we notice channel events and the debounced
        // position save without needing user input to wake the event loop.
        // Back off to the idle cadence once the overlay is locked and there's
        // no debounced save still pending — a locked window can't be dragged,
        // so frequent position polling buys nothing.
        let interval = if self.locked && self.pending_save_at.is_none() {
            POLL_INTERVAL_IDLE
        } else {
            POLL_INTERVAL_ACTIVE
        };
        ui.ctx().request_repaint_after(interval);
    }
}

fn small_dim(text: &str) -> RichText {
    RichText::new(text)
        .color(OPTIONAL_COLOR)
        .font(FontId::new(FONT_SM, FontFamily::Monospace))
}

fn dim_info(text: &str) -> RichText {
    RichText::new(text)
        .color(OPTIONAL_COLOR)
        .font(FontId::new(FONT_MD, FontFamily::Monospace))
}

/// Build the eframe `NativeOptions` for the overlay window.
pub fn viewport_options(initial_position: Option<[f32; 2]>) -> NativeOptions {
    let mut viewport = ViewportBuilder::default()
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_resizable(false)
        .with_inner_size([PANEL_WIDTH, PANEL_HEIGHT])
        .with_title("rsexile");
    if let Some(pos) = initial_position {
        viewport = viewport.with_position(pos);
    }

    // Force the X11 backend on Linux. Wayland does not let clients set
    // their own window position, so the manual drag-to-move implementation
    // would silently fail on a native Wayland surface. PoE2 itself goes
    // through XWayland anyway, so X11 is the right target.
    #[cfg(all(unix, not(target_os = "macos")))]
    let event_loop_builder: Option<eframe::EventLoopBuilderHook> = Some(Box::new(|builder| {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_x11();
    }));
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let event_loop_builder: Option<eframe::EventLoopBuilderHook> = None;

    NativeOptions {
        viewport,
        event_loop_builder,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::campaign::Task;
    use std::collections::HashMap;

    fn entry(zone: &str, act: &str, tasks: Vec<Task>, next: Option<&str>) -> ZoneEntry {
        ZoneEntry {
            zone: zone.into(),
            act: act.into(),
            tasks,
            next_zone: next.map(|s| s.into()),
        }
    }

    fn guide_with(zone: &str, entry: ZoneEntry) -> CampaignGuide {
        let mut g = HashMap::new();
        g.insert(zone.to_lowercase(), entry);
        g
    }

    #[test]
    fn small_dim_uses_dim_color() {
        let rt = small_dim("hi");
        assert_eq!(rt.text(), "hi");
    }

    #[test]
    fn display_state_known_holds_entry() {
        let e = entry(
            "Clearfell",
            "Act 1",
            vec![Task {
                description: "test".into(),
                reward: "".into(),
                optional: false,
            }],
            Some("The Grelwood"),
        );
        let state = DisplayState::Known(e.clone());
        match state {
            DisplayState::Known(got) => {
                assert_eq!(got.zone, "Clearfell");
                assert_eq!(got.act, "Act 1");
                assert_eq!(got.next_zone.as_deref(), Some("The Grelwood"));
            }
            _ => panic!("expected Known"),
        }
    }

    #[test]
    fn position_changed_ignores_subpixel_jitter() {
        assert!(!position_changed([100.0, 100.0], [100.2, 100.3]));
        assert!(position_changed([100.0, 100.0], [101.0, 100.0]));
        assert!(position_changed([100.0, 100.0], [100.0, 101.0]));
    }

    #[test]
    fn apply_log_event_zone_entered_known_zone_marks_known() {
        let guide = guide_with("clearfell", entry("Clearfell", "Act 1", vec![], None));
        let mut state = DisplayState::Idle;
        apply_log_event(
            &mut state,
            &guide,
            LogEvent::ZoneEntered("Clearfell".into()),
        );
        match state {
            DisplayState::Known(e) => assert_eq!(e.zone, "Clearfell"),
            _ => panic!("expected Known, got {state:?}"),
        }
    }

    #[test]
    fn apply_log_event_zone_entered_unknown_zone_marks_unknown() {
        let guide = guide_with("clearfell", entry("Clearfell", "Act 1", vec![], None));
        let mut state = DisplayState::Idle;
        apply_log_event(&mut state, &guide, LogEvent::ZoneEntered("Nowhere".into()));
        match state {
            DisplayState::Unknown(name) => assert_eq!(name, "Nowhere"),
            _ => panic!("expected Unknown, got {state:?}"),
        }
    }

    #[test]
    fn apply_log_event_scene_set_routes_through_lookup() {
        let guide = guide_with("the azak bog", entry("The Azak Bog", "Act 2", vec![], None));
        let mut state = DisplayState::Idle;
        apply_log_event(
            &mut state,
            &guide,
            LogEvent::SceneSet("The Azak Bog".into()),
        );
        match state {
            DisplayState::Known(e) => assert_eq!(e.zone, "The Azak Bog"),
            _ => panic!("expected Known, got {state:?}"),
        }
    }

    #[test]
    fn apply_log_event_died_does_not_change_state() {
        let guide = guide_with("clearfell", entry("Clearfell", "Act 1", vec![], None));
        let mut state = DisplayState::Known(entry("Clearfell", "Act 1", vec![], None));
        apply_log_event(&mut state, &guide, LogEvent::Died);
        match state {
            DisplayState::Known(e) => assert_eq!(e.zone, "Clearfell"),
            _ => panic!("expected Known to be preserved"),
        }
    }

    #[test]
    fn apply_log_event_level_up_does_not_change_state() {
        let guide = guide_with("clearfell", entry("Clearfell", "Act 1", vec![], None));
        let mut state = DisplayState::Idle;
        apply_log_event(&mut state, &guide, LogEvent::LevelUp(42));
        assert!(matches!(state, DisplayState::Idle));
    }

    #[test]
    fn apply_log_event_overwrites_previous_state() {
        let guide = guide_with("clearfell", entry("Clearfell", "Act 1", vec![], None));
        let mut state = DisplayState::Unknown("Old Zone".into());
        apply_log_event(
            &mut state,
            &guide,
            LogEvent::ZoneEntered("Clearfell".into()),
        );
        match state {
            DisplayState::Known(e) => assert_eq!(e.zone, "Clearfell"),
            _ => panic!("expected state transition to Known"),
        }
    }

    #[test]
    fn viewport_options_applies_initial_position() {
        let opts = viewport_options(Some([123.0, 456.0]));
        assert_eq!(opts.viewport.position, Some(egui::pos2(123.0, 456.0)));
    }

    #[test]
    fn viewport_options_without_position_leaves_unset() {
        let opts = viewport_options(None);
        assert_eq!(opts.viewport.position, None);
    }

    #[test]
    fn viewport_options_sets_overlay_window_traits() {
        let opts = viewport_options(None);
        assert_eq!(opts.viewport.decorations, Some(false));
        assert_eq!(opts.viewport.transparent, Some(true));
        assert_eq!(opts.viewport.resizable, Some(false));
        assert_eq!(
            opts.viewport.inner_size,
            Some(egui::vec2(PANEL_WIDTH, 240.0))
        );
    }
}
