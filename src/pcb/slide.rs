//! Moving tracks without losing their corners.
//!
//! On a board laid at 45°, moving a track, its end, or the part it's joined
//! to shouldn't leave tracks at odd angles. These keep each track's direction
//! and let its neighbours grow or shrink to meet it, as Altium's sliding does;
//! where that can't work they add a short 45° leg instead. Tracks that weren't
//! at a multiple of 45° to begin with are left to stretch as before, so
//! any-angle work stays as it was drawn.

use crate::pcb::geom::*;
use crate::pcb::model::*;
use crate::pcb::router::{Corner, legs};
use std::collections::HashSet;

/// Runs at a multiple of 45°.
pub fn octilinear(d: P) -> bool {
    let (x, y) = (d.x.abs(), d.y.abs());
    let l = d.len();
    l > 1e-9 && (x < 1e-6 * l.max(1.0) || y < 1e-6 * l.max(1.0) || (x - y).abs() < 1e-6 * l.max(1.0))
}

fn straight45(t: &Track) -> bool {
    t.mid.is_none() && octilinear(t.b.sub(t.a))
}

/// Where the line through `p1` along `d1` meets the line through `p2` along `d2`.
fn meet(p1: P, d1: P, p2: P, d2: P) -> Option<P> {
    let den = d1.cross(d2);
    if den.abs() < 1e-12 {
        return None;
    }
    let t = p2.sub(p1).cross(d2) / den;
    Some(p1.add(d1.scale(t)).rounded())
}

/// Something fixed sits here: a pad or a via.
fn pinned(board: &Board, q: P) -> bool {
    board.vias.iter().any(|v| v.at.same(q))
        || board.footprints.iter().any(|f| (0..f.pads.len()).any(|i| f.pad_center(i).same(q)))
}

/// The other tracks with an end at `q`, on `layer`.
fn others_at(board: &Board, q: P, layer: u8, not: u32) -> Vec<usize> {
    board.tracks.iter().enumerate().filter(|(_, t)| t.id != not && t.layer == layer && (t.a.same(q) || t.b.same(q))).map(|(i, _)| i).collect()
}

fn far_end(t: &Track, near: P) -> P {
    if t.a.same(near) { t.b } else { t.a }
}

fn set_end(t: &mut Track, old: P, new: P) {
    let a = if t.a.same(old) { new } else { t.a };
    let b = if t.b.same(old) { new } else { t.b };
    t.set_ends(a, b);
}

fn add_track(board: &mut Board, like: &Track, a: P, b: P) {
    if a.same(b) {
        return;
    }
    let id = board.new_id();
    board.tracks.push(Track { mid: None, id, a, b, w: like.w, layer: like.layer, net: like.net });
}

/// Takes out tracks with no length left.
pub fn tidy(board: &mut Board) {
    board.tracks.retain(|t| !t.a.same(t.b));
}

/// Slides a straight track sideways by `delta` (only the part square to it
/// counts), keeping its direction and its neighbours'. Changes nothing and
/// returns false if the track isn't straight or would turn inside out.
pub fn slide(board: &mut Board, id: u32, delta: P) -> bool {
    let Some(t) = board.track(id).cloned() else { return false };
    if t.mid.is_some() {
        return false;
    }
    let along = t.b.sub(t.a);
    let len = along.len();
    if len < 1e-9 {
        return false;
    }
    let u = along.scale(1.0 / len);
    let n = p(-u.y, u.x);
    let off = n.scale(delta.dot(n));
    if off.len() < 1e-9 {
        return true;
    }
    let keep45 = straight45(&t);
    let mut work = board.clone();
    let mut ends = [t.a, t.b];
    for (k, end) in [t.a, t.b].into_iter().enumerate() {
        let moved = end.add(off).rounded();
        let neighbours = others_at(&work, end, t.layer, id);
        let fixed = pinned(&work, end);
        if neighbours.is_empty() && !fixed {
            // A loose end just goes with it.
            ends[k] = moved;
            continue;
        }
        // One straight neighbour, nothing pinned: it grows or shrinks along
        // its own line to meet the track where it now is.
        if !fixed && neighbours.len() == 1 && work.tracks[neighbours[0]].mid.is_none() {
            let nb = work.tracks[neighbours[0]].clone();
            let far = far_end(&nb, end);
            let v = end.sub(far);
            if let Some(j) = meet(moved, u, far, v)
                && j.sub(far).dot(v) > 1e-6
            {
                set_end(&mut work.tracks[neighbours[0]], end, j);
                ends[k] = j;
                continue;
            }
        }
        if !keep45 {
            // Not a 45° track: the end stays and the track leans.
            continue;
        }
        // Otherwise the end stays where it is, and a 45° leg joins it to
        // the track's new line, heading in towards the track's middle.
        let inward = if k == 0 { u } else { u.scale(-1.0) };
        let side = off.scale(1.0 / off.len());
        let diag = inward.add(side).scale(1.0 / 2f64.sqrt());
        let q = end.add(diag.scale(off.len() * 2f64.sqrt())).rounded();
        add_track(&mut work, &t, end, q);
        ends[k] = q;
    }
    // It mustn't turn round or vanish.
    if ends[1].sub(ends[0]).dot(u) <= 1e-6 {
        return false;
    }
    if let Some(s) = work.tracks.iter_mut().find(|x| x.id == id) {
        s.a = ends[0];
        s.b = ends[1];
    }
    tidy(&mut work);
    *board = work;
    true
}

/// Re-lays a track from its far end `from` to `to` as a straight leg and a
/// 45° leg, starting off the way it used to where it can.
fn relay(board: &mut Board, i: usize, from: P, to: P) {
    let t = board.tracks[i].clone();
    let was = far_end(&t, from).sub(from);
    let options = [legs(from, to, Corner::D45, false), legs(from, to, Corner::D45, true)];
    let same_way = |l: &Vec<P>| l.len() > 1 && l[1].sub(l[0]).cross(was).abs() < 1e-6 * was.len().max(1.0) && l[1].sub(l[0]).dot(was) > 0.0;
    let pick = options.iter().find(|l| same_way(l)).unwrap_or(&options[0]).clone();
    board.tracks.remove(i);
    for s in pick.windows(2) {
        add_track(board, &t, s[0], s[1]);
    }
}

/// After pads, vias or tracks have moved (`moves` is old → new for each
/// point that moved), brings the ends of the other tracks along without
/// bending 45° work out of shape. `skip` are tracks that moved bodily.
pub fn follow_moves(board: &mut Board, moves: &[(P, P)], skip: &HashSet<u32>) {
    let moved_to = |q: P| moves.iter().find(|m| m.0.same(q)).map(|m| m.1);
    let ids: Vec<u32> = board.tracks.iter().filter(|t| !skip.contains(&t.id)).map(|t| t.id).collect();
    for id in ids {
        let Some(i) = board.tracks.iter().position(|t| t.id == id) else { continue };
        let t = board.tracks[i].clone();
        let (na, nb) = (moved_to(t.a), moved_to(t.b));
        match (na, nb) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                // Both ends moved together: the track goes with them.
                board.tracks[i].set_ends(a, b);
            }
            (Some(_), None) | (None, Some(_)) => {
                let (old, new) = if let Some(a) = na { (t.a, a) } else { (t.b, nb.unwrap_or(t.b)) };
                let other = far_end(&t, old);
                if !straight45(&t) {
                    set_end(&mut board.tracks[i], old, new);
                    continue;
                }
                let u = other.sub(old);
                // The joint at the other end slides along the next track.
                let next = others_at(board, other, t.layer, t.id);
                let mut done = false;
                if next.len() == 1 && !pinned(board, other) && moved_to(other).is_none() {
                    let j = next[0];
                    let nb_t = board.tracks[j].clone();
                    if nb_t.mid.is_none() && !skip.contains(&nb_t.id) {
                        let far = far_end(&nb_t, other);
                        let v = other.sub(far);
                        if let Some(q) = meet(new, u, far, v)
                            && q.sub(far).dot(v) > 1e-6
                            && q.sub(new).dot(u) > 1e-6
                        {
                            set_end(&mut board.tracks[j], other, q);
                            board.tracks[i].a = new;
                            board.tracks[i].b = q;
                            done = true;
                        }
                    }
                }
                if !done {
                    relay(board, i, other, new);
                }
            }
        }
    }
    tidy(board);
}

/// Drags the point where tracks meet from `from` to `to`, keeping 45° work
/// at 45°: each track there is re-laid from its far end.
pub fn drag_joint(board: &mut Board, from: P, to: P) {
    let at: Vec<u32> = board.tracks.iter().filter(|t| t.a.same(from) || t.b.same(from)).map(|t| t.id).collect();
    for id in at {
        let Some(i) = board.tracks.iter().position(|t| t.id == id) else { continue };
        let t = board.tracks[i].clone();
        if straight45(&t) {
            let far = far_end(&t, from);
            relay(board, i, far, to);
        } else {
            set_end(&mut board.tracks[i], from, to);
        }
    }
    for v in &mut board.vias {
        if v.at.same(from) {
            v.at = to;
        }
    }
    tidy(board);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(b: &mut Board, a: (f64, f64), e: (f64, f64)) -> u32 {
        let id = b.new_id();
        b.tracks.push(Track { mid: None, id, a: p(a.0, a.1), b: p(e.0, e.1), w: 0.25, layer: TOP, net: 1 });
        id
    }

    fn all_45(b: &Board) -> bool {
        b.tracks.iter().all(|t| octilinear(t.b.sub(t.a)))
    }

    fn joined(b: &Board, q: P) -> usize {
        b.tracks.iter().filter(|t| t.a.same(q) || t.b.same(q)).count()
    }

    #[test]
    fn sliding_a_track_keeps_its_neighbours_at_45() {
        // A run: up at 45°, along, down at 45°.
        let mut b = Board::default();
        track(&mut b, (0.0, 0.0), (2.0, 2.0));
        let mid = track(&mut b, (2.0, 2.0), (8.0, 2.0));
        track(&mut b, (8.0, 2.0), (10.0, 0.0));
        assert!(slide(&mut b, mid, p(0.3, 1.0)));
        let m = b.track(mid).unwrap();
        assert!(m.a.same(p(3.0, 3.0)) && m.b.same(p(7.0, 3.0)), "{m:?}");
        assert!(all_45(&b));
        assert_eq!(b.tracks.len(), 3);
        // The far ends haven't moved.
        assert_eq!(joined(&b, p(0.0, 0.0)), 1);
        assert_eq!(joined(&b, p(10.0, 0.0)), 1);
    }

    #[test]
    fn sliding_off_a_pad_adds_a_45_leg() {
        let mut b = crate::pcb::sample::board();
        // The LED track runs straight down from R3 pad 2 to D1 pad 2.
        let id = b.tracks.iter().find(|t| t.a.same(p(16.9125, 22.0))).unwrap().id;
        assert!(slide(&mut b, id, p(1.0, 0.0)));
        let led: Vec<&Track> = b.tracks.iter().filter(|t| t.net == b.net_by_name("LED").unwrap()).collect();
        assert_eq!(led.len(), 3, "{led:?}");
        assert!(led.iter().all(|t| octilinear(t.b.sub(t.a))));
        assert_eq!(joined(&b, p(16.9125, 22.0)), 1, "still joined to R3");
        assert_eq!(joined(&b, p(16.9125, 27.0)), 1, "still joined to D1");
    }

    #[test]
    fn a_part_moving_keeps_its_tracks_at_45() {
        let mut b = Board::default();
        // Pad at the origin; a track out to the right, then up at 45°.
        track(&mut b, (0.0, 0.0), (5.0, 0.0));
        track(&mut b, (5.0, 0.0), (8.0, 3.0));
        // The pad moves up by 1: the first track stays level, and the joint
        // slides up the 45° track to meet it.
        follow_moves(&mut b, &[(p(0.0, 0.0), p(0.0, 1.0))], &HashSet::new());
        assert!(all_45(&b), "{:?}", b.tracks);
        assert!(b.tracks.iter().any(|t| t.a.same(p(0.0, 1.0)) && t.b.same(p(6.0, 1.0))), "{:?}", b.tracks);
        assert_eq!(joined(&b, p(8.0, 3.0)), 1);
    }

    #[test]
    fn a_lone_track_is_relaid_in_45_legs() {
        let mut b = Board::default();
        track(&mut b, (0.0, 0.0), (5.0, 0.0));
        follow_moves(&mut b, &[(p(0.0, 0.0), p(0.0, 1.0))], &HashSet::new());
        assert!(all_45(&b));
        assert!(b.tracks.len() == 2, "{:?}", b.tracks);
        assert_eq!(joined(&b, p(0.0, 1.0)), 1);
        assert_eq!(joined(&b, p(5.0, 0.0)), 1);
    }

    #[test]
    fn any_angle_tracks_just_stretch() {
        let mut b = Board::default();
        let id = track(&mut b, (0.0, 0.0), (5.0, 1.0));
        follow_moves(&mut b, &[(p(0.0, 0.0), p(0.0, 2.0))], &HashSet::new());
        assert_eq!(b.tracks.len(), 1);
        assert!(b.track(id).unwrap().a.same(p(0.0, 2.0)));
    }

    #[test]
    fn dragging_a_corner_keeps_45() {
        let mut b = Board::default();
        track(&mut b, (0.0, 0.0), (4.0, 0.0));
        track(&mut b, (4.0, 0.0), (6.0, 2.0));
        drag_joint(&mut b, p(4.0, 0.0), p(4.0, 1.0));
        assert!(all_45(&b), "{:?}", b.tracks);
        assert_eq!(joined(&b, p(0.0, 0.0)), 1);
        assert_eq!(joined(&b, p(6.0, 2.0)), 1);
    }
}
