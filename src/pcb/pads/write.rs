//! Writing a board as a PADS ASCII file (see `pads` for the format).
//!
//! Version 9.5 in BASIC units, as PowerPCB and PADS Layout write it. Connectivity in this
//! format lives in the routes: each net is a set of pin-to-pin connections, each a run of points
//! (corners, vias) from one pin to another. A net's copper here is any shape of tracks and vias, so
//! it is cut into runs that end on pins; where a run ends in the open, an unrouted leg carries it to
//! the nearest pin, as PADS does for a connection that is only partly routed. Copper on a net with
//! fewer than two pins has no pins to run between, and isn't written.
//!
//! Checked by reading the output back with `read`. Whether PADS itself accepts it hasn't been tried.

use crate::pcb::geom::*;
use crate::pcb::model::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

/// What was written, and what it couldn't keep.
pub struct Written {
    pub text: String,
    pub notes: Vec<String>,
}

/// PADS's non-copper layers, in the numbering the layer table written here gives them.
const MASK_TOP: i32 = 21;
const PASTE_BOTTOM: i32 = 22;
const PASTE_TOP: i32 = 23;
const DRILL_DRAWING: i32 = 24;
const SILK_TOP: i32 = 26;
const ASSY_TOP: i32 = 27;
const MASK_BOTTOM: i32 = 28;
const SILK_BOTTOM: i32 = 29;
const ASSY_BOTTOM: i32 = 30;

/// A length in BASIC units: a mil is 38100 of them.
fn bu(mm: f64) -> i64 {
    (mm * 1_500_000.0).round() as i64
}

/// A name PADS can have as one word.
fn word(s: &str, fallback: &str) -> String {
    let w: String = s.chars().map(|c| if c.is_whitespace() || c.is_control() { '_' } else { c }).collect();
    if w.is_empty() { fallback.to_string() } else { w }
}

type Key = (i64, i64);

fn key(q: P) -> Key {
    ((q.x * 1e6).round() as i64, (q.y * 1e6).round() as i64)
}

/// A place on a layer where copper of a net starts, ends or meets.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Node {
    at: Key,
    layer: u8,
}

#[derive(Clone, Copy, Debug)]
enum Part {
    Track(f64),
    Via,
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    a: Node,
    b: Node,
    part: Part,
}

/// What a pad's rows say, for the stack lines.
fn shape_fields(pad: &Pad) -> (f64, String) {
    let (sx, sy) = (pad.size.x, pad.size.y);
    let corner = |k: f64, side: f64| bu(k * side);
    let square = (sx - sy).abs() < 1e-6;
    match pad.shape {
        PadShape::Circle => (sx, "R".into()),
        PadShape::Rect if square => (sx, "S 0".into()),
        PadShape::RoundRect(k) if square => (sx, format!("S {}", corner(k, sx))),
        PadShape::Oval if square => (sx, "R".into()),
        shape => {
            // A finger: SIZE is its width, then its orientation, length and offset (and a corner, if it has one).
            let (mut ori, mut len, mut width) = (pad.rot, sx, sy);
            if sx < sy {
                ori += 90.0;
                len = sy;
                width = sx;
            }
            let ori = ori.rem_euclid(360.0);
            match shape {
                PadShape::Oval => (width, format!("OF {ori:.3} {} 0", bu(len))),
                PadShape::RoundRect(k) => (width, format!("RF {ori:.3} {} 0 {}", bu(len), corner(k, width))),
                _ => (width, format!("RF {ori:.3} {} 0 0", bu(len))),
            }
        }
    }
}

/// The rows of a pad's stack.
fn stack_rows(pad: &Pad) -> Vec<String> {
    let (size, shape) = shape_fields(pad);
    if pad.drill > 0.0 && pad.bare_hole() {
        // A hole with no copper: no size, not plated.
        return vec![format!("-2 0 R {} N", bu(pad.drill)), "-1 0 R".into(), "0 0 R".into()];
    }
    let drill = if pad.drill > 0.0 { format!(" {} P", bu(pad.drill)) } else { String::new() };
    let mut rows = vec![format!("-2 {} {shape}{drill}", bu(size))];
    if pad.drill > 0.0 {
        rows.push(format!("-1 {} {shape}", bu(size)));
        rows.push(format!("0 {} {shape}", bu(size)));
    } else {
        rows.push("-1 0 S 0".into());
        rows.push("0 0 S 0".into());
    }
    rows
}

fn restrict_letters(r: &Restrict) -> String {
    let mut s = String::new();
    if r.tracks {
        s.push('R');
    }
    if r.pours {
        s.push('C');
    }
    if r.vias {
        s.push('V');
    }
    if s.is_empty() { "R".into() } else { s }
}

struct Ctx<'a> {
    b: &'a Board,
    n: usize,
    shift: P,
}

impl Ctx<'_> {
    fn level(&self, l: u8) -> i32 {
        match l {
            TOP => 1,
            BOTTOM => self.n as i32,
            k => k as i32 + 1,
        }
    }
    fn on_board(&self, l: u8) -> bool {
        l == TOP || l == BOTTOM || (l >= 1 && (l as usize) < self.n - 1)
    }
    fn x(&self, q: P) -> (i64, i64) {
        (bu(q.x + self.shift.x), bu(q.y + self.shift.y))
    }
    /// A drawing layer: silkscreen, assembly (the courtyard goes there too), or none.
    fn doc(&self, l: Layer) -> Option<i32> {
        match l {
            Layer::Silk(Side::Top) => Some(SILK_TOP),
            Layer::Silk(Side::Bottom) => Some(SILK_BOTTOM),
            Layer::Fab(Side::Top) | Layer::Court(Side::Top) => Some(ASSY_TOP),
            Layer::Fab(Side::Bottom) | Layer::Court(Side::Bottom) => Some(ASSY_BOTTOM),
            _ => None,
        }
    }
}

/// One footprint's decal, as text, and the key that says which footprints share it.
struct Decal {
    name: String,
    text: String,
}

fn decal_text(cx: &Ctx, f: &Footprint, name: &str, drops: &mut usize) -> String {
    let mut pieces = String::new();
    let mut count = 0;
    for g in &f.graphics {
        let Some(level) = cx.doc(g.layer()) else {
            *drops += 1;
            continue;
        };
        match g {
            Graphic::Line { a, b, w, .. } => {
                let _ = writeln!(pieces, "OPEN 2 {} 0 {level}\n{} {}\n{} {}", bu(*w), bu(a.x), bu(a.y), bu(b.x), bu(b.y));
                count += 1;
            }
            Graphic::Circle { c, r, w, .. } => {
                let _ = writeln!(pieces, "CIRCLE 2 {} 0 {level}\n{} {}\n{} {}", bu(*w), bu(c.x - r), bu(c.y), bu(c.x + r), bu(c.y));
                count += 1;
            }
        }
    }
    for k in &f.keepouts {
        let level = match k.layer {
            None => 0,
            Some(l) => cx.level(l),
        };
        let _ = writeln!(pieces, "KPTCLS {} 0 0 {level} {}", k.pts.len() + 1, restrict_letters(&k.restrict));
        for q in k.pts.iter().chain(k.pts.first()) {
            let _ = writeln!(pieces, "{} {}", bu(q.x), bu(q.y));
        }
        count += 1;
    }
    // The pads, as terminals and as the stacks they use: stack 0 is the first pad's, and a pad that differs has its own.
    let rows: Vec<Vec<String>> = f.pads.iter().map(stack_rows).collect();
    let base = rows.first().cloned().unwrap_or_default();
    let own: Vec<usize> = (0..f.pads.len()).filter(|k| rows[*k] != base).collect();
    let height = f.label_height.unwrap_or(cx.b.rules.text_height).max(0.2);
    let stroke = (height / 8.0).max(0.05);
    let mut s = format!("{name} I 0 0 {count} {} {} 0 2\n{pieces}", f.pads.len(), 1 + own.len());
    let _ = writeln!(s, "VALUE {} {} 0.000 {SILK_TOP} {} {} N CENTER CENTER\nRegular <Romansim Stroke Font>\nRef.Des.", bu(f.label_at.x), bu(f.label_at.y), bu(height), bu(stroke));
    let _ = writeln!(s, "NONE 0 0 0.000 {ASSY_TOP} {} {} N CENTER CENTER\nRegular <Romansim Stroke Font>\nPart Type", bu(height), bu(stroke));
    for (k, pad) in f.pads.iter().enumerate() {
        let _ = writeln!(s, "T{} {} {} {} {}", bu(pad.at.x), bu(pad.at.y), bu(pad.at.x), bu(pad.at.y), word(&pad.num, &(k + 1).to_string()));
    }
    let _ = writeln!(s, "PAD 0 {}", base.len());
    for r in &base {
        let _ = writeln!(s, "{r}");
    }
    for k in own {
        let _ = writeln!(s, "PAD {} {}", k + 1, rows[k].len());
        for r in &rows[k] {
            let _ = writeln!(s, "{r}");
        }
    }
    s
}

/// The shapes of a footprint with its nets taken off, to tell whether two are the same decal.
fn signature(f: &Footprint) -> String {
    let mut pads = f.pads.clone();
    for p in &mut pads {
        p.net = 0;
    }
    format!("{pads:?}{:?}{:?}{:?}{:?}", f.graphics, f.keepouts.iter().map(|k| (&k.pts, k.layer)).collect::<Vec<_>>(), f.label_at, f.label_height)
}

struct PinRef {
    token: String,
    centers: Vec<P>,
    /// The layers each of its pads has copper on.
    layers: Vec<Vec<u8>>,
    /// Which pads: footprint and pad.
    pads: Vec<(usize, usize)>,
}

fn nearest(pins: &[PinRef], q: P, not: Option<usize>) -> usize {
    let d = |p: &PinRef| p.centers.iter().map(|x| x.dist(q)).fold(f64::MAX, f64::min);
    pins.iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != not)
        .min_by(|a, c| d(a.1).partial_cmp(&d(c.1)).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// A point of a route. Its layer and width are those of the segment that leaves it.
struct Pt {
    at: P,
    layer: i32,
    width: f64,
    via: Option<String>,
    corner: bool,
    /// A via whose outgoing layer the next segment will say.
    open: bool,
}

fn put(out: &mut String, cx: &Ctx, q: &Pt) {
    let (x, y) = cx.x(q.at);
    let _ = write!(out, "{x} {y} {} {} {}", q.layer, bu(q.width), if q.corner { 1536 } else { 1792 });
    if let Some(v) = &q.via {
        let _ = write!(out, " {v}");
    }
    if !q.corner {
        let _ = write!(out, " THERMAL");
    }
    out.push('\n');
}

fn find(j: &mut [usize], i: usize) -> usize {
    let mut r = i;
    while j[r] != r {
        r = j[r];
    }
    let mut c = i;
    while j[c] != r {
        let n = j[c];
        j[c] = r;
        c = n;
    }
    r
}

/// One net's routes, written to `out`. Returns the tracks and vias that couldn't be (a net of fewer than two pins).
fn write_net(cx: &Ctx, b: &Board, net: u32, via_name: &dyn Fn(&Via) -> String, out: &mut String) -> (usize, usize) {
    // The net's pins, each possibly several pads.
    let mut pins: Vec<PinRef> = Vec::new();
    for (fi, f) in b.footprints.iter().enumerate() {
        for (pi, pad) in f.pads.iter().enumerate() {
            if pad.net != net {
                continue;
            }
            let token = format!("{}.{}", word(&f.refdes, "X"), word(&pad.num, &(pi + 1).to_string()));
            let layers: Vec<u8> = if pad.through() {
                (0..=BOTTOM).filter(|l| cx.on_board(*l)).collect()
            } else if f.side == Side::Bottom {
                vec![BOTTOM]
            } else {
                vec![TOP]
            };
            match pins.iter_mut().find(|p| p.token == token) {
                Some(p) => {
                    p.centers.push(f.pad_center(pi));
                    p.layers.push(layers);
                    p.pads.push((fi, pi));
                }
                None => pins.push(PinRef { token, centers: vec![f.pad_center(pi)], layers: vec![layers], pads: vec![(fi, pi)] }),
            }
        }
    }
    let tracks: Vec<&Track> = b.tracks.iter().filter(|t| t.net == net && cx.on_board(t.layer)).collect();
    let vias: Vec<&Via> = b.vias.iter().filter(|v| v.net == net).collect();
    if pins.len() < 2 {
        return (tracks.len(), vias.len());
    }
    let unkey = |k: Key| p(k.0 as f64 / 1e6, k.1 as f64 / 1e6);

    // The copper graph: tracks (an arc as short straight ones), then the vias that join the layers they meet.
    let mut edges: Vec<Edge> = Vec::new();
    for t in &tracks {
        let pts = match t.mid {
            Some(m) => crate::pcb::kicad::pcb::arc_points(t.a, m, t.b),
            None => vec![t.a, t.b],
        };
        for w in pts.windows(2) {
            if key(w[0]) != key(w[1]) {
                edges.push(Edge { a: Node { at: key(w[0]), layer: t.layer }, b: Node { at: key(w[1]), layer: t.layer }, part: Part::Track(t.w) });
            }
        }
    }
    // A track end that touches a pad is joined to the pad's centre, where PADS wants a connection to end.
    let mut pin_nodes: HashMap<Node, usize> = HashMap::new();
    let mut seen_end: HashSet<Node> = HashSet::new();
    let ends: Vec<(Node, f64)> = edges.iter().flat_map(|e| if let Part::Track(w) = e.part { vec![(e.a, w), (e.b, w)] } else { Vec::new() }).collect();
    for (node, w) in ends {
        if !seen_end.insert(node) {
            continue;
        }
        let q = unkey(node.at);
        'pins: for (i, pin) in pins.iter().enumerate() {
            for (k, &(fi, pi)) in pin.pads.iter().enumerate() {
                if !pin.layers[k].contains(&node.layer) {
                    continue;
                }
                if b.footprints[fi].pad_blob(pi, 0.0).dist_to(q) <= 0.005 {
                    let target = Node { at: key(pin.centers[k]), layer: node.layer };
                    if target != node {
                        edges.push(Edge { a: node, b: target, part: Part::Track(w) });
                    }
                    pin_nodes.insert(target, i);
                    break 'pins;
                }
            }
        }
    }
    let mut layers_at: BTreeMap<Key, Vec<u8>> = BTreeMap::new();
    for e in &edges {
        for n in [e.a, e.b] {
            let v = layers_at.entry(n.at).or_default();
            if !v.contains(&n.layer) {
                v.push(n.layer);
            }
        }
    }
    // A via joins the layers that meet at it; one with nothing to join hangs off its layer.
    let mut via_at: HashMap<Key, String> = HashMap::new();
    for v in &vias {
        let k = key(v.at);
        via_at.insert(k, via_name(v));
        let mut ls = layers_at.get(&k).cloned().unwrap_or_default();
        ls.sort();
        if ls.len() < 2 {
            let l = ls.first().copied().unwrap_or(TOP);
            edges.push(Edge { a: Node { at: k, layer: l }, b: Node { at: k, layer: u8::MAX }, part: Part::Via });
        } else {
            for w in ls.windows(2) {
                edges.push(Edge { a: Node { at: k, layer: w[0] }, b: Node { at: k, layer: w[1] }, part: Part::Via });
            }
        }
    }

    // Cut the copper into runs that end on pins, or where the copper does: each edge in one run.
    let mut adj: HashMap<Node, Vec<usize>> = HashMap::new();
    for (i, e) in edges.iter().enumerate() {
        adj.entry(e.a).or_default().push(i);
        adj.entry(e.b).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut runs: Vec<(Vec<Node>, Vec<usize>)> = Vec::new();
    for start in 0..edges.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (mut nodes, mut run) = (vec![edges[start].a, edges[start].b], vec![start]);
        for forward in [true, false] {
            loop {
                let end = if forward { nodes[nodes.len() - 1] } else { nodes[0] };
                if pin_nodes.contains_key(&end) {
                    break;
                }
                let Some(&next) = adj.get(&end).and_then(|l| l.iter().find(|i| !used[**i])) else { break };
                used[next] = true;
                let e = edges[next];
                let other = if e.a == end { e.b } else { e.a };
                if forward {
                    nodes.push(other);
                    run.push(next);
                } else {
                    nodes.insert(0, other);
                    run.insert(0, next);
                }
            }
        }
        runs.push((nodes, run));
    }

    // Each run is a connection between two pins.
    let width_default = b.rules.track;
    let mut joined: Vec<usize> = (0..pins.len()).collect();
    for (nodes, run) in &runs {
        let first = nodes[0];
        let last = nodes[nodes.len() - 1];
        // The pins at the ends. An end in the open is carried to the nearest pin that isn't the other end's.
        let (start_pin, end_pin) = (pin_nodes.get(&first).copied(), pin_nodes.get(&last).copied());
        let from = start_pin.unwrap_or_else(|| nearest(&pins, unkey(first.at), end_pin));
        // A run that comes back to the pin it left (a loop) is carried on from there to another pin, as one in the open is.
        let end_open = end_pin.is_none() || end_pin == Some(from);
        let to = if end_open { nearest(&pins, unkey(last.at), Some(from)) } else { end_pin.unwrap_or(from) };
        if from == to {
            continue;
        }
        let mut pts: Vec<Pt> = Vec::new();
        let mut width = width_default;
        // A run that starts in the open is reached from its pin by an unrouted leg.
        if !pin_nodes.contains_key(&first) {
            pts.push(Pt { at: pins[from].centers[0], layer: 0, width, via: None, corner: false, open: false });
        }
        for (i, &ei) in run.iter().enumerate() {
            let u = nodes[i];
            match edges[ei].part {
                Part::Track(w) => {
                    width = w;
                    let level = cx.level(u.layer);
                    match pts.last_mut() {
                        // The via before it takes the layer this segment is on.
                        Some(q) if q.open => {
                            q.layer = level;
                            q.width = w;
                            q.open = false;
                        }
                        _ => pts.push(Pt { at: unkey(u.at), layer: level, width: w, via: None, corner: !pin_nodes.contains_key(&u), open: false }),
                    }
                }
                Part::Via => pts.push(Pt { at: unkey(u.at), layer: 0, width, via: via_at.get(&u.at).cloned(), corner: false, open: true }),
            }
        }
        // The end: on a pin, it closes the connection; in the open, an unrouted leg goes on to a pin.
        let end_at = unkey(last.at);
        match pts.last_mut() {
            Some(q) if q.open => {
                q.open = false;
                pts.push(Pt { at: pins[to].centers[0], layer: 65, width, via: None, corner: false, open: false });
            }
            _ if !end_open => pts.push(Pt { at: end_at, layer: 65, width, via: None, corner: false, open: false }),
            _ => {
                pts.push(Pt { at: end_at, layer: 0, width, via: None, corner: false, open: false });
                pts.push(Pt { at: pins[to].centers[0], layer: 65, width, via: None, corner: false, open: false });
            }
        }
        let _ = writeln!(out, "{:<30} {}", pins[from].token, pins[to].token);
        for q in &pts {
            put(out, cx, q);
        }
        let (a, c) = (find(&mut joined, from), find(&mut joined, to));
        joined[a] = c;
    }
    // Pins no run joined are joined by unrouted connections, nearest first.
    loop {
        let root = find(&mut joined, 0);
        let Some(other) = (0..pins.len()).find(|i| find(&mut joined, *i) != root) else { break };
        let mut best = 0usize;
        let mut best_d = f64::MAX;
        for i in 0..pins.len() {
            if find(&mut joined, i) != root {
                continue;
            }
            let d = pins[i].centers[0].dist(pins[other].centers[0]);
            if d < best_d {
                (best, best_d) = (i, d);
            }
        }
        let _ = writeln!(out, "{:<30} {}", pins[best].token, pins[other].token);
        put(out, cx, &Pt { at: pins[best].centers[0], layer: 0, width: width_default, via: None, corner: false, open: false });
        put(out, cx, &Pt { at: pins[other].centers[0], layer: 65, width: width_default, via: None, corner: false, open: false });
        let (a, c) = (find(&mut joined, best), find(&mut joined, other));
        joined[a] = c;
    }
    (0, 0)
}

/// A board as a PADS ASCII file.
pub fn board(b: &Board) -> Result<Written, String> {
    let n = b.copper as usize;
    if n > 20 {
        return Err(format!("this board has {n} copper layers; PADS files here hold up to 20"));
    }
    let mut notes: Vec<String> = Vec::new();
    // PADS keeps its work in the positive corner; a board that reaches into the negative is moved.
    let mut all: Vec<P> = b.outline.clone();
    all.extend(b.footprints.iter().map(|f| f.at));
    all.extend(b.tracks.iter().flat_map(|t| [t.a, t.b]));
    all.extend(b.vias.iter().map(|v| v.at));
    let min = if all.is_empty() { p(0.0, 0.0) } else { BBox::of(&all).min };
    let shift = p(if min.x < 0.0 { 1.0 - min.x } else { 0.0 }, if min.y < 0.0 { 1.0 - min.y } else { 0.0 });
    if shift.x > 0.0 || shift.y > 0.0 {
        notes.push(format!("The board was moved by ({:.2}, {:.2}) mm to sit in the positive corner, as PADS keeps it.", shift.x, shift.y));
    }
    let cx = Ctx { b, n, shift };

    // Part types and decals.
    let mut decals: Vec<Decal> = Vec::new();
    let mut by_signature: HashMap<String, usize> = HashMap::new();
    let mut names_used: HashSet<String> = HashSet::new();
    let mut decal_of_part: Vec<usize> = Vec::new();
    let mut dropped_drawing = 0usize;
    for f in &b.footprints {
        let sig = format!("{}|{}", f.def, signature(f));
        let idx = match by_signature.get(&sig) {
            Some(i) => *i,
            None => {
                let base = word(&f.def, "DECAL");
                let mut name = base.clone();
                let mut k = 2;
                while !names_used.insert(name.clone()) {
                    name = format!("{base}_{k}");
                    k += 1;
                }
                let text = decal_text(&cx, f, &name, &mut dropped_drawing);
                decals.push(Decal { name, text });
                by_signature.insert(sig, decals.len() - 1);
                decals.len() - 1
            }
        };
        decal_of_part.push(idx);
    }
    // A part type for each decal and value: a value is the part type's name after the decal's.
    let mut types: Vec<(String, usize)> = Vec::new();
    let mut type_of_part: Vec<usize> = Vec::new();
    for (i, f) in b.footprints.iter().enumerate() {
        let d = &decals[decal_of_part[i]];
        let name = if f.value.is_empty() { d.name.clone() } else { format!("{}_{}", d.name, word(&f.value, "")) };
        let at = match types.iter().position(|(n, _)| *n == name) {
            Some(k) => k,
            None => {
                types.push((name, decal_of_part[i]));
                types.len() - 1
            }
        };
        type_of_part.push(at);
    }

    // Vias: a definition for each size.
    let mut via_defs: Vec<(String, f64, f64)> = Vec::new();
    let via_name_of = |dia: f64, drill: f64, defs: &mut Vec<(String, f64, f64)>| -> String {
        let name = format!("VIA_{}_{}", (dia * 1000.0).round() as i64, (drill * 1000.0).round() as i64);
        if !defs.iter().any(|d| d.0 == name) {
            defs.push((name.clone(), dia, drill));
        }
        name
    };
    let default_via = via_name_of(b.rules.via_dia, b.rules.via_drill, &mut via_defs);
    for v in &b.vias {
        via_name_of(v.dia, v.drill, &mut via_defs);
    }

    let mut o = String::new();
    o.push_str("!PADS-POWERPCB-V9.5-BASIC! DESIGN DATABASE ASCII FILE 1.0\n");
    let _ = writeln!(o, "*PCB*        GENERAL PARAMETERS OF THE PCB DESIGN\n\nUNITS        0\nMAXIMUMLAYER {n}\nORIGIN       0 0\nVIAPSHVIA    {default_via}\n");
    o.push_str("*TEXT*       FREE TEXT\n\n");

    // Lines: the board's edge, free copper and keepouts.
    o.push_str("*LINES*      LINES ITEMS\n\n");
    let mut item = 0;
    let (ox, oy) = (shift.x, shift.y);
    let ring = |pts: &[P]| -> Vec<(i64, i64)> { pts.iter().chain(pts.first()).map(|q| (bu(q.x + ox), bu(q.y + oy))).collect() };
    item += 1;
    let edge = ring(&b.outline);
    let _ = writeln!(o, "OUTLINE{item}      BOARD    0 0 1 0\nCLOSED {} 152400 0 0", edge.len());
    for (x, y) in &edge {
        let _ = writeln!(o, "{x} {y}");
    }
    for f in &b.fills {
        if !cx.on_board(f.layer) || f.pts.len() < 3 {
            continue;
        }
        item += 1;
        let pts = ring(&f.pts);
        let net = if f.net == 0 { "0".to_string() } else { word(b.net_name(f.net), "0") };
        let _ = writeln!(o, "COPPER{item}      COPPER   0 0 1 0 0 {net}\nCOPCLS {} 152400 0 {}", pts.len(), cx.level(f.layer));
        for (x, y) in &pts {
            let _ = writeln!(o, "{x} {y}");
        }
    }
    for k in &b.keepouts {
        let level = match k.layer {
            None => 0,
            Some(l) if cx.on_board(l) => cx.level(l),
            Some(_) => continue,
        };
        item += 1;
        let pts = ring(&k.pts);
        let _ = writeln!(o, "KEEPOUT{item}      KEEPOUT  0 0 1 0\nKPTCLS {} 0 0 {level} {}", pts.len(), restrict_letters(&k.restrict));
        for (x, y) in &pts {
            let _ = writeln!(o, "{x} {y}");
        }
    }
    o.push('\n');

    o.push_str("*VIA*        VIA DATA\n\n");
    for (name, dia, drill) in &via_defs {
        let _ = writeln!(o, "{name} {} 3\n-2 {} R\n-1 {} R\n0 {} R", bu(*drill), bu(*dia), bu(*dia), bu(*dia));
    }
    o.push('\n');

    o.push_str("*PARTDECAL*  ITEMS\n\n");
    for d in &decals {
        o.push_str(&d.text);
        o.push('\n');
    }
    o.push_str("*PARTTYPE*   ITEMS\n\n");
    for (name, di) in &types {
        let d = &decals[*di];
        // Its pins, from the first part that uses it.
        let first = type_of_part.iter().position(|t| types[*t].0 == *name).unwrap_or(0);
        let f = &b.footprints[first];
        let pins: Vec<String> = f.pads.iter().enumerate().map(|(k, pd)| format!("{}.0.U.", word(&pd.num, &(k + 1).to_string()))).collect();
        let _ = writeln!(o, "{name} {} UND {} 0 0 0 Y", d.name, if pins.is_empty() { 0 } else { 1 });
        if !pins.is_empty() {
            let _ = writeln!(o, "G 0 {}", pins.len());
            for chunk in pins.chunks(6) {
                let _ = writeln!(o, "{}", chunk.join(" "));
            }
        }
        o.push('\n');
    }
    o.push_str("*PART*       ITEMS\n\n");
    for (i, f) in b.footprints.iter().enumerate() {
        let (x, y) = cx.x(f.at);
        let ori = if f.side == Side::Bottom { -f.rot } else { f.rot }.rem_euclid(360.0);
        let _ = writeln!(o, "{} {} {x} {y} {ori:.3} {} {} 0 -1 0 -1 0", word(&f.refdes, "X"), types[type_of_part[i]].0, if f.locked { "G" } else { "U" }, if f.side == Side::Bottom { "M" } else { "N" });
    }
    o.push('\n');

    // Routes: every net, routed or not.
    o.push_str("*ROUTE*      ROUTES\n\n");
    let via_name = |v: &Via| format!("VIA_{}_{}", (v.dia * 1000.0).round() as i64, (v.drill * 1000.0).round() as i64);
    let (mut dropped_tracks, mut dropped_vias) = (0usize, 0usize);
    for net in &b.nets {
        let mut body = String::new();
        let (dt, dv) = write_net(&cx, b, net.id, &via_name, &mut body);
        dropped_tracks += dt;
        dropped_vias += dv;
        if body.is_empty() {
            continue;
        }
        let _ = writeln!(o, "*SIGNAL* {} 0 -2", word(&net.name, "NET"));
        o.push_str(&body);
    }
    o.push('\n');

    // Pours: their outlines (PADS pours them itself).
    let live: Vec<&Pour> = b.pours.iter().filter(|p| cx.on_board(p.layer)).collect();
    if !live.is_empty() {
        o.push_str("*POUR*       POUR DATA\n\n");
        for (k, pour) in live.iter().enumerate() {
            let outline = if pour.outline.len() >= 3 { &pour.outline } else { &b.outline };
            let pts = ring(outline);
            let net = if pour.net == 0 { "0".to_string() } else { word(b.net_name(pour.net), "0") };
            let name = format!("POR{}", k + 1);
            let _ = writeln!(o, "{name} POUROUT 0 0 1 0 {name} {net} -1 0.000000 {}\nPOLY {} 0 152400 {}", pour.order.max(1), pts.len(), cx.level(pour.layer));
            for (x, y) in &pts {
                let _ = writeln!(o, "{x} {y}");
            }
        }
        o.push('\n');
    }

    // Rules and layers.
    o.push_str("*MISC*      MISCELLANEOUS PARAMETERS\n");
    let r = &b.rules;
    let gap = |a: Kind, c: Kind| bu(r.pair(a, c).unwrap_or(r.clearance));
    o.push_str("RULES_SECTION PARENT\n{\nDESIGN RULES\n{\nRULE_SET (1)\n{\nFOR :\n{\nDEFAULT : \n}\nAGAINST :\n{\nDEFAULT : \n}\nLAYER 0\nCLEARANCE_RULE :\n{\n");
    for (key, a, c) in [
        ("TRACK_TO_TRACK", Kind::Track, Kind::Track),
        ("VIA_TO_TRACK", Kind::Via, Kind::Track),
        ("VIA_TO_VIA", Kind::Via, Kind::Via),
        ("PAD_TO_TRACK", Kind::ThPad, Kind::Track),
        ("PAD_TO_VIA", Kind::ThPad, Kind::Via),
        ("PAD_TO_PAD", Kind::ThPad, Kind::ThPad),
        ("SMD_TO_TRACK", Kind::SmdPad, Kind::Track),
        ("SMD_TO_VIA", Kind::SmdPad, Kind::Via),
        ("SMD_TO_PAD", Kind::SmdPad, Kind::ThPad),
        ("SMD_TO_SMD", Kind::SmdPad, Kind::SmdPad),
        ("COPPER_TO_TRACK", Kind::Pour, Kind::Track),
        ("COPPER_TO_VIA", Kind::Pour, Kind::Via),
        ("COPPER_TO_PAD", Kind::Pour, Kind::ThPad),
        ("COPPER_TO_COPPER", Kind::Pour, Kind::Pour),
    ] {
        let _ = writeln!(o, "{key} {}", gap(a, c));
    }
    let edge_gap = if r.edge_clearance > 0.0 { bu(r.edge_clearance) } else { bu(r.clearance) };
    for key in ["OUTLINE_TO_TRACK", "OUTLINE_TO_VIA", "OUTLINE_TO_PAD", "OUTLINE_TO_COPPER"] {
        let _ = writeln!(o, "{key} {edge_gap}");
    }
    let _ = writeln!(o, "MIN_TRACK_WIDTH {}\nMAX_TRACK_WIDTH {}\nREC_TRACK_WIDTH {}\n}}\n}}\n}}\n}}", bu(r.min_track), bu(if r.max_track > 0.0 { r.max_track } else { 100.0 }), bu(r.track));
    o.push_str("LAYER DATA\n{\n");
    let layer = |o: &mut String, num: usize, name: &str, kind: &str| {
        let _ = writeln!(o, "LAYER {num}\n{{\nLAYER_NAME {name}\nLAYER_TYPE {kind}\nPLANE NONE\n}}");
    };
    for l in 1..=n {
        let name = if l == 1 { "Top".to_string() } else if l == n { "Bottom".to_string() } else { format!("Inner Layer {}", l - 1) };
        layer(&mut o, l, &name, "ROUTING");
    }
    for l in n + 1..=20 {
        layer(&mut o, l, &format!("Layer_{l}"), "UNASSIGNED");
    }
    for (num, name, kind) in [
        (MASK_TOP, "Solder Mask Top", "SOLDER_MASK"),
        (PASTE_BOTTOM, "Paste Mask Bottom", "PASTE_MASK"),
        (PASTE_TOP, "Paste Mask Top", "PASTE_MASK"),
        (DRILL_DRAWING, "Drill Drawing", "DRILL"),
        (25, "Layer_25", "UNASSIGNED"),
        (SILK_TOP, "Silkscreen Top", "SILK_SCREEN"),
        (ASSY_TOP, "Assembly Drawing Top", "ASSEMBLY"),
        (MASK_BOTTOM, "Solder Mask Bottom", "SOLDER_MASK"),
        (SILK_BOTTOM, "Silkscreen Bottom", "SILK_SCREEN"),
        (ASSY_BOTTOM, "Assembly Drawing Bottom", "ASSEMBLY"),
    ] {
        layer(&mut o, num as usize, name, kind);
    }
    o.push_str("}\n*END*     OF ASCII OUTPUT FILE\n");

    // What didn't come across.
    let s = |k: usize| if k == 1 { "" } else { "s" };
    if !live.is_empty() {
        notes.push(format!("{} copper pour{} written as outlines only, with no copper poured: pour them again in PADS.", live.len(), s(live.len())));
    }
    if dropped_tracks + dropped_vias > 0 {
        notes.push(format!("{dropped_tracks} track{} and {dropped_vias} via{} on nets with fewer than two pins weren't written: PADS keeps copper only as connections between pins.", s(dropped_tracks), s(dropped_vias)));
    }
    if b.tracks.iter().any(|t| t.mid.is_some()) {
        notes.push("Arc tracks are written as short straight ones.".into());
    }
    if b.pours.iter().any(|p| p.plane) {
        notes.push("Power planes are written as pours over the board, not as plane layers.".into());
    }
    notes.push("Parts keep their footprint, designator and value only: part types have one gate holding every pin, with no schematic link.".into());
    notes.push("Each footprint's name sits where its first part has it; the free text, dimensions and board drawing aren't written.".into());
    notes.push("Rules: the clearance table, track widths and board-edge gap are written; the other rules, net classes and differential pairs are not.".into());
    notes.push("A pad has one shape on every layer; its stack is written the same on the inner and far layers, with no mask or paste rows.".into());
    notes.push("The layer stack has no thicknesses or materials; set the real stack-up in PADS.".into());
    if dropped_drawing > 0 {
        notes.push(format!("{dropped_drawing} piece{} of footprint drawing on mask, paste or copper layers weren't written.", s(dropped_drawing)));
    }
    notes.push("Courtyards are written on the assembly layers.".into());
    notes.push("Names with spaces have them replaced by underscores (PADS names are one word).".into());
    Ok(Written { text: o, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcb::copper::analyse;
    use crate::pcb::drc;

    fn there_and_back(a: &Board) -> (Board, Written) {
        let w = board(a).unwrap_or_else(|e| panic!("{e}"));
        let back = crate::pcb::pads::read::read(&w.text, "x").unwrap_or_else(|e| panic!("what was written can't be read: {e}")).board;
        (back, w)
    }

    fn summary(b: &Board) -> (usize, usize, usize, usize, usize, usize) {
        let c = analyse(b);
        let shorts = drc::check(b, &c).iter().filter(|p| p.title == "Short circuit").count();
        (b.footprints.len(), b.footprints.iter().map(|f| f.pads.len()).sum(), b.nets.len(), b.vias.len(), c.rats.len(), shorts)
    }

    use crate::pcb::pads::read::spec_tests::example;

    fn near(a: P, b: P) -> bool {
        a.dist(b) < 1e-4
    }

    #[test]
    fn a_board_written_for_pads_reads_back_part_for_part() {
        let a = crate::pcb::pads::read::read(example(), "small").unwrap().board;
        let (b, w) = there_and_back(&a);
        assert!(w.text.starts_with("!PADS-POWERPCB-V9.5-BASIC!"));
        assert_eq!((b.copper, b.footprints.len(), b.tracks.len(), b.vias.len(), b.nets.len()), (a.copper, a.footprints.len(), a.tracks.len(), a.vias.len(), a.nets.len()));
        let (x, y) = (&a.footprints[0], &b.footprints[0]);
        assert_eq!((x.refdes.as_str(), x.def.as_str(), x.side, x.rot), (y.refdes.as_str(), y.def.as_str(), y.side, y.rot));
        assert!(near(x.at, y.at));
        for i in 0..x.pads.len() {
            assert!(near(x.pad_center(i), y.pad_center(i)));
            assert_eq!((x.pads[i].num.as_str(), x.pads[i].drill, x.pads[i].size), (y.pads[i].num.as_str(), y.pads[i].drill, y.pads[i].size));
            assert_eq!(a.net_name(x.pads[i].net), b.net_name(y.pads[i].net));
        }
        assert_eq!(x.graphics.len(), y.graphics.len());
        assert!((ring_area(&a.outline) - ring_area(&b.outline)).abs() < 1e-6);
        assert!((a.vias[0].dia - b.vias[0].dia).abs() < 1e-6 && (a.vias[0].drill - b.vias[0].drill).abs() < 1e-6 && near(a.vias[0].at, b.vias[0].at));
        assert!(analyse(&b).rats.is_empty());
    }

    #[test]
    fn a_part_underneath_and_turned_goes_and_comes_back() {
        let text = example().replace("R1 RES 30000000 22500000 0.000 U N", "R1 RES 30000000 22500000 270.000 U M");
        let a = crate::pcb::pads::read::read(&text, "under").unwrap().board;
        let (b, _) = there_and_back(&a);
        let (x, y) = (&a.footprints[0], &b.footprints[0]);
        assert_eq!((x.side, x.rot), (y.side, y.rot));
        assert!(near(x.pad_center(0), y.pad_center(0)) && near(x.pad_center(1), y.pad_center(1)));
    }

    #[test]
    fn pads_of_every_shape_come_back_the_same_size_and_turn() {
        let mut a = crate::pcb::pads::read::read(example(), "shapes").unwrap().board;
        let base = a.footprints[0].pads[0].clone();
        let shapes = [
            (PadShape::Circle, p(1.6, 1.6), 0.0),
            (PadShape::Rect, p(1.6, 1.6), 0.0),
            (PadShape::Rect, p(2.4, 1.0), 0.0),
            (PadShape::Rect, p(1.0, 2.4), 0.0),
            (PadShape::Rect, p(2.4, 1.0), 45.0),
            (PadShape::Oval, p(2.4, 1.0), 90.0),
            (PadShape::RoundRect(0.25), p(1.6, 1.6), 0.0),
            (PadShape::RoundRect(0.25), p(2.4, 1.0), 0.0),
        ];
        a.footprints[0].pads = shapes.iter().enumerate().map(|(i, (shape, size, rot))| Pad { num: (i + 1).to_string(), at: p(i as f64 * 3.0, 0.0), size: *size, rot: *rot, shape: *shape, drill: 0.0, net: 0 }).collect();
        let (b, _) = there_and_back(&a);
        for (x, y) in a.footprints[0].pads.iter().zip(&b.footprints[0].pads) {
            let (bx, by) = (a.footprints[0].place(x.at), b.footprints[0].place(y.at));
            assert!(near(bx, by), "{:?} {:?}", x.shape, (bx, by));
            // The same copper: its box is the same size.
            let (i, j) = (a.footprints[0].pads.iter().position(|q| q == x).unwrap(), b.footprints[0].pads.iter().position(|q| q == y).unwrap());
            let (ca, cb) = (a.footprints[0].pad_blob(i, 0.0).bbox(), b.footprints[0].pad_blob(j, 0.0).bbox());
            assert!((ca.w() - cb.w()).abs() < 1e-4 && (ca.h() - cb.h()).abs() < 1e-4, "{:?} {:?}: {:?} vs {:?}", x.shape, x.size, (ca.w(), ca.h()), (cb.w(), cb.h()));
        }
        let _ = base;
    }

    #[test]
    fn the_real_boards_come_back_with_the_same_connections() {
        for name in ["ClaySight_MK1", "MAIS_FC", "LCORE_2", "TMS1mmX19", "Ems4_Rev2", "Dexter_MotorCtrl"] {
            let Some(got) = crate::pcb::pads::read::tests::sample(name) else { continue };
            let a = got.board;
            let (b, w) = there_and_back(&a);
            assert_eq!(summary(&a), summary(&b), "{name}");
            assert!(w.notes.iter().any(|n| n.contains("one gate")), "{name}");
        }
    }

    #[test]
    fn an_altium_board_goes_to_pads_with_its_copper() {
        let path = std::path::Path::new("/Users/dal/Layouts/EZDISP1V9_LAYOUTS/EZDisplay.PcbDoc");
        if !path.exists() {
            return;
        }
        let mut a = crate::pcb::altium::read_board(path).unwrap().board;
        // Without the pours, which PADS fills itself.
        a.pours.clear();
        a.fills.clear();
        let (b, w) = there_and_back(&a);
        // (A net with fewer than two pins has no route, so it isn't in the file.)
        assert_eq!((a.footprints.len(), a.copper), (b.footprints.len(), b.copper));
        assert!(b.nets.len() <= a.nets.len() && b.nets.len() > a.nets.len() * 9 / 10, "{} of {} nets", b.nets.len(), a.nets.len());
        let (open_a, open_b) = (analyse(&a).rats.len(), analyse(&b).rats.len());
        assert_eq!(open_a, open_b, "the same connections are open");
        // Copper on nets of fewer than two pins can't be written; everything else is.
        let lost = a.vias.len() - b.vias.len();
        assert!(lost < a.vias.len() / 20, "{lost} of {} vias lost", a.vias.len());
        assert!(w.notes.iter().any(|n| n.contains("positive corner")) || a.outline.iter().all(|q| q.x >= 1.0 && q.y >= 1.0));
    }

    /// cargo test dump_round -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_round() {
        for name in ["ClaySight_MK1", "MAIS_FC", "LCORE_2", "TMS1mmX19", "Ems4_Rev2", "Dexter_MotorCtrl"] {
            let Some(got) = crate::pcb::pads::read::tests::sample(name) else { continue };
            let a = got.board;
            let t = std::time::Instant::now();
            let (b, w) = there_and_back(&a);
            println!("{name}: before {:?} tracks {}; after {:?} tracks {} ({} bytes, {:?})", summary(&a), a.tracks.len(), summary(&b), b.tracks.len(), w.text.len(), t.elapsed());
        }
    }
}
