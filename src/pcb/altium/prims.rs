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
/// Altium's Keep-Out layer: what's drawn there is kept free on every layer.
pub const KEEPOUT_LAYER: u8 = 56;
/// The flag bit that makes a primitive on a copper layer a keepout.
pub const KEEPOUT_FLAG: u16 = 0x0200;

/// No net, no part, no polygon.
pub const NONE: u16 = 0xffff;

pub fn is_copper(layer: u8) -> bool {
    (TOP..=BOTTOM).contains(&layer) || layer == MULTI_LAYER
}

/// What every primitive says about itself.
#[derive(Clone, Copy, Debug)]
pub struct Head {
    pub layer: u8,
    /// Altium's flag bits for the primitive (locked, tenting, keepout…).
    pub flags: u16,
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
    /// `kind` 0 is copper, 1 a pour cutout, 2 a dashed outline, 3 a cavity;
    /// `cutout` marks a hole in the board.
    Region { head: Head, pts: Vec<P>, kind: u8, cutout: bool, restrict: u32 },
    Fill { head: Head, a: P, b: P, rot: f64 },
    /// Text, 3D bodies and the like: counted, not drawn.
    Other { kind: u8, head: Option<Head> },
}

/// A keepout as read: Altium's layer, its outline in whatever coordinates the
/// reader gave it, and Altium's restriction bits.
#[derive(Clone, Debug)]
pub struct RawKeepout {
    pub layer: u8,
    pub pts: Vec<P>,
    pub restrict: u32,
}

/// The keepout a primitive makes, if it is one: a fill, region or track
/// flagged as a keepout, or on the Keep-Out layer. (Arcs aren't taken.)
pub fn keepout_of(prim: &Prim) -> Option<RawKeepout> {
    let is = |h: &Head| h.flags & KEEPOUT_FLAG != 0 || h.layer == KEEPOUT_LAYER;
    match prim {
        Prim::Fill { head, a, b, rot } if is(head) => {
            let pts = rect_core(a.lerp(*b, 0.5), (b.x - a.x).abs(), (b.y - a.y).abs(), *rot);
            Some(RawKeepout { layer: head.layer, pts, restrict: 31 })
        }
        Prim::Region { head, pts, kind: 0, restrict, .. } if is(head) && pts.len() >= 3 => {
            Some(RawKeepout { layer: head.layer, pts: pts.clone(), restrict: *restrict })
        }
        Prim::Track { head, a, b, w } if is(head) && *w > 0.0 => {
            Some(RawKeepout { layer: head.layer, pts: Blob::line(*a, *b, *w).ring(), restrict: 31 })
        }
        _ => None,
    }
}

/// Is this a keepout that can't be taken, an arc?
pub fn is_keepout_arc(prim: &Prim) -> bool {
    matches!(prim, Prim::Arc { head, .. } if head.flags & KEEPOUT_FLAG != 0 || head.layer == KEEPOUT_LAYER)
}

/// A keepout in Coppersmith's terms, as it's imported: it keeps pours out and
/// nothing else (Altium leaves the restrictions at "everything" on keepouts
/// drawn only to stop a pour flooding in, and tracks and vias are wanted
/// there). One whose restrictions leave pours out, which wouldn't have stopped
/// a pour in Altium either, isn't taken. `copper_of` turns Altium's layer into a
/// copper layer of the board (none for a layer that isn't copper); `under` says
/// the keepout is on a part that's underneath, whose top and bottom swap.
/// None when it's on a layer that isn't copper.
pub fn to_keepout(raw: RawKeepout, copper_of: impl Fn(u8) -> Option<u8>, under: bool) -> Option<Keepout> {
    if !Restrict::from_altium(raw.restrict).pours {
        return None;
    }
    let layer = if raw.layer == MULTI_LAYER || raw.layer == KEEPOUT_LAYER {
        None
    } else {
        let l = copper_of(raw.layer)?;
        Some(match (under, l) {
            (true, crate::pcb::model::TOP) => crate::pcb::model::BOTTOM,
            (true, crate::pcb::model::BOTTOM) => crate::pcb::model::TOP,
            (_, l) => l,
        })
    };
    let mut pts = dedup_corners(raw.pts);
    if pts.len() < 3 {
        return None;
    }
    if ring_area(&pts) < 0.0 {
        pts.reverse();
    }
    Some(Keepout { id: 0, layer, pts, restrict: Restrict::pours_only() })
}

/// A ring without corners repeated one after another, or the first again at the end.
fn dedup_corners(mut pts: Vec<P>) -> Vec<P> {
    pts.dedup_by(|a, b| a.same(*b));
    while pts.len() > 1 && pts[0].same(pts[pts.len() - 1]) {
        pts.pop();
    }
    pts
}

fn head(r: &mut Reader) -> Result<Head, String> {
    let layer = r.u8()?;
    let flags = r.u16()?;
    let net = r.u16()?;
    let polygon = r.u16()?;
    let component = r.u16()?;
    r.skip(4)?;
    Ok(Head { layer, flags, net, polygon, component })
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
    r.skip(1)?; // layer
    let flags = r.u16()?;
    let net = r.u16()?;
    r.skip(8)?;
    let at = pt(&mut r)?;
    let dia = r.mm()?;
    let hole = r.mm()?;
    Ok(Prim::Via { head: Head { layer: MULTI_LAYER, flags, net, polygon: NONE, component: NONE }, at, dia, hole })
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
    let flags = r.u16()?;
    let net = r.u16()?;
    let polygon = r.u16()?;
    let component = r.u16()?;
    r.skip(5)?;
    r.u16()?; // holes
    r.skip(2)?;
    let props = super::params(r.block()?);
    let kind = props.get("KIND").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let cutout = props.get("ISBOARDCUTOUT").is_some_and(|v| v.trim() == "TRUE");
    // What a keepout region keeps out: all, unless it says.
    let restrict = props.get("KEEPOUTRESTRICTIONS").and_then(|v| v.trim().parse().ok()).unwrap_or(31);
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
    Ok(Prim::Region { head: Head { layer, flags, net, polygon, component }, pts, kind, cutout, restrict })
}

fn pad(blocks: &[&[u8]]) -> Result<Prim, String> {
    let num = Reader::new(blocks[0]).pascal()?;
    let mut r = Reader::new(blocks[4]);
    let layer = r.u8()?;
    let flags = r.u16()?;
    let net = r.u16()?;
    r.skip(2)?;
    let component = r.u16()?;
    r.skip(4)?;
    let head = Head { layer, flags, net, polygon: NONE, component };
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

#[cfg(test)]
mod keepout_tests {
    use super::*;

    fn head(layer: u8, flags: u16) -> Head {
        Head { layer, flags, net: NONE, polygon: NONE, component: NONE }
    }

    fn on_the_board(l: u8) -> Option<u8> {
        match l {
            TOP => Some(crate::pcb::model::TOP),
            BOTTOM => Some(crate::pcb::model::BOTTOM),
            2 => Some(1),
            _ => None,
        }
    }

    #[test]
    fn a_flagged_fill_is_a_keepout_and_an_unflagged_one_is_not() {
        let fill = |flags| Prim::Fill { head: head(TOP, flags), a: p(0.0, 0.0), b: p(4.0, 2.0), rot: 0.0 };
        let k = keepout_of(&fill(0x020c)).expect("flagged");
        assert_eq!(k.layer, TOP);
        assert_eq!(k.restrict, 31, "a fill keeps everything out");
        let bb = BBox::of(&k.pts);
        assert!((bb.w() - 4.0).abs() < 1e-9 && (bb.h() - 2.0).abs() < 1e-9, "{bb:?}");
        assert!(keepout_of(&fill(0x000c)).is_none(), "an ordinary fill is copper");
    }

    #[test]
    fn a_region_keeps_its_restrictions_and_only_plain_ones_count() {
        let square = vec![p(0.0, 0.0), p(2.0, 0.0), p(2.0, 2.0), p(0.0, 2.0)];
        let region = |flags, kind, restrict| Prim::Region { head: head(TOP, flags), pts: square.clone(), kind, cutout: false, restrict };
        assert_eq!(keepout_of(&region(0x020c, 0, 6)).unwrap().restrict, 6);
        // One that doesn't keep pours out (vias and tracks only: 1 + 2) isn't taken.
        assert!(to_keepout(keepout_of(&region(0x020c, 0, 3)).unwrap(), on_the_board, false).is_none());
        // One that does (tracks and copper: 2 + 4) is, as a keepout for pours.
        assert!(to_keepout(keepout_of(&region(0x020c, 0, 6)).unwrap(), on_the_board, false).is_some());
        assert!(keepout_of(&region(0x000c, 0, 31)).is_none());
        assert!(keepout_of(&region(0x020c, 1, 31)).is_none(), "a pour cutout is not a keepout");
        // The bits are read; imported, a keepout only keeps pours out.
        let k = to_keepout(keepout_of(&region(0x020c, 0, 31)).unwrap(), on_the_board, false).unwrap();
        assert_eq!(k.restrict, Restrict::pours_only());
        assert!(!k.restrict.vias && !k.restrict.tracks && !k.restrict.smd_pads && !k.restrict.th_pads);
    }

    #[test]
    fn a_keepout_track_is_a_strip_as_wide_as_the_track() {
        let t = Prim::Track { head: head(TOP, 0x020c), a: p(0.0, 0.0), b: p(10.0, 0.0), w: 1.0 };
        let bb = BBox::of(&keepout_of(&t).unwrap().pts);
        assert!((bb.h() - 1.0).abs() < 0.05 && (bb.w() - 11.0).abs() < 0.05, "{bb:?}");
        // On the Keep-Out layer a track is a keepout whatever its flags say.
        assert!(keepout_of(&Prim::Track { head: head(KEEPOUT_LAYER, 0), a: p(0.0, 0.0), b: p(1.0, 0.0), w: 0.2 }).is_some());
        assert!(keepout_of(&Prim::Track { head: head(TOP, 0x000c), a: p(0.0, 0.0), b: p(1.0, 0.0), w: 0.2 }).is_none());
    }

    #[test]
    fn layers_map_to_the_board_and_swap_on_a_part_underneath() {
        let raw = |layer| RawKeepout { layer, pts: vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)], restrict: 31 };
        assert_eq!(to_keepout(raw(TOP), on_the_board, false).unwrap().layer, Some(crate::pcb::model::TOP));
        assert_eq!(to_keepout(raw(BOTTOM), on_the_board, false).unwrap().layer, Some(crate::pcb::model::BOTTOM));
        assert_eq!(to_keepout(raw(2), on_the_board, false).unwrap().layer, Some(1), "an inner layer");
        // On a part underneath, the board's bottom is the part's top.
        assert_eq!(to_keepout(raw(BOTTOM), on_the_board, true).unwrap().layer, Some(crate::pcb::model::TOP));
        assert_eq!(to_keepout(raw(2), on_the_board, true).unwrap().layer, Some(1), "inner layers stay");
        // Every layer: the Keep-Out layer, and multi-layer.
        assert_eq!(to_keepout(raw(KEEPOUT_LAYER), on_the_board, false).unwrap().layer, None);
        assert_eq!(to_keepout(raw(MULTI_LAYER), on_the_board, true).unwrap().layer, None);
        // A layer that isn't copper can't hold one.
        assert!(to_keepout(raw(MECHANICAL_1), on_the_board, false).is_none());
    }

    #[test]
    fn corners_are_taken_counter_clockwise_and_degenerate_ones_refused() {
        let cw = RawKeepout { layer: TOP, pts: vec![p(0.0, 0.0), p(0.0, 2.0), p(2.0, 2.0), p(2.0, 0.0), p(0.0, 0.0)], restrict: 31 };
        let k = to_keepout(cw, on_the_board, false).unwrap();
        assert_eq!(k.pts.len(), 4, "the closing corner isn't doubled");
        assert!(ring_area(&k.pts) > 0.0);
        let line = RawKeepout { layer: TOP, pts: vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 0.0)], restrict: 31 };
        assert!(to_keepout(line, on_the_board, false).is_none());
    }

    #[test]
    fn arcs_flagged_as_keepouts_are_recognised_as_not_taken() {
        let arc = |flags| Prim::Arc { head: head(TOP, flags), c: p(0.0, 0.0), r: 1.0, start: 0.0, end: 90.0, w: 0.2 };
        assert!(is_keepout_arc(&arc(0x020c)));
        assert!(!is_keepout_arc(&arc(0x000c)));
        assert!(keepout_of(&arc(0x020c)).is_none());
    }
}
