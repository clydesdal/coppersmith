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
use crate::pcb::model::*;
use crate::pcb::router::{self, Conflict, Corner, Plan, Scene};
use crate::ui::app::App;

/// The rule check's names for copper too close to something.
const CLEARANCE: [&str; 4] = ["Too close", "Short circuit", "Too close to the edge", "Off the board"];

/// A route being laid: the segments and vias placed so far.
#[derive(Clone, Debug)]
pub struct Route {
    pub segs: Vec<(P, P, u8)>,
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
                            if end.dist(q) < tol.max(cu.blob.r * 1.5) {
                                return (end, Some(cu.item));
                            }
                        }
                        let on = dist_to_segment(q.snap(self.grid), a, b);
                        let at = if on.0 < 1e-6 { q.snap(self.grid) } else { dist_to_segment(q, a, b).1.rounded() };
                        return (at, Some(cu.item));
                    }
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

    /// The route already placed on this layer, which pushed tracks keep clear of.
    fn held(r: &Route) -> Vec<(P, P)> {
        r.segs.iter().filter(|s| s.2 == r.layer).map(|s| (s.0, s.1)).collect()
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
        ) + if m.via_push { " v" } else { "" };
        if self.plan.as_ref().is_some_and(|c| c.key == key) {
            return;
        }
        let area = self.doc.outline_bbox();
        self.route_scene();
        self.obstacles.scene.via_push = m.via_push;
        let plan = router::plan(&self.obstacles.scene, r.from, to, r.width, m.corner, r.flip, m.conflict, area, &App::held(&r));
        self.plan = Some(PlanCache { key, plan, to, item });
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
        let found = drc::check(&b, &copper::analyse(&b));
        found
            .into_iter()
            .find(|p| CLEARANCE.contains(&p.title.as_str()) && p.items.iter().any(|i| fresh.contains(i)))
            .map(|p| format!("{} — {}", p.title.to_lowercase(), p.detail))
    }

    /// The rule check on the route so far plus `segs` and `vias`, with
    /// pushes made, all as `net`.
    #[allow(clippy::too_many_arguments)]
    fn route_clash(&self, r: &Route, net: u32, segs: &[(P, P, u8)], vias: &[P], pushed: &[router::Moved], moved: &[(u32, P)]) -> Option<String> {
        let rules = self.doc.rules.clone();
        self.clash_with(|b| {
            let mut fresh: Vec<Item> = router::apply_pushed(b, pushed, moved).into_iter().map(Item::Track).collect();
            fresh.extend(moved.iter().map(|(id, _)| Item::Via(*id)));
            for (a, e, layer) in r.segs.iter().chain(segs) {
                let id = b.new_id();
                b.tracks.push(Track { id, a: *a, b: *e, w: r.width, layer: *layer, net });
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
        if pts.len() < 2 {
            return;
        }
        // Ignore shows what it would run over; it only places what's clear.
        let mut cut = false;
        if self.route_mode.conflict == Conflict::Ignore {
            self.route_scene();
            let (clear, clashed) = router::stop_short(&self.obstacles.scene.own, &pts, r.width);
            if clashed {
                cut = true;
                pts = clear;
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
            segs.extend(pts[..upto].windows(2).map(|s| (s[0], s[1])));
            match router::shove(&self.obstacles.scene, &segs, r.width) {
                Some(done) => done,
                None => {
                    self.notice = Some("Not placed: what's in the way can't be pushed aside.".into());
                    return;
                }
            }
        };
        let segs: Vec<(P, P, u8)> =
            pts[..upto].windows(2).filter(|s| !s[0].same(s[1])).map(|s| (s[0], s[1], r.layer)).collect();
        if let Some(why) = self.route_clash(&r, net, &segs, &[], &pushed, &moved) {
            self.notice = Some(format!("Not placed: {why}"));
            return;
        }
        let doc_before = (!pushed.is_empty() || !moved.is_empty()).then(|| self.doc.clone());
        if doc_before.is_some() {
            router::apply_pushed(&mut self.doc, &pushed, &moved);
            self.touch();
        }
        let Some(route) = &mut self.route else { return };
        route.marks.push(Mark { segs: route.segs.len(), vias: route.vias.len(), from: route.from, layer: route.layer, doc: doc_before });
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
            .filter(|c| c.net == r.net && c.on.has(r.layer) && !joined.contains(&c.item))
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
        self.plan = Some(PlanCache { key: String::new(), plan, to, item: Some(item) });
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
            r.marks.push(Mark { segs: r.segs.len(), vias: r.vias.len(), from: r.from, layer: r.layer, doc: None });
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
            for (a, e, layer) in &r.segs {
                let id = b.new_id();
                b.tracks.push(Track { id, a: *a, b: *e, w: r.width, layer: *layer, net: r.net });
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
            crate::ui::app::merge_straight(b);
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
