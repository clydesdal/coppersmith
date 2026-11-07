//! Every piece of copper on the board, which pieces touch, and the thin
//! "still to connect" lines between the pieces of a net that don't yet.

use crate::pcb::geom::*;
use crate::pcb::model::*;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Item {
    Pad(u32, u16),
    Track(u32),
    Via(u32),
    /// One island of a pour.
    Pour(u32, u16),
    Fill(u32),
}

impl Item {
    /// Copper that pours itself again after every change, so routing and
    /// moving don't treat it as in the way.
    pub fn poured(self) -> bool {
        matches!(self, Item::Pour(..))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum On {
    One(u8),
    /// Through-hole pads and vias are on every copper layer.
    All,
}

impl On {
    pub fn has(self, layer: u8) -> bool {
        match self {
            On::One(l) => l == layer,
            On::All => true,
        }
    }
    pub fn shares(self, o: On) -> bool {
        match (self, o) {
            (On::One(a), On::One(b)) => a == b,
            _ => true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Cu {
    pub item: Item,
    /// The main piece: the whole thing, or for an arc its chord.
    pub blob: Blob,
    /// All of it, as pieces: just `blob`, or the many pieces of an arc.
    pub parts: std::sync::Arc<Vec<Blob>>,
    /// Each part's box.
    pub boxes: std::sync::Arc<Vec<BBox>>,
    /// Finds the parts near a place, for copper made of many (a pour).
    pub grid: Option<std::sync::Arc<BoxGrid>>,
    pub bbox: BBox,
    pub on: On,
    pub net: u32,
    /// Where a "still to connect" line can end.
    pub anchors: Vec<P>,
    /// For a pour: the clearance it keeps from other nets and from the edge,
    /// where they differ from the board's rules.
    pub keeps: Option<(f64, f64)>,
}

impl Cu {
    fn new(item: Item, blob: Blob, parts: Vec<Blob>, on: On, net: u32, anchors: Vec<P>) -> Cu {
        Cu::shared(item, blob, Parts::new(parts), on, net, anchors)
    }

    /// As `new`, from pieces already worked out.
    fn shared(item: Item, blob: Blob, parts: Parts, on: On, net: u32, anchors: Vec<P>) -> Cu {
        Cu { item, blob, parts: parts.blobs, boxes: parts.boxes, grid: parts.grid, bbox: parts.bbox, on, net, anchors, keeps: None }
    }

    /// Is this the same copper, in the same place, on the same net and
    /// layers, as `o`? What a check reads is all that's compared.
    pub fn same_as(&self, o: &Cu) -> bool {
        self.item == o.item
            && self.net == o.net
            && self.on == o.on
            && self.keeps == o.keeps
            && self.anchors == o.anchors
            && self.blob == o.blob
            && (std::sync::Arc::ptr_eq(&self.parts, &o.parts) || self.parts == o.parts)
    }

    /// The gap to another piece of copper and the nearest points, worked
    /// out exactly only where it could be under `limit`;
    /// otherwise anything at least `limit`.
    pub fn gap_within(&self, o: &Cu, limit: f64) -> (f64, P, P) {
        let mut best = (f64::MAX, P::default(), P::default());
        let (small, big) = if self.parts.len() <= o.parts.len() { (self, o) } else { (o, self) };
        let mut near: Vec<u32> = Vec::new();
        for (a, ab) in small.parts.iter().zip(small.boxes.iter()) {
            if box_gap(ab, &big.bbox) >= best.0.min(limit) {
                continue;
            }
            let try_part =|k: usize, best: &mut (f64, P, P)| {
                if box_gap(ab, &big.boxes[k]) >= best.0.min(limit) {
                    return;
                }
                let g = a.gap(&big.parts[k]);
                if g.0 < best.0 {
                    *best = if std::ptr::eq(small, self) { g } else { (g.0, g.2, g.1) };
                }
            };
            match &big.grid {
                Some(grid) => {
                    grid.near(&ab.grown(best.0.min(limit)), &mut near);
                    for &k in &near {
                        try_part(k as usize, &mut best);
                    }
                }
                None => {
                    for k in 0..big.parts.len() {
                        try_part(k, &mut best);
                    }
                }
            }
        }
        best
    }

    /// The least distance from a point to this copper, worked out exactly
    /// only where it could be within `tol`; otherwise anything more.
    pub fn dist_within(&self, q: P, tol: f64) -> f64 {
        match &self.grid {
            Some(grid) => {
                let mut near = Vec::new();
                grid.near(&BBox { min: q, max: q }.grown(tol), &mut near);
                near.iter().map(|&k| self.parts[k as usize].dist_to(q)).fold(f64::MAX, f64::min)
            }
            None => self.dist_to(q),
        }
    }

    pub fn dist_to(&self, q: P) -> f64 {
        self.parts.iter().map(|b| b.dist_to(q)).fold(f64::MAX, f64::min)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Rat {
    pub a: P,
    pub b: P,
    pub net: u32,
}

#[derive(Default)]
pub struct Copper {
    pub items: Vec<Cu>,
    /// Which touching group each item is in.
    pub group: Vec<usize>,
    pub rats: Vec<Rat>,
    pub index: HashMap<Item, usize>,
    /// The pairs of items (as places in `items`) that touch and are of one
    /// net: what the groups are made from. Kept so the next analysis need
    /// only test pairs that involve copper that has changed.
    touching: Vec<(usize, usize)>,
    /// Worked out by `analyse`, so `touching` can be relied on.
    analysed: bool,
}

/// All the copper, pours included.
pub fn collect(board: &Board) -> Vec<Cu> {
    let mut items = collect_fixed(board);
    for pour in &board.pours {
        for (k, island) in pour.fill.iter().enumerate() {
            // Its pieces were worked out when it was poured.
            let parts = island.parts.clone();
            if parts.blobs.is_empty() {
                continue;
            }
            // Where a "still to connect" line may end: corners round it.
            let ring = &island.rings[0];
            let step = (ring.len() / 24).max(1);
            let anchors: Vec<P> = ring.iter().step_by(step).copied().collect();
            let first = parts.blobs[0].clone();
            let mut cu = Cu::shared(Item::Pour(pour.id, k as u16), first, parts, On::One(pour.layer), pour.net, anchors);
            let near = if pour.plane { board.rules.clearance } else { pour.clearance };
            cu.keeps = Some((near, pour.edge));
            items.push(cu);
        }
    }
    items
}

/// The copper that stays put: everything but pours.
pub fn collect_fixed(board: &Board) -> Vec<Cu> {
    collect_fixed_with(board, true)
}

/// As `collect_fixed`, with the tracks left out unless `tracks`.
fn collect_fixed_with(board: &Board, tracks: bool) -> Vec<Cu> {
    let mut items = Vec::new();
    for f in &board.footprints {
        for (i, pad) in f.pads.iter().enumerate() {
            if pad.bare_hole() {
                continue;
            }
            let blob = f.pad_blob(i, 0.0);
            let on = if pad.through() { On::All } else { On::One(f.copper()) };
            items.push(Cu::new(Item::Pad(f.id, i as u16), blob.clone(), vec![blob], on, pad.net, vec![f.pad_center(i)]));
        }
    }
    for t in board.tracks.iter().filter(|_| tracks) {
        items.push(Cu::new(Item::Track(t.id), t.blob(), t.blobs(), On::One(t.layer), t.net, vec![t.a, t.b]));
    }
    for v in &board.vias {
        let blob = v.blob();
        items.push(Cu::new(Item::Via(v.id), blob.clone(), vec![blob], On::All, v.net, vec![v.at]));
    }
    for f in &board.fills {
        let parts: Vec<Blob> = f.pieces().into_iter().map(|core| Blob { core, r: 0.0 }).collect();
        if parts.is_empty() {
            continue;
        }
        let anchors = vec![BBox::of(&f.pts).center()];
        items.push(Cu::new(Item::Fill(f.id), parts[0].clone(), parts, On::One(f.layer), f.net, anchors));
    }
    items
}

/// Pairs of items on a layer in common whose boxes come within `margin` of
/// each other, of those `wanted` says yes to. It's asked last, only of pairs
/// that are near, so a caller wanting few pairs builds a short list.
pub fn near_pairs_where(items: &[Cu], margin: f64, wanted: impl Fn(usize, usize) -> bool) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|a, b| items[*a].bbox.min.x.total_cmp(&items[*b].bbox.min.x));
    let mut pairs = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        let grown = items[i].bbox.grown(margin);
        for &j in &order[k + 1..] {
            if items[j].bbox.min.x > grown.max.x {
                break;
            }
            if grown.touches(&items[j].bbox) && items[i].on.shares(items[j].on) && wanted(i, j) {
                pairs.push((i.min(j), i.max(j)));
            }
        }
    }
    pairs
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Connectivity and still-to-connect lines, pours included.
pub fn analyse(board: &Board) -> Copper {
    analyse_items(collect(board), None)
}

/// The same as `analyse`, given the last one: pairs of copper that haven't
/// changed since aren't tested again for touching. The answer is what
/// `analyse` would give.
pub fn analyse_again(board: &Board, last: &Copper) -> Copper {
    analyse_items(collect(board), Some(last))
}

/// Just the copper that stays put, listed, without working out what touches
/// what or what's still to connect: all a router `Scene` reads, and much
/// quicker than `analyse`, which is the wrong thing to run on every step of
/// a push. Its `group` and `rats` are empty. Kept for tests, as the reference
/// a `Scene::for_push` is checked against.
#[cfg(test)]
pub fn scene_copper(board: &Board) -> Copper {
    let items = collect_fixed(board);
    let index = items.iter().enumerate().map(|(i, c)| (c.item, i)).collect();
    Copper { items, group: Vec::new(), rats: Vec::new(), index, touching: Vec::new(), analysed: false }
}

/// As `scene_copper`, without the tracks: for `Scene::for_push`, which finds
/// the tracks it needs on the board itself.
pub fn push_copper(board: &Board) -> Copper {
    let items = collect_fixed_with(board, false);
    let index = items.iter().enumerate().map(|(i, c)| (c.item, i)).collect();
    Copper { items, group: Vec::new(), rats: Vec::new(), index, touching: Vec::new(), analysed: false }
}

/// The same leaving pours out: for checking a change before the pours are
/// poured again round it.
pub fn analyse_fixed(board: &Board) -> Copper {
    analyse_items(collect_fixed(board), None)
}

fn analyse_items(items: Vec<Cu>, last: Option<&Copper>) -> Copper {
    // Which copper is new or different since last time, and the touching
    // pairs among the rest, which are as they were.
    let last = last.filter(|l| l.analysed);
    let mut dirty = vec![true; items.len()];
    let mut touching: Vec<(usize, usize)> = Vec::new();
    if let Some(l) = last {
        let mut now: Vec<Option<usize>> = vec![None; l.items.len()];
        for (i, c) in items.iter().enumerate() {
            if let Some(&k) = l.index.get(&c.item)
                && l.items[k].same_as(c)
            {
                dirty[i] = false;
                now[k] = Some(i);
            }
        }
        touching.extend(l.touching.iter().filter_map(|&(a, b)| Some((now[a]?, now[b]?))));
    }
    for (i, j) in near_pairs_where(&items, 0.0, |i, j| dirty[i] || dirty[j]) {
        let (a, b) = (&items[i], &items[j]);
        if a.net == 0 || a.net != b.net || !a.on.shares(b.on) {
            continue;
        }
        if a.gap_within(b, 1e-6).0 <= 1e-6 {
            touching.push((i, j));
        }
    }
    let mut parent: Vec<usize> = (0..items.len()).collect();
    for &(i, j) in &touching {
        let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
        parent[ri] = rj;
    }
    let group: Vec<usize> = (0..items.len()).map(|i| find(&mut parent, i)).collect();

    // Each net's groups, joined up by the shortest lines that link them all.
    // Groups are kept in the order their first item comes, so the same copper
    // always gives the same lines.
    let mut by_net: HashMap<u32, (HashMap<usize, usize>, Vec<Vec<P>>)> = HashMap::new();
    for (i, cu) in items.iter().enumerate() {
        if cu.net != 0 {
            let (slots, groups) = by_net.entry(cu.net).or_default();
            let slot = *slots.entry(group[i]).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[slot].extend(cu.anchors.iter().copied());
        }
    }
    let mut rats = Vec::new();
    let mut nets: Vec<_> = by_net.into_iter().collect();
    nets.sort_by_key(|(n, _)| *n);
    for (net, (_, groups)) in nets {
        let k = groups.len();
        if k < 2 {
            continue;
        }
        let closest = |a: &[P], b: &[P]| {
            let mut best = (f64::MAX, P::default(), P::default());
            for &x in a {
                for &y in b {
                    let d = x.dist(y);
                    if d < best.0 {
                        best = (d, x, y);
                    }
                }
            }
            best
        };
        // Prim's tree over the groups.
        let mut joined = vec![false; k];
        let mut best: Vec<(f64, P, P)> = vec![(f64::MAX, P::default(), P::default()); k];
        joined[0] = true;
        for j in 1..k {
            best[j] = closest(&groups[0], &groups[j]);
        }
        for _ in 1..k {
            let Some(next) = (0..k).filter(|j| !joined[*j]).min_by(|a, b| best[*a].0.total_cmp(&best[*b].0)) else { break };
            joined[next] = true;
            rats.push(Rat { a: best[next].1, b: best[next].2, net });
            for j in 0..k {
                if !joined[j] {
                    let c = closest(&groups[next], &groups[j]);
                    if c.0 < best[j].0 {
                        best[j] = c;
                    }
                }
            }
        }
    }
    let index = items.iter().enumerate().map(|(i, c)| (c.item, i)).collect();
    Copper { items, group, rats, index, touching, analysed: true }
}

impl Copper {
    /// Everything joined to this item by copper.
    pub fn connected(&self, item: Item) -> Vec<Item> {
        let Some(&i) = self.index.get(&item) else { return Vec::new() };
        let g = self.group[i];
        self.items.iter().enumerate().filter(|(j, _)| self.group[*j] == g).map(|(_, c)| c.item).collect()
    }

    /// The copper under a point on a layer, nearest first.
    pub fn at(&self, q: P, layer: u8, tol: f64) -> Vec<&Cu> {
        let mut hits: Vec<(f64, &Cu)> = self
            .items
            .iter()
            .filter(|c| c.on.has(layer) && c.bbox.grown(tol).contains(q))
            .map(|c| (c.dist_within(q, tol), c))
            .filter(|(d, _)| *d <= tol)
            .collect();
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        hits.into_iter().map(|(_, c)| c).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(b: &mut Board, at: P, net: u32) -> u32 {
        let id = b.new_id();
        b.footprints.push(Footprint {
            id,
            def: "TP".into(),
            refdes: format!("TP{id}"),
            value: String::new(),
            at,
            rot: 0.0,
            side: Side::Top,
            pads: vec![Pad { num: "1".into(), at: p(0.0, 0.0), size: p(1.0, 1.0), rot: 0.0, shape: PadShape::Circle, drill: 0.0, net }],
            graphics: vec![],
            label_at: P::default(),
            locked: false,
            keepouts: vec![],
        });
        id
    }

    fn laid(b: &mut Board, a: (f64, f64), e: (f64, f64), layer: u8, net: u32) -> u32 {
        let id = b.new_id();
        b.tracks.push(Track { mid: None, id, a: p(a.0, a.1), b: p(e.0, e.1), w: 0.25, layer, net });
        id
    }

    /// Which items are joined, however the groups happen to be numbered.
    fn partition(c: &Copper) -> Vec<usize> {
        let mut first: HashMap<usize, usize> = HashMap::new();
        c.group.iter().enumerate().map(|(i, g)| *first.entry(*g).or_insert(i)).collect()
    }

    /// Analysing again after each change gives what analysing from scratch does.
    #[test]
    fn analysing_again_gives_what_analysing_from_scratch_gives() {
        let mut b = Board::default();
        let (a, n, g) = (b.net("A"), b.net("B"), b.net("GND"));
        let pa1 = dot(&mut b, p(5.0, 5.0), a);
        let pa2 = dot(&mut b, p(30.0, 5.0), a);
        dot(&mut b, p(5.0, 20.0), n);
        dot(&mut b, p(30.0, 20.0), n);
        let ta = laid(&mut b, (5.0, 5.0), (30.0, 5.0), TOP, a);
        laid(&mut b, (5.0, 20.0), (17.0, 20.0), TOP, n);
        let id = b.new_id();
        b.pours.push(Pour::new(id, BOTTOM, g, Vec::new(), &b.rules));
        b.repour();

        let mut last = Copper::default();
        let mut step = 0;
        let mut go = |b: &mut Board, what: &str| -> usize {
            b.repour();
            let again = analyse_again(b, &last);
            let scratch = analyse(b);
            let items = |c: &Copper| c.items.iter().map(|x| x.item).collect::<Vec<_>>();
            assert_eq!(items(&again), items(&scratch), "step {step}: {what}: items");
            assert_eq!(partition(&again), partition(&scratch), "step {step}: {what}: groups");
            assert_eq!(format!("{:?}", again.rats), format!("{:?}", scratch.rats), "step {step}: {what}: lines");
            assert_eq!(again.index, scratch.index, "step {step}: {what}: index");
            let rats = scratch.rats.len();
            last = again;
            step += 1;
            rats
        };
        let start = go(&mut b, "the first time");
        assert_eq!(start, 1, "B's two pads aren't joined yet");
        assert_eq!(start, go(&mut b, "nothing changed"));

        // Joining B's pads takes the line away.
        let half = laid(&mut b, (17.0, 20.0), (30.0, 20.0), TOP, n);
        assert_eq!(go(&mut b, "the second half of B"), 0);
        // A gap opens in it and the line comes back, then the gap closes.
        b.tracks.iter_mut().find(|t| t.id == half).unwrap().a = p(20.0, 20.0);
        assert_eq!(go(&mut b, "a gap in B"), 1);
        b.tracks.iter_mut().find(|t| t.id == half).unwrap().a = p(17.0, 20.0);
        assert_eq!(go(&mut b, "the gap closed"), 0);
        // A's track goes, and its pads are apart.
        b.tracks.retain(|t| t.id != ta);
        assert_eq!(go(&mut b, "A's track gone"), 1);
        // A via where a track of A could join them: on its own it joins nothing.
        let vid = b.new_id();
        b.vias.push(Via { id: vid, at: p(17.0, 5.0), dia: 0.6, drill: 0.3, net: a });
        go(&mut b, "a via");
        // Tracks on both layers meeting at the via join A's pads.
        laid(&mut b, (5.0, 5.0), (17.0, 5.0), TOP, a);
        laid(&mut b, (17.0, 5.0), (30.0, 5.0), BOTTOM, a);
        let joined = go(&mut b, "A joined through the via");
        assert_eq!(joined, 1, "the second pad is on top and the track below only ends under it");
        // A pad moves, and a part goes.
        b.footprints.iter_mut().find(|f| f.id == pa2).unwrap().at = p(35.0, 8.0);
        go(&mut b, "a pad moved");
        b.footprints.retain(|f| f.id != pa1);
        go(&mut b, "a part removed");
        // The pour takes another net, and a track changes net.
        b.pours[0].net = n;
        go(&mut b, "a pour of another net");
        b.tracks[0].net = g;
        go(&mut b, "a track of another net");
    }

    #[test]
    fn a_track_joins_two_pads_and_the_line_goes() {
        let mut b = Board::default();
        let n = b.net("SIG");
        dot(&mut b, p(0.0, 0.0), n);
        dot(&mut b, p(10.0, 0.0), n);
        dot(&mut b, p(5.0, 8.0), n);
        assert_eq!(analyse(&b).rats.len(), 2);
        b.tracks.push(Track { mid: None, id: 99, a: p(0.0, 0.0), b: p(10.0, 0.0), w: 0.25, layer: TOP, net: n });
        let c = analyse(&b);
        assert_eq!(c.rats.len(), 1);
        // A track on the bottom doesn't reach surface pads on top.
        b.tracks[0].layer = BOTTOM;
        // A stray track is a piece of its own, so there's one more line.
        assert_eq!(analyse(&b).rats.len(), 3);
    }
}
