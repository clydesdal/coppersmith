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

/// The board's design rules that Coppersmith has a place for. Each comes
/// from the general rule of its kind: enabled, applying to everything, the
/// lowest priority (Altium's fallback).
fn read_rules(recs: &[HashMap<String, String>], rules: &mut Rules) {
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

pub fn read(bytes: &[u8], name: &str) -> Result<ImportedBoard, String> {
    let mut file = open(bytes)?;
    let storages: Vec<String> = file.read_root_storage().filter(|e| e.is_storage()).map(|e| e.name().to_string()).collect();
    let mut board_rec: HashMap<String, String> = HashMap::new();
    let mut nets_recs = Vec::new();
    let mut comp_recs = Vec::new();
    let mut polygons = 0usize;
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
            Kind::Polygons => polygons += text_records(&data).len(),
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
    let mut notes = Vec::new();
    let origin = p(mil(board_rec.get("ORIGINX")), mil(board_rec.get("ORIGINY")));
    let at = |q: P| q.sub(origin).rounded();
    let names = layer_names(&board_rec.iter().map(|(k, v)| format!("|{k}={v}")).collect::<String>().into_bytes());

    let mut b = Board { name: name.to_string(), footprints: Vec::new(), outline: Vec::new(), ..Board::default() };
    read_rules(&rule_recs, &mut b.rules);

    // The outline: corners, and arcs between them.
    let n = (0..).take_while(|i| board_rec.contains_key(&format!("VX{i}"))).count();
    for i in 0..n {
        let k = |s: &str| board_rec.get(&format!("{s}{i}"));
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
            b.outline.extend(pts.into_iter().map(at));
        } else {
            b.outline.push(at(v));
        }
    }
    b.outline = dedup_ring(b.outline);
    if ring_area(&b.outline) < 0.0 {
        b.outline.reverse();
    }

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
    if inner.iter().any(|l| (39..=54).contains(l)) {
        notes.push("Plane layers came across as ordinary copper layers, without their split planes".into());
    }

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
            Prim::Pad { head, .. } | Prim::Track { head, .. } | Prim::Arc { head, .. } | Prim::Region { head, .. } | Prim::Fill { head, .. } => head.component,
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
        let (mut pads, graphics) = footprint(&mine, &names, &mut skipped, local, rot, bottom);
        // Nets go on the pads.
        let pad_nets: Vec<u32> = mine.iter().filter_map(|pr| if let Prim::Pad { head, pad, .. } = pr { (pad.size.x > 0.0 && pad.size.y > 0.0).then(|| net(head.net)) } else { None }).collect();
        for (pad, n) in pads.iter_mut().zip(pad_nets) {
            pad.net = n;
        }
        let refdes = rec.get("SOURCEDESIGNATOR").cloned().unwrap_or_else(|| format!("U{}", i + 1));
        b.footprints.push(Footprint {
            id,
            def: rec.get("PATTERN").cloned().unwrap_or_default(),
            refdes,
            value: rec.get("SOURCELIBREFERENCE").cloned().unwrap_or_default(),
            at: pos,
            rot: nm(rot),
            side: if bottom { Side::Bottom } else { Side::Top },
            label_at: label_spot(&pads),
            pads,
            graphics,
            locked: rec.get("LOCKED").is_some_and(|v| v == "TRUE"),
        });
    }
    skipped.note(&mut notes, "Parts");

    // Free copper: tracks, arcs and vias. Pour fill (tracks and regions
    // belonging to polygons) is left out with the pours.
    let (mut pour_bits, mut free_pads, mut copper_fills, mut blind) = (0usize, 0usize, 0usize, 0usize);
    for prim in free {
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
            Prim::Region { head, .. } if prims::is_copper(head.layer) => pour_bits += 1,
            Prim::Fill { head, .. } if prims::is_copper(head.layer) => copper_fills += 1,
            Prim::Pad { head, .. } if head.layer == MULTI_LAYER || prims::is_copper(head.layer) => free_pads += 1,
            _ => {}
        }
    }
    let _ = &mut blind;
    if polygons > 0 {
        notes.push(format!("{polygons} copper pours left out (pours aren't in Coppersmith yet); their nets show as still to connect"));
    } else if pour_bits > 0 {
        notes.push(format!("{pour_bits} pieces of copper pour left out"));
    }
    if copper_fills > 0 {
        notes.push(format!("{copper_fills} copper fills left out"));
    }
    if free_pads > 0 {
        notes.push(format!("{free_pads} pads not belonging to a part left out"));
    }
    if circuit_studio {
        notes.push("Read as a Circuit Studio board".into());
    }
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
    }
}

