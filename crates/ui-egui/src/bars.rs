//! Dockable tool bars.
//!
//! Every group of the main tool bar, every category of Tool Sets, the assistant's bar and each
//! plugin's bar is a *bar* with a grip on its leading edge. Dragging the grip moves the bar on its
//! own: while the button is held the window edge nearest the pointer is detected and a rectangle
//! shows where the bar would land (above the text menus, under them, down either side, along the
//! bottom, or floating); releasing the button puts it there, Escape cancels. The arrangement is
//! kept in [`UiState::bars`](crate::UiState) with the rest of the interface settings.
//!
//! Placement itself is plain data ([`Layout`], [`detect`], [`preview`]) so it can be tested without
//! a window; the drawing code only reads it.

use std::collections::{BTreeMap, HashMap};

use egui::{Color32, CursorIcon, Id, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::CadApp;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

/// Cross-axis size of a bar: the height of a horizontal one, the width of a vertical one.
pub const THICK: f32 = 34.0;
/// Edge of a bar's square buttons.
const BTN: f32 = 26.0;
/// Space between two buttons.
const GAP: f32 = 6.0;
/// Length of the grip along the bar.
const GRIP: f32 = 16.0;
/// Space between two bars in one strip.
const BAR_GAP: f32 = 14.0;
/// How near an edge the pointer has to be to dock there.
pub const SNAP: f32 = 64.0;
/// How far under the menus the "below the menus" band reaches.
const BELOW_BAND: f32 = 44.0;

/// Where a bar sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Zone {
    /// The tool bar row above the text menus.
    Top,
    /// A strip under the text menus.
    BelowMenu,
    /// A vertical strip on the left edge.
    Left,
    /// A vertical strip on the right edge.
    Right,
    /// A strip above the status bar.
    Bottom,
    /// A free-standing bar.
    Float,
    /// Inside the Tool Sets palette (only categories).
    Palette,
}

impl Zone {
    /// The zones a bar can be docked to from its menu, in menu order.
    pub const DOCKS: [Zone; 6] = [Zone::Top, Zone::BelowMenu, Zone::Left, Zone::Right, Zone::Bottom, Zone::Float];

    pub fn label(self) -> &'static str {
        match self {
            Zone::Top => crate::tl!("Dock above the menus"),
            Zone::BelowMenu => crate::tl!("Dock below the menus"),
            Zone::Left => crate::tl!("Dock on the left edge"),
            Zone::Right => crate::tl!("Dock on the right edge"),
            Zone::Bottom => crate::tl!("Dock on the bottom edge"),
            Zone::Float => crate::tl!("Float"),
            Zone::Palette => crate::tl!("Return to Tool Sets"),
        }
    }

    fn vertical(self) -> bool {
        matches!(self, Zone::Left | Zone::Right)
    }
}

/// One bar's saved placement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Slot {
    pub zone: Zone,
    /// Position among the bars of the zone (smaller first).
    pub order: f32,
    /// Top-left corner, for floating bars.
    pub pos: [f32; 2],
}

impl Default for Slot {
    fn default() -> Self {
        Slot { zone: Zone::Float, order: 0.0, pos: [80.0, 120.0] }
    }
}

/// The saved arrangement: only bars the person moved appear in it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Layout {
    pub slots: BTreeMap<String, Slot>,
}

/// Where a bar starts out: the main tool bar's groups on the top row, Tool Sets categories in
/// the palette, everything else floating.
pub fn default_zone(id: &str) -> Zone {
    if id.starts_with("main.") {
        Zone::Top
    } else if id.starts_with("ts.") || id == LAYERS_ID {
        Zone::Palette
    } else {
        Zone::Float
    }
}

impl Layout {
    pub fn zone(&self, id: &str) -> Zone {
        self.slots.get(id).map_or_else(|| default_zone(id), |s| s.zone)
    }

    /// Put `id` in `zone` at `order` (and `pos`, when floating). A category returned to Tool Sets
    /// simply forgets its slot.
    pub fn place(&mut self, id: &str, zone: Zone, order: f32, pos: [f32; 2]) {
        if zone == Zone::Palette {
            self.slots.remove(id);
        } else {
            self.slots.insert(id.to_string(), Slot { zone, order, pos });
        }
    }

    /// Forget every move: bars go back to where they started.
    pub fn reset(&mut self) {
        self.slots.clear();
    }
}

/// What a button shows.
#[derive(Clone)]
pub enum Glyph {
    Icon(Icon),
    Text(String),
}

/// One button.
#[derive(Clone)]
pub struct Item {
    pub glyph: Glyph,
    pub tip: String,
    /// What a click does: `cmd:<id>` and `tool:<id>` start a command, anything else is the
    /// assistant's ([`crate::ai_tools::run_action`]).
    pub action: String,
    pub enabled: bool,
}

/// A group of buttons that moves as one.
#[derive(Clone)]
pub struct Bar {
    pub id: String,
    pub title: String,
    pub items: Vec<Item>,
    /// The assistant's and plugins' bars can be closed.
    pub closable: bool,
    /// Where the bar sorts when its slot has no order of its own.
    pub order: f32,
}

impl Bar {
    /// Length along the bar's axis.
    pub fn length(&self) -> f32 {
        length_for(self.items.len())
    }

    /// Size when laid along a horizontal strip.
    pub fn size(&self, vertical: bool) -> Vec2 {
        if vertical { vec2(THICK, self.length()) } else { vec2(self.length(), THICK) }
    }
}

/// Length along its axis of a bar with `n` buttons.
pub fn length_for(n: usize) -> f32 {
    GRIP + 2.0 + n.max(1) as f32 * (BTN + GAP) - GAP + 2.0
}

/// Where in a bar's grip the pointer holds it when a drag starts from a palette header.
pub fn centered_grab() -> Vec2 {
    vec2(GRIP / 2.0, THICK / 2.0)
}

/// The id of the Layers toolbar (the tools of the Layers palette).
pub const LAYERS_ID: &str = "layers";

/// Titles of the main tool bar's groups, in [`crate::chrome::TOOLBAR`] order.
pub const MAIN_TITLES: [&str; 8] = ["File", "History", "Plot", "Exchange", "Navigate", "Palettes", "Inquiry", "Help"];

/// Every bar the application has right now (docked or not).
pub fn collect(app: &mut CadApp, mac: bool) -> Vec<Bar> {
    let mut bars = Vec::new();
    for (i, group) in crate::chrome::TOOLBAR.iter().enumerate() {
        let items = group
            .iter()
            .map(|(icon, cmd, label, shortcut)| {
                let tip = match shortcut {
                    Some(sc) => format!("{} ({})", crate::i18n::t(label), crate::menus::shortcut_label(sc, mac)),
                    None => crate::i18n::t(label).to_string(),
                };
                Item { glyph: Glyph::Icon(*icon), tip, action: format!("cmd:{cmd}"), enabled: true }
            })
            .collect();
        let title = MAIN_TITLES.get(i).copied().unwrap_or("Tools");
        bars.push(Bar { id: format!("main.{i}"), title: crate::i18n::t(title).to_string(), items, closable: false, order: i as f32 });
    }
    // Tool Sets categories, so each can be pulled out of the palette on its own.
    let mut order = 100.0;
    for (tab, groups) in [("Drafting", crate::palettes::DRAFTING), ("Modeling", crate::palettes::MODELING)] {
        for (name, large, small) in groups {
            let items: Vec<Item> = large
                .iter()
                .chain(small.iter())
                .filter(|(_, cmd, _)| !app.ui.hidden_tools.iter().any(|h| h == cmd))
                .map(|(icon, cmd, tip)| Item {
                    glyph: Glyph::Icon(*icon),
                    tip: crate::i18n::t(tip).to_string(),
                    action: format!("tool:{cmd}"),
                    enabled: crate::palettes::command_known(cmd),
                })
                .collect();
            bars.push(Bar { id: ts_id(tab, name), title: crate::i18n::t(name).to_string(), items, closable: false, order });
            order += 1.0;
        }
    }
    // The Layers palette's tools, with the layer list as its first button.
    let mut items = vec![Item {
        glyph: Glyph::Icon(Icon::ChevronDown),
        tip: crate::i18n::t("Layer list").to_string(),
        action: "layers:picker".into(),
        enabled: true,
    }];
    items.extend(crate::palettes::LAYER_TOOLS.iter().map(|(icon, cmd, tip)| Item {
        glyph: Glyph::Icon(*icon),
        tip: crate::i18n::t(tip).to_string(),
        action: format!("cmd:{cmd}"),
        enabled: true,
    }));
    bars.push(Bar { id: LAYERS_ID.to_string(), title: crate::i18n::t("Layers").to_string(), items, closable: false, order: 200.0 });
    let mut order = 1000.0;
    for (id, title, items, closable) in crate::ai_tools::bars(app) {
        bars.push(Bar { id, title, items, closable, order });
        order += 1.0;
    }
    bars
}

/// The id of a Tool Sets category's bar.
pub fn ts_id(tab: &str, name: &str) -> String {
    format!("ts.{tab}.{name}")
}

/// The bars of `zone`, left-to-right or top-to-bottom.
pub fn in_zone<'a>(bars: &'a [Bar], layout: &Layout, zone: Zone) -> Vec<&'a Bar> {
    let mut list: Vec<(f32, &Bar)> = bars
        .iter()
        .filter(|b| layout.zone(&b.id) == zone)
        .filter(|b| zone != Zone::Palette || !b.items.is_empty())
        .map(|b| (layout.slots.get(&b.id).map_or(b.order, |s| s.order), b))
        .collect();
    list.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
    list.into_iter().map(|(_, b)| b).collect()
}

/// Where the window's parts were last frame: what [`detect`] and [`preview`] measure against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geom {
    pub window: Rect,
    /// The title/tool bar panel (empty when the tool bar is hidden).
    pub top: Rect,
    pub top_visible: bool,
    /// The in-window text menu bar (empty height when the menus are native).
    pub menu: Rect,
    /// The drawing's area: what the strips leave over.
    pub work: Rect,
    /// Space the strips take now.
    pub below_h: f32,
    pub left_w: f32,
    pub right_w: f32,
    pub bottom_h: f32,
}

impl Default for Geom {
    fn default() -> Self {
        let none = Rect::from_min_size(Pos2::ZERO, Vec2::ZERO);
        Geom { window: none, top: none, top_visible: false, menu: none, work: none, below_h: 0.0, left_w: 0.0, right_w: 0.0, bottom_h: 0.0 }
    }
}

/// The zone for a bar dragged to `p`.
pub fn detect(g: &Geom, p: Pos2) -> Zone {
    if !g.window.contains(p) {
        return Zone::Float;
    }
    if g.top_visible && p.y <= g.top.bottom() {
        return Zone::Top;
    }
    let under = g.menu.bottom().max(g.top.bottom()) + g.below_h;
    if p.y <= under + BELOW_BAND {
        return Zone::BelowMenu;
    }
    let left = p.x - g.window.left();
    let right = g.window.right() - p.x;
    let bottom = (g.work.bottom() + g.bottom_h - p.y).abs();
    let (zone, distance) = [(Zone::Left, left), (Zone::Right, right), (Zone::Bottom, bottom)]
        .into_iter()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((Zone::Float, f32::MAX));
    if distance <= SNAP { zone } else { Zone::Float }
}

/// The two rectangles shown while dragging: the strip the bar would join and the bar itself.
/// `size` is the bar's size along a horizontal strip; `grab` is where in the bar it is held.
pub fn preview(g: &Geom, zone: Zone, size: Vec2, grab: Vec2, p: Pos2) -> (Rect, Rect) {
    let horizontal = |strip: Rect| {
        let x = (p.x - grab.x).clamp(strip.left() + 2.0, (strip.right() - size.x - 2.0).max(strip.left() + 2.0));
        (strip, Rect::from_min_size(pos2(x, strip.center().y - THICK / 2.0), size))
    };
    let vertical = |strip: Rect| {
        let (w, h) = (THICK, size.x);
        let y = (p.y - grab.x).clamp(strip.top() + 2.0, (strip.bottom() - h - 2.0).max(strip.top() + 2.0));
        (strip, Rect::from_min_size(pos2(strip.center().x - w / 2.0, y), vec2(w, h)))
    };
    match zone {
        Zone::Top => horizontal(Rect::from_min_max(pos2(g.window.left(), g.top.bottom() - THICK), pos2(g.window.right(), g.top.bottom()))),
        Zone::BelowMenu => {
            let top = g.menu.bottom().max(g.top.bottom());
            let height = if g.below_h > 0.0 { g.below_h } else { THICK + 2.0 };
            horizontal(Rect::from_min_size(pos2(g.window.left(), top), vec2(g.window.width(), height)))
        }
        Zone::Bottom => {
            let height = if g.bottom_h > 0.0 { g.bottom_h } else { THICK + 2.0 };
            let top = if g.bottom_h > 0.0 { g.work.bottom() } else { g.work.bottom() - height };
            horizontal(Rect::from_min_size(pos2(g.window.left(), top), vec2(g.window.width(), height)))
        }
        Zone::Left => {
            let width = if g.left_w > 0.0 { g.left_w } else { THICK + 4.0 };
            vertical(Rect::from_min_size(pos2(g.window.left(), g.work.top()), vec2(width, g.work.height())))
        }
        Zone::Right => {
            let width = if g.right_w > 0.0 { g.right_w } else { THICK + 4.0 };
            vertical(Rect::from_min_size(pos2(g.window.right() - width, g.work.top()), vec2(width, g.work.height())))
        }
        Zone::Float | Zone::Palette => {
            let bar = Rect::from_min_size(p - grab, size);
            (bar, bar)
        }
    }
}

/// The order that puts a bar dropped at `p` among the bars already in the zone, whose rectangles were `rects`
/// last frame. Bars on lines before the pointer's come first, then those on the pointer's line left of it
/// (or above it, in a vertical strip), so it also works for a strip of several lines.
pub fn insertion_order(layout: &Layout, existing: &[&Bar], rects: &HashMap<String, Rect>, vertical: bool, p: Pos2) -> f32 {
    let key = |b: &Bar| layout.slots.get(&b.id).map_or(b.order, |s| s.order);
    let mut prev: Option<f32> = None;
    for b in existing {
        let Some(r) = rects.get(&b.id) else { continue };
        let (across, along, across_lo, across_hi, center) =
            if vertical { (p.x, p.y, r.left(), r.right(), r.center().y) } else { (p.y, p.x, r.top(), r.bottom(), r.center().x) };
        let after_pointer = if across < across_lo {
            true
        } else if across > across_hi {
            false
        } else {
            along < center
        };
        if after_pointer {
            return match prev {
                Some(q) => (q + key(b)) / 2.0,
                None => key(b) - 1.0,
            };
        }
        prev = Some(key(b));
    }
    prev.map_or(0.0, |q| q + 1.0)
}

/// A bar being dragged.
#[derive(Clone, Debug)]
pub struct Drag {
    pub id: String,
    pub title: String,
    pub length: f32,
    /// Where in the bar (along its axis, then across) the pointer holds it.
    pub grab: Vec2,
}

/// Something the drawing code saw that the app acts on after the frame's widgets are done.
#[derive(Clone, Debug)]
pub enum Event {
    Action(String),
    MoveTo(String, Zone),
    Close(String),
    ResetAll,
}

/// Per-session bar state (the saved part is [`Layout`]).
#[derive(Default)]
pub struct Runtime {
    pub drag: Option<Drag>,
    pub geom: Geom,
    /// Where each drawn bar was last frame.
    pub rects: HashMap<String, Rect>,
    events: Vec<Event>,
    /// Where the Layers toolbar's list is open (its corner, and which corner of the list sits there).
    pub layer_picker: Option<(Pos2, egui::Align2)>,
}

/// Start dragging bar `id` (called when its grip, or a Tool Sets category header, starts a drag).
pub fn begin_drag(app: &mut CadApp, id: &str, title: &str, length: f32, grab: Vec2) {
    app.bars.drag = Some(Drag { id: id.to_string(), title: title.to_string(), length, grab });
}

/// What drawing one bar found out.
pub struct Drawn {
    pub rect: Rect,
    pub clicked: Option<String>,
    pub grip: egui::Response,
    /// Where the pointer was when a button was pulled sideways instead of clicked: the bar is picked up from it.
    pub pulled_at: Option<Pos2>,
}

/// Draw `bar` with its top-left corner at `origin`, laid out along `vertical` or horizontal.
pub fn draw_bar(ui: &mut egui::Ui, bar: &Bar, origin: Pos2, vertical: bool) -> Drawn {
    let t = Tokens::get();
    let rect = Rect::from_min_size(origin, bar.size(vertical));
    let along = |d: f32| if vertical { vec2(0.0, d) } else { vec2(d, 0.0) };
    let grip_rect = Rect::from_min_size(origin, if vertical { vec2(THICK, GRIP) } else { vec2(GRIP, THICK) });
    // The whole bar is a drag handle (buttons sit on top and keep their clicks), so a person can pick the bar
    // up from the grip, the gaps between buttons or the margins. Not focusable: the keyboard reaches the same
    // choices through the buttons, and Tab keeps going from button to button.
    let grip = ui.interact(rect, Id::unique(("cc_bar_grip", bar.id.as_str())), Sense::CLICK | Sense::DRAG);
    icons::describe_control(ui, &grip, &format!("{} (move toolbar)", bar.title), None, false);
    let over_grip = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| grip_rect.contains(p));
    let grip = if over_grip && !grip.dragged() {
        grip.on_hover_text(format!("{}\n{}", bar.title, crate::tl!("Drag to dock this toolbar; right-click for more")))
    } else {
        grip
    };
    if (grip.hovered() && over_grip) || grip.dragged() {
        ui.ctx().set_cursor_icon(if grip.dragged() { CursorIcon::Grabbing } else { CursorIcon::Grab });
    }
    paint_grip(ui, grip_rect, vertical, grip.hovered() && over_grip || grip.dragged());
    let mut clicked = None;
    let mut pulled_at = None;
    for (idx, item) in bar.items.iter().enumerate() {
        let min = origin
            + along(GRIP + 2.0 + idx as f32 * (BTN + GAP))
            + if vertical { vec2((THICK - BTN) / 2.0, 0.0) } else { vec2(0.0, (THICK - BTN) / 2.0) };
        let br = Rect::from_min_size(min, vec2(BTN, BTN));
        let resp =
            ui.interact(br, Id::unique(("cc_bar_item", bar.id.as_str(), idx)), if item.enabled { Sense::click_and_drag() } else { Sense::hover() });
        // Pressing a button and pulling picks the whole bar up (a plain press and release still clicks it).
        if resp.drag_started() {
            pulled_at = resp.interact_pointer_pos().or(Some(br.center()));
        }
        if resp.hovered() && item.enabled {
            ui.painter().rect_filled(br, 3.0, t.control_hover.gamma_multiply(0.6));
        }
        match &item.glyph {
            Glyph::Icon(icon) => icons::paint(ui.painter(), br.shrink(4.0), *icon, !item.enabled),
            Glyph::Text(text) => {
                let color = if item.enabled { t.text } else { t.text_faint };
                ui.painter().text(br.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(13.0), color);
            }
        }
        icons::describe_control(ui, &resp, &item.tip, None, false);
        if resp.on_hover_text(item.tip.clone()).clicked() && item.enabled {
            clicked = Some(item.action.clone());
        }
    }
    Drawn { rect, clicked, grip, pulled_at }
}

/// The handle at the start of a bar: a rounded tab with two columns of dots, clearly visible at rest and
/// highlighted when the pointer is on it.
pub fn paint_grip(ui: &egui::Ui, grip_rect: Rect, vertical: bool, active: bool) {
    let t = Tokens::get();
    let tab = grip_rect.shrink2(if vertical { vec2(3.0, 2.0) } else { vec2(2.0, 5.0) });
    let fill = if active { t.control_hover } else { t.control_hover.gamma_multiply(0.45) };
    ui.painter().rect_filled(tab, 3.0, fill);
    let dot = if active { t.accent } else { t.text_dim };
    let c = tab.center();
    for a in [-6.0, -2.0, 2.0, 6.0] {
        for b in [-2.5, 2.5] {
            let off = if vertical { vec2(a, b) } else { vec2(b, a) };
            ui.painter().circle_filled(c + off, 1.4, dot);
        }
    }
}

/// Ask for a bar to go to `zone` (used by menus and the Tool Sets category headers).
pub fn request_move(app: &mut CadApp, id: &str, zone: Zone) {
    app.bars.events.push(Event::MoveTo(id.to_string(), zone));
}

/// What to do with a bar's grip: start a drag, and offer its menu.
fn grip_events(app: &mut CadApp, bar: &Bar, drawn: &Drawn) {
    if let Some(action) = &drawn.clicked {
        app.bars.events.push(Event::Action(action.clone()));
    }
    app.bars.rects.insert(bar.id.clone(), drawn.rect);
    let picked = if drawn.grip.drag_started() { Some(drawn.grip.interact_pointer_pos().unwrap_or(drawn.rect.min)) } else { drawn.pulled_at };
    if let Some(at) = picked {
        let held = at - drawn.rect.min;
        let (along, across) = if app.ui.bars.zone(&bar.id).vertical() { (held.y, held.x) } else { (held.x, held.y) };
        begin_drag(app, &bar.id, &bar.title, bar.length(), vec2(along.clamp(0.0, bar.length()), across.clamp(0.0, THICK)));
    }
    let zone = app.ui.bars.zone(&bar.id);
    drawn.grip.context_menu(|ui| {
        ui.label(egui::RichText::new(&bar.title).strong());
        for z in Zone::DOCKS {
            if ui.selectable_label(zone == z, crate::i18n::t(z.label())).clicked() {
                app.bars.events.push(Event::MoveTo(bar.id.clone(), z));
                ui.close();
            }
        }
        let back = if bar.id == LAYERS_ID { crate::tl!("Return to the Layers palette") } else { crate::i18n::t(Zone::Palette.label()) };
        if (bar.id.starts_with("ts.") || bar.id == LAYERS_ID) && ui.button(back).clicked() {
            app.bars.events.push(Event::MoveTo(bar.id.clone(), Zone::Palette));
            ui.close();
        }
        if bar.closable && ui.button(crate::tl!("Close toolbar")).clicked() {
            app.bars.events.push(Event::Close(bar.id.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button(crate::tl!("Reset toolbar layout")).clicked() {
            app.bars.events.push(Event::ResetAll);
            ui.close();
        }
    });
}

/// The lines the bars docked on top are drawn in: as many as the window's width needs, so none is ever hidden
/// behind a "more" button (a hidden bar cannot be picked up and moved).
pub fn top_lines(app: &CadApp, bars: &[Bar], x0: f32) -> Vec<Vec<usize>> {
    let list = in_zone(bars, &app.ui.bars, Zone::Top);
    let lengths: Vec<f32> = list.iter().map(|b| b.length()).collect();
    let width = app.bars.geom.window.width();
    let avail = if width > 0.0 { (width - x0 - 12.0).max(120.0) } else { 1200.0 };
    wrap(&lengths, avail)
}

/// The height of `lines` lines of bars.
pub fn lines_height(lines: usize) -> f32 {
    let n = lines.max(1) as f32;
    n * THICK + (n - 1.0) * 2.0
}

/// Draw the bars docked on top, line by line, inside the title/tool bar panel's `row` (the first line),
/// starting at `x` (later lines start at the panel's left edge).
pub fn top_row(app: &mut CadApp, ui: &mut egui::Ui, bars: &[Bar], row: Rect, x: f32, lines: &[Vec<usize>]) {
    let list = in_zone(bars, &app.ui.bars, Zone::Top);
    for (n, line) in lines.iter().enumerate() {
        let mut x = if n == 0 { x } else { row.left() + 12.0 };
        let y = row.top() + n as f32 * (THICK + 2.0);
        for &i in line {
            let Some(bar) = list.get(i) else { continue };
            let drawn = draw_bar(ui, bar, pos2(x, y), false);
            grip_events(app, bar, &drawn);
            x += bar.length() + BAR_GAP;
        }
    }
}

/// Split bar lengths into lines no longer than `avail` (one bar per line at least).
pub fn wrap(lengths: &[f32], avail: f32) -> Vec<Vec<usize>> {
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut used = 0.0;
    for (i, len) in lengths.iter().enumerate() {
        let fits = used > 0.0 && used + BAR_GAP + len <= avail;
        if fits {
            used += BAR_GAP + len;
            if let Some(line) = lines.last_mut() {
                line.push(i);
            }
        } else {
            lines.push(vec![i]);
            used = *len;
        }
    }
    lines
}

/// The panel for a docked zone (not [`Zone::Top`]); nothing is shown while it holds no bars.
pub fn strip(app: &mut CadApp, ui: &mut egui::Ui, bars: &[Bar], zone: Zone) {
    let t = Tokens::get();
    let list = in_zone(bars, &app.ui.bars, zone);
    let vertical = zone.vertical();
    let g = app.bars.geom;
    let lengths: Vec<f32> = list.iter().map(|b| b.length()).collect();
    let avail = if vertical { (g.work.height().max(200.0)) - 4.0 } else { (g.window.width().max(300.0)) - 16.0 };
    let lines = wrap(&lengths, avail);
    let reset = |app: &mut CadApp, zone: Zone| match zone {
        Zone::BelowMenu => app.bars.geom.below_h = 0.0,
        Zone::Left => app.bars.geom.left_w = 0.0,
        Zone::Right => app.bars.geom.right_w = 0.0,
        Zone::Bottom => app.bars.geom.bottom_h = 0.0,
        _ => {}
    };
    if list.is_empty() {
        reset(app, zone);
        return;
    }
    let thickness = lines.len() as f32 * (THICK + 2.0) + 2.0;
    let frame = egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::ZERO);
    let panel = match zone {
        Zone::BelowMenu => egui::Panel::top("cc_bars_below").exact_size(thickness),
        Zone::Bottom => egui::Panel::bottom("cc_bars_bottom").exact_size(thickness),
        Zone::Left => egui::Panel::left("cc_bars_left").exact_size(thickness).resizable(false),
        _ => egui::Panel::right("cc_bars_right").exact_size(thickness).resizable(false),
    };
    panel.frame(frame).show(ui, |ui| {
        let r = ui.max_rect();
        for (n, line) in lines.iter().enumerate() {
            let cross = r.min + if vertical { vec2(n as f32 * (THICK + 2.0) + 2.0, 0.0) } else { vec2(0.0, n as f32 * (THICK + 2.0) + 2.0) };
            let mut at = cross + if vertical { vec2(0.0, 4.0) } else { vec2(8.0, 0.0) };
            for &i in line {
                let Some(bar) = list.get(i) else { continue };
                let drawn = draw_bar(ui, bar, at, vertical);
                grip_events(app, bar, &drawn);
                if vertical {
                    at.y += bar.length() + BAR_GAP;
                } else {
                    at.x += bar.length() + BAR_GAP;
                }
            }
        }
        let size = if vertical { r.width() } else { r.height() };
        match zone {
            Zone::BelowMenu => app.bars.geom.below_h = size,
            Zone::Left => app.bars.geom.left_w = size,
            Zone::Right => app.bars.geom.right_w = size,
            _ => app.bars.geom.bottom_h = size,
        }
    });
}

/// The floating bars, each in its own small window-less frame.
pub fn floating(app: &mut CadApp, ctx: &egui::Context, bars: &[Bar]) {
    let t = Tokens::get();
    let window = ctx.content_rect();
    let list = in_zone(bars, &app.ui.bars, Zone::Float);
    for (n, bar) in list.iter().enumerate() {
        let size = bar.size(false);
        let saved = app.ui.bars.slots.get(&bar.id).map(|s| pos2(s.pos[0], s.pos[1]));
        let pos = saved.unwrap_or_else(|| pos2(80.0, 120.0 + 56.0 * n as f32));
        let pos = pos2(
            pos.x.clamp(window.left(), (window.right() - size.x - 8.0).max(window.left())),
            pos.y.clamp(window.top(), (window.bottom() - size.y - 8.0).max(window.top())),
        );
        egui::Area::new(Id::unique(("cc_bar_float", bar.id.as_str()))).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)).corner_radius(4.0).inner_margin(egui::Margin::same(2)).show(
                ui,
                |ui| {
                    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                    let drawn = draw_bar(ui, bar, rect.min, false);
                    grip_events(app, bar, &drawn);
                },
            );
        });
    }
}

/// The distance between the pointer and `rect`, along both axes, as a clamp target.
fn clamp_to(window: Rect, size: Vec2, pos: Pos2) -> [f32; 2] {
    let x = pos.x.clamp(window.left(), (window.right() - size.x).max(window.left()));
    let y = pos.y.clamp(window.top(), (window.bottom() - size.y).max(window.top()));
    [x, y]
}

/// Put bar `id` in `zone`. `at` is the pointer, for the position among the bars of the zone and
/// for a floating bar's corner (`None` appends, as the menu does).
pub fn place(app: &mut CadApp, bars: &[Bar], id: &str, zone: Zone, at: Option<(Pos2, Vec2)>) {
    let layout = app.ui.bars.clone();
    let existing: Vec<&Bar> = in_zone(bars, &layout, zone).into_iter().filter(|b| b.id != id).collect();
    let last = existing.last().map_or(0.0, |b| layout.slots.get(&b.id).map_or(b.order, |s| s.order) + 1.0);
    let order = match at {
        Some((p, _)) => insertion_order(&layout, &existing, &app.bars.rects, zone.vertical(), p),
        None => last,
    };
    let size = bars.iter().find(|b| b.id == id).map_or(vec2(120.0, THICK), |b| b.size(false));
    let pos = match at {
        Some((p, grab)) => clamp_to(app.bars.geom.window, size, p - grab),
        None => app.ui.bars.slots.get(id).map_or([80.0, 120.0], |s| s.pos),
    };
    app.ui.bars.place(id, zone, order, pos);
}

/// Process what the frame's widgets reported, then the drag in progress (rectangle, release,
/// Escape). Runs last in the frame so the geometry it measures against is the current one.
pub fn finish(app: &mut CadApp, ctx: &egui::Context, bars: &[Bar]) {
    for event in std::mem::take(&mut app.bars.events) {
        match event {
            Event::Action(action) => dispatch(app, &action),
            Event::MoveTo(id, zone) => place(app, bars, &id, zone, None),
            Event::Close(id) => crate::ai_tools::close_bar(app, &id),
            Event::ResetAll => app.ui.bars.reset(),
        }
    }
    layer_picker_area(app, ctx);
    let Some(drag) = app.bars.drag.clone() else { return };
    let pointer = ctx.input(|i| i.pointer.latest_pos());
    let (down, escape) = ctx.input(|i| (i.pointer.primary_down(), i.key_pressed(egui::Key::Escape)));
    let Some(p) = pointer else {
        app.bars.drag = None;
        return;
    };
    if escape {
        app.bars.drag = None;
        return;
    }
    let g = app.bars.geom;
    let zone = detect(&g, p);
    let size = vec2(drag.length, THICK);
    let (strip, bar) = preview(&g, zone, size, drag.grab, p);
    ctx.set_cursor_icon(CursorIcon::Grabbing);
    if down {
        paint_preview(ctx, zone, strip, bar, &drag, &Tokens::get());
        ctx.request_repaint();
        return;
    }
    app.bars.drag = None;
    place(app, bars, &drag.id, zone, Some((p, drag.grab)));
}

/// The Layers toolbar's list, open under (or beside) the toolbar until a click lands outside it or Escape.
fn layer_picker_area(app: &mut CadApp, ctx: &egui::Context) {
    let Some((pos, pivot)) = app.bars.layer_picker else { return };
    let t = Tokens::get();
    let shown = egui::Area::new(Id::unique("cc_layer_picker")).order(egui::Order::Foreground).pivot(pivot).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)).corner_radius(4.0).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            crate::palettes::layer_picker(app, ui);
        });
    });
    let (pressed_at, escape) = ctx.input(|i| (i.pointer.any_pressed().then(|| i.pointer.interact_pos()).flatten(), i.key_pressed(egui::Key::Escape)));
    let outside = pressed_at.is_some_and(|p| !shown.response.rect.contains(p) && !app.bars.rects.get(LAYERS_ID).is_some_and(|r| r.contains(p)));
    if outside || escape {
        app.bars.layer_picker = None;
    }
}

fn paint_preview(ctx: &egui::Context, zone: Zone, strip: Rect, bar: Rect, drag: &Drag, t: &Tokens) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("cc_bar_preview")));
    let accent = t.accent;
    if zone != Zone::Float {
        painter.rect_filled(strip, 0.0, accent.gamma_multiply(0.14));
        painter.rect_stroke(strip, 0.0, Stroke::new(1.0, accent.gamma_multiply(0.7)), egui::StrokeKind::Inside);
    }
    painter.rect_filled(bar, 4.0, accent.gamma_multiply(0.30));
    painter.rect_stroke(bar, 4.0, Stroke::new(2.0, accent), egui::StrokeKind::Inside);
    let label = format!("{} → {}", drag.title, crate::i18n::t(zone_name(zone)));
    painter.text(bar.center_top() - vec2(0.0, 4.0), egui::Align2::CENTER_BOTTOM, label, egui::FontId::proportional(12.0), Color32::WHITE);
}

fn zone_name(zone: Zone) -> &'static str {
    match zone {
        Zone::Top => crate::tl!("Above the menus"),
        Zone::BelowMenu => crate::tl!("Below the menus"),
        Zone::Left => crate::tl!("Left edge"),
        Zone::Right => crate::tl!("Right edge"),
        Zone::Bottom => crate::tl!("Bottom edge"),
        Zone::Float => crate::tl!("Floating"),
        Zone::Palette => crate::tl!("Tool Sets"),
    }
}

/// Run a button's action.
pub fn dispatch(app: &mut CadApp, action: &str) {
    if let Some(command) = action.strip_prefix("cmd:") {
        app.start(command);
    } else if action == "layers:picker" {
        let zone = app.ui.bars.zone(LAYERS_ID);
        app.bars.layer_picker = match (app.bars.layer_picker, app.bars.rects.get(LAYERS_ID)) {
            (Some(_), _) | (None, None) => None,
            (None, Some(r)) => Some(match zone {
                Zone::Left => (r.right_top(), egui::Align2::LEFT_TOP),
                Zone::Right => (r.left_top(), egui::Align2::RIGHT_TOP),
                Zone::Bottom => (r.left_top(), egui::Align2::LEFT_BOTTOM),
                _ => (r.left_bottom(), egui::Align2::LEFT_TOP),
            }),
        };
    } else if let Some(command) = action.strip_prefix("tool:") {
        app.cmd.buffer.clear();
        app.start(command);
    } else {
        crate::ai_tools::run_action(app, action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom() -> Geom {
        Geom {
            window: Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0)),
            top: Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 34.0)),
            top_visible: true,
            menu: Rect::from_min_size(pos2(0.0, 34.0), vec2(1200.0, 22.0)),
            work: Rect::from_min_max(pos2(0.0, 56.0), pos2(1200.0, 760.0)),
            ..Geom::default()
        }
    }

    fn bar(id: &str, n: usize, order: f32) -> Bar {
        let items =
            (0..n).map(|i| Item { glyph: Glyph::Text(i.to_string()), tip: String::new(), action: format!("cmd:c{i}"), enabled: true }).collect();
        Bar { id: id.into(), title: id.into(), items, closable: false, order }
    }

    #[test]
    fn the_pointer_picks_the_nearest_edge() {
        let g = geom();
        assert_eq!(detect(&g, pos2(600.0, 10.0)), Zone::Top, "over the tool bar row");
        assert_eq!(detect(&g, pos2(600.0, 70.0)), Zone::BelowMenu, "just under the text menus");
        assert_eq!(detect(&g, pos2(20.0, 400.0)), Zone::Left);
        assert_eq!(detect(&g, pos2(1190.0, 400.0)), Zone::Right);
        assert_eq!(detect(&g, pos2(600.0, 750.0)), Zone::Bottom);
        assert_eq!(detect(&g, pos2(600.0, 400.0)), Zone::Float, "far from every edge");
        assert_eq!(detect(&g, pos2(-20.0, 400.0)), Zone::Float, "outside the window");
        let mut hidden = g;
        hidden.top_visible = false;
        assert_eq!(detect(&hidden, pos2(600.0, 10.0)), Zone::BelowMenu, "a hidden tool bar is not a target");
    }

    #[test]
    fn the_corner_goes_to_the_closer_edge() {
        let g = geom();
        assert_eq!(detect(&g, pos2(10.0, 740.0)), Zone::Left, "10 from the left, 20 from the bottom");
        assert_eq!(detect(&g, pos2(40.0, 755.0)), Zone::Bottom, "5 from the bottom, 40 from the left");
    }

    #[test]
    fn the_preview_shows_the_bar_in_the_strip_it_would_join() {
        let g = geom();
        let size = vec2(130.0, THICK);
        let (strip, bar) = preview(&g, Zone::Left, size, vec2(6.0, 4.0), pos2(20.0, 300.0));
        assert!(strip.left() == 0.0 && strip.width() < 60.0 && strip.height() == g.work.height(), "{strip:?}");
        assert!(bar.width() == THICK && bar.height() == 130.0 && strip.contains_rect(bar), "{bar:?} in {strip:?}");
        let (strip, bar) = preview(&g, Zone::Right, size, vec2(6.0, 4.0), pos2(1190.0, 300.0));
        assert!(strip.right() == 1200.0 && strip.contains_rect(bar));
        let (strip, bar) = preview(&g, Zone::BelowMenu, size, vec2(6.0, 4.0), pos2(500.0, 70.0));
        assert!(strip.top() == 56.0 && strip.width() == 1200.0 && strip.contains_rect(bar));
        assert_eq!(bar.min.x, 494.0, "the bar follows the pointer along the strip");
        let (strip, bar) = preview(&g, Zone::Bottom, size, vec2(6.0, 4.0), pos2(500.0, 750.0));
        assert!(strip.bottom() == 760.0 && strip.contains_rect(bar));
        let (_, bar) = preview(&g, Zone::Float, size, vec2(6.0, 4.0), pos2(500.0, 400.0));
        assert_eq!(bar.min, pos2(494.0, 396.0));
        // An existing strip is shown whole.
        let mut with = g;
        with.left_w = 40.0;
        let (strip, _) = preview(&with, Zone::Left, size, Vec2::ZERO, pos2(20.0, 300.0));
        assert_eq!(strip.width(), 40.0);
    }

    #[test]
    fn a_bar_dropped_between_two_lands_between_them() {
        let bars = [bar("a", 2, 0.0), bar("b", 2, 1.0), bar("c", 2, 2.0)];
        let layout = Layout::default();
        let rects: HashMap<String, Rect> = [("a", 0.0), ("b", 100.0), ("c", 200.0)]
            .into_iter()
            .map(|(id, x)| (id.to_string(), Rect::from_min_size(pos2(x, 0.0), vec2(80.0, 34.0))))
            .collect();
        let all: Vec<&Bar> = bars.iter().collect();
        assert_eq!(insertion_order(&layout, &all, &rects, false, pos2(10.0, 10.0)), -1.0, "before the first");
        assert_eq!(insertion_order(&layout, &all, &rects, false, pos2(120.0, 10.0)), 0.5, "left of b's middle: between a (0) and b (1)");
        assert_eq!(insertion_order(&layout, &all, &rects, false, pos2(160.0, 10.0)), 1.5, "between b (1) and c (2)");
        assert_eq!(insertion_order(&layout, &all, &rects, false, pos2(900.0, 10.0)), 3.0, "after the last");
        assert_eq!(insertion_order(&layout, &[], &rects, false, pos2(5.0, 10.0)), 0.0, "into an empty strip");
    }

    #[test]
    fn long_rows_wrap_instead_of_hiding_bars() {
        assert_eq!(wrap(&[100.0, 100.0, 100.0], 500.0), vec![vec![0, 1, 2]]);
        assert_eq!(wrap(&[100.0, 100.0, 100.0], 230.0), vec![vec![0, 1], vec![2]]);
        assert_eq!(wrap(&[300.0, 100.0], 200.0), vec![vec![0], vec![1]], "a bar longer than the strip still gets a line");
        assert!(wrap(&[], 200.0).is_empty());
    }

    #[test]
    fn the_layout_keeps_what_the_person_moved_and_nothing_else() {
        let mut l = Layout::default();
        assert_eq!(l.zone("main.0"), Zone::Top);
        assert_eq!(l.zone("ts.Drafting.Draw"), Zone::Palette);
        assert_eq!(l.zone("ai"), Zone::Float);
        l.place("main.3", Zone::Left, 2.0, [0.0, 0.0]);
        l.place("ts.Drafting.Draw", Zone::BelowMenu, 0.5, [0.0, 0.0]);
        assert_eq!(l.zone("main.3"), Zone::Left);
        let json = serde_json::to_string(&l).unwrap();
        let back: Layout = serde_json::from_str(&json).unwrap();
        assert_eq!(back, l);
        l.place("ts.Drafting.Draw", Zone::Palette, 0.0, [0.0, 0.0]);
        assert!(!l.slots.contains_key("ts.Drafting.Draw"), "back in the palette it has no slot");
        assert_eq!(serde_json::from_str::<Layout>("{}").unwrap(), Layout::default(), "an old settings file has no bars");
        l.reset();
        assert_eq!(l.zone("main.3"), Zone::Top);
    }

    #[test]
    fn bars_come_back_in_order_per_zone() {
        let bars = [bar("main.0", 2, 0.0), bar("main.1", 2, 1.0), bar("main.2", 2, 2.0)];
        let mut l = Layout::default();
        l.place("main.2", Zone::Top, -1.0, [0.0, 0.0]);
        l.place("main.1", Zone::Right, 0.0, [0.0, 0.0]);
        let top: Vec<&str> = in_zone(&bars, &l, Zone::Top).iter().map(|b| b.id.as_str()).collect();
        assert_eq!(top, ["main.2", "main.0"]);
        let right: Vec<&str> = in_zone(&bars, &l, Zone::Right).iter().map(|b| b.id.as_str()).collect();
        assert_eq!(right, ["main.1"]);
    }
}
