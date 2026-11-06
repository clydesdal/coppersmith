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
use crate::pcb::copper::{Item, analyse};
use crate::pcb::router::{Corner, Obstacle, Scene, apply_pushed, chamfer, legs, shove_from};
use std::collections::HashSet;

/// Runs at a multiple of 45°.
pub fn octilinear(d: P) -> bool {
    // Corners are kept to the nanometre, so allow a few of those.
    let (x, y) = (d.x.abs(), d.y.abs());
    d.len() > 1e-9 && (x < 1e-5 || y < 1e-5 || (x - y).abs() < 1e-5)
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
        // An arc corner: the arc keeps its radius and stays tangent, and the
        // straight track beyond it grows or shrinks along its own line.
        if !fixed
            && neighbours.len() == 1
            && let Some((ai, t1, mid, t2, ni)) = refit_arc_corner(&work, neighbours[0], end, moved, if k == 1 { u } else { u.scale(-1.0) }, id)
        {
            let old_far = arc_far(&work.tracks, ai, end);
            let arc = &mut work.tracks[ai];
            if arc.a.same(end) {
                arc.a = t1;
                arc.b = t2;
            } else {
                arc.a = t2;
                arc.b = t1;
            }
            arc.mid = Some(mid);
            set_end(&mut work.tracks[ni], old_far, t2);
            ends[k] = t1;
            continue;
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

/// Slides a track as `slide` does, then pushes what's in the way as any
/// move does. None when either can't be done.
#[cfg(test)]
pub fn slide_push(board: &Board, id: u32, delta: P, via_push: bool) -> Option<Board> {
    let mut b = board.clone();
    if !slide(&mut b, id, delta) {
        return None;
    }
    push_after_move(board, b, via_push)
}

/// After a move (`before` → `after`), pushes other nets' tracks, and vias if
/// `via_push`, out of the way of everything that moved: the pads of moved
/// parts on their layers, moved vias on every layer, and tracks that moved or
/// were re-laid. None when it can't: something that moved would come within
/// the clearance of what can't be pushed (other parts' pads, holes, the edge,
/// or a via with via pushing off), or what's in the way can't give way.
pub fn push_after_move(before: &Board, after: Board, via_push: bool) -> Option<Board> {
    let changed: HashSet<u32> = after.tracks.iter().filter(|t| before.track(t.id) != Some(*t)).map(|t| t.id).collect();
    let moved_vias: HashSet<u32> =
        after.vias.iter().filter(|v| before.via(v.id).is_none_or(|old| !old.at.same(v.at))).map(|v| v.id).collect();
    let moved_fps: HashSet<u32> = after
        .footprints
        .iter()
        .filter(|f| before.fp(f.id).is_none_or(|old| !old.at.same(f.at) || old.rot != f.rot || old.side != f.side))
        .map(|f| f.id)
        .collect();
    if changed.is_empty() && moved_vias.is_empty() && moved_fps.is_empty() {
        return Some(after);
    }
    let mover = |item: Option<Item>| match item {
        Some(Item::Track(id)) => changed.contains(&id),
        Some(Item::Via(id)) => moved_vias.contains(&id),
        Some(Item::Pad(f, _)) => moved_fps.contains(&f),
        None => false,
    };
    let mut b = after;
    for layer in b.copper_layers() {
        // What moved, on this layer.
        let mut push: Vec<(Blob, u32)> = Vec::new();
        for t in b.tracks.iter().filter(|t| t.layer == layer && changed.contains(&t.id)) {
            push.extend(t.blobs().into_iter().map(|bl| (bl, t.net)));
        }
        for v in b.vias.iter().filter(|v| moved_vias.contains(&v.id)) {
            push.push((v.blob(), v.net));
        }
        for f in b.footprints.iter().filter(|f| moved_fps.contains(&f.id)) {
            for (i, pad) in f.pads.iter().enumerate() {
                if pad.through() || f.copper() == layer {
                    let blob = if pad.bare_hole() { Blob::dot(f.pad_center(i), pad.drill) } else { f.pad_blob(i, 0.0) };
                    push.push((blob, pad.net));
                }
            }
        }
        if push.is_empty() {
            continue;
        }
        let mut scene = Scene::new(&b, &analyse(&b), layer, 0);
        scene.via_push = via_push;
        scene.lines.retain(|l| !l.ids.iter().any(|i| changed.contains(i)));
        scene.vias.retain(|v| !moved_vias.contains(&v.0));
        // What moved can't come close to what can't be pushed.
        let fixed: Vec<&Obstacle> = scene.all.iter().filter(|o| !scene.movable(o) && !mover(o.item)).collect();
        for (blob, net) in &push {
            let bb = blob.bbox();
            for o in &fixed {
                let other = o.net != *net || *net == 0;
                if other && o.bbox.grown(o.gap).touches(&bb) && blob.gap(&o.blob).0 < o.gap + 1e-7 {
                    return None;
                }
            }
        }
        let (runs, vias) = shove_from(&scene, &push)?;
        apply_pushed(&mut b, &runs, &vias);
    }
    Some(b)
}

fn unit(d: P) -> P {
    d.scale(1.0 / d.len().max(1e-12))
}

/// Keeps 45° work free of square corners after a move. Where two straight
/// 45° tracks now meet at 90° or sharper (with no pad or via there), the
/// corner is cut with 45° steps `min` long. False when a corner can't be
/// cut, or when a 45° step between two tracks at right angles has been
/// squeezed shorter than `min`: the move should stop short of that.
pub fn fix_corners(board: &mut Board, changed: &HashSet<u32>, min: f64) -> bool {
    if min <= 0.0 {
        return true;
    }
    // The ends of what changed are where new corners can be.
    let mut joints: Vec<(P, u8)> = Vec::new();
    for t in board.tracks.iter().filter(|t| changed.contains(&t.id) && straight45(t)) {
        for q in [t.a, t.b] {
            if !joints.iter().any(|j| j.0.same(q) && j.1 == t.layer) {
                joints.push((q, t.layer));
            }
        }
    }
    for (q, layer) in &joints {
        let at: Vec<usize> = board
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.layer == *layer && (t.a.same(*q) || t.b.same(*q)))
            .map(|(i, _)| i)
            .collect();
        if at.len() != 2 || pinned(board, *q) || !at.iter().all(|&i| straight45(&board.tracks[i])) {
            continue;
        }
        let (t1, t2) = (board.tracks[at[0]].clone(), board.tracks[at[1]].clone());
        let (f1, f2) = (far_end(&t1, *q), far_end(&t2, *q));
        let (u, v) = (unit(q.sub(f1)), unit(f2.sub(*q)));
        if u.dot(v) > 1e-9 {
            continue;
        }
        let Some((a, b, cut)) = chamfer(*q, u, v, min) else { return false };
        if a >= q.dist(f1) - 1e-6 || b >= q.dist(f2) - 1e-6 {
            return false;
        }
        set_end(&mut board.tracks[at[0]], *q, cut[0]);
        set_end(&mut board.tracks[at[1]], *q, *cut.last().unwrap());
        for s in cut.windows(2) {
            add_track(board, &t1, s[0], s[1]);
        }
    }
    // Nothing between two tracks at right angles has got too short.
    let near: HashSet<u32> = board
        .tracks
        .iter()
        .filter(|t| changed.contains(&t.id) || joints.iter().any(|j| t.a.same(j.0) || t.b.same(j.0)))
        .map(|t| t.id)
        .collect();
    for s in board.tracks.iter().filter(|t| near.contains(&t.id) && straight45(t)) {
        if s.a.dist(s.b) >= min - 1e-6 {
            continue;
        }
        let n1 = others_at(board, s.a, s.layer, s.id);
        let n2 = others_at(board, s.b, s.layer, s.id);
        if n1.len() != 1 || n2.len() != 1 || pinned(board, s.a) || pinned(board, s.b) {
            continue;
        }
        let (t1, t2) = (&board.tracks[n1[0]], &board.tracks[n2[0]]);
        let din = unit(s.a.sub(far_end(t1, s.a)));
        let dout = unit(far_end(t2, s.b).sub(s.b));
        let ds = unit(s.b.sub(s.a));
        if din.cross(ds) * ds.cross(dout) > 0.0 && din.dot(dout).abs() < 1e-6 {
            return false;
        }
    }
    true
}

/// The tracks that differ between two boards.
pub fn changed_tracks(before: &Board, after: &Board) -> HashSet<u32> {
    after.tracks.iter().filter(|t| before.track(t.id) != Some(*t)).map(|t| t.id).collect()
}

/// The end of the arc track `ai` that isn't `near`, as it was on `tracks`.
fn arc_far(tracks: &[Track], ai: usize, near: P) -> P {
    let t = &tracks[ai];
    if t.a.same(near) { t.b } else { t.a }
}

/// A tangent arc of radius `r` rounding the corner `x`, coming in along
/// `din` and leaving along `dout` (unit lengths): where it starts, a point on
/// the way, and where it ends.
pub fn tangent_arc(x: P, din: P, dout: P, r: f64) -> Option<(P, P, P)> {
    let theta = din.dot(dout).clamp(-1.0, 1.0).acos();
    if theta < 1e-6 || theta > std::f64::consts::PI - 1e-3 || r <= 0.0 {
        return None;
    }
    let t = r * (theta / 2.0).tan();
    let turn = din.cross(dout).signum();
    let t1 = x.sub(din.scale(t));
    let t2 = x.add(dout.scale(t));
    let centre = t1.add(p(-din.y, din.x).scale(turn * r));
    let towards = x.sub(centre);
    let mid = centre.add(towards.scale(r / towards.len()));
    Some((t1.rounded(), mid.rounded(), t2.rounded()))
}

/// When the track `slid` slides and its end at `end` (which goes to
/// `moved`, travelling `din` towards the corner) is joined to arc track
/// `ai`: the arc's corner moves along the straight track beyond it, and the
/// arc is rebuilt there, same radius, tangent to both. Returns the arc's
/// index, its new start (on the slid track), middle and end, and the index
/// of the straight track beyond.
fn refit_arc_corner(board: &Board, ai: usize, end: P, moved: P, din: P, slid: u32) -> Option<(usize, P, P, P, usize)> {
    let arc = &board.tracks[ai];
    let mid = arc.mid?;
    let (_, r) = arc_center(arc.a, mid, arc.b)?;
    let far = far_end(arc, end);
    if pinned(board, far) {
        return None;
    }
    let beyond = others_at(board, far, arc.layer, arc.id);
    if beyond.len() != 1 || board.tracks[beyond[0]].id == slid {
        return None;
    }
    let n = &board.tracks[beyond[0]];
    if n.mid.is_some() {
        return None;
    }
    let f = far_end(n, far);
    let dout = unit(f.sub(far));
    let x = meet(moved, din, f, dout.scale(-1.0))?;
    let (t1, m, t2) = tangent_arc(x, din, dout, r)?;
    // The track beyond keeps some length, and stays the same way round.
    if f.sub(t2).dot(dout) <= 1e-6 {
        return None;
    }
    Some((ai, t1, m, t2, beyond[0]))
}

/// The arc is a corner between two straight tracks, so dragging it should
/// change its radius rather than move it.
pub fn arc_corner(board: &Board, id: u32) -> bool {
    let Some(arc) = board.track(id) else { return false };
    let one_straight = |q: P| {
        let n = others_at(board, q, arc.layer, id);
        n.len() == 1 && board.tracks[n[0]].mid.is_none() && !pinned(board, q)
    };
    arc.mid.is_some() && one_straight(arc.a) && one_straight(arc.b)
}

/// Drags an arc corner: its radius changes so that its middle follows the
/// drag along the line out from the corner, staying tangent to the straight
/// tracks either side, which grow or shrink to meet it. The radius never goes
/// under `min_r`, so a corner can't be tightened into what looks square.
/// False when it can't.
pub fn slide_arc(board: &mut Board, id: u32, delta: P, min_r: f64) -> bool {
    let Some(arc) = board.track(id).cloned() else { return false };
    let Some(mid) = arc.mid else { return false };
    let n1 = others_at(board, arc.a, arc.layer, id);
    let n2 = others_at(board, arc.b, arc.layer, id);
    if n1.len() != 1 || n2.len() != 1 || pinned(board, arc.a) || pinned(board, arc.b) {
        return false;
    }
    let (s1, s2) = (board.tracks[n1[0]].clone(), board.tracks[n2[0]].clone());
    if s1.mid.is_some() || s2.mid.is_some() {
        return false;
    }
    let (f1, f2) = (far_end(&s1, arc.a), far_end(&s2, arc.b));
    let (din, dout) = (unit(arc.a.sub(f1)), unit(f2.sub(arc.b)));
    let Some(x) = meet(f1, din, f2, dout.scale(-1.0)) else { return false };
    // How far the middle sits from the corner sets the radius.
    let out = unit(mid.sub(x));
    let dist = mid.dist(x) + delta.dot(out);
    let theta = din.dot(dout).clamp(-1.0, 1.0).acos();
    let half_inside = (std::f64::consts::PI - theta) / 2.0;
    let r = dist / (1.0 / half_inside.sin() - 1.0);
    if r < min_r.max(0.05) || r > 100.0 {
        return false;
    }
    let Some((t1, m, t2)) = tangent_arc(x, din, dout, r) else { return false };
    // The straight tracks keep some length, the same way round.
    if t1.sub(f1).dot(din) <= 1e-6 || f2.sub(t2).dot(dout) <= 1e-6 {
        return false;
    }
    set_end(&mut board.tracks[n1[0]], arc.a, t1);
    set_end(&mut board.tracks[n2[0]], arc.b, t2);
    if let Some(a) = board.tracks.iter_mut().find(|t| t.id == id) {
        a.a = t1;
        a.b = t2;
        a.mid = Some(m);
    }
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

#[cfg(test)]
mod pushing_while_sliding {
    use super::*;
    use crate::pcb::drc;

    fn tp(b: &mut Board, at: P, net: u32) {
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
    }

    /// Net A: pad, up at 45°, along at y = 2, down at 45°, pad. Net B: a
    /// straight run along y = 3, pad to pad, just above A's middle.
    fn board() -> (Board, u32, u32) {
        let mut b = Board { outline: rect_outline(p(-5.0, -5.0), p(25.0, 12.0)), ..Board::default() };
        let (a, n) = (b.net("A"), b.net("B"));
        tp(&mut b, p(0.0, 0.0), a);
        tp(&mut b, p(14.0, 0.0), a);
        tp(&mut b, p(-3.0, 3.0), n);
        tp(&mut b, p(17.0, 3.0), n);
        let add = |b: &mut Board, s: (f64, f64), e: (f64, f64), net: u32| {
            let id = b.new_id();
            b.tracks.push(Track { mid: None, id, a: p(s.0, s.1), b: p(e.0, e.1), w: 0.25, layer: TOP, net });
            id
        };
        add(&mut b, (0.0, 0.0), (2.0, 2.0), a);
        let mid = add(&mut b, (2.0, 2.0), (12.0, 2.0), a);
        add(&mut b, (12.0, 2.0), (14.0, 0.0), a);
        add(&mut b, (-3.0, 3.0), (17.0, 3.0), n);
        (b, a, mid)
    }

    fn clearance_problems(b: &Board) -> Vec<String> {
        drc::check(b, &analyse(b))
            .into_iter()
            .filter(|p| ["Too close", "Short circuit", "Too close to the edge", "Off the board"].contains(&p.title.as_str()))
            .map(|p| p.detail)
            .collect()
    }

    #[test]
    fn sliding_into_a_track_pushes_it() {
        let (b, _, mid) = board();
        assert!(clearance_problems(&b).is_empty());
        let after = slide_push(&b, mid, p(0.0, 1.0), true).expect("B gives way");
        let m = after.track(mid).unwrap();
        assert!((m.a.y - 3.0).abs() < 1e-9, "slid to y = 3: {m:?}");
        assert!(clearance_problems(&after).is_empty(), "{:?}", clearance_problems(&after));
        let n = after.net_by_name("B").unwrap();
        let bs: Vec<&Track> = after.tracks.iter().filter(|t| t.net == n).collect();
        assert!(bs.len() > 1, "B was bent out of the way");
        assert!(bs.iter().all(|t| octilinear(t.b.sub(t.a))), "and kept 45°");
    }

    #[test]
    fn pushes_carry_on_when_sliding() {
        let (mut b, _, mid) = board();
        // A third net, C, just above B.
        let c = b.net("C");
        tp(&mut b, p(-3.0, 4.6), c);
        tp(&mut b, p(17.0, 4.6), c);
        let id = b.new_id();
        b.tracks.push(Track { mid: None, id, a: p(-3.0, 4.6), b: p(17.0, 4.6), w: 0.25, layer: TOP, net: c });
        assert!(clearance_problems(&b).is_empty());
        let after = slide_push(&b, mid, p(0.0, 3.0), true).expect("B and C give way");
        assert!(clearance_problems(&after).is_empty(), "{:?}", clearance_problems(&after));
        let cs: Vec<&Track> = after.tracks.iter().filter(|t| t.net == c).collect();
        assert!(cs.len() > 1, "C was pushed too");
        assert!(after.tracks.iter().all(|t| octilinear(t.b.sub(t.a))));
    }

    #[test]
    fn sliding_stops_at_what_cant_move() {
        let (mut b, _, mid) = board();
        // A pad of another net sits above A's middle.
        let w = b.net("W");
        tp(&mut b, p(7.0, 3.6), w);
        assert!(slide_push(&b, mid, p(0.0, 0.5), true).is_some(), "a little is fine");
        assert!(slide_push(&b, mid, p(0.0, 1.5), true).is_none(), "into the pad isn't");
    }
}

#[cfg(test)]
mod pushing_while_moving {
    use super::*;
    use crate::pcb::drc;

    fn tp(b: &mut Board, at: P, net: u32) -> u32 {
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

    fn track(b: &mut Board, s: (f64, f64), e: (f64, f64), net: u32) -> u32 {
        let id = b.new_id();
        b.tracks.push(Track { mid: None, id, a: p(s.0, s.1), b: p(e.0, e.1), w: 0.25, layer: TOP, net });
        id
    }

    /// Net B runs straight along y = 3 from pad to pad.
    fn board() -> (Board, u32, u32) {
        let mut b = Board { outline: rect_outline(p(-5.0, -5.0), p(25.0, 12.0)), ..Board::default() };
        let (a, n) = (b.net("A"), b.net("B"));
        tp(&mut b, p(-3.0, 3.0), n);
        tp(&mut b, p(17.0, 3.0), n);
        track(&mut b, (-3.0, 3.0), (17.0, 3.0), n);
        (b, a, n)
    }

    fn check(after: &Board, n: u32) {
        let bad: Vec<String> = drc::check(after, &analyse(after))
            .into_iter()
            .filter(|p| ["Too close", "Short circuit"].contains(&p.title.as_str()))
            .map(|p| p.detail)
            .collect();
        assert!(bad.is_empty(), "{bad:?}");
        assert!(after.tracks.iter().filter(|t| t.net == n).count() > 1, "B was pushed");
        assert!(after.tracks.iter().all(|t| octilinear(t.b.sub(t.a))), "and kept 45°");
    }

    #[test]
    fn moving_a_part_pushes_tracks() {
        let (mut b, a, n) = board();
        let part = tp(&mut b, p(7.0, 0.0), a);
        let mut after = b.clone();
        after.fp_mut(part).unwrap().at = p(7.0, 2.4);
        let after = push_after_move(&b, after, true).expect("B gives way");
        check(&after, n);
    }

    #[test]
    fn moving_a_via_pushes_tracks() {
        let (mut b, a, n) = board();
        let id = b.new_id();
        b.vias.push(Via { id, at: p(7.0, 0.0), dia: 0.6, drill: 0.3, net: a });
        let mut after = b.clone();
        after.vias.iter_mut().find(|v| v.id == id).unwrap().at = p(7.0, 2.6);
        let after = push_after_move(&b, after, true).expect("B gives way");
        check(&after, n);
    }

    #[test]
    fn moving_tracks_and_dragging_corners_pushes_too() {
        let (mut b, a, n) = board();
        tp(&mut b, p(2.0, 0.0), a);
        track(&mut b, (2.0, 0.0), (5.0, 0.0), a);
        track(&mut b, (5.0, 0.0), (7.0, 2.0), a);
        // Drag the corner up under B.
        let mut after = b.clone();
        drag_joint(&mut after, p(7.0, 2.0), p(9.0, 2.75));
        let after = push_after_move(&b, after, true).expect("B gives way");
        check(&after, n);
    }

    #[test]
    fn a_part_cant_be_pushed_into_another_parts_pad() {
        let (mut b, a, _) = board();
        let part = tp(&mut b, p(7.0, -2.0), a);
        let w = b.net("W");
        tp(&mut b, p(7.0, 0.0), w);
        let mut after = b.clone();
        after.fp_mut(part).unwrap().at = p(7.0, -0.8);
        assert!(push_after_move(&b, after, true).is_none());
    }
}

#[cfg(test)]
mod min_45 {
    use super::*;

    fn track(b: &mut Board, a: (f64, f64), e: (f64, f64)) -> u32 {
        let id = b.new_id();
        b.tracks.push(Track { mid: None, id, a: p(a.0, a.1), b: p(e.0, e.1), w: 0.25, layer: TOP, net: 1 });
        id
    }

    #[test]
    fn a_square_corner_a_move_makes_is_cut() {
        let mut b = Board::default();
        let t1 = track(&mut b, (0.0, 0.0), (5.0, 0.0));
        let t2 = track(&mut b, (5.0, 0.0), (5.0, 5.0));
        let changed: HashSet<u32> = [t1, t2].into();
        assert!(fix_corners(&mut b, &changed, 0.5));
        assert_eq!(b.tracks.len(), 3);
        assert!(b.tracks.iter().all(|t| octilinear(t.b.sub(t.a))));
        let diag = b.tracks.iter().find(|t| t.id != t1 && t.id != t2).unwrap();
        assert!((diag.a.dist(diag.b) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn a_slide_stops_before_its_piece_gets_too_short() {
        // Up at 45°, along 10 mm, down at 45°: sliding up shortens the middle
        // by twice the slide.
        let make = || {
            let mut b = Board::default();
            track(&mut b, (0.0, 0.0), (2.0, 2.0));
            let mid = track(&mut b, (2.0, 2.0), (12.0, 2.0));
            track(&mut b, (12.0, 2.0), (14.0, 0.0));
            (b, mid)
        };
        let ok = |d: f64| {
            let (mut b, mid) = make();
            let before = b.clone();
            if !slide(&mut b, mid, p(0.0, d)) {
                return false;
            }
            let changed = changed_tracks(&before, &b);
            fix_corners(&mut b, &changed, 0.5)
        };
        assert!(ok(4.7), "leaves 0.6 mm");
        assert!(!ok(4.8), "would leave 0.4 mm");
    }
}

#[cfg(test)]
mod arcs_stay_tangent {
    use super::*;

    /// A U-turn-free run with rounded corners: down a vertical, round a
    /// 2 mm arc, along the bottom, round another, up the other side.
    fn board() -> (Board, u32, u32) {
        let mut b = Board::default();
        let s = 2f64.sqrt();
        let add = |b: &mut Board, a: P, e: P, mid: Option<P>| {
            let id = b.new_id();
            b.tracks.push(Track { mid, id, a, b: e, w: 0.25, layer: TOP, net: 1 });
            id
        };
        add(&mut b, p(0.0, 10.0), p(0.0, 2.0), None);
        let arc = add(&mut b, p(0.0, 2.0), p(2.0, 0.0), Some(p(2.0 - s, 2.0 - s)));
        let bottom = add(&mut b, p(2.0, 0.0), p(10.0, 0.0), None);
        add(&mut b, p(10.0, 0.0), p(12.0, 2.0), Some(p(10.0 + s, 2.0 - s)));
        add(&mut b, p(12.0, 2.0), p(12.0, 10.0), None);
        (b, bottom, arc)
    }

    /// Every arc is tangent to the straight tracks it joins.
    fn tangent(b: &Board) -> bool {
        b.tracks.iter().filter(|t| t.mid.is_some()).all(|arc| {
            let (c, r) = arc_center(arc.a, arc.mid.unwrap(), arc.b).unwrap();
            [arc.a, arc.b].iter().all(|end| {
                b.tracks.iter().filter(|t| t.mid.is_none() && (t.a.same(*end) || t.b.same(*end))).all(|s| {
                    // The radius at the joint is square to the straight track.
                    let along = s.b.sub(s.a);
                    let radius = end.sub(c);
                    (along.dot(radius) / (along.len() * r)).abs() < 1e-4
                })
            })
        })
    }

    #[test]
    fn sliding_between_arcs_keeps_them_tangent() {
        let (mut b, bottom, _) = board();
        assert!(tangent(&b));
        assert!(slide(&mut b, bottom, p(0.0, -1.0)));
        assert_eq!(b.tracks.len(), 5, "no links added: {:?}", b.tracks);
        let t = b.track(bottom).unwrap();
        assert!(t.a.same(p(2.0, -1.0)) && t.b.same(p(10.0, -1.0)), "{t:?}");
        assert!(tangent(&b), "{:?}", b.tracks);
        // The arcs keep their 2 mm radius.
        for arc in b.tracks.iter().filter(|t| t.mid.is_some()) {
            let (_, r) = arc_center(arc.a, arc.mid.unwrap(), arc.b).unwrap();
            assert!((r - 2.0).abs() < 1e-4, "{r}");
        }
    }

    #[test]
    fn dragging_an_arc_changes_its_radius() {
        let (mut b, _, arc) = board();
        // The arc's middle is 0.83 mm out from the corner at the origin;
        // pulling it 0.83 mm further out doubles the radius.
        let out = p(1.0, 1.0).scale(1.0 / 2f64.sqrt());
        let dist = (2.0 - 2f64.sqrt()) * 2f64.sqrt();
        assert!(slide_arc(&mut b, arc, out.scale(dist), 0.5));
        let a = b.track(arc).unwrap().clone();
        let (_, r) = arc_center(a.a, a.mid.unwrap(), a.b).unwrap();
        assert!((r - 4.0).abs() < 1e-3, "{r}");
        assert!(a.a.same(p(0.0, 4.0)) && a.b.same(p(4.0, 0.0)), "{a:?}");
        assert!(tangent(&b));
        // Too far in and there's nothing left of the arc.
        assert!(!slide_arc(&mut b, arc, out.scale(-10.0), 0.5));
        // Never tighter than the smallest radius allowed.
        let mut tight = b.clone();
        // From 4 mm, the middle moves (√2 − 1) mm per millimetre of radius.
        let to_r = |r: f64| (r * (2f64.sqrt() - 1.0)) - (4.0 * (2f64.sqrt() - 1.0));
        assert!(slide_arc(&mut tight, arc, out.scale(to_r(0.6)), 0.5), "0.6 mm is fine");
        let mut tighter = b.clone();
        assert!(!slide_arc(&mut tighter, arc, out.scale(to_r(0.4)), 0.5), "0.4 mm is under the rule");
    }
}
