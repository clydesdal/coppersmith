//! The open board, what's selected, what the editor is in the middle of, and
//! the edits themselves. Drawing and clicking are in view.rs, the panels in
//! panels.rs.

use crate::copper::{self, Copper, Item};
use crate::drc::{self, Problem};
use crate::geom::*;
use crate::library::{FootprintDef, Library};
use crate::model::*;
use crate::pinch::Pinch;
use crate::theme;
use eframe::egui::{self, ColorImage, Key, Panel, Rect, Ui, vec2};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Route,
    Via,
    Outline,
    Place,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Sel {
    Fp(u32),
    Track(u32),
    Via(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Mm,
    Mil,
    Inch,
}

impl Unit {
    pub fn name(self) -> &'static str {
        match self {
            Unit::Mm => "mm",
            Unit::Mil => "mil",
            Unit::Inch => "in",
        }
    }
    pub fn from_mm(self, mm: f64) -> f64 {
        match self {
            Unit::Mm => mm,
            Unit::Mil => mm / 0.0254,
            Unit::Inch => mm / 25.4,
        }
    }
    pub fn to_mm(self, v: f64) -> f64 {
        match self {
            Unit::Mm => v,
            Unit::Mil => v * 0.0254,
            Unit::Inch => v * 25.4,
        }
    }
    pub fn decimals(self) -> usize {
        match self {
            Unit::Mm => 3,
            Unit::Mil => 1,
            Unit::Inch => 4,
        }
    }
    pub fn num(self, mm: f64) -> String {
        format!("{:.*}", self.decimals(), self.from_mm(mm))
    }
    pub fn len(self, mm: f64) -> String {
        format!("{} {}", self.num(mm), self.name())
    }
}

/// A route being laid: the segments so far, and the vias on the way.
#[derive(Clone, Debug)]
pub struct Route {
    pub segs: Vec<(P, P, u8)>,
    pub vias: Vec<P>,
    pub from: P,
    pub layer: u8,
    pub net: u32,
    pub width: f64,
    pub diagonal_first: bool,
    /// What it started on, which doesn't count as arriving.
    pub start: Option<Item>,
}

#[derive(Clone, Debug)]
pub enum DragKind {
    Move,
    /// One end of a track; `true` for the far end.
    End(u32, bool),
}

#[derive(Clone, Debug)]
pub struct Drag {
    pub kind: DragKind,
    pub before: Board,
    pub grab: P,
    /// The point that lands on the grid.
    pub reference: P,
    pub moved: bool,
}

#[derive(Default)]
pub struct Launch {
    pub path: Option<PathBuf>,
    pub screenshot: Option<PathBuf>,
    pub play: Option<String>,
    pub zoom: Option<(P, f64)>,
    pub select: Option<String>,
}

pub enum Asked {
    Open(PathBuf),
    SaveAs(PathBuf),
    Export(PathBuf),
}

/// What waits on the "save changes?" question.
#[derive(Clone, Debug)]
pub enum Then {
    New,
    Open(PathBuf),
    Quit,
}

pub const GRIDS: [f64; 8] = [0.05, 0.1, 0.25, 0.5, 1.0, 0.635, 1.27, 2.54];
pub const WIDTHS: [f64; 9] = [0.15, 0.2, 0.25, 0.3, 0.4, 0.5, 0.8, 1.0, 1.5];

pub struct App {
    pub doc: Board,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    undo: Vec<Board>,
    redo: Vec<Board>,
    /// Goes up with every change, so what's worked out from the board is redone.
    pub rev: u64,
    pub copper: Copper,
    copper_rev: u64,
    pub problems: Vec<Problem>,
    pub lib: Library,
    pub lib_filter: String,

    pub center: P,
    pub scale: f64,
    pub mirror: bool,
    pub fit_pending: bool,
    pub fitted: bool,
    pub canvas_rect: Rect,

    pub tool: Tool,
    pub layer: u8,
    pub hidden: HashSet<Layer>,
    pub show_rats: bool,
    pub show_problems: bool,
    pub sel: Vec<Sel>,
    pub hover: Option<Sel>,
    pub hover_q: Option<P>,
    pub hover_text: Option<String>,
    pub cursor: Option<P>,
    pub drag: Option<Drag>,
    pub box_from: Option<egui::Pos2>,
    pub route: Option<Route>,
    pub outline: Vec<P>,
    pub placing: Option<(Footprint, String)>,
    pub grid: f64,
    pub show_grid: bool,
    pub unit: Unit,
    pub width: f64,
    pub highlight: Option<u32>,
    pub focus: Option<usize>,
    pub notice: Option<String>,
    pub exported: Option<PathBuf>,
    pub new_net: String,
    pub show_left: bool,
    pub show_right: bool,
    pub last_field_edit: f64,
    pub confirm: Option<Then>,
    after_save: Option<Then>,
    may_close: bool,

    pub pinch: Pinch,
    pub logo: Option<egui::TextureHandle>,
    pub asked_tx: mpsc::Sender<Asked>,
    asked_rx: mpsc::Receiver<Asked>,
    pub launch: Launch,
    pub play: Vec<crate::play::Step>,
    pub frames: u32,
    pub shot_sent: bool,
    applied_launch: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, launch: Launch) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        cc.egui_ctx.set_visuals(theme::visuals());
        cc.egui_ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(10.0, 5.0);
            s.spacing.interact_size.y = 24.0;
            s.visuals.handle_shape = egui::style::HandleShape::Circle;
            s.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
            s.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
            s.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        });
        let (asked_tx, asked_rx) = mpsc::channel();
        let doc = crate::sample::board();
        let mut hidden = HashSet::new();
        hidden.insert(Layer::Fab(Side::Top));
        hidden.insert(Layer::Fab(Side::Bottom));
        let mut app = App {
            width: doc.rules.track,
            doc,
            path: None,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            rev: 1,
            copper: Copper::default(),
            copper_rev: 0,
            problems: Vec::new(),
            lib: Library::load(),
            lib_filter: String::new(),
            center: P::default(),
            scale: 10.0,
            mirror: false,
            fit_pending: true,
            fitted: true,
            canvas_rect: Rect::NOTHING,
            tool: Tool::Select,
            layer: TOP,
            hidden,
            show_rats: true,
            show_problems: true,
            sel: Vec::new(),
            hover: None,
            hover_q: None,
            hover_text: None,
            cursor: None,
            drag: None,
            box_from: None,
            route: None,
            outline: Vec::new(),
            placing: None,
            grid: 0.25,
            show_grid: true,
            unit: Unit::Mm,
            highlight: None,
            focus: None,
            notice: None,
            exported: None,
            new_net: String::new(),
            show_left: true,
            show_right: true,
            last_field_edit: -10.0,
            confirm: None,
            after_save: None,
            may_close: false,
            pinch: Pinch::default(),
            logo: crate::icons::picture_pixels(64).map(|(pixels, side)| {
                let side = side as usize;
                let picture = ColorImage::from_rgba_unmultiplied([side, side], &pixels);
                cc.egui_ctx.load_texture("logo", picture, egui::TextureOptions::LINEAR)
            }),
            asked_tx,
            asked_rx,
            play: crate::play::parse(launch.play.as_deref().unwrap_or("")),
            launch,
            frames: 0,
            shot_sent: false,
            applied_launch: false,
        };
        if let Ok(handle) = cc.display_handle()
            && let RawDisplayHandle::Wayland(wayland) = handle.as_raw()
        {
            app.pinch.listen(wayland.display.as_ptr(), &cc.egui_ctx);
        }
        if let Some(path) = app.launch.path.take() {
            app.open(path);
        }
        if let Some(problem) = app.lib.problems.first() {
            app.notice = Some(format!("A footprint file couldn't be read: {problem}"));
        }
        app
    }

    // -- What's worked out from the board ------------------------------------

    /// Brings the copper groups and the checks up to date with the board.
    pub fn refresh(&mut self) {
        if self.copper_rev != self.rev {
            self.copper = copper::analyse(&self.doc);
            self.problems = drc::check(&self.doc, &self.copper);
            self.copper_rev = self.rev;
            if self.focus.is_some_and(|i| i >= self.problems.len()) {
                self.focus = None;
            }
        }
    }

    pub fn errors(&self) -> usize {
        self.problems.iter().filter(|p| p.severity == drc::Severity::Error).count()
    }

    // -- Changing the board ----------------------------------------------------

    /// Keeps the board as it was, so the change can be undone.
    pub fn remember(&mut self, before: Board) {
        self.undo.push(before);
        if self.undo.len() > 300 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.touch();
    }

    /// The board changed, without a new undo step.
    pub fn touch(&mut self) {
        self.rev += 1;
        self.dirty = true;
    }

    pub fn edit(&mut self, f: impl FnOnce(&mut Board)) {
        let before = self.doc.clone();
        f(&mut self.doc);
        if self.doc != before {
            self.remember(before);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        self.cancel();
        if let Some(b) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.doc, b));
            self.after_history();
        }
    }

    pub fn redo(&mut self) {
        self.cancel();
        if let Some(b) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.doc, b));
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        self.touch();
        let doc = &self.doc;
        self.sel.retain(|s| match *s {
            Sel::Fp(id) => doc.fp(id).is_some(),
            Sel::Track(id) => doc.track(id).is_some(),
            Sel::Via(id) => doc.via(id).is_some(),
        });
    }

    /// Stops whatever is half done: a route, an outline, a drag.
    pub fn cancel(&mut self) {
        if let Some(d) = self.drag.take() {
            self.doc = d.before;
            self.touch();
        }
        self.route = None;
        self.outline.clear();
        self.box_from = None;
    }

    pub fn set_tool(&mut self, tool: Tool) {
        if tool != Tool::Route {
            self.finish_route();
        }
        self.outline.clear();
        if tool != Tool::Place {
            self.placing = None;
        }
        self.tool = tool;
    }

    pub fn start_placing(&mut self, def: &FootprintDef) {
        self.finish_route();
        let prefix = def.prefix.clone();
        let mut f = def.instantiate(0, self.doc.next_refdes(&prefix), self.cursor.unwrap_or(self.center));
        f.side = if self.layer == BOTTOM { Side::Bottom } else { Side::Top };
        self.placing = Some((f, prefix));
        self.tool = Tool::Place;
        self.sel.clear();
    }

    pub fn place_here(&mut self) {
        let Some((mut f, prefix)) = self.placing.clone() else { return };
        let before = self.doc.clone();
        f.id = self.doc.new_id();
        f.refdes = self.doc.next_refdes(&prefix);
        let id = f.id;
        self.doc.footprints.push(f);
        self.remember(before);
        self.sel = vec![Sel::Fp(id)];
        if let Some((ghost, _)) = &mut self.placing {
            ghost.refdes = self.doc.next_refdes(&prefix);
        }
    }

    /// Moves what's selected, dragging the ends of tracks that were joined to it.
    pub fn move_selection(board: &mut Board, sel: &[Sel], d: P) {
        if d.len() == 0.0 {
            return;
        }
        let mut anchors: Vec<P> = Vec::new();
        let picked: HashSet<Sel> = sel.iter().copied().collect();
        for f in board.footprints.iter_mut().filter(|f| picked.contains(&Sel::Fp(f.id)) && !f.locked) {
            anchors.extend((0..f.pads.len()).map(|i| f.pad_center(i)));
            f.at = f.at.add(d).rounded();
        }
        for v in board.vias.iter_mut().filter(|v| picked.contains(&Sel::Via(v.id))) {
            anchors.push(v.at);
            v.at = v.at.add(d).rounded();
        }
        for t in board.tracks.iter_mut().filter(|t| picked.contains(&Sel::Track(t.id))) {
            anchors.push(t.a);
            anchors.push(t.b);
            t.a = t.a.add(d).rounded();
            t.b = t.b.add(d).rounded();
        }
        for t in board.tracks.iter_mut().filter(|t| !picked.contains(&Sel::Track(t.id))) {
            if anchors.iter().any(|q| q.same(t.a)) {
                t.a = t.a.add(d).rounded();
            }
            if anchors.iter().any(|q| q.same(t.b)) {
                t.b = t.b.add(d).rounded();
            }
        }
    }

    /// Moves one end of a track, and any other track ends or vias sitting on it.
    pub fn move_end(board: &mut Board, id: u32, far: bool, to: P) {
        let Some(t) = board.track(id) else { return };
        let from = if far { t.b } else { t.a };
        for t in &mut board.tracks {
            if t.a.same(from) {
                t.a = to;
            }
            if t.b.same(from) {
                t.b = to;
            }
        }
        for v in &mut board.vias {
            if v.at.same(from) {
                v.at = to;
            }
        }
    }

    pub fn rotate_selection(&mut self, deg: f64) {
        if let Some((f, _)) = &mut self.placing {
            f.rot = (f.rot + deg).rem_euclid(360.0);
            return;
        }
        let sel = self.sel.clone();
        let fps: Vec<u32> = sel.iter().filter_map(|s| if let Sel::Fp(id) = s { Some(*id) } else { None }).collect();
        self.edit(|b| {
            for id in fps {
                let Some(f) = b.fp(id) else { continue };
                if f.locked {
                    continue;
                }
                let before: Vec<P> = (0..f.pads.len()).map(|i| f.pad_center(i)).collect();
                let f = b.fp_mut(id).unwrap();
                f.rot = (f.rot + deg).rem_euclid(360.0);
                let after: Vec<P> = (0..f.pads.len()).map(|i| f.pad_center(i)).collect();
                // Tracks follow their pads round.
                for t in &mut b.tracks {
                    for (from, to) in before.iter().zip(&after) {
                        if t.a.same(*from) {
                            t.a = *to;
                        }
                        if t.b.same(*from) {
                            t.b = *to;
                        }
                    }
                }
            }
        });
    }

    pub fn flip_selection(&mut self) {
        if let Some((f, _)) = &mut self.placing {
            f.side = f.side.other();
            return;
        }
        let sel = self.sel.clone();
        self.edit(|b| {
            for s in &sel {
                if let Sel::Fp(id) = s
                    && let Some(f) = b.fp_mut(*id)
                    && !f.locked
                {
                    f.side = f.side.other();
                }
                if let Sel::Track(id) = s
                    && let Some(t) = b.tracks.iter_mut().find(|t| t.id == *id)
                {
                    t.layer = if t.layer == TOP { BOTTOM } else { TOP };
                }
            }
        });
    }

    pub fn delete_selection(&mut self) {
        let sel: HashSet<Sel> = self.sel.drain(..).collect();
        self.edit(|b| {
            b.footprints.retain(|f| !sel.contains(&Sel::Fp(f.id)) || f.locked);
            b.tracks.retain(|t| !sel.contains(&Sel::Track(t.id)));
            b.vias.retain(|v| !sel.contains(&Sel::Via(v.id)));
        });
    }

    pub fn duplicate_selection(&mut self) {
        let sel = self.sel.clone();
        let step = (self.grid * 8.0).max(1.0);
        let mut made = Vec::new();
        self.edit(|b| {
            for s in &sel {
                match *s {
                    Sel::Fp(id) => {
                        let Some(mut f) = b.fp(id).cloned() else { continue };
                        let prefix: String = f.refdes.chars().take_while(|c| c.is_alphabetic()).collect();
                        f.id = b.new_id();
                        f.refdes = b.next_refdes(if prefix.is_empty() { "U" } else { &prefix });
                        f.at = f.at.add(p(step, -step)).rounded();
                        f.locked = false;
                        made.push(Sel::Fp(f.id));
                        b.footprints.push(f);
                    }
                    Sel::Track(id) => {
                        let Some(mut t) = b.track(id).cloned() else { continue };
                        t.id = b.new_id();
                        t.a = t.a.add(p(step, -step)).rounded();
                        t.b = t.b.add(p(step, -step)).rounded();
                        made.push(Sel::Track(t.id));
                        b.tracks.push(t);
                    }
                    Sel::Via(id) => {
                        let Some(mut v) = b.via(id).cloned() else { continue };
                        v.id = b.new_id();
                        v.at = v.at.add(p(step, -step)).rounded();
                        made.push(Sel::Via(v.id));
                        b.vias.push(v);
                    }
                }
            }
        });
        if !made.is_empty() {
            self.sel = made;
        }
    }

    pub fn select_all(&mut self) {
        self.sel = self
            .doc
            .footprints
            .iter()
            .map(|f| Sel::Fp(f.id))
            .chain(self.doc.tracks.iter().map(|t| Sel::Track(t.id)))
            .chain(self.doc.vias.iter().map(|v| Sel::Via(v.id)))
            .collect();
    }

    // -- Routing ---------------------------------------------------------------

    /// The two legs from `a` to `b`: one straight, one at 45°.
    pub fn legs(a: P, b: P, diagonal_first: bool) -> Vec<P> {
        let d = b.sub(a);
        let (ax, ay) = (d.x.abs(), d.y.abs());
        if ax < 1e-9 || ay < 1e-9 || (ax - ay).abs() < 1e-9 {
            return vec![a, b];
        }
        let m = ax.min(ay);
        let diag = p(m * d.x.signum(), m * d.y.signum());
        let corner = if diagonal_first { a.add(diag) } else { b.sub(diag) };
        vec![a, corner.rounded(), b]
    }

    /// Where a click in the route tool lands: a pad, via or track end of the
    /// right sort under the pointer, or else the grid.
    pub fn route_snap(&self, q: P, tol: f64) -> (P, Option<Item>) {
        for cu in self.copper.at(q, self.layer, tol) {
            match cu.item {
                Item::Pad(..) | Item::Via(_) => return (cu.anchors[0], Some(cu.item)),
                Item::Track(_) => {
                    let (a, b) = (cu.anchors[0], cu.anchors[1]);
                    for end in [a, b] {
                        if end.dist(q) < tol.max(cu.blob.r * 1.5) {
                            return (end, Some(cu.item));
                        }
                    }
                    // Anywhere along a track, square to the grid where it can be.
                    let on = dist_to_segment(q.snap(self.grid), a, b);
                    let at = if on.0 < 1e-6 { q.snap(self.grid) } else { dist_to_segment(q, a, b).1.rounded() };
                    return (at, Some(cu.item));
                }
            }
        }
        (q.snap(self.grid), None)
    }

    pub fn start_route(&mut self, q: P, tol: f64) {
        let (at, item) = self.route_snap(q, tol);
        let net = item.and_then(|i| self.copper.index.get(&i)).map(|&i| self.copper.items[i].net).unwrap_or(0);
        self.route = Some(Route {
            segs: Vec::new(),
            vias: Vec::new(),
            from: at,
            layer: self.layer,
            net,
            width: self.width,
            diagonal_first: false,
            start: item,
        });
    }

    /// Lays the legs to `to`. True when it arrived on its net, which ends it.
    pub fn route_to(&mut self, to: P, item: Option<Item>) -> bool {
        let net_of = |app: &App, i: Item| app.copper.index.get(&i).map(|&k| app.copper.items[k].net).unwrap_or(0);
        let target = item.map(|i| net_of(self, i));
        let Some(r) = &mut self.route else { return false };
        let legs = App::legs(r.from, to, r.diagonal_first);
        for w in legs.windows(2) {
            if !w[0].same(w[1]) {
                r.segs.push((w[0], w[1], r.layer));
            }
        }
        r.from = to;
        if r.net == 0
            && let Some(n) = target
        {
            r.net = n;
        }
        let arrived = item.is_some() && item != r.start && target == Some(r.net) && r.net != 0 && !r.segs.is_empty();
        if arrived {
            self.finish_route();
        }
        arrived
    }

    pub fn route_via(&mut self) {
        let other = |l: u8| if l == TOP { BOTTOM } else { TOP };
        let Some(r) = &mut self.route else { return };
        if r.vias.last().is_some_and(|v| v.same(r.from)) {
            return;
        }
        r.vias.push(r.from);
        r.layer = other(r.layer);
        self.layer = r.layer;
    }

    pub fn route_back(&mut self) {
        let Some(r) = &mut self.route else { return };
        match r.segs.pop() {
            Some((a, _, layer)) => {
                r.from = a;
                if r.vias.last().is_some_and(|v| v.same(a)) && layer != r.layer {
                    r.vias.pop();
                }
                r.layer = layer;
                self.layer = layer;
            }
            None => self.route = None,
        }
    }

    /// Puts the route's tracks and vias on the board.
    pub fn finish_route(&mut self) {
        let Some(r) = self.route.take() else { return };
        if r.segs.is_empty() {
            return;
        }
        let rules = self.doc.rules.clone();
        self.edit(|b| {
            for (a, e, layer) in &r.segs {
                let id = b.new_id();
                b.tracks.push(Track { id, a: *a, b: *e, w: r.width, layer: *layer, net: r.net });
            }
            for v in &r.vias {
                if b.vias.iter().any(|old| old.at.same(*v)) {
                    continue;
                }
                let id = b.new_id();
                b.vias.push(Via { id, at: *v, dia: rules.via_dia, drill: rules.via_drill, net: r.net });
            }
            merge_straight(b);
        });
    }

    pub fn place_via(&mut self, q: P, tol: f64) {
        let mut at = q.snap(self.grid);
        let mut net = 0;
        for layer in self.doc.copper_layers() {
            if let Some(cu) = self.copper.at(q, layer, tol).first() {
                net = cu.net;
                if !matches!(cu.item, Item::Track(_)) {
                    at = cu.anchors[0];
                } else {
                    at = dist_to_segment(q, cu.anchors[0], cu.anchors[1]).1.rounded();
                }
                break;
            }
        }
        let rules = self.doc.rules.clone();
        self.edit(|b| {
            if !b.vias.iter().any(|v| v.at.same(at)) {
                let id = b.new_id();
                b.vias.push(Via { id, at, dia: rules.via_dia, drill: rules.via_drill, net });
            }
        });
    }

    pub fn close_outline(&mut self) {
        if self.outline.len() < 3 {
            self.notice = Some("An outline needs three corners or more.".into());
            return;
        }
        let mut ring = std::mem::take(&mut self.outline);
        if ring_area(&ring) < 0.0 {
            ring.reverse();
        }
        self.edit(|b| b.outline = ring);
        self.fit_pending = true;
    }

    // -- Viewing -------------------------------------------------------------

    pub fn fit(&mut self, rect: Rect) {
        const SIDES: f32 = 40.0;
        const ABOVE: f32 = 40.0;
        const BELOW: f32 = 70.0;
        let b = self.doc.extent();
        let sx = (rect.width() - SIDES).max(50.0) as f64 / b.w().max(0.1);
        let sy = (rect.height() - ABOVE - BELOW).max(50.0) as f64 / b.h().max(0.1);
        self.scale = sx.min(sy);
        self.center = b.center();
        self.center.y -= ((BELOW - ABOVE) / 2.0) as f64 / self.scale;
        self.fitted = true;
    }

    pub fn zoom_to(&mut self, at: P, width: f64) {
        let room = self.canvas_rect.width().min(self.canvas_rect.height()).max(200.0) as f64;
        self.scale = (room / width.max(0.5)).clamp(2.0, 2000.0);
        self.center = at;
        self.fitted = false;
    }

    pub fn active_side(&self) -> Side {
        if self.layer == BOTTOM { Side::Bottom } else { Side::Top }
    }

    pub fn next_layer(&mut self, step: i32) {
        let layers = self.doc.copper_layers();
        let i = layers.iter().position(|l| *l == self.layer).unwrap_or(0) as i32;
        let n = layers.len() as i32;
        let next = layers[(i + step).rem_euclid(n) as usize];
        self.switch_layer(next);
    }

    pub fn switch_layer(&mut self, layer: u8) {
        if self.route.as_ref().is_some_and(|r| r.layer != layer) {
            // Changing layer mid-route drops a via where the route is.
            if let Some(r) = &mut self.route {
                if !r.vias.last().is_some_and(|v| v.same(r.from)) {
                    r.vias.push(r.from);
                }
                r.layer = layer;
            }
        }
        self.layer = layer;
        self.hidden.remove(&Layer::Cu(layer));
    }

    // -- Files ---------------------------------------------------------------

    pub fn title(&self) -> String {
        let name = match &self.path {
            Some(p) => p.file_name().unwrap_or_default().to_string_lossy().to_string(),
            None => self.doc.name.clone(),
        };
        if self.dirty { format!("{name}  •") } else { name }
    }

    pub fn new_board(&mut self) {
        if self.dirty {
            self.confirm = Some(Then::New);
            return;
        }
        let before = self.doc.clone();
        self.cancel();
        self.doc = Board::default();
        self.path = None;
        self.sel.clear();
        self.highlight = None;
        self.remember(before);
        self.dirty = false;
        self.fit_pending = true;
    }

    pub fn open(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Board::load(&t)) {
            Ok(b) => {
                let before = std::mem::replace(&mut self.doc, b);
                self.cancel();
                self.remember(before);
                self.dirty = false;
                self.path = Some(path);
                self.sel.clear();
                self.highlight = None;
                self.width = self.doc.rules.track;
                self.fit_pending = true;
            }
            Err(e) => self.notice = Some(format!("{} couldn't be opened: {e}", path.display())),
        }
    }

    pub fn save_to(&mut self, path: PathBuf) {
        let path = if path.extension().is_none() { path.with_extension("cspcb") } else { path };
        match std::fs::write(&path, self.doc.save()) {
            Ok(()) => {
                self.notice = Some(format!("Saved {}", path.display()));
                self.path = Some(path);
                self.dirty = false;
            }
            Err(e) => self.notice = Some(format!("{} couldn't be saved: {e}", path.display())),
        }
    }

    pub fn save(&mut self, ctx: &egui::Context) {
        match self.path.clone() {
            Some(path) => self.save_to(path),
            None => self.ask_save_as(ctx),
        }
    }

    pub fn ask_save_as(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.asked_tx.clone(), ctx.clone());
        let name = format!("{}.cspcb", self.doc.name);
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new()
                .set_title("Save the board")
                .set_file_name(&name)
                .add_filter("Coppersmith board", &["cspcb"]);
            if let Some(path) = dialog.save_file() {
                let _ = tx.send(Asked::SaveAs(path));
                ctx.request_repaint();
            }
        });
    }

    pub fn ask_open(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.asked_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new().set_title("Open a board").add_filter("Coppersmith board", &["cspcb", "json"]);
            if let Some(path) = dialog.pick_file() {
                let _ = tx.send(Asked::Open(path));
                ctx.request_repaint();
            }
        });
    }

    pub fn ask_export(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.asked_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new().set_title("Choose a folder for the Gerber files").pick_folder() {
                let _ = tx.send(Asked::Export(path));
                ctx.request_repaint();
            }
        });
    }

    pub fn export_to(&mut self, dir: PathBuf) {
        self.refresh();
        match crate::gerber::write(&self.doc, &dir) {
            Ok(files) => {
                let errors = self.errors();
                let warn = if errors > 0 { format!(" The checks found {errors} problems.") } else { String::new() };
                self.notice = Some(format!("Wrote {} files to {}.{warn}", files.len(), dir.display()));
                self.exported = Some(dir);
            }
            Err(e) => self.notice = Some(e),
        }
    }

    /// Does what was waiting, now the changes are saved or let go.
    fn go_on(&mut self, then: Then) {
        self.dirty = false;
        match then {
            Then::New => self.new_board(),
            Then::Open(path) => self.open(path),
            Then::Quit => self.may_close = true,
        }
    }

    fn save_changes_dialog(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty && !self.may_close && self.launch.screenshot.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Then::Quit);
        }
        if self.may_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let Some(then) = self.confirm.clone() else { return };
        let frame = egui::Frame::new()
            .fill(theme::RAISED)
            .stroke(egui::Stroke::new(1.0, theme::LINE))
            .corner_radius(egui::CornerRadius::same(theme::CARD_RADIUS))
            .inner_margin(egui::Margin::same(20));
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::unique("save-changes")).frame(frame).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(egui::RichText::new(format!("Save the changes to {}?", self.doc.name)).font(theme::semibold(16.0)));
            ui.add_space(4.0);
            ui.label(egui::RichText::new("They'll be lost if they aren't saved.").color(theme::DIM));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(0);
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(1);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(2);
                }
            });
        });
        if modal.should_close() && choice.is_none() {
            choice = Some(2);
        }
        match choice {
            Some(0) => {
                self.confirm = None;
                match self.path.clone() {
                    Some(path) => {
                        self.save_to(path);
                        if !self.dirty {
                            self.go_on(then);
                        }
                    }
                    None => {
                        self.after_save = Some(then);
                        self.ask_save_as(ctx);
                    }
                }
            }
            Some(1) => {
                self.confirm = None;
                self.go_on(then);
            }
            Some(2) => self.confirm = None,
            _ => {}
        }
    }

    pub fn copperview() -> Option<PathBuf> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let installed = home.join("Applications/copperview");
        installed.exists().then_some(installed)
    }

    pub fn view_in_copperview(&mut self) {
        let (Some(dir), Some(program)) = (self.exported.clone(), App::copperview()) else { return };
        if let Err(e) = std::process::Command::new(program).arg(&dir).spawn() {
            self.notice = Some(format!("Copperview couldn't start: {e}"));
        }
    }

    fn poll(&mut self) {
        while let Ok(asked) = self.asked_rx.try_recv() {
            match asked {
                Asked::Open(path) if self.dirty => self.confirm = Some(Then::Open(path)),
                Asked::Open(path) => self.open(path),
                Asked::SaveAs(path) => {
                    self.save_to(path);
                    if !self.dirty
                        && let Some(then) = self.after_save.take()
                    {
                        self.go_on(then);
                    }
                }
                Asked::Export(path) => self.export_to(path),
            }
        }
    }

    // -- Keys ----------------------------------------------------------------

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        // Each press is matched with the keys held with it, so Ctrl+S never
        // also counts as S.
        let presses: Vec<(Key, egui::Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        let command = presses.iter().any(|(_, m)| m.command);
        let shifted = presses.iter().any(|(_, m)| m.shift);
        let pressed = |k: Key| presses.iter().any(|(key, m)| *key == k && m.command == command);
        let m = egui::Modifiers { shift: shifted, command, ..Default::default() };
        if m.command {
            if pressed(Key::Z) {
                if m.shift { self.redo() } else { self.undo() }
            }
            if pressed(Key::Y) {
                self.redo();
            }
            if pressed(Key::S) {
                if m.shift { self.ask_save_as(ctx) } else { self.save(ctx) }
            }
            if pressed(Key::O) {
                self.ask_open(ctx);
            }
            if pressed(Key::N) {
                self.new_board();
            }
            if pressed(Key::E) {
                self.ask_export(ctx);
            }
            if pressed(Key::A) {
                self.select_all();
            }
            if pressed(Key::D) {
                self.duplicate_selection();
            }
            return;
        }
        if pressed(Key::Escape) {
            if self.route.is_some() {
                self.finish_route();
            } else if !self.outline.is_empty() || self.drag.is_some() {
                self.cancel();
            } else if self.tool != Tool::Select {
                self.set_tool(Tool::Select);
            } else if self.highlight.is_some() {
                self.highlight = None;
            } else {
                self.sel.clear();
                self.focus = None;
            }
        }
        if pressed(Key::Enter) {
            if self.route.is_some() {
                self.finish_route();
            }
            if self.tool == Tool::Outline {
                self.close_outline();
            }
        }
        if pressed(Key::Backspace) {
            if self.route.is_some() {
                self.route_back();
            } else if !self.outline.is_empty() {
                self.outline.pop();
            } else {
                self.delete_selection();
            }
        }
        if pressed(Key::Delete) {
            self.delete_selection();
        }
        if pressed(Key::S) {
            self.set_tool(Tool::Select);
        }
        if pressed(Key::W) {
            self.set_tool(Tool::Route);
        }
        if pressed(Key::V) {
            if self.route.is_some() {
                self.route_via();
            } else {
                self.set_tool(Tool::Via);
            }
        }
        if pressed(Key::O) {
            self.set_tool(Tool::Outline);
        }
        if pressed(Key::Slash) || pressed(Key::Space) {
            if let Some(r) = &mut self.route {
                r.diagonal_first = !r.diagonal_first;
            }
        }
        if pressed(Key::R) {
            self.rotate_selection(if m.shift { -90.0 } else { 90.0 });
        }
        if pressed(Key::L) {
            self.flip_selection();
        }
        if pressed(Key::F) {
            self.fit_pending = true;
        }
        if pressed(Key::X) {
            self.mirror = !self.mirror;
        }
        if pressed(Key::G) {
            self.show_grid = !self.show_grid;
        }
        if pressed(Key::U) {
            self.unit = match self.unit {
                Unit::Mm => Unit::Mil,
                Unit::Mil => Unit::Inch,
                Unit::Inch => Unit::Mm,
            };
        }
        if pressed(Key::N) {
            self.highlight = match (self.highlight, self.hover_net()) {
                (Some(h), Some(n)) if h == n => None,
                (_, Some(n)) => Some(n),
                _ => None,
            };
        }
        if pressed(Key::PageUp) {
            self.switch_layer(TOP);
        }
        if pressed(Key::PageDown) {
            self.switch_layer(BOTTOM);
        }
        if pressed(Key::Plus) || pressed(Key::Equals) {
            self.next_layer(1);
        }
        if pressed(Key::Minus) {
            self.next_layer(-1);
        }
        let layers = self.doc.copper_layers();
        for (i, key) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6].iter().enumerate() {
            if pressed(*key) && i < layers.len() {
                self.switch_layer(layers[i]);
            }
        }
    }

    pub fn hover_net(&self) -> Option<u32> {
        let net = |item: Item| self.copper.index.get(&item).map(|&i| self.copper.items[i].net);
        match self.hover? {
            Sel::Track(id) => net(Item::Track(id)),
            Sel::Via(id) => net(Item::Via(id)),
            Sel::Fp(id) => {
                let q = self.hover_q?;
                let f = self.doc.fp(id)?;
                (0..f.pads.len()).find(|&i| f.pad_blob(i, 0.0).dist_to(q) <= 0.0).map(|i| f.pads[i].net)
            }
        }
        .filter(|n| *n != 0)
    }

    fn apply_launch(&mut self) {
        if self.applied_launch || self.frames < 8 || self.canvas_rect == Rect::NOTHING {
            return;
        }
        self.applied_launch = true;
        if let Some((at, w)) = self.launch.zoom {
            self.zoom_to(at, w);
        }
        if let Some(name) = self.launch.select.clone()
            && let Some(f) = self.doc.footprints.iter().find(|f| f.refdes == name)
        {
            self.sel = vec![Sel::Fp(f.id)];
        }
    }
}

/// Joins a track to the next when they carry straight on, so a route laid in
/// clicks doesn't leave a string of short pieces.
fn merge_straight(b: &mut Board) {
    loop {
        let mut joined = false;
        'outer: for i in 0..b.tracks.len() {
            for j in 0..b.tracks.len() {
                if i == j {
                    continue;
                }
                let (t, u) = (&b.tracks[i], &b.tracks[j]);
                if t.layer != u.layer || t.net != u.net || (t.w - u.w).abs() > 1e-9 || !t.b.same(u.a) {
                    continue;
                }
                let (d1, d2) = (t.b.sub(t.a), u.b.sub(u.a));
                if d1.cross(d2).abs() > 1e-9 * d1.len() * d2.len() || d1.dot(d2) <= 0.0 {
                    continue;
                }
                // Something else joining in the middle keeps the corner.
                let mid = t.b;
                let others = b.tracks.iter().filter(|o| o.a.same(mid) || o.b.same(mid)).count()
                    + b.vias.iter().filter(|v| v.at.same(mid)).count();
                if others != 2 {
                    continue;
                }
                let end = u.b;
                b.tracks[i].b = end;
                b.tracks.remove(j);
                joined = true;
                break 'outer;
            }
        }
        if !joined {
            break;
        }
    }
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.pinch.stop();
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.play_step(raw_input);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        self.save_changes_dialog(&ctx);
        if self.confirm.is_none() {
            self.keys(&ctx);
        }
        self.refresh();
        self.drive_screenshot(&ctx);
        self.apply_launch();
        let width = ctx.content_rect().width();
        let top_frame = egui::Frame::new().fill(theme::WELL).inner_margin(egui::Margin::symmetric(12, 7));
        Panel::top("top").resizable(false).frame(top_frame).show(ui, |ui| self.top_bar(ui));
        if self.show_left && width >= 600.0 {
            let frame = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 10));
            Panel::left("left").resizable(false).exact_size(272.0).frame(frame).show(ui, |ui| self.left_panel(ui));
        }
        if self.show_right && width >= 900.0 {
            let frame = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 12));
            Panel::right("right").resizable(false).exact_size(300.0).frame(frame).show(ui, |ui| self.right_panel(ui));
        }
        let status_frame = egui::Frame::new().fill(theme::WELL).inner_margin(egui::Margin::symmetric(12, 5));
        Panel::bottom("status").resizable(false).frame(status_frame).show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::WELL)).show(ui, |ui| self.canvas(ui));
        // A change made this frame shows its effect straight away.
        if self.copper_rev != self.rev {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legs_are_straight_then_diagonal() {
        let legs = App::legs(p(0.0, 0.0), p(5.0, 2.0), false);
        assert_eq!(legs, vec![p(0.0, 0.0), p(3.0, 0.0), p(5.0, 2.0)]);
        let legs = App::legs(p(0.0, 0.0), p(5.0, 2.0), true);
        assert_eq!(legs, vec![p(0.0, 0.0), p(2.0, 2.0), p(5.0, 2.0)]);
    }

    #[test]
    fn moving_a_part_drags_its_tracks() {
        let mut b = crate::sample::board();
        let r3 = b.footprints.iter().find(|f| f.refdes == "R3").unwrap().id;
        App::move_selection(&mut b, &[Sel::Fp(r3)], p(0.0, -1.0));
        let led = b.tracks.iter().find(|t| t.b.same(p(16.9125, 27.0))).unwrap();
        assert!(led.a.same(p(16.9125, 21.0)));
    }

    #[test]
    fn straight_runs_become_one_track() {
        let mut b = Board::default();
        for (id, a, e) in [(1, p(0.0, 0.0), p(1.0, 0.0)), (2, p(1.0, 0.0), p(3.0, 0.0)), (3, p(3.0, 0.0), p(4.0, 1.0))] {
            b.tracks.push(Track { id, a, b: e, w: 0.25, layer: TOP, net: 1 });
        }
        merge_straight(&mut b);
        assert_eq!(b.tracks.len(), 2);
        assert!(b.tracks.iter().any(|t| t.a.same(p(0.0, 0.0)) && t.b.same(p(3.0, 0.0))));
    }
}
