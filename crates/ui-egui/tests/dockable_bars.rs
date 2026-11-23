//! Tool bars and Tool Sets categories dock on their own: dragging a bar's grip detects the nearest
//! window edge, shows the rectangle it would land in while the button is held, and drops it there
//! on release (Escape cancels).

use cadcraft_engine::Session;
use cadcraft_ui_egui::bars::{self, Zone};
use cadcraft_ui_egui::{CadApp, Services};
use egui::{Event, PointerButton, Pos2, Rect, pos2, vec2};

struct Window {
    ctx: egui::Context,
    app: CadApp,
    time: f64,
}

impl Window {
    fn new() -> Self {
        let mut w = Window { ctx: egui::Context::default(), app: CadApp::new(Session::new(), Services::default()), time: 0.0 };
        // The first frames lay the window out; the geometry the drag measures against is the
        // previous frame's.
        w.frame(Vec::new());
        w.frame(Vec::new());
        w
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 0.05;
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }

    fn rect(&self, id: &str) -> Rect {
        *self.app.bars.rects.get(id).unwrap_or_else(|| panic!("{id} was not drawn: {:?}", self.app.bars.rects.keys().collect::<Vec<_>>()))
    }

    fn moved(&mut self, p: Pos2) {
        self.frame(vec![Event::PointerMoved(p)]);
    }

    fn button(&mut self, p: Pos2, pressed: bool) {
        self.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Default::default() }]);
    }

    /// Hold the grip of `id`, carry it to `to` and report what was shown before letting go.
    fn drag(&mut self, id: &str, to: Pos2) -> (Zone, bool) {
        let grip = self.rect(id).min + vec2(6.0, 17.0);
        self.moved(grip);
        self.button(grip, true);
        self.moved(grip + vec2(12.0, 6.0));
        self.moved(to);
        self.moved(to + vec2(1.0, 0.0));
        let g = self.app.bars.geom;
        let showing = self.app.bars.drag.is_some();
        (bars::detect(&g, to), showing)
    }

    fn drop(&mut self, at: Pos2) {
        self.button(at, false);
        self.frame(Vec::new());
    }

    fn zone(&self, id: &str) -> Zone {
        self.app.ui.bars.zone(id)
    }
}

#[test]
fn the_main_tool_bar_starts_as_one_row_of_separate_groups() {
    let w = Window::new();
    for i in 0..8 {
        let id = format!("main.{i}");
        assert_eq!(w.zone(&id), Zone::Top, "{id}");
        assert!(w.app.bars.rects.contains_key(&id), "{id} is drawn");
    }
    assert!(w.rect("main.0").right() < w.rect("main.1").left(), "side by side, in order");
}

#[test]
fn dragging_a_group_to_the_left_edge_previews_then_docks_it_there() {
    let mut w = Window::new();
    let (zone, showing) = w.drag("main.5", pos2(18.0, 380.0));
    assert_eq!(zone, Zone::Left);
    assert!(showing, "the drag is in progress while the button is held");
    assert_eq!(w.zone("main.5"), Zone::Top, "nothing moves until the button is released");
    w.drop(pos2(18.0, 380.0));
    assert!(w.app.bars.drag.is_none());
    assert_eq!(w.zone("main.5"), Zone::Left);
    w.frame(Vec::new());
    let r = w.rect("main.5");
    assert!(r.left() < 60.0 && r.height() > r.width(), "now a vertical bar on the left edge: {r:?}");
    assert!(w.app.bars.geom.left_w > 0.0);
    // The other groups stay where they were.
    assert_eq!(w.zone("main.4"), Zone::Top);
    assert!(w.rect("main.4").top() < 40.0);
}

#[test]
fn a_group_can_go_below_the_text_menus_down_the_right_side_or_along_the_bottom() {
    let mut w = Window::new();
    let menu_bottom = w.app.bars.geom.menu.bottom();
    let (zone, _) = w.drag("main.1", pos2(500.0, menu_bottom + 12.0));
    assert_eq!(zone, Zone::BelowMenu);
    w.drop(pos2(500.0, menu_bottom + 12.0));
    assert_eq!(w.zone("main.1"), Zone::BelowMenu);
    w.frame(Vec::new());
    assert!(w.rect("main.1").top() >= menu_bottom, "under the menus: {:?} vs {menu_bottom}", w.rect("main.1"));
    assert!(w.rect("main.1").width() > w.rect("main.1").height());

    let (zone, _) = w.drag("main.2", pos2(1270.0, 400.0));
    assert_eq!(zone, Zone::Right);
    w.drop(pos2(1270.0, 400.0));
    w.frame(Vec::new());
    assert!(w.rect("main.2").left() > 1180.0, "{:?}", w.rect("main.2"));

    let bottom = w.app.bars.geom.work.bottom();
    let (zone, _) = w.drag("main.3", pos2(600.0, bottom - 6.0));
    assert_eq!(zone, Zone::Bottom);
    w.drop(pos2(600.0, bottom - 6.0));
    w.frame(Vec::new());
    let r = w.rect("main.3");
    assert!(r.bottom() <= bottom && r.top() > bottom - 60.0 && r.width() > r.height(), "{r:?} above the status bar at {bottom}");
    assert!(w.app.bars.geom.work.bottom() < bottom, "the drawing area gave the strip its space");
}

#[test]
fn away_from_every_edge_the_bar_floats_where_it_is_dropped() {
    let mut w = Window::new();
    let (zone, _) = w.drag("main.6", pos2(600.0, 400.0));
    assert_eq!(zone, Zone::Float);
    w.drop(pos2(600.0, 400.0));
    w.frame(Vec::new());
    assert_eq!(w.zone("main.6"), Zone::Float);
    let r = w.rect("main.6");
    assert!((r.center().x - 600.0).abs() < 120.0 && (r.center().y - 400.0).abs() < 40.0, "{r:?}");
}

#[test]
fn escape_cancels_a_drag() {
    let mut w = Window::new();
    w.drag("main.5", pos2(18.0, 380.0));
    w.frame(vec![Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
    assert!(w.app.bars.drag.is_none());
    w.drop(pos2(18.0, 380.0));
    assert_eq!(w.zone("main.5"), Zone::Top, "cancelled, so it stayed");
}

#[test]
fn a_tool_sets_category_leaves_the_palette_as_a_bar_and_goes_back() {
    let mut w = Window::new();
    let id = bars::ts_id("Drafting", "Draw");
    assert_eq!(w.zone(&id), Zone::Palette);
    assert!(!w.app.bars.rects.contains_key(&id), "inside the palette it is not a bar");
    let list = bars::collect(&mut w.app, false);
    bars::place(&mut w.app, &list, &id, Zone::Left, None);
    w.frame(Vec::new());
    w.frame(Vec::new());
    assert_eq!(w.zone(&id), Zone::Left);
    let r = w.rect(&id);
    assert!(r.left() < 60.0 && r.height() > r.width(), "{r:?}");
    let list = bars::collect(&mut w.app, false);
    bars::place(&mut w.app, &list, &id, Zone::Palette, None);
    w.frame(Vec::new());
    assert_eq!(w.zone(&id), Zone::Palette);
    assert!(!w.app.ui.bars.slots.contains_key(&id));
}

#[test]
fn the_arrangement_is_part_of_the_saved_interface_settings() {
    let mut w = Window::new();
    let list = bars::collect(&mut w.app, false);
    bars::place(&mut w.app, &list, "main.4", Zone::Right, None);
    let saved = serde_json::to_value(&w.app.ui).unwrap();
    assert_eq!(saved["bars"]["slots"]["main.4"]["zone"], "right");
    let back: cadcraft_ui_egui::UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(back.bars.zone("main.4"), Zone::Right);
    // Settings saved before bars existed still load.
    let mut old = serde_json::to_value(&w.app.ui).unwrap();
    old.as_object_mut().unwrap().remove("bars");
    let back: cadcraft_ui_egui::UiState = serde_json::from_value(old).unwrap();
    assert_eq!(back.bars.zone("main.4"), Zone::Top);
}

/// Pulling a category's header out of the Tool Sets palette starts the same drag, and dropping it
/// on an edge turns the category into a bar that no longer shows in the palette.
#[test]
fn dragging_a_tool_sets_header_out_of_the_palette_detaches_it() {
    let mut found = None;
    for y in (60..260).step_by(3) {
        let mut w = Window::new();
        let from = pos2(90.0, y as f32);
        w.moved(from);
        w.button(from, true);
        w.moved(from + vec2(14.0, 6.0));
        w.moved(from + vec2(60.0, 10.0));
        if w.app.bars.drag.as_ref().is_some_and(|d| d.id.starts_with("ts.Drafting.")) {
            found = Some((w, y));
            break;
        }
    }
    let (mut w, y) = found.expect("some row of the palette is a category header");
    let id = w.app.bars.drag.as_ref().unwrap().id.clone();
    let to = pos2(1270.0, 420.0);
    w.moved(to);
    w.moved(to + vec2(1.0, 0.0));
    w.drop(to);
    assert_eq!(w.zone(&id), Zone::Right, "category at y={y} dropped on the right edge");
    w.frame(Vec::new());
    assert!(w.rect(&id).left() > 1180.0, "{:?}", w.rect(&id));
}
