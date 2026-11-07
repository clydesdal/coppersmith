//! Boards from an Altium `.PcbDoc` or Circuit Studio `.CSPcbDoc`.
//!
//! A board file keeps each kind of thing in its own storage: `Board6` (the
//! outline, origin and layer stack), `Nets6` and `Components6` (text
//! records), and `Pads6`, `Tracks6`, `Arcs6`, `Vias6`, `Fills6`,
//! `Regions6`… (binary records, see prims.rs). Altium names the storages;
//! Circuit Studio names them with random ids, so there each is recognised
//! by what's in it.
//!
//! A part's pads and drawing are stored placed, where they sit on the board,
//! each marked with the part it belongs to; they're taken back into the
//! part's own coordinates to make its footprint.

use super::pcblib::{Skipped, footprint, label_spot, layer_names};
use super::prims::{self, BOTTOM, MULTI_LAYER, NONE, Prim, TOP};
use super::{Reader, open, params, stream};
use crate::pcb::geom::*;
use crate::pcb::model::*;
use std::collections::HashMap;

/// What a board file turned into, and what didn't come across.
pub struct ImportedBoard {
    pub board: Board,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Board,
    Nets,
    Components,
    Polygons,
    Prims,
    Rules,
    Ignore,
}

fn kind_by_name(name: &str) -> Option<Kind> {
    Some(match name {
        "Board6" => Kind::Board,
        "Nets6" => Kind::Nets,
        "Components6" => Kind::Components,
        "Polygons6" => Kind::Polygons,
        "Rules6" => Kind::Rules,
        "Arcs6" | "Pads6" | "Vias6" | "Tracks6" | "Texts6" | "Fills6" | "Regions6" | "ShapeBasedRegions6" | "ComponentBodies6"
        | "ShapeBasedComponentBodies6" => Kind::Prims,
        _ => return None,
    })
}

/// Text records: four-byte length, then `|KEY=VALUE…`.
fn text_records(data: &[u8]) -> Vec<HashMap<String, String>> {
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    while r.left() >= 4 {
        match r.block() {
            Ok(b) => out.push(params(b)),
            Err(_) => break,
        }
    }
    out
}

/// Rule records: a two-byte kind, then a length and `|KEY=VALUE…`.
fn rule_records(data: &[u8]) -> Vec<HashMap<String, String>> {
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    while r.left() >= 6 {
        // A two-byte kind, which varies; then the record.
        if r.i16().is_err() {
            break;
        }
        match r.block() {
            Ok(b) => out.push(params(b)),
            Err(_) => break,
        }
    }
    out
}

/// How pours and planes are made, from the rules about them.
#[derive(Clone, Copy, Debug)]
struct AreaRules {
    pour_clearance: f64,
    pour_connect: Connect,
    pour_spoke: f64,
    pour_gap: f64,
    plane_clearance: f64,
    plane_connect: Connect,
    plane_spoke: f64,
    plane_gap: f64,
}

/// The board's design rules that Coppersmith has a place for. Each comes
/// from the general rule of its kind: enabled, applying to everything, the
/// lowest priority (Altium's fallback).
fn read_rules(recs: &[HashMap<String, String>], rules: &mut Rules) -> AreaRules {
    // Altium's defaults where a board has no rule.
    let mut area = AreaRules {
        pour_clearance: rules.clearance,
        pour_connect: Connect::Relief,
        pour_spoke: 0.254,
        pour_gap: 0.254,
        plane_clearance: 0.508,
        plane_connect: Connect::Relief,
        plane_spoke: 0.254,
        plane_gap: 0.254,
    };
    let general = |kind: &str| -> Option<&HashMap<String, String>> {
        recs.iter()
            .filter(|r| r.get("RULEKIND").is_some_and(|k| k.eq_ignore_ascii_case(kind)))
            .filter(|r| r.get("ENABLED").is_none_or(|e| e == "TRUE"))
            .filter(|r| {
                r.get("SCOPE1EXPRESSION").is_none_or(|s| s.trim() == "All") && r.get("SCOPE2EXPRESSION").is_none_or(|s| s.trim() == "All")
            })
            .max_by_key(|r| r.get("PRIORITY").and_then(|p| p.trim().parse::<i32>().ok()).unwrap_or(1))
    };
    let len = |r: &HashMap<String, String>, k: &str| r.get(k).map(|v| mil(Some(&v.trim_end_matches('\0').to_string())));
    if let Some(r) = general("Clearance") {
        // Newer files keep a table of gaps for each pair of kinds of thing
        // (pad to track, track to via…), in Altium units, which overrides the
        // general gap. Coppersmith has one gap, so the least in the table.
        let table = r.get("OBJECTCLEARANCES").map(|t| {
            t.trim_end_matches('\0')
                .split(';')
                .filter_map(|e| e.split_once(':'))
                .filter_map(|(_, v)| v.trim().parse::<f64>().ok())
                .map(|v| v * super::UNIT)
                .fold(f64::MAX, f64::min)
        });
        match (table, len(r, "GAP")) {
            (Some(t), _) if t < f64::MAX && t > 0.0 => rules.clearance = nm(t),
            (_, Some(v)) => rules.clearance = nm(v),
            _ => {}
        }
        // Pours keep the table's polygon gaps, or where it has none, the
        // general gap.
        let poly = r.get("OBJECTCLEARANCES").map(|t| {
            t.trim_end_matches('\0')
                .split(';')
                .filter_map(|e| e.split_once(':'))
                .filter(|(k, _)| k.contains("Poly"))
                .filter_map(|(_, v)| v.trim().parse::<f64>().ok())
                .map(|v| v * super::UNIT)
                .fold(f64::MAX, f64::min)
        });
        area.pour_clearance = match (poly, len(r, "GAP")) {
            (Some(t), _) if t < f64::MAX && t > 0.0 => nm(t),
            (_, Some(v)) => nm(v),
            _ => rules.clearance,
        };
    } else {
        area.pour_clearance = rules.clearance;
    }
    let style = |v: Option<&String>| if v.is_some_and(|s| s.trim().eq_ignore_ascii_case("Direct")) { Connect::Direct } else { Connect::Relief };
    if let Some(r) = general("PolygonConnect") {
        area.pour_connect = style(r.get("CONNECTSTYLE"));
        area.pour_spoke = len(r, "RELIEFCONDUCTORWIDTH").map(nm).unwrap_or(area.pour_spoke);
        area.pour_gap = len(r, "AIRGAPWIDTH").map(nm).unwrap_or(area.pour_gap);
    }
    if let Some(r) = general("PlaneConnect") {
        area.plane_connect = style(r.get("PLANECONNECTSTYLE"));
        area.plane_spoke = len(r, "RELIEFCONDUCTORWIDTH").map(nm).unwrap_or(area.plane_spoke);
        area.plane_gap = len(r, "RELIEFAIRGAP").map(nm).unwrap_or(area.plane_gap);
    }
    if let Some(r) = general("PlaneClearance")
        && let Some(v) = len(r, "CLEARANCE")
    {
        area.plane_clearance = nm(v);
    }
    if let Some(r) = general("Width") {
        if let Some(v) = len(r, "PREFEREDWIDTH") {
            rules.track = nm(v);
        }
        if let Some(v) = len(r, "MINLIMIT") {
            rules.min_track = nm(v);
        }
    }
    if let Some(r) = general("RoutingVias") {
        if let Some(v) = len(r, "WIDTH") {
            rules.via_dia = nm(v);
        }
        if let Some(v) = len(r, "HOLEWIDTH") {
            rules.via_drill = nm(v);
        }

    }
    if let Some(r) = general("HoleSize")
        && let Some(v) = len(r, "MINLIMIT")
    {
        rules.min_drill = nm(v);
    }
    // Altium only checks rings with a ring rule; without one, nor does this.
    rules.min_ring = match general("MinimumAnnularRing").and_then(|r| len(r, "MINIMUMRING")) {
        Some(v) => nm(v),
        None if !recs.is_empty() => 0.0,
        None => rules.min_ring,
    };
    if let Some(r) = general("SolderMaskExpansion")
        && let Some(v) = len(r, "EXPANSION")
    {
        rules.mask_margin = nm(v);
    }
    if let Some(r) = general("BoardOutlineClearance")
        && let Some(v) = len(r, "GAP")
    {
        rules.edge_clearance = nm(v);
    }
    area
}

/// A ring from `VX0`, `VY0`, `KIND0`… (arcs given by `CX`, `CY`, `R`,
/// `SA`, `EA`), counter-clockwise, through `at`.
fn vertex_ring(rec: &HashMap<String, String>, at: &dyn Fn(P) -> P) -> Vec<P> {
    let mut ring = Vec::new();
    let n = (0..).take_while(|i| rec.contains_key(&format!("VX{i}"))).count();
    for i in 0..n {
        let k = |s: &str| rec.get(&format!("{s}{i}"));
        let v = p(mil(k("VX")), mil(k("VY")));
        if k("KIND").map(|s| s.trim()) == Some("1") {
            let c = p(mil(k("CX")), mil(k("CY")));
            let r = mil(k("R"));
            let num = |s: Option<&String>| s.and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0);
            let (sa, ea) = (num(k("SA")), num(k("EA")));
            // Arcs run from the corner they start at; which way round
            // depends on which end that corner is.
            let start = c.add(p(sa.to_radians().cos(), sa.to_radians().sin()).scale(r));
            let lines = prims::arc_lines(c, r, sa, ea);
            let mut pts: Vec<P> = lines.iter().map(|l| l.0).chain(lines.last().map(|l| l.1)).collect();
            if !start.same(v) {
                pts.reverse();
            }
            ring.extend(pts.into_iter().map(at));
        } else {
            ring.push(at(v));
        }
    }
    let mut ring = dedup_ring(ring);
    if ring_area(&ring) < 0.0 {
        ring.reverse();
    }
    ring
}

/// Altium's number for a layer named in a text record.
fn layer_number(name: &str) -> Option<u8> {
    let n = name.trim().to_ascii_uppercase();
    match n.as_str() {
        "TOP" => Some(TOP),
        "BOTTOM" => Some(BOTTOM),
        _ => {
            if let Some(k) = n.strip_prefix("MID") {
                k.parse::<u8>().ok().filter(|k| (1..=30).contains(k)).map(|k| k + 1)
            } else if let Some(k) = n.strip_prefix("PLANE") {
                k.parse::<u8>().ok().filter(|k| (1..=16).contains(k)).map(|k| k + 38)
            } else {
                None
            }
        }
    }
}

/// What an unnamed (Circuit Studio) storage holds, from its first record.
fn kind_by_content(data: &[u8]) -> Kind {
    let Some(&first) = data.first() else { return Kind::Ignore };
    if rule_records(data).first().is_some_and(|r| r.contains_key("RULEKIND")) {
        return Kind::Rules;
    }
    if matches!(first, 1 | 2 | 3 | 4 | 5 | 6 | 11 | 12) && prims::parse(data).is_ok() {
        return Kind::Prims;
    }
    let Some(rec) = text_records(data).into_iter().next() else { return Kind::Ignore };
    if rec.contains_key("ORIGINX") {
        Kind::Board
    } else if rec.contains_key("PATTERN") && rec.contains_key("SOURCEDESIGNATOR") {
        Kind::Components
    } else if rec.contains_key("POLYGONTYPE") || rec.contains_key("POUROVER") {
        Kind::Polygons
    } else if rec.contains_key("NAME") && rec.contains_key("COLOR") && !rec.contains_key("KIND") && !rec.contains_key("RECORD") {
        Kind::Nets
    } else {
        Kind::Ignore
    }
}

/// "1234.5mil" (or a bare number of mils) in millimetres.
fn mil(v: Option<&String>) -> f64 {
    let Some(v) = v else { return 0.0 };
    let v = v.trim();
    let (num, scale) = if let Some(n) = v.strip_suffix("mil") {
        (n, 0.0254)
    } else if let Some(n) = v.strip_suffix("mm") {
        (n, 1.0)
    } else {
        (v, 0.0254)
    };
    num.trim().parse::<f64>().map(|x| x * scale).unwrap_or(0.0)
}

/// A part's name as Altium drew it: how tall, and where its middle is (as
/// Altium has it, not yet moved to the board's origin). It's the text on an
/// overlay layer that says the part's designator, spelt out or as the special
/// string `.Designator`; Altium starts it at its lower left, and turns it.
fn designator(prims: &[Prim], refdes: &str) -> Option<(f64, P)> {
    prims.iter().find_map(|pr| match pr {
        Prim::Text { head, at, height, rot, mirror, text }
            if (head.layer == prims::TOP_OVERLAY || head.layer == prims::BOTTOM_OVERLAY)
                && *height > 0.0
                && (text == refdes || text.eq_ignore_ascii_case(".Designator")) =>
        {
            let w = crate::pcb::font::width(refdes, *height);
            let across = if *mirror { -w / 2.0 } else { w / 2.0 };
            Some((*height, at.add(p(across, height / 2.0).rot(*rot))))
        }
        _ => None,
    })
}

pub fn read(bytes: &[u8], name: &str) -> Result<ImportedBoard, String> {
    let mut file = open(bytes)?;
    let storages: Vec<String> = file.read_root_storage().filter(|e| e.is_storage()).map(|e| e.name().to_string()).collect();
    let mut board_rec: HashMap<String, String> = HashMap::new();
    let mut nets_recs = Vec::new();
    let mut comp_recs = Vec::new();
    let mut polygon_recs = Vec::new();
    let mut prims: Vec<Prim> = Vec::new();
    let mut circuit_studio = false;
    let mut rule_recs = Vec::new();
    for s in &storages {
        let Ok(data) = stream(&mut file, &format!("/{s}/Data")) else { continue };
        let kind = match kind_by_name(s) {
            Some(k) => k,
            None if s.len() >= 24 && s.chars().all(|c| c.is_ascii_hexdigit()) => {
                circuit_studio = true;
                kind_by_content(&data)
            }
            None => Kind::Ignore,
        };
        match kind {
            Kind::Board => board_rec = text_records(&data).into_iter().next().unwrap_or_default(),
            Kind::Nets => nets_recs = text_records(&data),
            Kind::Components => comp_recs = text_records(&data),
            Kind::Polygons => polygon_recs = text_records(&data),
            Kind::Rules => rule_recs = rule_records(&data),
            Kind::Prims => match prims::parse(&data) {
                Ok(p) => prims.extend(p),
                Err(e) => return Err(format!("{s}: {e}")),
            },
            Kind::Ignore => {}
        }
    }
    if board_rec.is_empty() {
        return Err("no board settings found: is this a PcbDoc?".into());
    }
    // Regions are kept twice, plainly and shape-based (with their arcs): one of each.
    let mut seen = std::collections::HashSet::new();
    prims.retain(|pr| match pr {
        Prim::Region { head, pts, kind, cutout, .. } => {
            let bb = BBox::of(pts);
            let k = |v: f64| (v * 1000.0).round() as i64;
            seen.insert((head.layer, head.net, head.component, head.polygon, *kind, *cutout, k(bb.min.x), k(bb.min.y), k(bb.max.x), k(bb.max.y)))
        }
        _ => true,
    });
    let mut notes = Vec::new();
    let origin = p(mil(board_rec.get("ORIGINX")), mil(board_rec.get("ORIGINY")));
    let at = |q: P| q.sub(origin).rounded();
    let names = layer_names(&board_rec.iter().map(|(k, v)| format!("|{k}={v}")).collect::<String>().into_bytes());

    let mut b = Board { name: name.to_string(), footprints: Vec::new(), outline: Vec::new(), ..Board::default() };
    let area_rules = read_rules(&rule_recs, &mut b.rules);

    // The outline: corners, and arcs between them.
    b.outline = vertex_ring(&board_rec, &at);

    // The copper layers, top to bottom, following the stack.
    let mut stack = vec![TOP];
    let mut here = TOP;
    for _ in 0..40 {
        let next = board_rec.get(&format!("LAYER{here}NEXT")).and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(BOTTOM);
        if next == 0 || next == BOTTOM || stack.contains(&next) {
            break;
        }
        // Planes (39–54) are copper too.
        stack.push(next);
        here = next;
    }
    stack.push(BOTTOM);
    let inner: Vec<u8> = stack[1..stack.len() - 1].to_vec();
    b.copper = ((inner.len() + 2) as u8).min(32);
    let copper_of = |l: u8| -> Option<u8> {
        match l {
            TOP => Some(crate::pcb::model::TOP),
            BOTTOM => Some(crate::pcb::model::BOTTOM),
            _ => inner.iter().position(|x| *x == l).map(|i| i as u8 + 1),
        }
    };

    // Nets, in the order the primitives count them.
    let mut net_ids: Vec<u32> = Vec::new();
    for rec in &nets_recs {
        let name = rec.get("NAME").cloned().unwrap_or_default();
        net_ids.push(if name.is_empty() { 0 } else { b.net(&name) });
    }
    let net = |i: u16| if i == NONE { 0 } else { net_ids.get(i as usize).copied().unwrap_or(0) };

    // Parts, each with its own primitives taken back to its own coordinates.
    let mut by_comp: HashMap<u16, Vec<Prim>> = HashMap::new();
    let mut free: Vec<Prim> = Vec::new();
    for prim in prims {
        let comp = match &prim {
            Prim::Pad { head, .. }
            | Prim::Track { head, .. }
            | Prim::Arc { head, .. }
            | Prim::Region { head, .. }
            | Prim::Fill { head, .. }
            | Prim::Text { head, .. } => head.component,
            Prim::Via { .. } => NONE,
            Prim::Other { head, .. } => head.map(|h| h.component).unwrap_or(NONE),
        };
        if comp == NONE { free.push(prim) } else { by_comp.entry(comp).or_default().push(prim) }
    }
    let mut skipped = Skipped::default();
    for (i, rec) in comp_recs.iter().enumerate() {
        let id = b.new_id();
        let pos = at(p(mil(rec.get("X")), mil(rec.get("Y"))));
        let rot = rec.get("ROTATION").and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0).rem_euclid(360.0);
        let bottom = rec.get("LAYER").is_some_and(|l| l.eq_ignore_ascii_case("BOTTOM"));
        let mine = by_comp.remove(&(i as u16)).unwrap_or_default();
        // Board → part: unturn, then unmirror for a part underneath.
        let local = |q: P| {
            let d = at(q).sub(pos).rot(-rot);
            if bottom { p(-d.x, d.y) } else { d }
        };
        let (mut pads, graphics, raw_keepouts) = footprint(&mine, &names, &mut skipped, local, rot, bottom);
        // Its keepouts, in its own terms: layers swap for a part underneath.
        let keepouts: Vec<Keepout> = raw_keepouts
            .into_iter()
            .filter_map(|k| {
                let mut k = prims::to_keepout(k, &copper_of, bottom)?;
                k.id = 0;
                Some(k)
            })
            .collect();
        // Nets go on the pads.
        let pad_nets: Vec<u32> = mine.iter().filter_map(|pr| if let Prim::Pad { head, pad, .. } = pr { (pad.size.x > 0.0 && pad.size.y > 0.0).then(|| net(head.net)) } else { None }).collect();
        for (pad, n) in pads.iter_mut().zip(pad_nets) {
            pad.net = n;
        }
        let refdes = rec.get("SOURCEDESIGNATOR").cloned().unwrap_or_else(|| format!("U{}", i + 1));
        // The name as Altium had it: its height and where its middle is.
        let (label_at, label_height) = match designator(&mine, &refdes) {
            Some((height, middle)) => (local(middle).rounded(), Some(nm(height))),
            None => (label_spot(&pads), None),
        };
        b.footprints.push(Footprint {
            id,
            def: rec.get("PATTERN").cloned().unwrap_or_default(),
            refdes,
            value: rec.get("SOURCELIBREFERENCE").cloned().unwrap_or_default(),
            at: pos,
            rot: nm(rot),
            side: if bottom { Side::Bottom } else { Side::Top },
            label_at,
            label_height,
            pads,
            graphics,
            locked: rec.get("LOCKED").is_some_and(|v| v == "TRUE"),
            keepouts,
        });
    }
    skipped.note(&mut notes, "Parts");
    // Most names are one height: that's the board's, and only the odd ones
    // (a bigger name on a big part) keep a height of their own.
    let micron = |h: f64| (h * 1000.0).round() as i64;
    let mut heights: HashMap<i64, usize> = HashMap::new();
    for f in &b.footprints {
        if let Some(h) = f.label_height {
            *heights.entry(micron(h)).or_default() += 1;
        }
    }
    if let Some((&common, _)) = heights.iter().max_by_key(|(k, n)| (**n, std::cmp::Reverse(**k))) {
        b.rules.text_height = common as f64 / 1000.0;
        for f in &mut b.footprints {
            if f.label_height.is_some_and(|h| micron(h) == common) {
                f.label_height = None;
            }
        }
    }

    // Free copper: tracks, arcs and vias. Pour fill (tracks and regions
    // belonging to polygons) is left out with the pours.
    let (mut pour_bits, mut free_pads, mut copper_fills, mut blind) = (0usize, 0usize, 0usize, 0usize);
    let (mut keepout_arcs, mut keepout_outlines) = (0usize, 0usize);
    for prim in free {
        // Areas kept free of copper, not copper: tracks, fills and regions
        // flagged as keepouts, and what's drawn on the Keep-Out layer.
        if let Some(raw) = prims::keepout_of(&prim) {
            let raw = prims::RawKeepout { pts: raw.pts.iter().map(|q| at(*q)).collect(), ..raw };
            // Altium boards often draw the board's shape on the Keep-Out layer, to
            // mark where copper may go: a shape on it that covers most of the board
            // is that boundary, not an area to keep out of.
            if raw.layer == prims::KEEPOUT_LAYER && ring_area(&raw.pts).abs() >= 0.8 * ring_area(&b.outline).abs() {
                keepout_outlines += 1;
                continue;
            }
            if let Some(mut k) = prims::to_keepout(raw, copper_of, false) {
                k.id = b.new_id();
                b.keepouts.push(k);
            }
            continue;
        }
        if prims::is_keepout_arc(&prim) {
            keepout_arcs += 1;
            continue;
        }
        match prim {
            Prim::Track { head, a, b: e, w } => {
                if head.polygon != NONE {
                    pour_bits += 1;
                } else if let Some(l) = copper_of(head.layer) {
                    let id = b.new_id();
                    b.tracks.push(Track { mid: None, id, a: at(a), b: at(e), w: nm(w), layer: l, net: net(head.net) });
                }
            }
            Prim::Arc { head, c, r, start, end, w } => {
                if head.polygon != NONE {
                    pour_bits += 1;
                } else if let Some(l) = copper_of(head.layer) {
                    let (s, m, e) = prims::arc_points(at(c), r, start, end);
                    let id = b.new_id();
                    b.tracks.push(Track { mid: Some(m), id, a: s, b: e, w: nm(w), layer: l, net: net(head.net) });
                }
            }
            Prim::Via { head, at: v, dia, hole } => {
                let id = b.new_id();
                b.vias.push(Via { id, at: at(v), dia: nm(dia), drill: nm(hole), net: net(head.net) });
                let _ = head;
            }
            Prim::Region { head, pts, kind, cutout, .. } if prims::is_copper(head.layer) => {
                if head.polygon != NONE {
                    // Poured copper: poured again from the pour's outline.
                    pour_bits += 1;
                } else if kind != 0 || cutout {
                    copper_fills += 1;
                } else if let Some(l) = copper_of(head.layer)
                    && pts.len() >= 3
                {
                    let mut pts: Vec<P> = dedup_ring(pts.into_iter().map(at).collect());
                    if ring_area(&pts) < 0.0 {
                        pts.reverse();
                    }
                    let id = b.new_id();
                    b.fills.push(Fill { id, layer: l, net: net(head.net), pts });
                }
            }
            Prim::Fill { head, a, b: e, rot } if prims::is_copper(head.layer) => {
                if let Some(l) = copper_of(head.layer) {
                    let (a, e) = (at(a), at(e));
                    let pts = rect_core(a.lerp(e, 0.5), (e.x - a.x).abs(), (e.y - a.y).abs(), rot).into_iter().map(|q| q.rounded()).collect();
                    let id = b.new_id();
                    b.fills.push(Fill { id, layer: l, net: net(head.net), pts });
                }
            }
            Prim::Pad { head, .. } if head.layer == MULTI_LAYER || prims::is_copper(head.layer) => free_pads += 1,
            _ => {}
        }
    }
    let _ = (&mut blind, pour_bits);
    if keepout_arcs > 0 {
        notes.push(format!("{keepout_arcs} keepout arcs weren't taken"));
    }
    if keepout_outlines > 0 {
        notes.push(format!("{keepout_outlines} shapes on the Keep-Out layer outline the board, so aren't keepouts"));
    }

    // Pours and split planes, poured again here from their outlines.
    let (mut pours, mut hatched, mut shelved) = (0usize, 0usize, 0usize);
    let mut split_layers: Vec<u8> = Vec::new();
    for rec in &polygon_recs {
        let get = |k: &str| rec.get(k).map(|s| s.trim().trim_end_matches('\0').to_string());
        let Some(layer) = get("LAYER").and_then(|l| layer_number(&l)) else { continue };
        let Some(l) = copper_of(layer) else { continue };
        if get("SHELVED").as_deref() == Some("TRUE") {
            shelved += 1;
            continue;
        }
        let outline = vertex_ring(rec, &at);
        if outline.len() < 3 {
            continue;
        }
        let n = get("NET").and_then(|v| v.parse::<u16>().ok()).map(net).unwrap_or(0);
        let id = b.new_id();
        let mut pour = Pour::new(id, l, n, outline, &b.rules);
        if get("POLYGONTYPE").is_some_and(|t| t.eq_ignore_ascii_case("Split Plane")) {
            split_layers.push(layer);
            let k = layer - 38;
            pour.plane = true;
            pour.clearance = area_rules.plane_clearance;
            pour.edge = nm(mil(board_rec.get(&format!("PLANE{k}PULLBACK")).or(Some(&"20mil".to_string()))));
            pour.connect = area_rules.plane_connect;
            pour.spoke = area_rules.plane_spoke;
            pour.gap = area_rules.plane_gap;
            pour.remove_islands = false;
        } else {
            pours += 1;
            if get("HATCHSTYLE").is_some_and(|h| !h.eq_ignore_ascii_case("Solid")) {
                hatched += 1;
            }
            pour.clearance = area_rules.pour_clearance;
            pour.connect = area_rules.pour_connect;
            pour.spoke = area_rules.pour_spoke;
            pour.gap = area_rules.pour_gap;
            pour.remove_islands = get("REMOVEDEAD").as_deref() != Some("FALSE");
            if get("REMOVEISLANDSBYAREA").as_deref() == Some("TRUE") {
                // In Altium's units squared.
                let a: f64 = get("AREATHRESHOLD").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                pour.min_area = a * super::UNIT * super::UNIT;
            }
            if get("REMOVENECKS").as_deref() == Some("TRUE") {
                pour.neck = nm(mil(get("NECKWIDTHTHRESHOLD").as_ref()));
            }
        }
        pour.order = get("POURINDEX").and_then(|v| v.parse().ok()).unwrap_or(id);
        b.pours.push(pour);
    }
    // A plane layer with a net and no split planes is one plane, the whole board.
    for &layer in &inner {
        if !(39..=54).contains(&layer) || split_layers.contains(&layer) {
            continue;
        }
        let k = layer - 38;
        let name = board_rec.get(&format!("PLANE{k}NETNAME")).map(|s| s.trim().trim_end_matches('\0').to_string()).unwrap_or_default();
        let n = if name.is_empty() || name == "(No Net)" { 0 } else { b.net_by_name(&name).unwrap_or(0) };
        let Some(l) = copper_of(layer) else { continue };
        let id = b.new_id();
        let mut pour = Pour::new(id, l, n, Vec::new(), &b.rules);
        pour.plane = true;
        pour.clearance = area_rules.plane_clearance;
        pour.edge = nm(mil(board_rec.get(&format!("PLANE{k}PULLBACK")).or(Some(&"20mil".to_string()))));
        pour.connect = area_rules.plane_connect;
        pour.spoke = area_rules.plane_spoke;
        pour.gap = area_rules.plane_gap;
        pour.remove_islands = false;
        b.pours.push(pour);
    }
    if pours > 0 {
        notes.push(format!("{pours} copper pour{} poured again here from {} outline{}", if pours == 1 { "" } else { "s" }, if pours == 1 { "its" } else { "their" }, if pours == 1 { "" } else { "s" }));
    }
    if hatched > 0 {
        notes.push(format!("{hatched} hatched pour{} poured solid", if hatched == 1 { "" } else { "s" }));
    }
    if shelved > 0 {
        notes.push(format!("{shelved} shelved pour{} left out", if shelved == 1 { "" } else { "s" }));
    }
    let planes = b.pours.iter().filter(|p| p.plane).count();
    if planes > 0 {
        notes.push(format!("{planes} power plane{} poured as positive copper", if planes == 1 { "" } else { "s" }));
    }
    if copper_fills > 0 {
        notes.push(format!("{copper_fills} pour cutout{} left out", if copper_fills == 1 { "" } else { "s" }));
    }
    if free_pads > 0 {
        notes.push(format!("{free_pads} pads not belonging to a part left out"));
    }
    if circuit_studio {
        notes.push("Read as a Circuit Studio board".into());
    }
    b.repour();
    Ok(ImportedBoard { board: b, notes })
}

/// A ring without a corner repeated.
fn dedup_ring(mut ring: Vec<P>) -> Vec<P> {
    ring.dedup_by(|a, b| a.same(*b));
    while ring.len() > 1 && ring[0].same(*ring.last().unwrap()) {
        ring.pop();
    }
    ring
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(layer: u8, at: P, height: f64, rot: f64, mirror: bool, s: &str) -> Prim {
        Prim::Text { head: prims::Head { layer, flags: 0x000c, net: NONE, polygon: NONE, component: 0 }, at, height, rot, mirror, text: s.into() }
    }

    #[test]
    fn a_designator_comes_with_its_height_and_its_middle() {
        let w = crate::pcb::font::width("R25", 0.635);
        // Altium starts the text at its lower left: its middle is half its width along, half its height up.
        let (h, mid) = designator(&[text(prims::TOP_OVERLAY, p(10.0, 20.0), 0.635, 0.0, false, "R25")], "R25").unwrap();
        assert_eq!(h, 0.635);
        assert!(mid.dist(p(10.0 + w / 2.0, 20.0 + 0.635 / 2.0)) < 1e-9, "{mid:?}");
        // Turned a quarter, along and up swap: the middle is up the page and to the left.
        let (_, mid) = designator(&[text(prims::TOP_OVERLAY, p(10.0, 20.0), 0.635, 90.0, false, "R25")], "R25").unwrap();
        assert!(mid.dist(p(10.0 - 0.635 / 2.0, 20.0 + w / 2.0)) < 1e-9, "{mid:?}");
        // Mirrored, as text underneath is, the other way along.
        let (_, mid) = designator(&[text(prims::BOTTOM_OVERLAY, p(10.0, 20.0), 0.635, 0.0, true, "R25")], "R25").unwrap();
        assert!(mid.dist(p(10.0 - w / 2.0, 20.0 + 0.635 / 2.0)) < 1e-9, "{mid:?}");
    }

    #[test]
    fn only_the_designator_on_an_overlay_counts() {
        let others = [
            // The part's comment, taller, on the same layer.
            text(prims::TOP_OVERLAY, p(0.0, 0.0), 1.524, 0.0, false, "RC0402FR-071KL"),
            // The special string on an assembly layer.
            text(61, p(0.0, 0.0), 1.0, 0.0, false, ".Designator"),
            // Text of no height.
            text(prims::TOP_OVERLAY, p(0.0, 0.0), 0.0, 0.0, false, "R25"),
        ];
        assert!(designator(&others, "R25").is_none());
        // The name spelt out, or as Altium's special string, either way; the first wins.
        let mut both = others.to_vec();
        both.push(text(prims::TOP_OVERLAY, p(1.0, 1.0), 0.889, 0.0, false, ".Designator"));
        both.push(text(prims::TOP_OVERLAY, p(2.0, 2.0), 0.5, 0.0, false, "R25"));
        assert_eq!(designator(&both, "R25").unwrap().0, 0.889);
    }

    #[test]
    fn a_circuit_studio_board_reads() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/altium/TestFixtureISSIPCB.CSPcbDoc");
        if !path.exists() {
            return;
        }
        let got = read(&std::fs::read(&path).unwrap(), "fixture").unwrap();
        let b = &got.board;
        assert!(b.outline.len() >= 4, "{:?}", b.outline);
        assert_eq!(b.copper, 4);
        assert!(!b.footprints.is_empty());
        assert!(!b.nets.is_empty());
        let p1 = b.footprints.iter().find(|f| f.refdes == "P1").expect("P1");
        assert_eq!(p1.def, "HDR1X2");
        assert_eq!(p1.pads.len(), 2);
        // Each part's pads sit where Altium had them on the board.
        assert!(p1.pads.iter().all(|p| p.net != 0 || p.drill > 0.0));
        // Its GND pour on the second inner layer, poured again here, joins
        // every GND pad: nothing's left to connect and nothing's too close.
        assert_eq!(b.pours.len(), 1);
        let pour = &b.pours[0];
        assert_eq!((b.net_name(pour.net), pour.layer), ("GND", 2));
        assert!(pour.copper_area() > 1000.0, "{}", pour.copper_area());
        let c = crate::pcb::copper::analyse(b);
        let found = crate::pcb::drc::check(b, &c);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    #[ignore]
    fn list_fills() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/projects/J1939Plug/J1939PLUG.CSPcbDoc");
        let got = read(&std::fs::read(&path).unwrap(), "j").unwrap();
        assert_eq!(got.board.fills.len(), 19);
        let mut b = got.board.clone();
        let t = std::time::Instant::now();
        b.repour();
        let poured = t.elapsed();
        let t = std::time::Instant::now();
        let c = crate::pcb::copper::analyse(&b);
        let found = crate::pcb::drc::check(&b, &c);
        println!("repour {poured:?}, analyse+check {:?} ({} problems)", t.elapsed(), found.len());
        for f in &got.board.fills {
            let bb = BBox::of(&f.pts);
            println!("{} {} {:.2} corners {} at {:.2},{:.2} size {:.2}x{:.2}", f.layer, got.board.net_name(f.net), ring_area(&f.pts), f.pts.len(), bb.center().x, bb.center().y, bb.w(), bb.h());
        }
    }
}

