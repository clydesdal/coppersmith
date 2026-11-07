//! Footprints from an Altium `.PcbLib`.
//!
//! Each footprint is a storage holding `Parameters` (its name and
//! description as `|PATTERN=…|DESCRIPTION=…|`) and `Data`: the name again,
//! then records, each a type byte and one or more length-prefixed blocks:
//! 1 arc, 2 pad (six blocks), 3 via, 4 track, 5 text (two blocks), 6 fill,
//! 11 region, 12 3D body.

use super::{Imported, Reader, open, params, stream};
use crate::pcb::geom::*;
use crate::pcb::library::FootprintDef;
use crate::pcb::model::*;
use std::collections::HashMap;

use super::prims::{self, BOTTOM_OVERLAY, MECHANICAL_1, MECHANICAL_16, Prim, TOP_OVERLAY};

/// Where an Altium layer's drawing goes in Coppersmith, if anywhere.
pub fn drawing_layer(layer: u8, names: &HashMap<u8, String>) -> Option<Layer> {
    match layer {
        TOP_OVERLAY => Some(Layer::Silk(Side::Top)),
        BOTTOM_OVERLAY => Some(Layer::Silk(Side::Bottom)),
        MECHANICAL_1..=MECHANICAL_16 => {
            let name = names.get(&layer).map(|n| n.to_lowercase()).unwrap_or_default();
            if name.contains("court") {
                Some(Layer::Court(Side::Top))
            } else if name.contains("3d") || name.contains("body") && !name.contains("assembly") {
                None
            } else {
                Some(Layer::Fab(Side::Top))
            }
        }
        _ => None,
    }
}

/// The library's own names for its layers, from `LAYER<n>NAME=…`.
pub fn layer_names(text: &[u8]) -> HashMap<u8, String> {
    params(text)
        .into_iter()
        .filter_map(|(k, v)| k.strip_prefix("LAYER").and_then(|s| s.strip_suffix("NAME")).and_then(|n| n.parse::<u8>().ok()).map(|n| (n, v)))
        .collect()
}

/// Counts of what was left out of a footprint, for the notes.
#[derive(Default)]
pub struct Skipped {
    pub copper_drawing: usize,
    pub fills: usize,
    pub regions: usize,
    pub octagons: usize,
    pub slots: usize,
    pub empty_pads: usize,
    /// Keepouts that were arcs, which aren't taken.
    pub keepout_arcs: usize,
}

impl Skipped {
    pub fn note(&self, notes: &mut Vec<String>, name: &str) {
        let mut bits = Vec::new();
        if self.copper_drawing > 0 {
            bits.push(format!("{} copper tracks or arcs outside pads", self.copper_drawing));
        }
        if self.fills > 0 {
            bits.push(format!("{} filled rectangles", self.fills));
        }
        if self.regions > 0 {
            bits.push(format!("{} copper regions (custom pad shapes)", self.regions));
        }
        if self.octagons > 0 {
            bits.push(format!("{} octagonal pads made rounded", self.octagons));
        }
        if self.slots > 0 {
            bits.push(format!("{} slotted holes made round", self.slots));
        }
        if self.empty_pads > 0 {
            bits.push(format!("{} pads with no size", self.empty_pads));
        }
        if self.keepout_arcs > 0 {
            bits.push(format!("{} keepout arcs (not taken)", self.keepout_arcs));
        }
        if !bits.is_empty() {
            notes.push(format!("{name}: {}", bits.join(", ")));
        }
    }
}

pub fn read(bytes: &[u8], lib: &str) -> Result<Imported, String> {
    let mut file = open(bytes)?;
    let mut names: HashMap<u8, String> = HashMap::new();
    if let Ok(head) = stream(&mut file, "/Library/Data")
        && let Ok(text) = Reader::new(&head).block()
    {
        names = layer_names(text);
    }
    let storages: Vec<String> = file
        .read_root_storage()
        .filter(|e| e.is_storage())
        .map(|e| e.name().to_string())
        .filter(|n| n != "Library" && n != "FileVersionInfo")
        .collect();
    let mut out = Imported::default();
    for storage in storages {
        let Ok(data) = stream(&mut file, &format!("/{storage}/Data")) else { continue };
        let props = stream(&mut file, &format!("/{storage}/Parameters"))
            .ok()
            .and_then(|p| Reader::new(&p).block().map(params).ok())
            .unwrap_or_default();
        let name = props.get("PATTERN").cloned().unwrap_or_else(|| storage.clone());
        let mut skipped = Skipped::default();
        let parsed = (|| {
            let mut r = Reader::new(&data);
            r.block()?; // the name again
            prims::parse(&data[r.i..])
        })();
        let prims = match parsed {
            Ok(p) => p,
            Err(e) => {
                out.notes.push(format!("{name}: couldn't be read ({e}); left out"));
                continue;
            }
        };
        let (pads, graphics, raw_keepouts) = footprint(&prims, &names, &mut skipped, |q| q, 0.0, false);
        // A library footprint has only a top and a bottom; an inner layer's keepout stands for every layer.
        let lib_layer = |l: u8| match l {
            prims::TOP => Some(TOP),
            prims::BOTTOM => Some(BOTTOM),
            _ => None,
        };
        let keepouts: Vec<Keepout> = raw_keepouts
            .into_iter()
            .filter_map(|k| {
                let inner = (prims::TOP + 1..prims::BOTTOM).contains(&k.layer) || (39..=54).contains(&k.layer);
                let k = if inner { prims::RawKeepout { layer: prims::MULTI_LAYER, ..k } } else { k };
                prims::to_keepout(k, lib_layer, false)
            })
            .collect();
        if pads.is_empty() && graphics.is_empty() && keepouts.is_empty() {
            out.notes.push(format!("{name}: empty, left out"));
            continue;
        }
        skipped.note(&mut out.notes, &name);
        out.footprints.push(FootprintDef {
            label_at: label_spot(&pads),
            name,
            category: lib.to_string(),
            description: props.get("DESCRIPTION").cloned().unwrap_or_default(),
            prefix: "U".into(),
            pads,
            graphics,
            keepouts,
            source: format!("{lib} (Altium)"),
        });
    }
    Ok(out)
}

/// Above the pads, where a part's name goes.
pub fn label_spot(pads: &[Pad]) -> P {
    let mut bb = BBox::default();
    for pad in pads {
        bb.union(&pad.blob_local(0.0).bbox());
    }
    if bb.is_empty() { p(0.0, 1.5) } else { p(bb.center().x, bb.max.y + 1.0).rounded() }
}

/// Do these corners go all the way round `c`, enough of them and no gap of a
/// third of a turn or more? A round dot's outline does; a thin stroke's
/// corners, bunched at its two ends, don't.
fn surrounds(pts: &[P], c: P) -> bool {
    if pts.len() < 8 {
        return false;
    }
    let mut angles: Vec<f64> = pts.iter().map(|q| (q.y - c.y).atan2(q.x - c.x)).collect();
    angles.sort_by(|a, b| a.total_cmp(b));
    let wrap = angles[0] + std::f64::consts::TAU - angles[angles.len() - 1];
    let gap = angles.windows(2).map(|w| w[1] - w[0]).fold(wrap, f64::max);
    gap < std::f64::consts::TAU / 3.0
}

/// A long thin region drawn as a line as wide as it is: its two ends and its
/// width. None when it isn't at least three times as long as it is wide.
fn stroke_of(pts: &[P]) -> Option<(P, P, f64)> {
    // The smallest rectangle round it, with a side along one of its edges.
    let a = pts[0];
    let mut best: Option<(f64, P, f64, f64, f64, f64)> = None; // area, u, t0, t1, s0, s1
    for k in 0..pts.len() {
        let e = pts[(k + 1) % pts.len()].sub(pts[k]);
        let l = e.dot(e).sqrt();
        if l < 1e-9 {
            continue;
        }
        let u = e.scale(1.0 / l);
        let n = p(-u.y, u.x);
        let (mut t0, mut t1, mut s0, mut s1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for q in pts {
            let d = q.sub(a);
            let (t, s) = (d.dot(u), d.dot(n));
            t0 = t0.min(t);
            t1 = t1.max(t);
            s0 = s0.min(s);
            s1 = s1.max(s);
        }
        let area = (t1 - t0) * (s1 - s0);
        if best.is_none_or(|b| area < b.0) {
            best = Some((area, u, t0, t1, s0, s1));
        }
    }
    let (_, u, t0, t1, s0, s1) = best?;
    let n = p(-u.y, u.x);
    let (len, width) = (t1 - t0, s1 - s0);
    if width <= 0.0 || len < 3.0 * width {
        return None;
    }
    // The line's round ends reach to the region's ends.
    let mid = (s0 + s1) / 2.0;
    let (from, to) = (t0 + width / 2.0, t1 - width / 2.0);
    Some((a.add(u.scale(from)).add(n.scale(mid)), a.add(u.scale(to)).add(n.scale(mid)), width))
}

/// A footprint's pads and drawing from its primitives. `local` takes an
/// Altium position to the footprint's own coordinates; `rot` is the part's
/// turn and `bottom` whether it sits underneath, so pads and drawings on a
/// placed part come back to how the footprint was drawn.
pub fn footprint(
    prims: &[Prim],
    names: &HashMap<u8, String>,
    skipped: &mut Skipped,
    local: impl Fn(P) -> P,
    rot: f64,
    bottom: bool,
) -> (Vec<Pad>, Vec<Graphic>, Vec<prims::RawKeepout>) {
    let mut pads = Vec::new();
    let mut graphics = Vec::new();
    let mut keepouts = Vec::new();
    let layer_for = |l: u8| drawing_layer(l, names).map(|g| if bottom { g.flipped() } else { g });
    let turn = |r: f64| nm(if bottom { rot - r } else { r - rot }.rem_euclid(360.0));
    for prim in prims {
        // A keepout isn't drawn or counted as copper: it's kept as an area.
        if let Some(k) = prims::keepout_of(prim) {
            let pts = k.pts.iter().map(|q| local(*q).rounded()).collect();
            keepouts.push(prims::RawKeepout { pts, ..k });
            continue;
        }
        if prims::is_keepout_arc(prim) {
            skipped.keepout_arcs += 1;
            continue;
        }
        match prim {
            Prim::Pad { pad, slot, octagon, .. } => {
                if pad.size.x <= 0.0 || pad.size.y <= 0.0 {
                    skipped.empty_pads += 1;
                    continue;
                }
                skipped.slots += usize::from(*slot);
                skipped.octagons += usize::from(*octagon);
                let mut pad = pad.clone();
                pad.at = local(pad.at).rounded();
                pad.rot = turn(pad.rot);
                pads.push(pad);
            }
            Prim::Track { head, a, b, w } => match layer_for(head.layer) {
                Some(l) => graphics.push(Graphic::Line { a: local(*a).rounded(), b: local(*b).rounded(), w: nm(*w), layer: l }),
                None if prims::is_copper(head.layer) => skipped.copper_drawing += 1,
                None => {}
            },
            Prim::Arc { head, c, r, start, end, w } => match layer_for(head.layer) {
                Some(l) => {
                    let sweep = (end - start).rem_euclid(360.0);
                    if sweep < 1e-9 || (sweep - 360.0).abs() < 1e-9 {
                        graphics.push(Graphic::Circle { c: local(*c).rounded(), r: nm(*r), w: nm(*w), layer: l, fill: false });
                    } else {
                        for (a, b) in prims::arc_lines(*c, *r, *start, *end) {
                            graphics.push(Graphic::Line { a: local(a).rounded(), b: local(b).rounded(), w: nm(*w), layer: l });
                        }
                    }
                }
                None if prims::is_copper(head.layer) => skipped.copper_drawing += 1,
                None => {}
            },
            Prim::Region { head, pts, .. } => match layer_for(head.layer) {
                Some(l) if pts.len() >= 3 => {
                    let pts: Vec<P> = pts.iter().map(|q| local(*q).rounded()).collect();
                    let bb = BBox::of(&pts);
                    let c = bb.center();
                    let (lo, hi) = pts.iter().map(|q| q.dist(c)).fold((f64::MAX, 0.0f64), |(a, b), r| (a.min(r), b.max(r)));
                    if hi > 0.0 && (hi - lo) / hi < 0.15 && surrounds(&pts, c) {
                        graphics.push(Graphic::Circle { c, r: nm(hi), w: 0.0, layer: l, fill: true });
                    } else if let Some((a, b, w)) = stroke_of(&pts) {
                        graphics.push(Graphic::Line { a: a.rounded(), b: b.rounded(), w: nm(w), layer: l });
                    } else {
                        for k in 0..pts.len() {
                            graphics.push(Graphic::Line { a: pts[k], b: pts[(k + 1) % pts.len()], w: 0.1, layer: l });
                        }
                    }
                }
                Some(_) => {}
                None if prims::is_copper(head.layer) => skipped.regions += 1,
                None => {}
            },
            Prim::Fill { head, .. } => {
                if prims::is_copper(head.layer) {
                    skipped.copper_drawing += 1;
                } else if layer_for(head.layer).is_some() {
                    skipped.fills += 1;
                }
            }
            Prim::Via { .. } | Prim::Text { .. } | Prim::Other { .. } => {}
        }
    }
    (pads, graphics, keepouts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(name: &str) -> Imported {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/altium").join(name);
        read(&std::fs::read(path).unwrap(), name).unwrap()
    }

    #[test]
    fn a_tssop_reads_true() {
        let got = lib("fixturelib.PcbLib");
        let f = got.footprints.iter().find(|f| f.name == "tssop_sram").expect("tssop_sram");
        assert_eq!(f.pads.len(), 48);
        let p1 = f.pads.iter().find(|p| p.num == "1").unwrap();
        // 11.81 × 66.93 mil, turned a quarter.
        assert!((p1.size.x - 0.3).abs() < 1e-3 && (p1.size.y - 1.7).abs() < 1e-3, "{:?}", p1.size);
        assert!((p1.rot - 90.0).abs() < 1e-9);
        assert_eq!(p1.drill, 0.0);
        // 0.5 mm apart.
        let p2 = f.pads.iter().find(|p| p.num == "2").unwrap();
        assert!((p1.at.dist(p2.at) - 0.5).abs() < 1e-3, "{:?} {:?}", p1.at, p2.at);
        assert!(f.graphics.iter().any(|g| g.layer() == Layer::Silk(Side::Top)));
    }

    fn silk_region(pts: Vec<P>) -> Vec<Graphic> {
        let head = prims::Head { layer: TOP_OVERLAY, flags: 0, net: prims::NONE, polygon: prims::NONE, component: prims::NONE };
        let prim = Prim::Region { head, pts, kind: 0, cutout: false, restrict: 31 };
        footprint(&[prim], &HashMap::new(), &mut Skipped::default(), |q| q, 0.0, false).1
    }

    #[test]
    fn a_thin_silk_stroke_is_a_line_not_a_circle() {
        // From the SC680 part: a 3 mm stroke 0.1 mm wide with rounded ends, seven corners.
        let pts = [(19.035, 21.446), (22.000, 21.446), (22.038, 21.462), (22.054, 21.500), (22.038, 21.538), (22.000, 21.554), (19.035, 21.554)].map(|(x, y)| p(x, y)).to_vec();
        let g = silk_region(pts);
        assert_eq!(g.len(), 1, "{g:?}");
        let Graphic::Line { a, b, w, .. } = &g[0] else { panic!("not a line: {g:?}") };
        assert!((w - 0.108).abs() < 0.01, "as wide as the stroke: {w}");
        let (lo, hi) = if a.x < b.x { (a, b) } else { (b, a) };
        assert!((lo.x - 19.089).abs() < 0.01 && (hi.x - 22.0).abs() < 0.01 && (lo.y - 21.5).abs() < 0.01, "{a:?} {b:?}");
    }

    #[test]
    fn a_round_silk_dot_is_still_a_filled_circle() {
        let pts: Vec<P> = (0..16).map(|k| p(5.0 + 0.4 * (k as f64 * std::f64::consts::TAU / 16.0).cos(), 3.0 + 0.4 * (k as f64 * std::f64::consts::TAU / 16.0).sin())).collect();
        let g = silk_region(pts);
        assert!(matches!(g.as_slice(), [Graphic::Circle { fill: true, r, .. }] if (r - 0.4).abs() < 0.01), "{g:?}");
    }

    #[test]
    fn a_wide_silk_region_is_still_an_outline() {
        let g = silk_region(vec![p(0.0, 0.0), p(4.0, 0.0), p(4.0, 3.0), p(0.0, 3.0)]);
        assert_eq!(g.len(), 4);
        assert!(g.iter().all(|x| matches!(x, Graphic::Line { .. })));
    }

    #[test]
    fn every_footprint_in_the_big_library_reads() {
        let got = lib("DAL_LIB.PcbLib");
        assert!(got.footprints.len() >= 50, "{}", got.footprints.len());
        for f in &got.footprints {
            for p in &f.pads {
                assert!(p.size.x > 0.0 && p.size.y > 0.0 && p.size.x < 50.0 && p.size.y < 50.0, "{}: pad {} {:?}", f.name, p.num, p.size);
                assert!(p.at.x.abs() < 1000.0 && p.at.y.abs() < 1000.0, "{}: pad {} at {:?}", f.name, p.num, p.at);
            }
        }
    }
}

#[cfg(test)]
mod dump {
    use super::*;

    /// Counts a footprint's primitives by kind and layer:
    /// COPPERSMITH_LIB=file COPPERSMITH_PART=name cargo test dump_part -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_part() {
        let (Some(file), Some(part)) = (std::env::var_os("COPPERSMITH_LIB"), std::env::var("COPPERSMITH_PART").ok()) else { return };
        let bytes = std::fs::read(file).unwrap();
        let mut f = open(&bytes).unwrap();
        let data = stream(&mut f, &format!("/{part}/Data")).unwrap();
        let mut r = Reader::new(&data);
        r.block().unwrap();
        let prims = prims::parse(&data[r.i..]).unwrap();
        let mut counts: std::collections::BTreeMap<(String, u8), usize> = Default::default();
        for p in &prims {
            let (k, l) = match p {
                Prim::Pad { head, .. } => ("pad", head.layer),
                Prim::Track { head, .. } => ("track", head.layer),
                Prim::Arc { head, .. } => ("arc", head.layer),
                Prim::Via { head, .. } => ("via", head.layer),
                Prim::Region { head, .. } => ("region", head.layer),
                Prim::Fill { head, .. } => ("fill", head.layer),
                Prim::Text { head, .. } => ("text", head.layer),
                Prim::Other { kind, head } => (if *kind == 12 { "body3d" } else { "other" }, head.map(|h| h.layer).unwrap_or(0)),
            };
            *counts.entry((k.to_string(), l)).or_default() += 1;
        }
        for ((k, l), n) in counts {
            println!("{k:8} layer {l:3}  {n}");
        }
        let got = read(&bytes, "x").unwrap();
        if let Some(fp) = got.footprints.iter().find(|f| f.name == part) {
            for g in &fp.graphics {
                if let Graphic::Circle { c, r, w, layer, fill } = g {
                    println!("CIRCLE {layer:?} c=({:.3},{:.3}) r={r:.3} w={w:.3} fill={fill}", c.x, c.y);
                }
            }
        }
        for p in &prims {
            if let Prim::Region { head, pts, kind, .. } = p {
                let bb = BBox::of(pts);
                println!("region layer {} kind {kind} {} pts, x {:.2}..{:.2} y {:.2}..{:.2}", head.layer, pts.len(), bb.min.x, bb.max.x, bb.min.y, bb.max.y);
                println!("   {}", pts.iter().map(|q| format!("({:.3},{:.3})", q.x, q.y)).collect::<Vec<_>>().join(" "));
            }
            if let Prim::Track { head, a, b, w } = p {
                println!("track layer {} ({:.2},{:.2})-({:.2},{:.2}) w={w:.3}", head.layer, a.x, a.y, b.x, b.y);
            }
            if let Prim::Arc { head, c, r, start, end, w } = p {
                println!("arc layer {} c=({:.3},{:.3}) r={r:.3} {start:.1}..{end:.1} w={w:.3}", head.layer, c.x, c.y);
            }
        }
    }
}

#[cfg(test)]
mod picture {
    /// Writes every footprint of the test IntLib onto one board, for looking
    /// at: COPPERSMITH_PARTS=/path/board.cspcb cargo test parts_board -- --ignored
    #[test]
    #[ignore]
    fn parts_board() {
        use crate::pcb::geom::p;
        let Some(out) = std::env::var_os("COPPERSMITH_PARTS") else { return };
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/altium/DAL_INTLIB.IntLib");
        let got = super::super::read(&path).unwrap();
        let mut b = crate::pcb::model::Board { name: "Altium parts".into(), ..Default::default() };
        let (mut x, mut y, mut row) = (0.0, 0.0, 0.0f64);
        for d in &got.footprints {
            let bb = d.bbox();
            let (w, h) = (bb.w().max(2.0) + 4.0, bb.h().max(2.0) + 5.0);
            if x + w > 120.0 {
                x = 0.0;
                y -= row;
                row = 0.0;
            }
            let id = b.new_id();
            let at = p(x + w / 2.0 - bb.center().x, y - h / 2.0 - bb.center().y).snap(0.5);
            b.footprints.push(d.instantiate(id, d.name.clone(), at));
            x += w;
            row = row.max(h);
        }
        let ext = b.extent().grown(3.0);
        b.outline = crate::pcb::model::rect_outline(ext.min, ext.max);
        std::fs::write(out, b.save()).unwrap();
    }
}
