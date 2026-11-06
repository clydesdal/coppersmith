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

// Altium's layer numbers.
const TOP: u8 = 1;
const BOTTOM: u8 = 32;
const TOP_OVERLAY: u8 = 33;
const BOTTOM_OVERLAY: u8 = 34;
const MECHANICAL_1: u8 = 57;
const MECHANICAL_16: u8 = 72;
const MULTI_LAYER: u8 = 74;

/// Where an Altium layer's drawing goes in Coppersmith, if anywhere.
fn drawing_layer(layer: u8, names: &HashMap<u8, String>) -> Option<Layer> {
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

/// Counts of what was left out of a footprint, for the notes.
#[derive(Default)]
struct Skipped {
    copper_drawing: usize,
    fills: usize,
    regions: usize,
    octagons: usize,
    slots: usize,
    empty_pads: usize,
    other_layers: usize,
}

pub fn read(bytes: &[u8], lib: &str) -> Result<Imported, String> {
    let mut file = open(bytes)?;
    // The library's own names for its mechanical layers.
    let mut layer_names: HashMap<u8, String> = HashMap::new();
    if let Ok(head) = stream(&mut file, "/Library/Data") {
        let mut r = Reader::new(&head);
        if let Ok(text) = r.block() {
            for (k, v) in params(text) {
                if let Some(n) = k.strip_prefix("LAYER").and_then(|s| s.strip_suffix("NAME")).and_then(|n| n.parse::<u8>().ok()) {
                    layer_names.insert(n, v);
                }
            }
        }
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
            .map(|p| {
                let mut r = Reader::new(&p);
                r.block().map(params).unwrap_or_default()
            })
            .unwrap_or_default();
        let name = props.get("PATTERN").cloned().unwrap_or_else(|| storage.clone());
        let mut skipped = Skipped::default();
        match footprint(&data, &layer_names, &mut skipped) {
            Ok((pads, graphics)) => {
                if pads.is_empty() && graphics.is_empty() {
                    out.notes.push(format!("{name}: empty, left out"));
                    continue;
                }
                let mut bb = BBox::default();
                for pad in &pads {
                    bb.union(&pad.blob_local(0.0).bbox());
                }
                let label_at = if bb.is_empty() { p(0.0, 1.5) } else { p(bb.center().x, bb.max.y + 1.0).rounded() };
                note(&mut out.notes, &name, &skipped);
                out.footprints.push(FootprintDef {
                    name,
                    category: lib.to_string(),
                    description: props.get("DESCRIPTION").cloned().unwrap_or_default(),
                    prefix: "U".into(),
                    pads,
                    graphics,
                    label_at,
                    source: format!("{lib} (Altium)"),
                });
            }
            Err(e) => out.notes.push(format!("{name}: couldn't be read ({e}); left out")),
        }
    }
    Ok(out)
}

fn note(notes: &mut Vec<String>, name: &str, s: &Skipped) {
    let mut bits = Vec::new();
    if s.copper_drawing > 0 {
        bits.push(format!("{} copper tracks or arcs outside pads", s.copper_drawing));
    }
    if s.fills > 0 {
        bits.push(format!("{} filled rectangles", s.fills));
    }
    if s.regions > 0 {
        bits.push(format!("{} copper regions (custom pad shapes)", s.regions));
    }
    if s.octagons > 0 {
        bits.push(format!("{} octagonal pads made rounded", s.octagons));
    }
    if s.slots > 0 {
        bits.push(format!("{} slotted holes made round", s.slots));
    }
    if s.empty_pads > 0 {
        bits.push(format!("{} pads with no size", s.empty_pads));
    }
    if !bits.is_empty() {
        notes.push(format!("{name}: {}", bits.join(", ")));
    }
}

type Parts = (Vec<Pad>, Vec<Graphic>);

fn footprint(data: &[u8], names: &HashMap<u8, String>, skipped: &mut Skipped) -> Result<Parts, String> {
    let mut r = Reader::new(data);
    r.block()?; // the name again
    let mut pads = Vec::new();
    let mut graphics = Vec::new();
    while !r.done() {
        let kind = r.u8()?;
        match kind {
            1 => arc(r.block()?, names, &mut graphics, skipped)?,
            2 => {
                let blocks: Vec<&[u8]> = (0..6).map(|_| r.block()).collect::<Result<_, _>>()?;
                if let Some(pad) = pad(&blocks, skipped)? {
                    pads.push(pad);
                }
            }
            4 => track(r.block()?, names, &mut graphics, skipped)?,
            5 => {
                // Text: the name and value are Coppersmith's own.
                r.block()?;
                r.block()?;
            }
            6 => {
                let b = r.block()?;
                let layer = b.first().copied().unwrap_or(0);
                if layer == TOP || layer == BOTTOM || layer == MULTI_LAYER {
                    skipped.copper_drawing += 1;
                } else if drawing_layer(layer, names).is_some() {
                    skipped.fills += 1;
                }
            }
            11 => region(r.block()?, names, &mut graphics, skipped)?,
            3 | 12 => {
                // Vias and 3D bodies aren't part of a footprint here.
                r.block()?;
            }
            other => return Err(format!("a record of a kind not known ({other})")),
        }
    }
    Ok((pads, graphics))
}

fn pt(r: &mut Reader) -> Result<P, String> {
    Ok(p(r.mm()?, r.mm()?).rounded())
}

/// The fields every primitive starts with: its layer, then flags, net and
/// owner, which a library footprint doesn't need.
fn layer_of(r: &mut Reader) -> Result<u8, String> {
    let layer = r.u8()?;
    r.skip(2 + 2 + 2 + 2 + 4)?;
    Ok(layer)
}

fn track(b: &[u8], names: &HashMap<u8, String>, out: &mut Vec<Graphic>, skipped: &mut Skipped) -> Result<(), String> {
    let mut r = Reader::new(b);
    let layer = layer_of(&mut r)?;
    let (a, e) = (pt(&mut r)?, pt(&mut r)?);
    let w = r.mm()?;
    match drawing_layer(layer, names) {
        Some(l) => out.push(Graphic::Line { a, b: e, w, layer: l }),
        None if matches!(layer, TOP | BOTTOM | MULTI_LAYER) || layer < TOP_OVERLAY => skipped.copper_drawing += 1,
        None => skipped.other_layers += 1,
    }
    Ok(())
}

fn arc(b: &[u8], names: &HashMap<u8, String>, out: &mut Vec<Graphic>, skipped: &mut Skipped) -> Result<(), String> {
    let mut r = Reader::new(b);
    let layer = layer_of(&mut r)?;
    let c = pt(&mut r)?;
    let radius = r.mm()?;
    let (start, end) = (r.f64()?, r.f64()?);
    let w = r.mm()?;
    let Some(l) = drawing_layer(layer, names) else {
        if matches!(layer, TOP | BOTTOM | MULTI_LAYER) || layer < TOP_OVERLAY {
            skipped.copper_drawing += 1;
        }
        return Ok(());
    };
    let mut sweep = (end - start).rem_euclid(360.0);
    if sweep < 1e-9 {
        sweep = 360.0;
    }
    if (sweep - 360.0).abs() < 1e-9 {
        out.push(Graphic::Circle { c, r: radius, w, layer: l, fill: false });
        return Ok(());
    }
    // A part of a circle: as short lines, anticlockwise from the start.
    let n = ((sweep / 360.0) * circle_steps(radius) as f64).ceil().max(2.0) as usize;
    let at = |t: f64| c.add(p(t.to_radians().cos(), t.to_radians().sin()).scale(radius)).rounded();
    for k in 0..n {
        let (t0, t1) = (start + sweep * k as f64 / n as f64, start + sweep * (k + 1) as f64 / n as f64);
        out.push(Graphic::Line { a: at(t0), b: at(t1), w, layer: l });
    }
    Ok(())
}

/// A filled shape. On a drawing layer it's kept: a round one (like a pin 1
/// dot) as a filled dot, anything else as its outline. Copper ones, which
/// are custom pad shapes, are left out.
fn region(b: &[u8], names: &HashMap<u8, String>, out: &mut Vec<Graphic>, skipped: &mut Skipped) -> Result<(), String> {
    let mut r = Reader::new(b);
    let layer = r.u8()?;
    r.skip(1 + 1 + 2 + 2 + 2 + 5)?;
    let holes = r.u16()?;
    r.skip(2)?;
    r.block()?; // properties
    let n = r.u32()? as usize;
    let mut pts = Vec::with_capacity(n);
    for _ in 0..n {
        pts.push(p(r.f64()? * super::UNIT, r.f64()? * super::UNIT).rounded());
    }
    let _ = holes;
    let Some(l) = drawing_layer(layer, names) else {
        if matches!(layer, TOP | BOTTOM | MULTI_LAYER) {
            skipped.regions += 1;
        }
        return Ok(());
    };
    if pts.len() < 3 {
        return Ok(());
    }
    let bb = BBox::of(&pts);
    let c = bb.center();
    let radii: Vec<f64> = pts.iter().map(|q| q.dist(c)).collect();
    let (lo, hi) = radii.iter().fold((f64::MAX, 0.0f64), |(a, b), r| (a.min(*r), b.max(*r)));
    if hi > 0.0 && (hi - lo) / hi < 0.15 {
        out.push(Graphic::Circle { c, r: nm(hi), w: 0.0, layer: l, fill: true });
    } else {
        for k in 0..pts.len() {
            out.push(Graphic::Line { a: pts[k], b: pts[(k + 1) % pts.len()], w: 0.1, layer: l });
        }
    }
    Ok(())
}

fn pad(blocks: &[&[u8]], skipped: &mut Skipped) -> Result<Option<Pad>, String> {
    let num = {
        let mut r = Reader::new(blocks[0]);
        r.pascal()?
    };
    let mut r = Reader::new(blocks[4]);
    let layer = layer_of(&mut r)?;
    let at = pt(&mut r)?;
    let top = p(r.mm()?, r.mm()?);
    r.skip(16)?; // middle and bottom sizes
    let hole = r.mm()?;
    let shape = r.u8()?;
    r.skip(2)?; // middle and bottom shapes
    let rot = r.f64()?;
    let plated = r.u8()? != 0;
    // The rest of the stack, masks and so on: not needed for the shape.

    // Pads on one side of the board only, or through it.
    let smd = match layer {
        TOP => true,
        MULTI_LAYER => false,
        BOTTOM => true,
        _ => return Ok(None),
    };
    // The second, longer description: rounded rectangles and slots.
    let (mut round_ratio, mut slot) = (None, false);
    if blocks[5].len() >= 596 {
        let mut s = Reader::new(blocks[5]);
        s.skip(29 * 4 * 2 + 29 + 1)?;
        let hole_shape = s.u8()?;
        slot = hole_shape == 2;
        s.skip(4 + 8 + 32 * 4 * 2 + 1)?;
        let alt = s.bytes(32)?;
        let corner = s.bytes(32)?;
        if alt[0] == 9 {
            // A share of the shorter side's half: Coppersmith keeps a share of the side.
            round_ratio = Some(corner[0].min(100) as f64 / 200.0);
        }
    }
    if slot && layer == MULTI_LAYER && hole > 0.0 {
        skipped.slots += 1;
    }
    if top.x <= 0.0 || top.y <= 0.0 {
        // No copper on the top of the stack: a placeholder.
        skipped.empty_pads += 1;
        return Ok(None);
    }
    let pad_shape = match (shape, round_ratio) {
        (_, Some(k)) => PadShape::RoundRect(k),
        (1, _) if (top.x - top.y).abs() < 1e-6 => PadShape::Circle,
        (1, _) => PadShape::Oval,
        (2, _) => PadShape::Rect,
        (3, _) => {
            skipped.octagons += 1;
            PadShape::RoundRect(0.25)
        }
        _ => PadShape::Rect,
    };
    let mut size = p(nm(top.x), nm(top.y));
    let drill = if smd { 0.0 } else { nm(hole) };
    // A hole with no plating has no copper round it.
    if !smd && !plated {
        size = p(drill, drill);
    }
    Ok(Some(Pad { num, at, size, rot: nm(rot), shape: pad_shape, drill, net: 0 }))
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
