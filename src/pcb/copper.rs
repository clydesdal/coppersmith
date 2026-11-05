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
    pub blob: Blob,
    pub bbox: BBox,
    pub on: On,
    pub net: u32,
    /// Where a "still to connect" line can end.
    pub anchors: Vec<P>,
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
}

pub fn collect(board: &Board) -> Vec<Cu> {
    let mut items = Vec::new();
    for f in &board.footprints {
        for (i, pad) in f.pads.iter().enumerate() {
            if pad.bare_hole() {
                continue;
            }
            let blob = f.pad_blob(i, 0.0);
            let on = if pad.through() { On::All } else { On::One(f.copper()) };
            items.push(Cu {
                item: Item::Pad(f.id, i as u16),
                bbox: blob.bbox(),
                blob,
                on,
                net: pad.net,
                anchors: vec![f.pad_center(i)],
            });
        }
    }
    for t in &board.tracks {
        let blob = t.blob();
        items.push(Cu { item: Item::Track(t.id), bbox: blob.bbox(), blob, on: On::One(t.layer), net: t.net, anchors: vec![t.a, t.b] });
    }
    for v in &board.vias {
        let blob = v.blob();
        items.push(Cu { item: Item::Via(v.id), bbox: blob.bbox(), blob, on: On::All, net: v.net, anchors: vec![v.at] });
    }
    items
}

/// Pairs of items whose boxes come within `margin` of each other.
pub fn near_pairs(items: &[Cu], margin: f64) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|a, b| items[*a].bbox.min.x.total_cmp(&items[*b].bbox.min.x));
    let mut pairs = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        let grown = items[i].bbox.grown(margin);
        for &j in &order[k + 1..] {
            if items[j].bbox.min.x > grown.max.x {
                break;
            }
            if grown.touches(&items[j].bbox) {
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

pub fn analyse(board: &Board) -> Copper {
    let items = collect(board);
    let mut parent: Vec<usize> = (0..items.len()).collect();
    for (i, j) in near_pairs(&items, 0.0) {
        let (a, b) = (&items[i], &items[j]);
        if a.net == 0 || a.net != b.net || !a.on.shares(b.on) {
            continue;
        }
        if a.blob.gap(&b.blob).0 <= 1e-6 {
            let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
            parent[ri] = rj;
        }
    }
    let group: Vec<usize> = (0..items.len()).map(|i| find(&mut parent, i)).collect();

    // Each net's groups, joined up by the shortest lines that link them all.
    let mut by_net: HashMap<u32, HashMap<usize, Vec<P>>> = HashMap::new();
    for (i, cu) in items.iter().enumerate() {
        if cu.net != 0 {
            by_net.entry(cu.net).or_default().entry(group[i]).or_default().extend(cu.anchors.iter().copied());
        }
    }
    let mut rats = Vec::new();
    let mut nets: Vec<_> = by_net.into_iter().collect();
    nets.sort_by_key(|(n, _)| *n);
    for (net, groups) in nets {
        let groups: Vec<Vec<P>> = groups.into_values().collect();
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
    Copper { items, group, rats, index }
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
            .map(|c| (c.blob.dist_to(q), c))
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
        });
        id
    }

    #[test]
    fn a_track_joins_two_pads_and_the_line_goes() {
        let mut b = Board::default();
        let n = b.net("SIG");
        dot(&mut b, p(0.0, 0.0), n);
        dot(&mut b, p(10.0, 0.0), n);
        dot(&mut b, p(5.0, 8.0), n);
        assert_eq!(analyse(&b).rats.len(), 2);
        b.tracks.push(Track { id: 99, a: p(0.0, 0.0), b: p(10.0, 0.0), w: 0.25, layer: TOP, net: n });
        let c = analyse(&b);
        assert_eq!(c.rats.len(), 1);
        // A track on the bottom doesn't reach surface pads on top.
        b.tracks[0].layer = BOTTOM;
        // A stray track is a piece of its own, so there's one more line.
        assert_eq!(analyse(&b).rats.len(), 3);
    }
}
