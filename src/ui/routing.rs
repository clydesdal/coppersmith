//! Laying tracks by hand, the way Altium's interactive router does it.
//!
//! While routing, the router shows what a click would lay from the route's
//! end to the pointer. All of it but the last segment is placed by a click;
//! the last (hollow, the "look-ahead") stays to be placed by the next one, so
//! you set where a corner goes without committing to what comes after.
//! Reaching a pad of the same net places the lot and ends the route.
//!
//! The thinking about where a track can go is in `pcb::router`; this is the
//! editor's side: what's been clicked, what's on show, and the keys.

use crate::pcb::copper::{self, Item};
use crate::pcb::drc;
use crate::pcb::geom::*;
#[allow(unused_imports)]
use crate::pcb::geom::ARC_ERROR;
use crate::pcb::model::*;
use crate::pcb::router::{self, Conflict, Corner, Plan, Scene};
use crate::ui::app::App;

/// The rule check's names for copper too close to something.
const CLEARANCE: [&str; 5] = ["Too close", "Short circuit", "Too close to the edge", "Off the board", "In a keepout"];

/// A route being laid: the segments and vias placed so far.
#[derive(Clone, Debug)]
pub struct Route {
    /// Placed pieces: from, to, layer, and the middle of an arc.
    pub segs: Vec<(P, P, u8, Option<P>)>,
    pub vias: Vec<P>,
    pub from: P,
    pub layer: u8,
    pub net: u32,
    pub width: f64,
    /// Which way the corner goes (Space).
    pub flip: bool,
    /// What it started on, which doesn't count as arriving.
    pub start: Option<Item>,
    /// How things stood before each click, for Backspace.
    marks: Vec<Mark>,
    /// The board before the route began, so the whole route (and anything
    /// it pushed) is one undo.
    pub before: Option<Board>,
}

#[derive(Clone, Debug)]
struct Mark {
    segs: usize,
    vias: usize,
    from: P,
    layer: u8,
    /// The board before this click pushed anything aside.
    doc: Option<Board>,
    /// The last placed piece before this click shortened it to cut a corner.
    cut: Option<(P, P, u8, Option<P>)>,
}

/// Where the width of a new track comes from (3 cycles it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WidthFrom {
    User,
    RuleMinimum,
    RulePreferred,
}

impl WidthFrom {
    pub fn name(self) -> &'static str {
        match self {
            WidthFrom::User => "your choice",
            WidthFrom::RuleMinimum => "rule minimum",
            WidthFrom::RulePreferred => "rule preferred",
        }
    }
}

/// How routing behaves; kept from one route to the next.
#[derive(Clone, Copy, Debug)]
pub struct RouteMode {
    pub corner: Corner,
    pub conflict: Conflict,
    pub look_ahead: bool,
    pub remove_loops: bool,
    pub show_clearance: bool,
    pub width_from: WidthFrom,
    /// Push may move other nets' vias too.
    pub via_push: bool,
    /// The radius corners are rounded to in the arc corner styles.
    pub arc_radius: f64,
}

impl Default for RouteMode {
    fn default() -> Self {
        RouteMode {
            corner: Corner::D45,
            conflict: Conflict::Walkaround,
            look_ahead: true,
            remove_loops: true,
            show_clearance: false,
            width_from: WidthFrom::User,
            via_push: true,
            arc_radius: 1.0,
        }
    }
}

/// The router's latest answer, and what it was asked, so it's only worked
/// out again when something changes.
#[derive(Clone, Debug)]
pub struct PlanCache {
    key: String,
    pub plan: Plan,
    pub to: P,
    pub item: Option<Item>,
    /// When the plan moves vias: the whole board once every layer has been
    /// pushed out of their way (and that of their tracks), for showing and
    /// placing.
    pub settled: Option<Board>,
}

#[derive(Default)]
pub struct ObstacleCache {
    key: Option<(u64, u8, u32)>,
    pub scene: Scene,
}

impl App {
    /// The width new tracks get, from wherever widths come from just now.
    pub fn route_width(&self) -> f64 {
        match self.route_mode.width_from {
            WidthFrom::User => self.width,
            WidthFrom::RuleMinimum => self.doc.rules.min_track,
            WidthFrom::RulePreferred => self.doc.rules.track,
        }
    }

    pub fn cycle_width_source(&mut self) {
        self.route_mode.width_from = match self.route_mode.width_from {
            WidthFrom::User => WidthFrom::RuleMinimum,
            WidthFrom::RuleMinimum => WidthFrom::RulePreferred,
            WidthFrom::RulePreferred => WidthFrom::User,
        };
        let w = self.route_width();
        if let Some(r) = &mut self.route {
            r.width = w;
        }
        self.notice = Some(format!("Track width: {} ({})", self.unit.len(w), self.route_mode.width_from.name()));
    }

    /// Where the pointer lands while routing: the centre of a pad or via, a
    /// track's end or a point along it, or else the grid. Ctrl stops it
    /// snapping to copper.
    pub fn route_snap(&self, q: P, tol: f64, snap: bool) -> (P, Option<Item>) {
        if snap {
            for cu in self.copper.at(q, self.layer, tol) {
                match cu.item {
                    Item::Pad(..) | Item::Via(_) => return (cu.anchors[0], Some(cu.item)),
                    Item::Track(_) => {
                        let (a, b) = (cu.anchors[0], cu.anchors[1]);
                        for end in [a, b] {
                            if end.dist(q) < tol.max(cu.blob.r * 1.5) && cu.parts.len() == 1 {
                                return (end, Some(cu.item));
                            }
                        }
                        let on = dist_to_segment(q.snap(self.grid), a, b);
                        let at = if on.0 < 1e-6 { q.snap(self.grid) } else { dist_to_segment(q, a, b).1.rounded() };
                        return (at, Some(cu.item));
                    }
                    // Onto a pour or fill anywhere: it's joined there.
                    Item::Pour(..) | Item::Fill(_) => return (q.snap(self.grid), Some(cu.item)),
                }
            }
        }
        (q.snap(self.grid), None)
    }

    fn net_of(&self, item: Item) -> u32 {
        self.copper.index.get(&item).map(|&k| self.copper.items[k].net).unwrap_or(0)
    }

    pub fn start_route(&mut self, q: P, tol: f64) {
        let (at, item) = self.route_snap(q, tol, true);
        let net = item.map(|i| self.net_of(i)).unwrap_or(0);
        self.route = Some(Route {
            segs: Vec::new(),
            vias: Vec::new(),
            from: at,
            layer: self.layer,
            net,
            width: self.route_width(),
            flip: false,
            start: item,
            marks: Vec::new(),
            before: Some(self.doc.clone()),
        });
        self.plan = None;
    }

    /// What's on the routing layer, worked out once per change.
    pub fn route_scene(&mut self) {
        let (layer, net) = self.route.as_ref().map(|r| (r.layer, r.net)).unwrap_or((self.layer, 0));
        let key = (self.rev, layer, net);
        if self.obstacles.key != Some(key) {
            self.obstacles.scene = Scene::new(&self.doc, &self.copper, layer, net);
            self.obstacles.key = Some(key);
        }
    }

    /// Where the last placed piece starts, when the route carries straight on
    /// from it on the same layer: the next corner can be cut back along it.
    fn lead(r: &Route) -> Option<P> {
        let last = r.segs.last()?;
        let via_here = r.vias.last().is_some_and(|v| v.same(r.from));
        (last.2 == r.layer && last.3.is_none() && last.1.same(r.from) && !via_here).then_some(last.0)
    }

    /// The route already placed on this layer, which pushed tracks keep clear of.
    fn held(r: &Route) -> Vec<(P, P)> {
        let pieces: Vec<(P, P, Option<P>)> = r.segs.iter().filter(|s| s.2 == r.layer).map(|s| (s.0, s.1, s.3)).collect();
        router::as_steps(&pieces)
    }

    /// Asks the router what it would lay to `to`, unless it already knows.
    pub fn update_plan(&mut self, to: P, item: Option<Item>) {
        let Some(r) = self.route.clone() else {
            self.plan = None;
            return;
        };
        let m = self.route_mode;
        let key = format!(
            "{} {:?} {:?} {} {} {:?} {:?} {} {} {}",
            self.rev, r.from, to, r.layer, r.net, m.corner, m.conflict, r.flip, r.width, r.segs.len()
        ) + &format!(" r{}", m.arc_radius) + if m.via_push { " v" } else { "" };
        if self.plan.as_ref().is_some_and(|c| c.key == key) {
            return;
        }
        let area = self.doc.outline_bbox();
        self.route_scene();
        self.obstacles.scene.via_push = m.via_push;
        self.obstacles.scene.arc_radius = m.arc_radius;
        self.obstacles.scene.lead = App::lead(&r);
        let started = std::time::Instant::now();
        let scene = &self.obstacles.scene;
        let held = App::held(&r);
        let mut plan = router::with_budget(150, || router::plan(scene, r.from, to, r.width, m.corner, r.flip, m.conflict, area, &held));
        // Out of time pushing: walk round instead, which is quick.
        if router::out_of_time_after(started, 150) && !matches!(m.conflict, Conflict::Walkaround | Conflict::Ignore | Conflict::Stop) {
            plan = router::with_budget(150, || router::plan(scene, r.from, to, r.width, m.corner, r.flip, Conflict::Walkaround, area, &held));
            plan.note = Some("too much to push here; walking round");
        }
        crate::ui::app::note_slow("plan", started);
        let mut settled = None;
        if !plan.vias.is_empty() {
            let pieces: Vec<(P, P, u8, Option<P>)> = plan.pieces().into_iter().map(|(a, b, mid)| (a, b, r.layer, mid)).collect();
            settled = router::with_budget(150, || self.settle(&r, &pieces, &plan.pushed, &plan.vias));
            if settled.is_none() {
                // Another layer can't make room: keep the vias still.
                self.obstacles.scene.via_push = false;
                plan = router::plan(&self.obstacles.scene, r.from, to, r.width, m.corner, r.flip, m.conflict, area, &App::held(&r));
                self.obstacles.scene.via_push = m.via_push;
            }
        }
        self.plan = Some(PlanCache { key, plan, to, item, settled });
    }

    /// The board with these pushes made and every other layer pushed out of
    /// the way of the vias they moved (and of the tracks that followed them).
    /// The route so far and `pieces` stand in as fixed for that, then come
    /// out again. None when another layer can't make room.
    fn settle(&self, r: &Route, pieces: &[(P, P, u8, Option<P>)], pushed: &[router::Moved], moved: &[(u32, P)]) -> Option<Board> {
        let mut b = self.doc.clone();
        router::apply_pushed(&mut b, pushed, moved);
        if moved.is_empty() {
            return Some(b);
        }
        // Just these come out again afterwards: tracks the pushes re-lay get
        // new ids too, and stay.
        let mut temp = std::collections::HashSet::new();
        for (a, e, layer, mid) in r.segs.iter().chain(pieces) {
            let id = b.new_id();
            b.tracks.push(Track { mid: *mid, id, a: *a, b: *e, w: r.width, layer: *layer, net: r.net });
            temp.insert(id);
        }
        let mut temp_vias = std::collections::HashSet::new();
        for at in &r.vias {
            let id = b.new_id();
            b.vias.push(Via { id, at: *at, dia: self.doc.rules.via_dia, drill: self.doc.rules.via_drill, net: r.net });
            temp_vias.insert(id);
        }
        let mut out = crate::pcb::slide::push_after_move(&self.doc, b, self.route_mode.via_push)?;
        out.tracks.retain(|t| !temp.contains(&t.id));
        out.vias.retain(|v| !temp_vias.contains(&v.id));
        Some(out)
    }

    /// Does arriving here finish the route? A pad, via or track of its own
    /// net, other than the one it started on.
    fn arrives(&self, r: &Route, end: P, to: P, item: Option<Item>) -> bool {
        let Some(item) = item else { return false };
        let net = self.net_of(item);
        end.same(to) && Some(item) != r.start && net != 0 && (net == r.net || r.net == 0)
    }

    /// What the rule check would say if this went on the board: `change`
    /// makes the change on a copy and returns what it added or moved. The
    /// answer is the first clearance problem touching any of that, if any.
    pub fn clash_with(&self, change: impl FnOnce(&mut Board) -> Vec<Item>) -> Option<String> {
        let mut b = self.doc.clone();
        let fresh = change(&mut b);
        if fresh.is_empty() {
            return None;
        }
        let found = drc::check(&b, &copper::analyse_fixed(&b));
        found
            .into_iter()
            .find(|p| CLEARANCE.contains(&p.title.as_str()) && p.items.iter().any(|i| fresh.contains(i)))
            .map(|p| format!("{} — {}", p.title.to_lowercase(), p.detail))
    }

    /// A clearance problem on the board now that wasn't there in `before`,
    /// leaving out ones only between things that were moved (a dragged part
    /// and its own tracks tangling is the drag's business, and the checks
    /// show it).
    pub fn new_clearance_problem_from_moves(&self, before: &Board) -> Option<String> {
        let changed = crate::pcb::slide::changed_tracks(before, &self.doc);
        let moved = |item: &Item| match *item {
            Item::Track(id) => changed.contains(&id),
            Item::Via(id) => before.via(id).is_none_or(|v| self.doc.via(id).is_some_and(|n| !n.at.same(v.at))),
            Item::Pad(f, _) => before.fp(f).is_none_or(|o| {
                self.doc.fp(f).is_some_and(|n| !n.at.same(o.at) || n.rot != o.rot || n.side != o.side)
            }),
            Item::Pour(..) | Item::Fill(_) => false,
        };
        let key = |p: &drc::Problem| {
            let mut items = p.items.clone();
            items.sort();
            (p.title.clone(), items)
        };
        let was: std::collections::HashSet<(String, Vec<Item>)> = drc::check(before, &copper::analyse_fixed(before))
            .iter()
            .filter(|p| CLEARANCE.contains(&p.title.as_str()))
            .map(key)
            .collect();
        drc::check(&self.doc, &copper::analyse_fixed(&self.doc))
            .into_iter()
            .filter(|p| CLEARANCE.contains(&p.title.as_str()))
            .filter(|p| !(p.items.len() == 2 && p.items.iter().all(moved)))
            .find(|p| !was.contains(&key(p)))
            .map(|p| format!("{} — {}", p.title.to_lowercase(), p.detail))
    }

    /// The rule check on a settled board with the route so far and `segs`
    /// added: anything new too close to what was there before.
    fn board_clash(&self, r: &Route, net: u32, segs: &[(P, P, u8, Option<P>)], settled: &Board) -> Option<String> {
        let before = self.doc.clone();
        let mut after = settled.clone();
        for (a, e, layer, mid) in r.segs.iter().chain(segs) {
            let id = after.new_id();
            after.tracks.push(Track { mid: *mid, id, a: *a, b: *e, w: r.width, layer: *layer, net });
        }
        for at in &r.vias {
            if !after.vias.iter().any(|v| v.at.same(*at)) {
                let id = after.new_id();
                after.vias.push(Via { id, at: *at, dia: before.rules.via_dia, drill: before.rules.via_drill, net });
            }
        }
        let key = |p: &drc::Problem| {
            let mut items = p.items.clone();
            items.sort();
            (p.title.clone(), items)
        };
        let was: std::collections::HashSet<(String, Vec<Item>)> =
            drc::check(&before, &copper::analyse_fixed(&before)).iter().filter(|p| CLEARANCE.contains(&p.title.as_str())).map(key).collect();
        drc::check(&after, &copper::analyse_fixed(&after))
            .into_iter()
            .filter(|p| CLEARANCE.contains(&p.title.as_str()))
            .find(|p| !was.contains(&key(p)))
            .map(|p| format!("{} — {}", p.title.to_lowercase(), p.detail))
    }

    /// The rule check on the route so far plus `segs` and `vias`, with
    /// pushes made, all as `net`.
    #[allow(clippy::too_many_arguments)]
    fn route_clash(&self, r: &Route, net: u32, segs: &[(P, P, u8, Option<P>)], vias: &[P], pushed: &[router::Moved], moved: &[(u32, P)]) -> Option<String> {
        let rules = self.doc.rules.clone();
        self.clash_with(|b| {
            let mut fresh: Vec<Item> = router::apply_pushed(b, pushed, moved).into_iter().map(Item::Track).collect();
            fresh.extend(moved.iter().map(|(id, _)| Item::Via(*id)));
            for (a, e, layer, mid) in r.segs.iter().chain(segs) {
                let id = b.new_id();
                b.tracks.push(Track { mid: *mid, id, a: *a, b: *e, w: r.width, layer: *layer, net });
                fresh.push(Item::Track(id));
            }
            for at in r.vias.iter().chain(vias) {
                if b.vias.iter().any(|v| v.at.same(*at)) {
                    continue;
                }
                let id = b.new_id();
                b.vias.push(Via { id, at: *at, dia: rules.via_dia, drill: rules.via_drill, net });
                fresh.push(Item::Via(id));
            }
            fresh
        })
    }

    /// A click (or Enter) while routing: places what the router planned,
    /// all but the look-ahead segment unless it finishes the route. Nothing
    /// is placed that the rule check would call too close.
    pub fn route_click(&mut self) {
        let Some(c) = self.plan.clone() else { return };
        let Some(r) = self.route.clone() else { return };
        let mut pts = c.plan.pts.clone();
        let mut mids: Vec<Option<P>> = (0..pts.len().saturating_sub(1)).map(|i| c.plan.mid(i)).collect();
        if pts.len() < 2 {
            return;
        }
        // Ignore shows what it would run over; it only places what's clear.
        let mut cut = false;
        if self.route_mode.conflict == Conflict::Ignore {
            self.route_scene();
            let (clear, clear_mids, clashed) = router::cut_at_clash(&self.obstacles.scene.own, &c.plan.pieces(), r.width);
            if clashed {
                cut = true;
                pts = clear;
                mids = clear_mids;
                self.notice = Some("Placed up to the first clash: the clearance always holds.".into());
            }
            if pts.len() < 2 {
                return;
            }
        }
        let end = *pts.last().unwrap();
        let arrived = !cut && !c.plan.blocked && self.arrives(&r, end, c.to, c.item);
        let upto = if arrived || !self.route_mode.look_ahead || pts.len() <= 2 { pts.len() } else { pts.len() - 1 };
        let mut net = if arrived && r.net == 0 { c.item.map(|i| self.net_of(i)).unwrap_or(0) } else { r.net };
        if net == 0
            && let Some(item) = c.item
            && pts[upto - 1].same(c.to)
        {
            // Started in the open and landed on copper: take its net.
            net = self.net_of(item);
        }
        // What this click pushes aside: all of the plan's pushes when it
        // places the whole plan, else just enough for the part it places.
        let (pushed, moved) = if c.plan.pushed.is_empty() && c.plan.vias.is_empty() {
            (Vec::new(), Vec::new())
        } else if upto == pts.len() && !cut {
            (c.plan.pushed.clone(), c.plan.vias.clone())
        } else {
            self.route_scene();
            self.obstacles.scene.via_push = self.route_mode.via_push;
            let mut segs = App::held(&r);
            let part: Vec<(P, P, Option<P>)> = pts[..upto].windows(2).enumerate().map(|(i, s)| (s[0], s[1], mids[i])).collect();
            segs.extend(router::as_steps(&part));
            let width = if mids.iter().any(|m| m.is_some()) { r.width + 2.0 * ARC_ERROR } else { r.width };
            match router::shove(&self.obstacles.scene, &segs, width) {
                Some(done) => done,
                None => {
                    self.notice = Some("Not placed: what's in the way can't be pushed aside.".into());
                    return;
                }
            }
        };
        let segs: Vec<(P, P, u8, Option<P>)> = pts[..upto]
            .windows(2)
            .enumerate()
            .filter(|(_, s)| !s[0].same(s[1]))
            .map(|(i, s)| (s[0], s[1], r.layer, mids[i]))
            .collect();
        // Vias that moved push the other layers too; what's placed is the
        // settled board.
        let settled = if moved.is_empty() {
            None
        } else {
            match (upto == pts.len() && !cut).then(|| c.settled.clone()).flatten().or_else(|| self.settle(&r, &segs, &pushed, &moved)) {
                Some(b) => Some(b),
                None => {
                    self.notice = Some("Not placed: another layer can't make room for the vias this pushes.".into());
                    return;
                }
            }
        };
        let clash = match &settled {
            Some(board) => self.board_clash(&r, net, &segs, board),
            None => self.route_clash(&r, net, &segs, &[], &pushed, &moved),
        };
        if let Some(why) = clash {
            self.notice = Some(format!("Not placed: {why}"));
            return;
        }
        // The corner at the click point was cut back along the last placed piece.
        let cut_back = !pts[0].same(r.from) && App::lead(&r).is_some();
        if !pts[0].same(r.from) && !cut_back {
            return;
        }
        let doc_before = (!pushed.is_empty() || !moved.is_empty()).then(|| self.doc.clone());
        if doc_before.is_some() {
            match settled {
                Some(board) => self.doc = board,
                None => {
                    router::apply_pushed(&mut self.doc, &pushed, &moved);
                }
            }
            self.touch();
        }
        let Some(route) = &mut self.route else { return };
        route.marks.push(Mark { segs: route.segs.len(), vias: route.vias.len(), from: route.from, layer: route.layer, doc: doc_before, cut: None });
        if cut_back {
            // Backspace puts the piece back as it was, by keeping a copy.
            if let Some(last) = route.segs.pop() {
                route.segs.push((last.0, pts[0], last.2, None));
                route.marks.last_mut().unwrap().cut = Some(last);
            }
        }
        route.segs.extend(segs);
        route.from = pts[upto - 1];
        route.net = net;
        self.plan = None;
        if arrived {
            self.finish_route();
        }
    }

    /// Ctrl+click: route the rest of the way to the nearest piece of the net
    /// that isn't joined up yet, round whatever is in the way.
    pub fn route_autocomplete(&mut self) {
        let Some(r) = self.route.clone() else { return };
        if r.net == 0 {
            self.notice = Some("Start on a pad with a net to finish a connection by itself.".into());
            return;
        }
        let joined: Vec<Item> = r.start.map(|s| self.copper.connected(s)).unwrap_or_default();
        let target = self
            .copper
            .items
            .iter()
            .filter(|c| c.net == r.net && c.on.has(r.layer) && !c.item.poured() && !joined.contains(&c.item))
            .flat_map(|c| c.anchors.iter().map(move |a| (*a, c.item)))
            .min_by(|a, b| a.0.dist(r.from).total_cmp(&b.0.dist(r.from)));
        let Some((to, item)) = target else {
            self.notice = Some(format!("{} has nothing left to join on this layer.", self.doc.net_name(r.net)));
            return;
        };
        let area = self.doc.outline_bbox();
        self.route_scene();
        let plan = router::plan(&self.obstacles.scene, r.from, to, r.width, self.route_mode.corner, r.flip, Conflict::Walkaround, area, &[]);
        if plan.blocked {
            self.notice = Some("There's no clear way to finish this on this layer.".into());
            return;
        }
        self.plan = Some(PlanCache { key: String::new(), plan, to, item: Some(item), settled: None });
        let look = self.route_mode.look_ahead;
        self.route_mode.look_ahead = false;
        self.route_click();
        self.route_mode.look_ahead = look;
        self.finish_route();
    }

    /// Drops a via where the route has got to; with a layer, carries on there.
    pub fn route_via(&mut self, to_layer: Option<u8>) {
        let Some(r) = self.route.clone() else { return };
        let fresh = !r.vias.last().is_some_and(|v| v.same(r.from));
        // A via is wider than the track: it has to fit here on every layer.
        if fresh && let Some(why) = self.route_clash(&r, r.net, &[], &[r.from], &[], &[]) {
            self.notice = Some(format!("No room for a via here: {why}"));
            return;
        }
        let Some(r) = &mut self.route else { return };
        if fresh || to_layer.is_some() {
            r.marks.push(Mark { segs: r.segs.len(), vias: r.vias.len(), from: r.from, layer: r.layer, doc: None, cut: None });
        }
        if fresh {
            r.vias.push(r.from);
        }
        if let Some(layer) = to_layer {
            r.layer = layer;
            self.layer = layer;
            self.hidden.remove(&Layer::Cu(layer));
        }
        self.plan = None;
    }

    /// Top and bottom swap; an inner layer goes to the top.
    pub fn route_via_other_side(&mut self) {
        let next = if self.layer == TOP { BOTTOM } else { TOP };
        self.route_via(Some(next));
    }

    /// Backspace: takes back the last click.
    pub fn route_back(&mut self) {
        let Some(r) = &mut self.route else { return };
        match r.marks.pop() {
            Some(m) => {
                r.segs.truncate(m.segs);
                if let Some(piece) = m.cut {
                    r.segs.pop();
                    r.segs.push(piece);
                }
                r.vias.truncate(m.vias);
                r.from = m.from;
                r.layer = m.layer;
                self.layer = m.layer;
                // Whatever that click pushed goes back where it was.
                if let Some(doc) = m.doc {
                    self.doc = doc;
                    self.touch();
                }
            }
            None => self.route = None,
        }
        self.plan = None;
    }

    /// Puts the route's tracks and vias on the board, and takes out any old
    /// path the new one makes redundant.
    pub fn finish_route(&mut self) {
        let started = std::time::Instant::now();
        self.finish_route_inner();
        crate::ui::app::note_slow("finishing a route", started);
    }

    fn finish_route_inner(&mut self) {
        self.plan = None;
        let Some(mut r) = self.route.take() else { return };
        let before = r.before.take().unwrap_or_else(|| self.doc.clone());
        if r.segs.is_empty() {
            if self.doc != before {
                self.doc = before;
                self.touch();
            }
            return;
        }
        let rules = self.doc.rules.clone();
        let loops = self.route_mode.remove_loops && r.net != 0;
        let spare_before = if loops { router::spare_tracks(&self.doc, r.net) } else { Vec::new() };
        let mut removed = 0;
        // One undo for the route and everything it pushed.
        let mut board = self.doc.clone();
        let b = &mut board;
        {
            let mut new = Vec::new();
            for (a, e, layer, mid) in &r.segs {
                let id = b.new_id();
                b.tracks.push(Track { mid: *mid, id, a: *a, b: *e, w: r.width, layer: *layer, net: r.net });
                new.push(id);
            }
            for v in &r.vias {
                if b.vias.iter().any(|old| old.at.same(*v)) {
                    continue;
                }
                let id = b.new_id();
                b.vias.push(Via { id, at: *v, dia: rules.via_dia, drill: rules.via_drill, net: r.net });
            }
            if loops {
                let count = b.tracks.len();
                router::remove_loops(b, r.net, &spare_before, &new);
                removed = count - b.tracks.len();
            }
            // The new route may run along an old track of the net: one track, not two.
            crate::ui::app::tidy_tracks(b, &new.iter().copied().collect());
        }
        self.doc = board;
        if self.doc != before {
            self.remember(before);
        }
        if removed > 0 {
            self.notice = Some(format!("Took out {removed} old track{} the new route replaces.", if removed == 1 { "" } else { "s" }));
        }
    }
}

impl Route {
    /// The board as it was before the route began, if the route has
    /// pushed anything since.
    pub fn take_before(&mut self) -> Option<Board> {
        self.before.take()
    }
}
