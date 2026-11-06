//! The binary drawing records shared by PcbLib footprints and PcbDoc boards:
//! a type byte, then length-prefixed blocks. Positions come out in
//! millimetres, as Altium stores them (not yet moved to any origin).

use super::{Reader, UNIT};
use crate::pcb::geom::*;
use crate::pcb::model::*;

// Altium's layer numbers.
pub const TOP: u8 = 1;
pub const BOTTOM: u8 = 32;
pub const TOP_OVERLAY: u8 = 33;
pub const BOTTOM_OVERLAY: u8 = 34;
pub const MECHANICAL_1: u8 = 57;
pub const MECHANICAL_16: u8 = 72;
pub const MULTI_LAYER: u8 = 74;

/// No net, no part, no polygon.
pub const NONE: u16 = 0xffff;

pub fn is_copper(layer: u8) -> bool {
    (TOP..=BOTTOM).contains(&layer) || layer == MULTI_LAYER
}

/// What every primitive says about itself.
#[derive(Clone, Copy, Debug)]
pub struct Head {
    pub layer: u8,
    pub net: u16,
    pub polygon: u16,
    pub component: u16,
}

// Fills and other kinds are kept for when they're imported; noted for now.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum Prim {
    /// A pad, with its centre where Altium has it.
    Pad { head: Head, pad: Pad, slot: bool, octagon: bool },
    Track { head: Head, a: P, b: P, w: f64 },
    /// Angles in degrees, anticlockwise from `start` to `end`.
    Arc { head: Head, c: P, r: f64, start: f64, end: f64, w: f64 },
    Via { head: Head, at: P, dia: f64, hole: f64 },
    Region { head: Head, pts: Vec<P> },
    Fill { head: Head, a: P, b: P, rot: f64 },
    /// Text, 3D bodies and the like: counted, not drawn.
    Other { kind: u8, head: Option<Head> },
}

fn head(r: &mut Reader) -> Result<Head, String> {
    let layer = r.u8()?;
    r.skip(2)?; // flags
    let net = r.u16()?;
    let polygon = r.u16()?;
    let component = r.u16()?;
    r.skip(4)?;
    Ok(Head { layer, net, polygon, component })
}

fn pt(r: &mut Reader) -> Result<P, String> {
    Ok(p(r.mm()?, r.mm()?))
}

/// Reads records until the data runs out.
pub fn parse(data: &[u8]) -> Result<Vec<Prim>, String> {
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    while !r.done() {
        let kind = r.u8()?;
        let prim = match kind {
            1 => arc(r.block()?)?,
            2 => {
                let blocks: Vec<&[u8]> = (0..6).map(|_| r.block()).collect::<Result<_, _>>()?;
                pad(&blocks)?
            }
            3 => via(r.block()?)?,
            4 => track(r.block()?)?,
            5 => {
                let b = r.block()?;
                r.block()?;
                Prim::Other { kind, head: head(&mut Reader::new(b)).ok() }
            }
            6 => fill(r.block()?)?,
            11 => region(r.block()?)?,
            12 => {
                let b = r.block()?;
                Prim::Other { kind, head: head(&mut Reader::new(b)).ok() }
            }
            other => return Err(format!("a record of a kind not known ({other})")),
        };
        out.push(prim);
    }
    Ok(out)
}

fn track(b: &[u8]) -> Result<Prim, String> {
    let mut r = Reader::new(b);
    let head = head(&mut r)?;
    let (a, e) = (pt(&mut r)?, pt(&mut r)?);
    let w = r.mm()?;
    Ok(Prim::Track { head, a, b: e, w })
}

fn arc(b: &[u8]) -> Result<Prim, String> {
    let mut r = Reader::new(b);
    let head = head(&mut r)?;
    let c = pt(&mut r)?;
    let radius = r.mm()?;
    let (start, end) = (r.f64()?, r.f64()?);
    let w = r.mm()?;
    Ok(Prim::Arc { head, c, r: radius, start, end, w })
}

fn via(b: &[u8]) -> Result<Prim, String> {
    let mut r = Reader::new(b);
    r.skip(3)?; // layer and flags
    let net = r.u16()?;
    r.skip(8)?;
    let at = pt(&mut r)?;
    let dia = r.mm()?;
    let hole = r.mm()?;
    Ok(Prim::Via { head: Head { layer: MULTI_LAYER, net, polygon: NONE, component: NONE }, at, dia, hole })
}

fn fill(b: &[u8]) -> Result<Prim, String> {
    let mut r = Reader::new(b);
    let head = head(&mut r)?;
    let (a, e) = (pt(&mut r)?, pt(&mut r)?);
    let rot = r.f64()?;
    Ok(Prim::Fill { head, a, b: e, rot })
}

fn region(b: &[u8]) -> Result<Prim, String> {
    let mut r = Reader::new(b);
    let layer = r.u8()?;
    r.skip(2)?;
    let net = r.u16()?;
    let polygon = r.u16()?;
    let component = r.u16()?;
    r.skip(5)?;
    r.u16()?; // holes
    r.skip(2)?;
    r.block()?; // properties
    let n = r.u32()? as usize;
    // Two layouts: plain corners as two doubles each; or, in shape-based
    // regions, an extra closing corner and each corner as a flag, integer
    // point, centre, radius and two angles (37 bytes).
    let rest = r.left();
    let extended = rest != n * 16 && rest >= (n + 1) * 37;
    let mut pts = Vec::with_capacity(n.min(100_000));
    if extended {
        for _ in 0..n + 1 {
            r.u8()?;
            let q = p(r.mm()?, r.mm()?);
            r.skip(4 + 4 + 4 + 8 + 8)?;
            pts.push(q);
        }
    } else {
        for _ in 0..n {
            pts.push(p(r.f64()? * UNIT, r.f64()? * UNIT));
        }
    }
    // Anything not a sensible number (or further than ten metres out) is a
    // misread, not a shape.
    pts.retain(|q| q.x.is_finite() && q.y.is_finite() && q.x.abs() < 1e4 && q.y.abs() < 1e4);
    Ok(Prim::Region { head: Head { layer, net, polygon, component }, pts })
}

fn pad(blocks: &[&[u8]]) -> Result<Prim, String> {
    let num = Reader::new(blocks[0]).pascal()?;
    let mut r = Reader::new(blocks[4]);
    let layer = r.u8()?;
    r.skip(2)?;
    let net = r.u16()?;
    r.skip(2)?;
    let component = r.u16()?;
    r.skip(4)?;
    let head = Head { layer, net, polygon: NONE, component };
    let at = pt(&mut r)?;
    let top = p(r.mm()?, r.mm()?);
    r.skip(16)?; // middle and bottom sizes
    let hole = r.mm()?;
    let shape = r.u8()?;
    r.skip(2)?;
    let rot = r.f64()?;
    let plated = r.u8()? != 0;
    let (mut round_ratio, mut slot) = (None, false);
    if blocks[5].len() >= 596 {
        let mut s = Reader::new(blocks[5]);
        s.skip(29 * 4 * 2 + 29 + 1)?;
        slot = s.u8()? == 2;
        s.skip(4 + 8 + 32 * 4 * 2 + 1)?;
        let alt = s.bytes(32)?;
        let corner = s.bytes(32)?;
        if alt[0] == 9 {
            round_ratio = Some(corner[0].min(100) as f64 / 200.0);
        }
    }
    let through = layer == MULTI_LAYER;
    let octagon = shape == 3 && round_ratio.is_none();
    let pad_shape = match (shape, round_ratio) {
        (_, Some(k)) => PadShape::RoundRect(k),
        (1, _) if (top.x - top.y).abs() < 1e-6 => PadShape::Circle,
        (1, _) => PadShape::Oval,
        (3, _) => PadShape::RoundRect(0.25),
        _ => PadShape::Rect,
    };
    let drill = if through { nm(hole) } else { 0.0 };
    let mut size = p(nm(top.x), nm(top.y));
    if through && !plated {
        size = p(drill, drill);
    }
    let pad = Pad { num, at, size, rot: nm(rot), shape: pad_shape, drill, net: 0 };
    Ok(Prim::Pad { head, pad, slot: slot && through && hole > 0.0, octagon })
}

/// An arc's start, a point on the way, and end: a Coppersmith arc track.
pub fn arc_points(c: P, r: f64, start: f64, end: f64) -> (P, P, P) {
    let mut sweep = (end - start).rem_euclid(360.0);
    if sweep < 1e-9 {
        sweep = 360.0;
    }
    let at = |t: f64| c.add(p(t.to_radians().cos(), t.to_radians().sin()).scale(r)).rounded();
    (at(start), at(start + sweep / 2.0), at(start + sweep))
}

/// An arc as short lines, for drawing layers.
pub fn arc_lines(c: P, r: f64, start: f64, end: f64) -> Vec<(P, P)> {
    let mut sweep = (end - start).rem_euclid(360.0);
    if sweep < 1e-9 {
        sweep = 360.0;
    }
    let n = ((sweep / 360.0) * circle_steps(r) as f64).ceil().max(2.0) as usize;
    let at = |t: f64| c.add(p(t.to_radians().cos(), t.to_radians().sin()).scale(r)).rounded();
    (0..n).map(|k| (at(start + sweep * k as f64 / n as f64), at(start + sweep * (k + 1) as f64 / n as f64))).collect()
}
