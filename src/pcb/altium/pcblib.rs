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
        let (pads, graphics) = footprint(&prims, &names, &mut skipped, |q| q, 0.0, false);
        if pads.is_empty() && graphics.is_empty() {
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
) -> (Vec<Pad>, Vec<Graphic>) {
    let mut pads = Vec::new();
    let mut graphics = Vec::new();
    let layer_for = |l: u8| drawing_layer(l, names).map(|g| if bottom { g.flipped() } else { g });
    let turn = |r: f64| nm(if bottom { rot - r } else { r - rot }.rem_euclid(360.0));
    for prim in prims {
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
                    if hi > 0.0 && (hi - lo) / hi < 0.15 {
                        graphics.push(Graphic::Circle { c, r: nm(hi), w: 0.0, layer: l, fill: true });
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
            Prim::Via { .. } | Prim::Other { .. } => {}
        }
    }
    (pads, graphics)
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
