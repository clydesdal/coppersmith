//! The board view: drawing every layer, and what the mouse does in each tool.

use crate::ui::app::*;
use crate::pcb::copper::Item;
use crate::pcb::drc::Severity;
use crate::pcb::geom::*;
use crate::ui::icons::Icon;
use crate::pcb::model::*;
use crate::ui::panels::{icon_button, segmented_tool};
use crate::ui::theme::{self, ACCENT, DIM, FAINT, FG, LINE, rgb};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Painter, PointerButton, Pos2, Rect, RichText, Sense, Shape,
    Stroke, StrokeKind, Ui, pos2, vec2,
};
use std::time::Duration;

const SHOT_FRAMES: u32 = 90;
/// Scripted input starts once the picture has settled.
pub const PLAY_FROM: u32 = 20;

pub const BOARD: Color32 = rgb(0x15181f);
pub const THROUGH: Color32 = rgb(0xd6b25e);
pub const VIA: Color32 = rgb(0xb4b8bf);
pub const EDGE: Color32 = rgb(0xf07ac2);
pub const RATS: Color32 = rgb(0xdedcd6);

const INNER: [Color32; 4] = [rgb(0x5dc091), rgb(0xe09b3f), rgb(0x7fd1e6), rgb(0xc58fe0)];

pub fn layer_color(layer: Layer) -> Color32 {
    match layer {
        Layer::Cu(TOP) => rgb(0xe5484d),
        Layer::Cu(BOTTOM) => rgb(0x4f8cff),
        Layer::Cu(n) => INNER[(n as usize + 3) % 4],
        Layer::Silk(Side::Top) => rgb(0xf2d24b),
        Layer::Silk(Side::Bottom) => rgb(0xd9a24e),
        Layer::Court(_) => rgb(0x8a6fb0),
        Layer::Fab(_) => rgb(0x8d8e8e),
        Layer::Mask(_) | Layer::Paste(_) => rgb(0xa8adb5),
        Layer::Edge => EDGE,
    }
}

/// A colour faded towards the board, for what isn't being worked on.
pub fn fade(c: Color32, keep: f32) -> Color32 {
    let mix = |a: u8, b: u8| (a as f32 * keep + b as f32 * (1.0 - keep)).round() as u8;
    Color32::from_rgb(mix(c.r(), BOARD.r()), mix(c.g(), BOARD.g()), mix(c.b(), BOARD.b()))
}

/// Maps the board onto the screen. Y goes up on the board.
#[derive(Clone, Copy)]
pub struct Tr {
    pub rect: Rect,
    pub c: P,
    pub s: f64,
    pub m: f64,
}

impl Tr {
    pub fn pt(&self, q: P) -> Pos2 {
        let mid = self.rect.center();
        pos2(mid.x + ((q.x - self.c.x) * self.s * self.m) as f32, mid.y - ((q.y - self.c.y) * self.s) as f32)
    }
    pub fn board(&self, at: Pos2) -> P {
        let mid = self.rect.center();
        p((at.x - mid.x) as f64 / (self.s * self.m) + self.c.x, -(at.y - mid.y) as f64 / self.s + self.c.y)
    }
    pub fn len(&self, mm: f64) -> f32 {
        (mm * self.s) as f32
    }
}

pub fn fill_blob(painter: &Painter, tr: &Tr, b: &Blob, color: Color32) {
    match b.core.len() {
        0 => {}
        1 => {
            painter.circle_filled(tr.pt(b.core[0]), tr.len(b.r).max(0.6), color);
        }
        2 if b.r > 0.0 => {
            let w = tr.len(2.0 * b.r).max(1.0);
            let (a, e) = (tr.pt(b.core[0]), tr.pt(b.core[1]));
            painter.line_segment([a, e], Stroke::new(w, color));
            painter.circle_filled(a, w / 2.0, color);
            painter.circle_filled(e, w / 2.0, color);
        }
        _ => {
            let mut pts: Vec<Pos2> = b.ring().iter().map(|q| tr.pt(*q)).collect();
            if tr.m > 0.0 {
                pts.reverse();
            }
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
    }
}

pub fn stroke_blob(painter: &Painter, tr: &Tr, b: &Blob, stroke: Stroke) {
    let mut pts: Vec<Pos2> = b.ring().iter().map(|q| tr.pt(*q)).collect();
    if let Some(first) = pts.first().copied() {
        pts.push(first);
    }
    painter.add(Shape::line(pts, stroke));
}

fn ink(painter: &Painter, tr: &Tr, i: Ink, color: Color32) {
    match i {
        Ink::Line(a, b, w) => {
            let w = tr.len(w).max(1.0);
            painter.line_segment([tr.pt(a), tr.pt(b)], Stroke::new(w, color));
            if w > 2.5 {
                painter.circle_filled(tr.pt(a), w / 2.0, color);
                painter.circle_filled(tr.pt(b), w / 2.0, color);
            }
        }
        Ink::Dot(at, d) => {
            painter.circle_filled(tr.pt(at), tr.len(d / 2.0).max(1.0), color);
        }
        Ink::Ring(at, r, w) => {
            painter.circle_stroke(tr.pt(at), tr.len(r), Stroke::new(tr.len(w).max(1.0), color));
        }
    }
}

fn nice_step(grid: f64, scale: f64) -> f64 {
    let mut step = grid.max(0.01);
    for k in [2.0, 2.5, 2.0, 2.0, 2.5, 2.0, 2.0, 2.5, 2.0] {
        if step * scale >= 14.0 {
            break;
        }
        step *= k;
    }
    step
}

impl App {
    pub fn tr(&self, rect: Rect) -> Tr {
        Tr { rect, c: self.center, s: self.scale, m: if self.mirror { -1.0 } else { 1.0 } }
    }

    /// The layers from the far side to the near one, the active copper last.
    fn draw_order(&self) -> Vec<Layer> {
        let (far, near) = if self.mirror { (Side::Top, Side::Bottom) } else { (Side::Bottom, Side::Top) };
        let mut copper: Vec<u8> = self.doc.copper_layers();
        if self.mirror == false {
            copper.reverse();
        }
        copper.retain(|l| *l != self.layer);
        copper.push(self.layer);
        let mut v = vec![Layer::Fab(far), Layer::Court(far), Layer::Silk(far)];
        v.extend(copper.into_iter().map(Layer::Cu));
        v.extend([Layer::Fab(near), Layer::Court(near), Layer::Silk(near), Layer::Edge]);
        v
    }

    fn shows(&self, layer: Layer) -> bool {
        !self.hidden.contains(&layer)
    }

    /// Picks what's under a point, in the order a click should reach things.
    pub fn pick(&self, q: P, tol: f64) -> Option<Sel> {
        let mut hits: Vec<(u8, f64, Sel)> = Vec::new();
        let any_copper = self.doc.copper_layers().iter().any(|l| self.shows(Layer::Cu(*l)));
        if any_copper {
            for v in &self.doc.vias {
                let d = v.blob().dist_to(q);
                if d <= tol {
                    hits.push((0, d, Sel::Via(v.id)));
                }
            }
        }
        for t in &self.doc.tracks {
            if !self.shows(Layer::Cu(t.layer)) {
                continue;
            }
            let d = t.blob().dist_to(q);
            if d <= tol {
                hits.push((if t.layer == self.layer { 1 } else { 4 }, d, Sel::Track(t.id)));
            }
        }
        let side = self.active_side();
        for f in &self.doc.footprints {
            if !self.shows(Layer::Cu(f.copper())) && !self.shows(Layer::Silk(f.side)) {
                continue;
            }
            let near = f.side == side || self.layer != TOP && self.layer != BOTTOM;
            let on_pad = (0..f.pads.len()).any(|i| f.pad_blob(i, 0.0).dist_to(q) <= tol);
            let b = f.bbox();
            if on_pad {
                hits.push((if near { 2 } else { 5 }, 0.0, Sel::Fp(f.id)));
            } else if b.contains(q) {
                hits.push((if near { 3 } else { 6 }, b.w() * b.h(), Sel::Fp(f.id)));
            }
        }
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        hits.first().map(|h| h.2)
    }

    fn describe_hover(&self, q: P) -> Option<String> {
        let net = |n: u32| self.doc.net_name(n).to_string();
        match self.hover? {
            Sel::Via(id) => {
                let v = self.doc.via(id)?;
                Some(format!("Via {} / {} · {}", self.unit.len(v.dia), self.unit.len(v.drill), net(v.net)))
            }
            Sel::Track(id) => {
                let t = self.doc.track(id)?;
                Some(format!(
                    "Track {} on {} · {} · {} long",
                    self.unit.len(t.w),
                    Layer::Cu(t.layer).name().to_lowercase(),
                    net(t.net),
                    self.unit.len(t.a.dist(t.b))
                ))
            }
            Sel::Fp(id) => {
                let f = self.doc.fp(id)?;
                match (0..f.pads.len()).find(|&i| f.pad_blob(i, 0.0).dist_to(q) <= 0.0) {
                    Some(i) => Some(format!("{} pad {} · {}", f.refdes, f.pads[i].num, net(f.pads[i].net))),
                    None => Some(format!("{} {} · {}", f.refdes, f.value, f.def)),
                }
            }
        }
    }

    pub fn canvas(&mut self, ui: &mut Ui) {
        let full = ui.available_rect_before_wrap();
        if self.fitted && (full.size() - self.canvas_rect.size()).length() > 0.5 {
            self.fit_pending = true;
        }
        self.canvas_rect = full;
        if self.fit_pending {
            self.fit(full);
            self.fit_pending = false;
        }
        let response = ui.interact(full, ui.scope_id().with("board"), Sense::click_and_drag());
        self.navigate(ui, &response, full);
        self.interact(ui, &response, full);
        self.refresh();

        let painter = ui.painter_at(full);
        painter.rect_filled(full, 0.0, theme::WELL);
        let tr = self.tr(full);
        if self.show_grid {
            self.draw_grid(&painter, &tr);
        }
        self.draw_board(&painter, &tr);
        self.draw_overlays(&painter, &tr);
        self.draw_scale(&painter, full);
        self.badges(ui, full);
        self.floating_tools(ui, full);
    }

    fn navigate(&mut self, ui: &Ui, response: &egui::Response, rect: Rect) {
        let tr = self.tr(rect);
        if response.dragged_by(PointerButton::Middle) || response.dragged_by(PointerButton::Secondary) {
            let d = response.drag_delta();
            self.center.x -= d.x as f64 / (self.scale * tr.m);
            self.center.y += d.y as f64 / self.scale;
            self.fitted = false;
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        let (pinched, moved) = self.pinch.take();
        if response.hovered()
            && let Some(at) = ui.input(|i| i.pointer.hover_pos())
        {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta().y, i.zoom_delta()));
            let factor = (zoom * pinched) as f64 * (scroll as f64 * 0.003).exp();
            if (factor - 1.0).abs() > 1e-4 {
                let before = self.tr(rect).board(at);
                self.scale = (self.scale * factor).clamp(1.0, 4000.0);
                let after = self.tr(rect).board(at);
                self.center = self.center.add(before.sub(after));
                self.fitted = false;
            }
            if moved != [0.0, 0.0] {
                self.center.x -= moved[0] as f64 / (self.scale * tr.m);
                self.center.y += moved[1] as f64 / self.scale;
                self.fitted = false;
            }
        }
    }

    fn interact(&mut self, ui: &Ui, response: &egui::Response, rect: Rect) {
        let tr = self.tr(rect);
        let pointer = response.hover_pos().or(response.interact_pointer_pos());
        let Some(at) = pointer else {
            self.hover = None;
            self.hover_q = None;
            self.hover_text = None;
            return;
        };
        let q = tr.board(at);
        let tol = 4.0 / self.scale;
        self.hover_q = Some(q);
        self.cursor = Some(q.snap(self.grid));
        let shift = ui.input(|i| i.modifiers.shift);
        match self.tool {
            Tool::Select => {
                self.hover = if self.drag.is_some() { self.hover } else { self.pick(q, tol) };
                if response.drag_started_by(PointerButton::Primary) {
                    let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(at);
                    let q0 = tr.board(origin);
                    self.begin_drag(q0, tol, shift, origin);
                }
                if response.dragged_by(PointerButton::Primary) {
                    self.continue_drag(q);
                }
                if response.drag_stopped() {
                    self.end_drag(&tr, at);
                }
                if response.clicked() {
                    let hit = self.pick(q, tol);
                    match (hit, shift) {
                        (Some(h), true) => {
                            if let Some(i) = self.sel.iter().position(|s| *s == h) {
                                self.sel.remove(i);
                            } else {
                                self.sel.push(h);
                            }
                        }
                        (Some(h), false) => self.sel = vec![h],
                        (None, false) => self.sel.clear(),
                        (None, true) => {}
                    }
                }
                if response.double_clicked() {
                    // Picks out everything joined to what was clicked.
                    let item = match self.pick(q, tol) {
                        Some(Sel::Track(id)) => Some(Item::Track(id)),
                        Some(Sel::Via(id)) => Some(Item::Via(id)),
                        _ => None,
                    };
                    if let Some(item) = item {
                        self.sel = self
                            .copper
                            .connected(item)
                            .into_iter()
                            .filter_map(|i| match i {
                                Item::Track(id) => Some(Sel::Track(id)),
                                Item::Via(id) => Some(Sel::Via(id)),
                                Item::Pad(..) => None,
                            })
                            .collect();
                    }
                }
                if self.hover.is_some() && self.drag.is_none() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
            }
            Tool::Route => {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                self.hover = None;
                // Ctrl stops the pointer snapping to copper; Ctrl+click finishes
                // the connection by itself.
                let ctrl = ui.input(|i| i.modifiers.command);
                let (to, item) = self.route_snap(q, tol, !ctrl);
                self.route_target = Some((to, item));
                if self.route.is_some() {
                    self.update_plan(to, item);
                }
                if response.clicked() {
                    if self.route.is_none() {
                        self.start_route(q, tol);
                    } else if ctrl {
                        self.route_autocomplete();
                    } else {
                        self.route_click();
                    }
                }
                if response.double_clicked() || response.secondary_clicked() {
                    self.finish_route();
                }
            }
            Tool::Via => {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                if response.clicked() {
                    self.place_via(q, tol);
                }
                if response.secondary_clicked() {
                    self.set_tool(Tool::Select);
                }
            }
            Tool::Outline => {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                let near_first = self.outline.len() >= 3 && tr.pt(self.outline[0]).distance(at) < 9.0;
                if response.double_clicked() {
                    self.close_outline();
                } else if response.clicked() {
                    if near_first {
                        self.close_outline();
                    } else {
                        let c = q.snap(self.grid);
                        if !self.outline.last().is_some_and(|l| l.same(c)) {
                            self.outline.push(c);
                        }
                    }
                }
                if response.secondary_clicked() {
                    self.outline.pop();
                }
            }
            Tool::Place => {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                if let Some((f, _)) = &mut self.placing {
                    f.at = q.snap(self.grid);
                }
                if response.clicked() {
                    self.place_here();
                }
                if response.secondary_clicked() {
                    self.set_tool(Tool::Select);
                }
            }
        }
        self.hover_text = self.describe_hover(q);
    }

    fn begin_drag(&mut self, q0: P, tol: f64, shift: bool, origin: Pos2) {
        let hit = self.pick(q0, tol);
        let Some(hit) = hit else {
            self.box_from = Some(origin);
            return;
        };
        // Grabbing a track near its end moves just that end.
        if let Sel::Track(id) = hit
            && let Some(t) = self.doc.track(id)
        {
            let reach = (t.w / 2.0).max(tol * 2.0);
            for (end, far) in [(t.a, false), (t.b, true)] {
                if end.dist(q0) <= reach && t.a.dist(t.b) > reach * 2.0 {
                    self.sel = vec![hit];
                    self.drag = Some(Drag {
                        kind: DragKind::End(id, far),
                        before: self.doc.clone(),
                        grab: q0,
                        reference: end,
                        moved: false,
                    });
                    return;
                }
            }
        }
        if !self.sel.contains(&hit) {
            if shift {
                self.sel.push(hit);
            } else {
                self.sel = vec![hit];
            }
        }
        let reference = match hit {
            Sel::Fp(id) => self.doc.fp(id).map(|f| f.at),
            Sel::Via(id) => self.doc.via(id).map(|v| v.at),
            Sel::Track(id) => self.doc.track(id).map(|t| if t.a.dist(q0) < t.b.dist(q0) { t.a } else { t.b }),
        }
        .unwrap_or(q0);
        self.drag = Some(Drag { kind: DragKind::Move, before: self.doc.clone(), grab: q0, reference, moved: false });
    }

    fn continue_drag(&mut self, q: P) {
        let Some(drag) = &mut self.drag else { return };
        let target = drag.reference.add(q.sub(drag.grab)).snap(self.grid);
        let mut doc = drag.before.clone();
        match drag.kind {
            DragKind::Move => {
                let d = target.sub(drag.reference).rounded();
                App::move_selection(&mut doc, &self.sel, d);
                drag.moved = d.len() > 0.0;
            }
            DragKind::End(id, far) => {
                App::move_end(&mut doc, id, far, target);
                drag.moved = !target.same(drag.reference);
            }
        }
        if doc != self.doc {
            self.doc = doc;
            self.rev += 1;
        }
    }

    fn end_drag(&mut self, tr: &Tr, at: Pos2) {
        if let Some(drag) = self.drag.take() {
            if drag.moved {
                self.remember(drag.before);
            } else {
                self.doc = drag.before;
                self.rev += 1;
            }
        }
        if let Some(from) = self.box_from.take() {
            let r = Rect::from_two_pos(from, at);
            let (a, b) = (tr.board(r.left_top()), tr.board(r.right_bottom()));
            let area = BBox::of(&[a, b]);
            // Left to right takes what's wholly inside; right to left, anything touched.
            let touching = at.x < from.x;
            let take = |bb: BBox| if touching { area.touches(&bb) } else { area.contains(bb.min) && area.contains(bb.max) };
            let mut sel: Vec<Sel> = Vec::new();
            for f in &self.doc.footprints {
                if take(f.bbox()) {
                    sel.push(Sel::Fp(f.id));
                }
            }
            for t in self.doc.tracks.iter().filter(|t| self.shows(Layer::Cu(t.layer))) {
                if take(t.blob().bbox()) {
                    sel.push(Sel::Track(t.id));
                }
            }
            for v in &self.doc.vias {
                if take(v.blob().bbox()) {
                    sel.push(Sel::Via(v.id));
                }
            }
            self.sel = sel;
        }
    }

    // -- Drawing -------------------------------------------------------------

    fn draw_grid(&self, painter: &Painter, tr: &Tr) {
        let step = nice_step(self.grid, self.scale);
        let (a, b) = (tr.board(tr.rect.left_top()), tr.board(tr.rect.right_bottom()));
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        let color = Color32::from_white_alpha(9);
        let strong = Color32::from_white_alpha(18);
        let major = step * 5.0;
        let mut x = (x0 / step).ceil() * step;
        while x <= x1 {
            let c = if ((x / major).round() * major - x).abs() < step * 0.01 { strong } else { color };
            painter.vline(tr.pt(p(x, 0.0)).x, tr.rect.y_range(), Stroke::new(1.0, c));
            x += step;
        }
        let mut y = (y0 / step).ceil() * step;
        while y <= y1 {
            let c = if ((y / major).round() * major - y).abs() < step * 0.01 { strong } else { color };
            painter.hline(tr.rect.x_range(), tr.pt(p(0.0, y)).y, Stroke::new(1.0, c));
            y += step;
        }
        // The board's zero.
        let o = tr.pt(p(0.0, 0.0));
        painter.line_segment([o - vec2(7.0, 0.0), o + vec2(7.0, 0.0)], Stroke::new(1.0, FAINT));
        painter.line_segment([o - vec2(0.0, 7.0), o + vec2(0.0, 7.0)], Stroke::new(1.0, FAINT));
    }

    fn draw_board(&self, painter: &Painter, tr: &Tr) {
        let doc = &self.doc;
        // The board itself.
        if doc.outline.len() >= 3 {
            let idx = triangulate(&doc.outline);
            let mut mesh = egui::Mesh::default();
            for q in &doc.outline {
                mesh.colored_vertex(tr.pt(*q), BOARD);
            }
            mesh.indices = idx;
            painter.add(Shape::mesh(mesh));
        }
        let routing = self.route.as_ref().map(|r| r.net);
        let highlight = self.highlight.or(routing.filter(|n| *n != 0));
        let keep = |net: u32| match highlight {
            Some(h) if h == net => 1.0,
            Some(_) => 0.3,
            None => 1.0,
        };
        let active = Layer::Cu(self.layer);
        // Tracks a push would move are drawn where they'd go, not where they are.
        let pushed: std::collections::HashSet<u32> =
            self.plan.iter().flat_map(|c| c.plan.pushed.iter().flat_map(|m| m.ids.iter().copied())).collect();

        for layer in self.draw_order() {
            if !self.shows(layer) {
                continue;
            }
            let base = layer_color(layer);
            let strength = match layer {
                Layer::Cu(_) if layer == active => 1.0,
                Layer::Cu(_) => 0.42,
                Layer::Silk(s) if Some(s) == active.side() || active.side().is_none() => 1.0,
                Layer::Silk(_) => 0.5,
                _ => 0.8,
            };
            match layer {
                Layer::Cu(l) => {
                    for t in doc.tracks.iter().filter(|t| t.layer == l && !pushed.contains(&t.id)) {
                        fill_blob(painter, tr, &t.blob(), fade(base, strength * keep(t.net)));
                    }
                    for f in doc.footprints.iter().filter(|f| f.copper() == l) {
                        for (i, pad) in f.pads.iter().enumerate() {
                            if !pad.through() {
                                fill_blob(painter, tr, &f.pad_blob(i, 0.0), fade(base, strength.max(0.6) * keep(pad.net)));
                            }
                        }
                    }
                    if layer == active {
                        self.draw_through(painter, tr, &keep);
                    }
                }
                Layer::Edge => {
                    let o = &doc.outline;
                    for i in 0..o.len() {
                        painter.line_segment([tr.pt(o[i]), tr.pt(o[(i + 1) % o.len()])], Stroke::new(1.6, EDGE));
                    }
                }
                _ => {
                    for f in &doc.footprints {
                        for i in f.art(layer, &doc.rules) {
                            ink(painter, tr, i, fade(base, strength));
                        }
                    }
                }
            }
        }
        // Through-hole copper sits on every layer; if the active one is hidden it still shows.
        if !self.shows(active) {
            self.draw_through(painter, tr, &keep);
        }
        self.draw_pad_names(painter, tr);
    }

    fn draw_through(&self, painter: &Painter, tr: &Tr, keep: &dyn Fn(u32) -> f32) {
        let doc = &self.doc;
        for f in &doc.footprints {
            for (i, pad) in f.pads.iter().enumerate() {
                if pad.through() {
                    if !pad.bare_hole() {
                        fill_blob(painter, tr, &f.pad_blob(i, 0.0), fade(THROUGH, keep(pad.net)));
                    }
                    painter.circle_filled(tr.pt(f.pad_center(i)), tr.len(pad.drill / 2.0), theme::WELL);
                }
            }
        }
        // A via a push would move is drawn where it would go.
        let moved = |id: u32| self.plan.as_ref().and_then(|c| c.plan.vias.iter().find(|m| m.0 == id).map(|m| m.1));
        for v in &doc.vias {
            let at = moved(v.id).unwrap_or(v.at);
            fill_blob(painter, tr, &Blob::dot(at, v.dia), fade(VIA, keep(v.net)));
            painter.circle_filled(tr.pt(at), tr.len(v.drill / 2.0), theme::WELL);
            if !at.same(v.at) {
                painter.circle_stroke(tr.pt(at), tr.len(v.dia / 2.0) + 1.0, Stroke::new(1.0, Color32::from_white_alpha(90)));
            }
        }
    }

    /// Pad numbers, and nets when there's room, once zoomed in.
    fn draw_pad_names(&self, painter: &Painter, tr: &Tr) {
        if self.scale < 14.0 {
            return;
        }
        for f in &self.doc.footprints {
            if !self.shows(Layer::Cu(f.copper())) && f.pads.iter().all(|p| !p.through()) {
                continue;
            }
            for (i, pad) in f.pads.iter().enumerate() {
                if pad.bare_hole() {
                    continue;
                }
                let b = f.pad_blob(i, 0.0).bbox();
                let small = tr.len(b.w().min(b.h()));
                if small < 9.0 {
                    continue;
                }
                let c = tr.pt(f.pad_center(i));
                let size = (small * 0.42).clamp(7.0, 13.0);
                let color = Color32::from_rgba_unmultiplied(255, 255, 255, 215);
                let name = self.doc.net_name(pad.net);
                if pad.net != 0 && small > 24.0 {
                    painter.text(c - vec2(0.0, size * 0.55), Align2::CENTER_CENTER, &pad.num, theme::mono(size), color);
                    let n = theme::mono((size * 0.75).max(7.0));
                    painter.text(c + vec2(0.0, size * 0.55), Align2::CENTER_CENTER, name, n, Color32::from_white_alpha(170));
                } else {
                    painter.text(c, Align2::CENTER_CENTER, &pad.num, theme::mono(size), color);
                }
            }
        }
    }

    fn draw_overlays(&self, painter: &Painter, tr: &Tr) {
        let doc = &self.doc;
        // Still to connect.
        if self.show_rats {
            for r in &self.copper.rats {
                let c = if self.highlight.is_none_or(|h| h == r.net) { RATS } else { Color32::from_white_alpha(30) };
                painter.line_segment([tr.pt(r.a), tr.pt(r.b)], Stroke::new(1.0, c));
            }
        }

        // What's selected, and what the pointer is over.
        let mark = |sel: Sel, stroke: Stroke, fill: Option<Color32>| match sel {
            Sel::Fp(id) => {
                if let Some(f) = doc.fp(id) {
                    let b = f.bbox();
                    let r = Rect::from_two_pos(tr.pt(b.min), tr.pt(b.max)).expand(2.0);
                    if let Some(fill) = fill {
                        painter.rect_filled(r, 4.0, fill);
                    }
                    painter.rect_stroke(r, 4.0, stroke, StrokeKind::Outside);
                }
            }
            Sel::Track(id) => {
                if let Some(t) = doc.track(id) {
                    stroke_blob(painter, tr, &t.blob(), stroke);
                }
            }
            Sel::Via(id) => {
                if let Some(v) = doc.via(id) {
                    stroke_blob(painter, tr, &v.blob(), stroke);
                }
            }
        };
        for s in &self.sel {
            mark(*s, Stroke::new(1.6, ACCENT), Some(Color32::from_rgba_unmultiplied(117, 171, 255, 22)));
        }
        if let Some(h) = self.hover
            && !self.sel.contains(&h)
            && self.tool == Tool::Select
        {
            mark(h, Stroke::new(1.0, Color32::from_white_alpha(120)), None);
        }

        // The rule checks.
        if self.show_problems {
            for (i, prob) in self.problems.iter().enumerate() {
                if prob.title == "Not connected" || prob.items.is_empty() && prob.title != "Parts overlap" {
                    continue;
                }
                let c = if prob.severity == Severity::Error { theme::RED } else { theme::YELLOW };
                let at = tr.pt(prob.at);
                let r = if self.focus == Some(i) { 11.0 } else { 7.0 };
                painter.circle_stroke(at, r, Stroke::new(1.6, c));
                painter.line_segment([at + vec2(-r * 0.45, -r * 0.45), at + vec2(r * 0.45, r * 0.45)], Stroke::new(1.6, c));
                painter.line_segment([at + vec2(r * 0.45, -r * 0.45), at + vec2(-r * 0.45, r * 0.45)], Stroke::new(1.6, c));
            }
        }

        // The route being laid.
        if let Some(r) = &self.route {
            self.draw_route(painter, tr, r);
        } else if self.tool == Tool::Route
            && let Some((at, item)) = self.route_target
        {
            let c = if item.is_some() { ACCENT } else { FG };
            painter.circle_stroke(tr.pt(at), 5.0, Stroke::new(1.2, c));
        }

        if self.tool == Tool::Via
            && let Some(q) = self.cursor
        {
            fill_blob(painter, tr, &Blob::dot(q, doc.rules.via_dia), VIA.gamma_multiply(0.6));
        }

        // An outline being drawn.
        if self.tool == Tool::Outline && (!self.outline.is_empty() || self.cursor.is_some()) {
            let mut pts: Vec<Pos2> = self.outline.iter().map(|q| tr.pt(*q)).collect();
            if let Some(c) = self.cursor {
                pts.push(tr.pt(c));
            }
            painter.add(Shape::line(pts.clone(), Stroke::new(1.6, EDGE)));
            for (i, q) in pts.iter().enumerate() {
                let first = i == 0 && self.outline.len() >= 3;
                painter.circle_filled(*q, if first { 5.0 } else { 3.0 }, EDGE);
            }
        }

        // A part on its way to the board.
        if let Some((f, _)) = &self.placing {
            let c = layer_color(Layer::Cu(f.copper()));
            for i in 0..f.pads.len() {
                let colour = if f.pads[i].through() { THROUGH } else { c };
                fill_blob(painter, tr, &f.pad_blob(i, 0.0), colour.gamma_multiply(0.7));
            }
            for layer in [Layer::Silk(f.side), Layer::Court(f.side)] {
                for i in f.art(layer, &doc.rules) {
                    ink(painter, tr, i, layer_color(layer).gamma_multiply(0.8));
                }
            }
        }

        if let (Some(from), Some(q)) = (self.box_from, self.hover_q) {
            let to = tr.pt(q);
            let r = Rect::from_two_pos(from, to);
            let touching = to.x < from.x;
            painter.rect_filled(r, 2.0, Color32::from_rgba_unmultiplied(117, 171, 255, if touching { 14 } else { 26 }));
            let stroke = Stroke::new(1.0, ACCENT);
            if touching {
                painter.add(Shape::dashed_line(
                    &[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()],
                    stroke,
                    5.0,
                    4.0,
                ));
            } else {
                painter.rect_stroke(r, 2.0, stroke, StrokeKind::Inside);
            }
        }
    }

    /// Placed segments solid, what a click would place shaded, the
    /// look-ahead segment hollow; red where it's blocked or, ignoring
    /// obstacles, running into something.
    fn draw_route(&self, painter: &Painter, tr: &Tr, r: &Route) {
        let doc = &self.doc;
        let colour = |layer: u8, alpha: u8| {
            let c = layer_color(Layer::Cu(layer));
            Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
        };
        // Other nets' clearance, near where the route is going (Ctrl+W).
        if self.route_mode.show_clearance {
            let around = self.route_target.map(|t| t.0).unwrap_or(r.from);
            let reach = BBox::of(&[around, r.from]).grown(6.0);
            for o in &self.obstacles.scene.own {
                if o.bbox.touches(&reach) && o.item.is_some() {
                    let grown = Blob { core: o.blob.core.clone(), r: o.blob.r + o.gap + r.width / 2.0 };
                    fill_blob(painter, tr, &grown, Color32::from_white_alpha(14));
                    stroke_blob(painter, tr, &grown, Stroke::new(1.0, Color32::from_white_alpha(45)));
                }
            }
        }
        if let Some(c) = &self.plan {
            for m in &c.plan.pushed {
                for s in m.pts.windows(2) {
                    let seg = Blob::line(s[0], s[1], m.w);
                    fill_blob(painter, tr, &seg, fade(layer_color(Layer::Cu(m.layer)), 0.85));
                    stroke_blob(painter, tr, &seg, Stroke::new(1.0, Color32::from_white_alpha(70)));
                }
            }
        }
        for (a, b, layer) in &r.segs {
            fill_blob(painter, tr, &Blob::line(*a, *b, r.width), colour(*layer, 235));
        }
        for v in &r.vias {
            fill_blob(painter, tr, &Blob::dot(*v, doc.rules.via_dia), VIA);
            painter.circle_filled(tr.pt(*v), tr.len(doc.rules.via_drill / 2.0), theme::WELL);
        }
        if let Some(c) = &self.plan {
            let pts = &c.plan.pts;
            let n = pts.len().saturating_sub(1);
            let hollow_last = self.route_mode.look_ahead && n >= 2;
            for (i, s) in pts.windows(2).enumerate() {
                let seg = Blob::line(s[0], s[1], r.width);
                if hollow_last && i == n - 1 {
                    stroke_blob(painter, tr, &seg, Stroke::new(1.4, colour(r.layer, 230)));
                } else {
                    fill_blob(painter, tr, &seg, colour(r.layer, 120));
                    stroke_blob(painter, tr, &seg, Stroke::new(1.0, colour(r.layer, 220)));
                }
            }
            for item in &c.plan.hits {
                if let Some(&k) = self.copper.index.get(item) {
                    stroke_blob(painter, tr, &self.copper.items[k].blob, Stroke::new(2.0, theme::RED));
                }
            }
            if c.plan.blocked
                && let Some(end) = c.plan.end()
            {
                let e = tr.pt(end);
                painter.circle_stroke(e, 7.0, Stroke::new(1.6, theme::RED));
                let t = tr.pt(c.to);
                painter.line_segment([t + vec2(-5.0, -5.0), t + vec2(5.0, 5.0)], Stroke::new(1.6, theme::RED));
                painter.line_segment([t + vec2(5.0, -5.0), t + vec2(-5.0, 5.0)], Stroke::new(1.6, theme::RED));
            }
        }
        if let Some((at, item)) = self.route_target {
            let c = if item.is_some() { ACCENT } else { FG };
            painter.circle_stroke(tr.pt(at), 5.0, Stroke::new(1.2, c));
        }
    }

    fn draw_scale(&self, painter: &Painter, rect: Rect) {
        let mut step = 0.1;
        while step * self.scale < 40.0 {
            step *= if (step * 10f64.powf(-step.log10().floor())).round() as i64 == 2 { 2.5 } else { 2.0 };
        }
        let w = (step * self.scale) as f32;
        let (x, y) = (rect.left() + 18.0, rect.bottom() - 70.0);
        painter.line(vec![pos2(x, y - 4.0), pos2(x, y), pos2(x + w, y), pos2(x + w, y - 4.0)], Stroke::new(1.2, DIM));
        painter.text(pos2(x, y - 7.0), Align2::LEFT_BOTTOM, self.unit.len(step), theme::mono(11.0), DIM);
    }

    /// The chips at the top left: the layer being worked on, and the net lit up.
    fn badges(&mut self, ui: &mut Ui, full: Rect) {
        let mut x = full.left() + 14.0;
        let y = full.top() + 14.0;
        let mut chip = |ui: &mut Ui, text: &str, dot: Color32, id: &str| -> egui::Response {
            let font = egui::FontId::proportional(13.0);
            let galley = ui.painter().layout_no_wrap(text.to_string(), font, FG);
            let rect = Rect::from_min_size(pos2(x, y), vec2(galley.size().x + 32.0, 26.0));
            x = rect.right() + 6.0;
            let response = ui.interact(rect, ui.scope_id().with(id), Sense::click());
            let fill = if response.hovered() { theme::SELECTION } else { theme::CARD.gamma_multiply(0.95) };
            ui.painter().rect(rect, 8.0, fill, Stroke::new(1.0, LINE), StrokeKind::Inside);
            ui.painter().circle_filled(pos2(rect.left() + 13.0, rect.center().y), 4.0, dot);
            ui.painter().galley(pos2(rect.left() + 23.0, rect.center().y - galley.size().y / 2.0), galley, FG);
            response
        };
        let layer = Layer::Cu(self.layer);
        if chip(ui, &layer.name(), layer_color(layer), "layer-chip").on_hover_text("Click for the next copper layer (+)").clicked() {
            self.next_layer(1);
        }
        if let Some(n) = self.highlight {
            let name = format!("{}  ×", self.doc.net_name(n));
            if chip(ui, &name, ACCENT, "net-chip").on_hover_text("Lit-up net. Click to clear (N)").clicked() {
                self.highlight = None;
            }
        }
        if let Some(r) = &self.route {
            let m = &self.route_mode;
            let blocked = match &self.plan {
                Some(c) if c.plan.blocked => " · blocked".to_string(),
                Some(c) if c.plan.note.is_some() => format!(" · {}", c.plan.note.unwrap_or_default()),
                Some(c) if !c.plan.pushed.is_empty() || !c.plan.vias.is_empty() => {
                    let (n, v) = (c.plan.pushed.len(), c.plan.vias.len());
                    let mut t = String::from(" · pushing");
                    if n > 0 {
                        t += &format!(" {n} track{}", if n == 1 { "" } else { "s" });
                    }
                    if v > 0 {
                        t += &format!("{} {v} via{}", if n > 0 { " and" } else { "" }, if v == 1 { "" } else { "s" });
                    }
                    t
                }
                _ => String::new(),
            };
            let text = format!(
                "Routing {} · {} · {} · {}{blocked}",
                self.doc.net_name(r.net),
                self.unit.len(r.width),
                m.corner.name(),
                m.conflict.name()
            );
            chip(ui, &text, layer_color(Layer::Cu(r.layer)), "route-chip");
        }
    }

    fn floating_tools(&mut self, ui: &mut Ui, full: Rect) {
        let area = egui::Area::new(ui.scope_id().with("tools"))
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(full.center().x, full.bottom() - 12.0))
            .order(egui::Order::Foreground)
            .interactable(true);
        area.show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme::CARD.gamma_multiply(0.96))
                .stroke(Stroke::new(1.0, LINE))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin::same(5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let tools = [
                            (Tool::Select, Icon::Cursor, "Select", "Click to pick, drag to move. R turns, L flips to the other side (S)"),
                            (Tool::Route, Icon::Route, "Route", "Click a pad to start a track, click to turn, V for a via (W)"),
                            (Tool::Via, Icon::Via, "Via", "Click to drop a via (V)"),
                            (Tool::Outline, Icon::Outline, "Outline", "Click the board's corners; click the first one to close (O)"),
                        ];
                        for (tool, icon, label, tip) in tools {
                            if segmented_tool(ui, icon, label, self.tool == tool).on_hover_text(tip).clicked() {
                                self.set_tool(tool);
                            }
                        }
                        ui.add_space(2.0);
                        ui.separator();
                        ui.add_space(2.0);

                        let layer = Layer::Cu(self.layer);
                        let pick = layer_pill(ui, &layer.name(), layer_color(layer)).on_hover_text("The copper layer you're working on (1–6, +, −)");
                        egui::Popup::from_toggle_button_response(&pick)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                            .align(egui::RectAlign::TOP_START)
                            .show(|ui| {
                                for l in self.doc.copper_layers() {
                                    let name = Layer::Cu(l).name();
                                    if ui.selectable_label(l == self.layer, name).clicked() {
                                        self.switch_layer(l);
                                    }
                                }
                            });

                        let width = segmented_tool(ui, Icon::Route, &self.unit.len(self.width), false)
                            .on_hover_text("Track width for new routes");
                        egui::Popup::from_toggle_button_response(&width)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                            .align(egui::RectAlign::TOP_START)
                            .show(|ui| {
                                for w in WIDTHS {
                                    if ui.selectable_label((w - self.width).abs() < 1e-9, self.unit.len(w)).clicked() {
                                        self.width = w;
                                        if let Some(r) = &mut self.route {
                                            r.width = w;
                                        }
                                    }
                                }
                            });

                        if self.tool == Tool::Route {
                            let m = self.route_mode;
                            let label = format!("{} · {}", m.corner.name(), m.conflict.name());
                            let pill = segmented_tool(ui, Icon::Filter, &label, false)
                                .on_hover_text("How the router turns corners and gets past things (Shift+Space, Shift+R)");
                            egui::Popup::from_toggle_button_response(&pill)
                                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                .align(egui::RectAlign::TOP_START)
                                .show(|ui| self.routing_menu(ui));
                        }

                        let grid = segmented_tool(ui, Icon::Grid, &self.unit.len(self.grid), self.show_grid)
                            .on_hover_text("Snap grid (G shows or hides it)");
                        egui::Popup::from_toggle_button_response(&grid)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                            .align(egui::RectAlign::TOP_START)
                            .show(|ui| {
                                for g in GRIDS {
                                    if ui.selectable_label((g - self.grid).abs() < 1e-9, self.unit.len(g)).clicked() {
                                        self.grid = g;
                                    }
                                }
                                ui.separator();
                                if ui.selectable_label(self.show_grid, "Show the grid").clicked() {
                                    self.show_grid = !self.show_grid;
                                }
                            });

                        ui.add_space(2.0);
                        ui.separator();
                        ui.add_space(2.0);
                        if segmented_tool(ui, Icon::Fit, "Fit", self.fitted).on_hover_text("Fit the board to the window (F)").clicked() {
                            self.fit_pending = true;
                        }
                        if icon_button(ui, Icon::Flip, self.mirror).on_hover_text("See it from underneath (X)").clicked() {
                            self.mirror = !self.mirror;
                        }
                        ui.add_space(2.0);
                        ui.separator();
                        ui.label(RichText::new(format!("{:.0} px/mm", self.scale)).font(theme::mono(11.5)).color(FAINT));
                    });
                });
        });
    }

    /// The router's settings, with the keys that change them as you route.
    fn routing_menu(&mut self, ui: &mut Ui) {
        use crate::pcb::router::{Conflict, Corner};
        ui.set_min_width(250.0);
        let m = &mut self.route_mode;
        let label = |ui: &mut Ui, text: &str| {
            ui.label(RichText::new(text).size(12.5).color(DIM));
        };
        label(ui, "Corners  ·  Shift+Space");
        for c in [Corner::D45, Corner::D90, Corner::Any] {
            if ui.selectable_label(m.corner == c, c.name()).clicked() {
                m.corner = c;
            }
        }
        ui.separator();
        label(ui, "When something's in the way  ·  Shift+R");
        for c in [Conflict::Walkaround, Conflict::Push, Conflict::HugPush, Conflict::Stop, Conflict::Ignore] {
            if ui.selectable_label(m.conflict == c, c.name()).clicked() {
                m.conflict = c;
            }
        }
        ui.separator();
        ui.checkbox(&mut m.via_push, "Push vias too");
        ui.label(RichText::new("Nothing is ever placed closer than the clearance rule.").size(11.5).color(FAINT));
        ui.separator();
        ui.checkbox(&mut m.look_ahead, "Look-ahead segment  (1)");
        ui.checkbox(&mut m.remove_loops, "Take out old paths a new route replaces  (Shift+D)");
        ui.checkbox(&mut m.show_clearance, "Show clearance round other nets  (Ctrl+W)");
        ui.separator();
        label(ui, &format!("Width: {}  ·  3 cycles where it comes from", m.width_from.name()));
        ui.add_space(2.0);
        let keys = "Space flips the corner · V via to the other side · 2 via here\n\
                    + − PgUp PgDn change layer with a via · Backspace takes back a click\n\
                    Ctrl+click finishes the connection · Ctrl stops snapping · Esc ends";
        ui.label(RichText::new(keys).size(11.5).color(FAINT));
    }

    pub fn drive_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.launch.screenshot.clone() else { return };
        if self.frames < 10 {
            let width = ctx.content_rect().width();
            if width < 1300.0 {
                ctx.set_zoom_factor(ctx.zoom_factor() * width / 1320.0);
            }
            if self.launch.zoom.is_none() {
                self.fit_pending = true;
            }
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        ctx.request_repaint();
        self.frames += 1;
        if self.frames < SHOT_FRAMES || self.shot_sent || !self.play.is_empty() {
            return;
        }
        self.shot_sent = true;
        let finish = ctx.clone();
        ctx.request_screenshot(move |picture| {
            let code = match save_png(&picture, &path) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("coppersmith: couldn't save the picture: {error}");
                    1
                }
            };
            finish.send_viewport_cmd(egui::ViewportCommand::Close);
            finish.request_repaint();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                std::process::exit(code);
            });
        });
    }
}

fn layer_pill(ui: &mut Ui, text: &str, dot: Color32) -> egui::Response {
    let font = egui::FontId::proportional(13.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, FG);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 32.0, 30.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 8.0, theme::RAISED);
    }
    ui.painter().circle_filled(pos2(rect.left() + 13.0, rect.center().y), 4.5, dot);
    ui.painter().galley(pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0), galley, FG);
    response
}

pub fn save_png(picture: &egui::ColorImage, path: &std::path::Path) -> Result<(), String> {
    let [w, h] = picture.size;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = picture.pixels.iter().flat_map(|c| c.to_array()).collect();
    writer.write_image_data(&bytes).map_err(|e| e.to_string())
}
