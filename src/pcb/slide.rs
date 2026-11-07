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
use crate::pcb::copper::Item;
#[cfg(test)]
use crate::pcb::copper::{analyse, push_copper, scene_copper};
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
            && let Some(fit) = refit_arc_corner(&work, neighbours[0], end, moved, if k == 1 { u } else { u.scale(-1.0) }, id)
        {
            ends[k] = apply_arc_corner(&mut work, end, fit);
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
    push_after_move_with(before, after, via_push, &push_scene)
}

/// What a push reads of the board on one layer.
fn push_scene(board: &Board, layer: u8) -> Scene {
    Scene::for_push(board, layer)
}

/// As `push_after_move`, with the scene for each layer made by `build`.
fn push_after_move_with(before: &Board, after: Board, via_push: bool, build: &dyn Fn(&Board, u8) -> Scene) -> Option<Board> {
    let mut b = after;
    // Pushing on one layer can move vias, whose tracks and selves then push
    // on the other layers, and so on: go round the layers until it settles.
    for _ in 0..4 {
        if crate::pcb::router::out_of_time() {
            return None;
        }
        let start = b.clone();
        b = push_layers(before, b, via_push, build)?;
        if b == start {
            break;
        }
    }
    Some(b)
}

/// One go round the copper layers, pushing out of the way of everything
/// that differs from `before`.
fn push_layers(before: &Board, after: Board, via_push: bool, build: &dyn Fn(&Board, u8) -> Scene) -> Option<Board> {
    let changed = changed_tracks(before, &after);
    let vias_was: std::collections::HashMap<u32, &Via> = before.vias.iter().map(|v| (v.id, v)).collect();
    let moved_vias: HashSet<u32> =
        after.vias.iter().filter(|v| vias_was.get(&v.id).is_none_or(|old| !old.at.same(v.at))).map(|v| v.id).collect();
    let fps_was: std::collections::HashMap<u32, &Footprint> = before.footprints.iter().map(|f| (f.id, f)).collect();
    let moved_fps: HashSet<u32> = after
        .footprints
        .iter()
        .filter(|f| fps_was.get(&f.id).is_none_or(|old| !old.at.same(f.at) || old.rot != f.rot || old.side != f.side))
        .map(|f| f.id)
        .collect();
    if changed.is_empty() && moved_vias.is_empty() && moved_fps.is_empty() {
        return Some(after);
    }
    let mover = |item: Option<Item>| match item {
        Some(Item::Track(id)) => changed.contains(&id),
        Some(Item::Via(id)) => moved_vias.contains(&id),
        Some(Item::Pad(f, _)) => moved_fps.contains(&f),
        Some(Item::Pour(..) | Item::Fill(_)) | None => false,
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
        let mut scene = build(&b, layer);
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
    // Looked up by id in a map: `Board::track` searches the whole list, which
    // for every track of a big board is slow.
    let was: std::collections::HashMap<u32, &Track> = before.tracks.iter().map(|t| (t.id, t)).collect();
    after.tracks.iter().filter(|t| was.get(&t.id).copied() != Some(*t)).map(|t| t.id).collect()
}

/// Puts a refitted arc corner on the board: the arc where it now goes, and
/// the straight track beyond it meeting it. Returns where the track on the
/// near side (the one that was at `end`) now meets the arc.
fn apply_arc_corner(board: &mut Board, end: P, (ai, t1, mid, t2, ni): (usize, P, P, P, usize)) -> P {
    let old_far = arc_far(&board.tracks, ai, end);
    let arc = &mut board.tracks[ai];
    if arc.a.same(end) {
        arc.a = t1;
        arc.b = t2;
    } else {
        arc.a = t2;
        arc.b = t1;
    }
    arc.mid = Some(mid);
    set_end(&mut board.tracks[ni], old_far, t2);
    t1
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
    relay_keeping(board, i, from, to, false);
}

/// As `relay`; with `last` it's the leg arriving at `to` that keeps the
/// track's old direction, so it stays tangent to an arc there.
fn relay_keeping(board: &mut Board, i: usize, from: P, to: P, last: bool) {
    let t = board.tracks[i].clone();
    let was = far_end(&t, from).sub(from);
    let options = [legs(from, to, Corner::D45, false), legs(from, to, Corner::D45, true)];
    let parallel = |d: P| d.cross(was).abs() < 1e-6 * (d.len() * was.len()).max(1e-9) && d.dot(was) > 0.0;
    let same_way = |l: &Vec<P>| {
        l.len() > 1 && if last { parallel(l[l.len() - 1].sub(l[l.len() - 2])) } else { parallel(l[1].sub(l[0])) }
    };
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
    // Points that have moved, and whether a track arriving there has to stay
    // tangent to an arc that moved with them.
    let mut moves: Vec<(P, P, bool)> = moves.iter().map(|m| (m.0, m.1, false)).collect();
    let mut done: HashSet<u32> = skip.clone();
    // Re-laying makes new tracks, so cap the work rather than trust it ends.
    let mut left = 4 * board.tracks.len() + 64;
    loop {
        if left == 0 {
            break;
        }
        left -= 1;
        let moved_to = |moves: &[(P, P, bool)], q: P| moves.iter().find(|m| m.0.same(q)).map(|m| (m.1, m.2));
        let Some(id) = board
            .tracks
            .iter()
            .find(|t| !done.contains(&t.id) && (moved_to(&moves, t.a).is_some() || moved_to(&moves, t.b).is_some()))
            .map(|t| t.id)
        else {
            break;
        };
        done.insert(id);
        let Some(i) = board.tracks.iter().position(|t| t.id == id) else { continue };
        let t = board.tracks[i].clone();
        let (na, nb) = (moved_to(&moves, t.a), moved_to(&moves, t.b));
        if let (Some(a), Some(b)) = (na, nb) {
            // Both ends moved together: the track goes with them.
            board.tracks[i].set_ends(a.0, b.0);
            continue;
        }
        let ((new, tangent), old) = match (na, nb) {
            (Some(a), _) => (a, t.a),
            (_, Some(b)) => (b, t.b),
            _ => continue,
        };
        let other = far_end(&t, old);
        // An arc leaving the moved point goes with it, unchanged; what's
        // beyond it then follows, keeping tangent.
        if t.mid.is_some() {
            if pinned(board, other) || moved_to(&moves, other).is_some() {
                set_end(&mut board.tracks[i], old, new);
            } else {
                let d = new.sub(old);
                let arc = &mut board.tracks[i];
                arc.a = arc.a.add(d).rounded();
                arc.b = arc.b.add(d).rounded();
                arc.mid = arc.mid.map(|m| m.add(d).rounded());
                moves.push((other, other.add(d).rounded(), true));
            }
            continue;
        }
        let u = unit(other.sub(old));
        let next = others_at(board, other, t.layer, t.id);
        let free = next.len() == 1 && !pinned(board, other) && moved_to(&moves, other).is_none() && !skip.contains(&board.tracks[next[0]].id);
        // A straight track into an arc corner: the corner moves along the
        // straight track beyond, and the arc is rebuilt there, tangent.
        if free
            && board.tracks[next[0]].mid.is_some()
            && let Some(fit) = refit_arc_corner(board, next[0], other, new, u, t.id)
            && fit.1.sub(new).dot(u) > 1e-6
        {
            let q = apply_arc_corner(board, other, fit);
            let s = &mut board.tracks[i];
            s.a = new;
            s.b = q;
            continue;
        }
        if !straight45(&t) {
            set_end(&mut board.tracks[i], old, new);
            continue;
        }
        // The joint at the other end slides along the next track.
        if free && board.tracks[next[0]].mid.is_none() {
            let j = next[0];
            let nb_t = board.tracks[j].clone();
            let far = far_end(&nb_t, other);
            let v = other.sub(far);
            if let Some(q) = meet(new, u, far, v)
                && q.sub(far).dot(v) > 1e-6
                && q.sub(new).dot(u) > 1e-6
            {
                set_end(&mut board.tracks[j], other, q);
                board.tracks[i].a = new;
                board.tracks[i].b = q;
                continue;
            }
        }
        // Otherwise re-laid from its far end; next to an arc, the leg that
        // meets it keeps the old direction.
        relay_keeping(board, i, other, new, tangent);
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
    use crate::pcb::drc;

    #[test]
    fn a_scene_reads_the_same_from_the_quick_listing_as_from_the_full_analysis() {
        let mut b = crate::pcb::sample::board();
        let id = b.new_id();
        let net = b.pours.first().map(|p| p.net).unwrap_or(0);
        b.pours.push(Pour::new(id, TOP, net, Vec::new(), &b.rules));
        b.repour();
        assert!(b.pours.iter().any(|p| !p.fill.is_empty()), "something poured");
        let shape = |s: &Scene| {
            let all: Vec<_> = s.all.iter().map(|o| (o.item, o.net, o.movable, o.blob.core.clone(), o.blob.r)).collect();
            let lines: Vec<_> = s.lines.iter().map(|l| l.ids.clone()).collect();
            let others: Vec<_> = s.others.iter().map(|(l, all, runs)| (*l, all.len(), runs.len())).collect();
            (all, lines, others, s.vias.len())
        };
        for layer in b.copper_layers() {
            let full = Scene::new(&b, &analyse(&b), layer, 0);
            let quick = Scene::new(&b, &scene_copper(&b), layer, 0);
            assert_eq!(shape(&full), shape(&quick), "layer {layer}");
        }
    }

    /// Parallel tracks close together, several nets, vias and pads among them,
    /// and an arc: a board where sliding one track pushes others.
    fn crowded() -> Board {
        let mut b = Board::default();
        let nets = [b.net("A"), b.net("B"), b.net("C")];
        let add = |b: &mut Board, a: P, e: P, mid: Option<P>, layer: u8, net: u32| {
            let id = b.new_id();
            b.tracks.push(Track { mid, id, a, b: e, w: 0.25, layer, net });
        };
        let pad = |b: &mut Board, at: P, net: u32, drill: f64| {
            let id = b.new_id();
            b.footprints.push(Footprint {
                id,
                def: "TP".into(),
                refdes: format!("TP{id}"),
                value: String::new(),
                at,
                rot: 0.0,
                side: Side::Top,
                pads: vec![Pad { num: "1".into(), at: p(0.0, 0.0), size: p(1.0, 1.0), rot: 0.0, shape: PadShape::Circle, drill, net }],
                graphics: vec![],
                label_at: P::default(),
                locked: false,
            });
        };
        // Tracks from pad to pad, side by side, of three nets in turn.
        for k in 0..8 {
            let y = 8.0 + 1.6 * k as f64;
            pad(&mut b, p(5.0, y), nets[k % 3], 0.0);
            pad(&mut b, p(40.0, y), nets[k % 3], 0.0);
            add(&mut b, p(5.0, y), p(40.0, y), None, TOP, nets[k % 3]);
        }
        // Some on the other side, joined to through-hole pads, with a bend.
        for k in 0..4 {
            let y = 25.0 + 1.6 * k as f64;
            let net = nets[(k + 1) % 3];
            pad(&mut b, p(8.0, y), net, 0.5);
            pad(&mut b, p(26.0, y + 2.0), net, 0.5);
            add(&mut b, p(8.0, y), p(20.0, y), None, BOTTOM, net);
            add(&mut b, p(20.0, y), p(26.0, y + 2.0), None, BOTTOM, net);
        }
        // Half circles of a third net between stack tracks: an arc of half a
        // circle or more is in no run, so it can't be bent and is a wall.
        add(&mut b, p(21.7, 13.6), p(22.3, 13.6), Some(p(22.0, 13.9)), TOP, nets[2]);
        add(&mut b, p(29.7, 16.8), p(30.3, 16.8), Some(p(30.0, 17.1)), TOP, nets[2]);
        // An arc with a track close beside it.
        add(&mut b, p(40.0, 25.0), p(45.0, 30.0), Some(p(43.5355, 26.4645)), TOP, nets[0]);
        add(&mut b, p(36.0, 24.0), p(44.0, 24.0), None, TOP, nets[1]);
        // Vias beside the stack and pads clear of it.
        for (i, (x, y)) in [(12.0, 7.0), (25.0, 6.5), (33.0, 5.5), (22.0, 4.0)].into_iter().enumerate() {
            let id = b.new_id();
            b.vias.push(Via { id, at: p(x, y), dia: 0.6, drill: 0.3, net: nets[i % 3] });
        }
        for (i, (x, y)) in [(3.0, 36.0), (6.0, 36.0), (43.0, 33.0), (46.0, 35.0)].into_iter().enumerate() {
            pad(&mut b, p(x, y), nets[i % 3], if i % 2 == 0 { 0.0 } else { 0.5 });
        }
        b
    }

    /// The push scene must push exactly as the full one did.
    #[test]
    fn a_push_scene_pushes_what_the_full_scene_does() {
        let legacy = |b: &Board, layer: u8| Scene::new(b, &scene_copper(b), layer, 0);
        let base = crowded();
        let (mut tried, mut pushed, mut refused) = (0, 0, 0);
        for t in base.tracks.iter().filter(|t| t.mid.is_none()) {
            for d in [p(0.0, 0.1), p(0.0, -0.5), p(0.0, 1.5), p(0.0, -1.8), p(0.0, 2.6), p(0.7, 0.0), p(0.0, 0.3)] {
                let mut moved = base.clone();
                if !slide(&mut moved, t.id, d) {
                    continue;
                }
                for via_push in [true, false] {
                    let new = push_after_move(&base, moved.clone(), via_push);
                    let old = push_after_move_with(&base, moved.clone(), via_push, &legacy);
                    assert_eq!(new, old, "track {} by {d:?}, via pushing {via_push}", t.id);
                    tried += 1;
                    if new.is_none() {
                        refused += 1;
                    }
                    if new.as_ref().is_some_and(|n| *n != moved) {
                        pushed += 1;
                    }
                }
            }
        }
        assert!(tried > 40, "{tried} tried");
        assert!(pushed > 10, "something was pushed in {pushed} of {tried} ({refused} refused)");
    }

    /// The crowded board, and versions of it with something changed that a
    /// push scene is made from: a via, a part, a rule, the outline, a wall.
    fn changed_boards() -> Vec<(&'static str, Board)> {
        let base = crowded();
        let mut variants: Vec<(&str, Board)> = vec![("as it was", base.clone())];
        let mut b = base.clone();
        b.vias[0].at = p(12.0, 9.0);
        variants.push(("a via moved", b));
        let mut b = base.clone();
        b.vias.pop();
        variants.push(("a via gone", b));
        let mut b = base.clone();
        b.footprints[3].at = b.footprints[3].at.add(p(0.0, 0.4));
        variants.push(("a part moved", b));
        let mut b = base.clone();
        b.rules.clearance = 0.4;
        variants.push(("a wider clearance", b));
        let mut b = base.clone();
        b.outline = rect_outline(p(0.0, 0.0), p(50.0, 19.0));
        variants.push(("a smaller board", b));
        let mut b = base.clone();
        b.footprints[5].pads[0].net = 2;
        variants.push(("a pad of another net", b));
        // Walls (half circles) that move or aren't there any more, and nothing
        // else different: put together, and next to the board as it was, so
        // each change of walls follows another with the rest the same.
        let walls: Vec<usize> = base.tracks.iter().enumerate().filter(|(_, t)| t.mid.is_some() && t.a.dist(t.b) < 1.0).map(|(i, _)| i).collect();
        assert_eq!(walls.len(), 2);
        let mut wall_variants: Vec<(&str, Board)> = Vec::new();
        let mut b = base.clone();
        b.tracks[walls[0]].a = b.tracks[walls[0]].a.add(p(0.0, 0.2));
        wall_variants.push(("the first wall moved", b));
        let mut b = base.clone();
        b.tracks[walls[1]].a = b.tracks[walls[1]].a.add(p(0.0, 0.2));
        wall_variants.push(("the second wall moved", b));
        let mut b = base.clone();
        b.tracks.remove(walls[0]);
        wall_variants.push(("the first wall gone", b));
        let mut b = base.clone();
        b.tracks.remove(walls[1]);
        wall_variants.push(("the second wall gone", b));
        let mut b = base.clone();
        b.tracks.remove(walls[1]);
        b.tracks.remove(walls[0]);
        wall_variants.push(("both walls gone", b));
        variants.splice(1..1, wall_variants);
        let mut b = base.clone();
        let id = b.new_id();
        b.fills.push(Fill { id, layer: TOP, net: 1, pts: rect_outline(p(44.0, 2.0), p(48.0, 6.0)) });
        variants.push(("a fill", b));
        let mut b = base.clone();
        b.copper = 4;
        variants.push(("four copper layers", b));
        variants
    }

    /// The scene a push is made from, from a base left by an earlier board,
    /// is exactly what it is made from scratch: for every change, in turn,
    /// there and back. (The scenes are compared in full, so it doesn't depend
    /// on a push happening to go near what changed.)
    #[test]
    fn a_reused_push_scene_is_the_one_made_from_scratch() {
        let variants = changed_boards();
        let said = |s: &Scene| format!("{:?} {:?} {:?} {:?} {:?}", s.all, s.lines, s.others, s.vias, s.clearance);
        let mut reused = 0;
        // The original between each pair, so each change is met straight after
        // a base made from a board that differs from it in that one thing.
        let original = &variants[0];
        let mut order: Vec<&(&str, Board)> = Vec::new();
        for v in &variants[1..] {
            order.push(original);
            order.push(v);
        }
        order.push(original);
        for (what, b) in order {
            for layer in b.copper_layers() {
                // From whatever the last board left.
                let cached = Scene::for_push(b, layer);
                let before = said(&cached);
                crate::pcb::router::forget_push_base();
                let fresh = Scene::for_push(b, layer);
                assert_eq!(before, said(&fresh), "{what}, layer {layer}");
                // And again, now the base is this board's: handed on as it is.
                let again = Scene::for_push(b, layer);
                assert!(std::sync::Arc::ptr_eq(&fresh.all, &again.all), "{what}, layer {layer}: reused, not rebuilt");
                reused += 1;
            }
        }
        assert!(reused >= 40, "{reused} scenes compared");
    }

    /// The push scene reuses what hasn't changed, and only that: pushing on a
    /// board, then on versions with a via, a part, a rule or the outline
    /// changed, and back again, always gives what the full scene gives.
    #[test]
    fn the_push_scene_is_not_stale_when_the_board_changes_under_it() {
        let legacy = |b: &Board, layer: u8| Scene::new(b, &scene_copper(b), layer, 0);
        let variants = changed_boards();
        let mut pushed = 0;
        // There and back, so every change comes after and before every other.
        {
            for (what, b) in variants.iter().chain(variants.iter().rev()) {
                for t in b.tracks.iter().filter(|t| t.mid.is_none() && t.layer == TOP) {
                    for d in [p(0.0, 0.3), p(0.0, -1.5), p(0.0, 2.2)] {
                        let mut moved = b.clone();
                        if !slide(&mut moved, t.id, d) {
                            continue;
                        }
                        let new = push_after_move(b, moved.clone(), true);
                        let old = push_after_move_with(b, moved.clone(), true, &legacy);
                        assert_eq!(new, old, "{what}: track {} by {d:?}", t.id);
                        if new.as_ref().is_some_and(|n| *n != moved) {
                            pushed += 1;
                        }
                    }
                }
            }
        }
        assert!(pushed > 20, "{pushed} pushes");
    }

    /// Times the pieces of one drag step on a real board:
    /// COPPERSMITH_BENCH_BOARD=/path/to/board cargo test --release bench_drag -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_drag_step() {
        let Ok(path) = std::env::var("COPPERSMITH_BENCH_BOARD") else { return };
        let b = crate::pcb::altium::read_board(std::path::Path::new(&path)).unwrap().board;
        // How many tracks are in no run, and so are walls that can't move?
        {
            let c = push_copper(&b);
            let mut outside = 0;
            let mut arcs = 0;
            for l in b.copper_layers() {
                let runs = crate::pcb::router::lines(&b, &c, l, 0);
                let in_runs: HashSet<u32> = runs.iter().flat_map(|r| r.ids.iter().copied()).collect();
                outside += b.tracks.iter().filter(|t| t.layer == l && !in_runs.contains(&t.id)).count();
            }
            arcs += b.tracks.iter().filter(|t| t.mid.is_some()).count();
            eprintln!("tracks in no run: {outside} of {} ({arcs} arcs)", b.tracks.len());
        }
        let t = |what: &str, f: &mut dyn FnMut()| {
            let s = std::time::Instant::now();
            f();
            eprintln!("{what:<40} {:?}", s.elapsed());
        };
        let mut c = b.clone();
        t("Board::clone", &mut || c = b.clone());
        t("Board == Board", &mut || assert!(c == b));
        // A straight track on a layer with no pours.
        let pour_layers: Vec<u8> = b.pours.iter().map(|p| p.layer).collect();
        let id = b.tracks.iter().find(|t| t.mid.is_none() && !pour_layers.contains(&t.layer) && t.a.dist(t.b) > 3.0).map(|t| t.id).unwrap();
        eprintln!("sliding track {id} on layer {}", b.track(id).unwrap().layer);
        let before = b.clone();
        let mut moved = b.clone();
        t("slide()", &mut || assert!(slide(&mut moved, id, p(0.0, 0.1))));
        let mut pushed = None;
        t("push_after_move()", &mut || pushed = push_after_move(&before, moved.clone(), true));
        eprintln!("pushed ok: {}", pushed.is_some());
        // The same on layer 1 (the second copper layer), which here carries a pour.
        for (name, layer) in [("layer 1", 1u8), ("layer 0", 0u8)] {
            let Some(id) = b.tracks.iter().find(|t| t.layer == layer && t.mid.is_none() && t.a.dist(t.b) > 3.0).map(|t| t.id) else { continue };
            for d in [0.1, 1.0] {
                let mut moved = b.clone();
                if !slide(&mut moved, id, p(0.0, d)) {
                    eprintln!("{name}: slide {d} refused");
                    continue;
                }
                let s0 = std::time::Instant::now();
                let r = push_after_move(&b, moved.clone(), true);
                eprintln!("{name}: slide {d} mm, push_after_move {:?} -> {}", s0.elapsed(), if r.is_some() { "ok" } else { "None" });
            }
            let cu = analyse(&b);
            let s0 = std::time::Instant::now();
            let sc = Scene::new(&b, &cu, layer, 0);
            eprintln!("{name}: Scene::new {:?} ({} obstacles here, {} other layers)", s0.elapsed(), sc.all.len(), sc.others.len());
            let s0 = std::time::Instant::now();
            let ps = push_scene(&b, layer);
            eprintln!("{name}: push scene (copper + for_push) {:?} ({} obstacles here)", s0.elapsed(), ps.all.len());
        }
        // The push scene against the full one, on real copper, in all places.
        {
            let legacy = |b: &Board, layer: u8| Scene::new(b, &scene_copper(b), layer, 0);
            let (mut same, mut pushed) = (0, 0);
            let straight: Vec<u32> = b.tracks.iter().filter(|t| t.mid.is_none() && t.a.dist(t.b) > 2.0).map(|t| t.id).collect();
            for &tid in straight.iter().step_by((straight.len() / 12).max(1)) {
                for d in [p(0.0, 0.15), p(-0.3, 0.3)] {
                    let mut moved = b.clone();
                    if !slide(&mut moved, tid, d) {
                        continue;
                    }
                    let s0 = std::time::Instant::now();
                    let new = push_after_move(&b, moved.clone(), true);
                    let t_new = s0.elapsed();
                    let s0 = std::time::Instant::now();
                    let old = push_after_move_with(&b, moved.clone(), true, &legacy);
                    eprintln!("  track {tid} by {d:?}: new {t_new:?}, full {:?}", s0.elapsed());
                    assert_eq!(new, old, "track {tid} by {d:?}");
                    same += 1;
                    if new.as_ref().is_some_and(|n| *n != moved) {
                        pushed += 1;
                    }
                }
            }
            eprintln!("push scene == full scene on {same} real slides ({pushed} of them pushed something)");
        }
        // One drag step as the app now does it: analyse, then check what changed.
        {
            let cu0 = analyse(&b);
            let (_, cache) = drc::check_again(&b, &cu0, None);
            let mut moved = b.clone();
            assert!(slide(&mut moved, id, p(0.0, 0.1)));
            let mut cu1 = None;
            t("step: analyse after a slide (from scratch)", &mut || cu1 = Some(analyse(&moved)));
            t("step: analyse_again after a slide", &mut || drop(crate::pcb::copper::analyse_again(&moved, &cu0)));
            let cu1 = cu1.unwrap();
            let mut again = None;
            t("step: drc::check_again", &mut || again = Some(drc::check_again(&moved, &cu1, Some(&cache))));
            let scratch = drc::check(&moved, &cu1);
            assert_eq!(again.unwrap().0.len(), scratch.len());
        }
        t("analyse (with pours)",&mut || drop(analyse(&b)));
        let cu = analyse(&b);
        t("drc::check", &mut || drop(drc::check(&b, &cu)));
        t("analyse_fixed (no pours)", &mut || drop(crate::pcb::copper::analyse_fixed(&b)));
        let cf = crate::pcb::copper::analyse_fixed(&b);
        t("drc::check on fixed", &mut || drop(drc::check(&b, &cf)));
        let mut nopour = b.clone();
        nopour.pours.clear();
        t("analyse+drc with pours removed", &mut || drop(drc::check(&nopour, &analyse(&nopour))));
    }

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

#[cfg(test)]
mod moving_parts_keeps_arcs {
    use super::*;

    fn add(b: &mut Board, a: P, e: P, mid: Option<P>) -> u32 {
        let id = b.new_id();
        b.tracks.push(Track { mid, id, a, b: e, w: 0.25, layer: TOP, net: 1 });
        id
    }

    /// Every arc is tangent to the straight tracks it joins, and has radius `r`.
    fn tangent(b: &Board, r: f64) -> bool {
        b.tracks.iter().filter(|t| t.mid.is_some()).all(|arc| {
            let (c, rad) = arc_center(arc.a, arc.mid.unwrap(), arc.b).unwrap();
            (rad - r).abs() < 1e-4
                && [arc.a, arc.b].iter().all(|end| {
                    b.tracks.iter().filter(|t| t.mid.is_none() && (t.a.same(*end) || t.b.same(*end))).all(|s| {
                        let along = s.b.sub(s.a);
                        (along.dot(end.sub(c)) / (along.len() * rad)).abs() < 1e-4
                    })
                })
        })
    }

    #[test]
    fn a_track_into_an_arc_corner_keeps_it_tangent() {
        // Pad at (0, 10): down to a 2 mm arc corner, then along to (10, 0).
        let s = 2f64.sqrt();
        let mut b = Board::default();
        add(&mut b, p(0.0, 10.0), p(0.0, 2.0), None);
        add(&mut b, p(0.0, 2.0), p(2.0, 0.0), Some(p(2.0 - s, 2.0 - s)));
        add(&mut b, p(2.0, 0.0), p(10.0, 0.0), None);
        follow_moves(&mut b, &[(p(0.0, 10.0), p(-1.0, 10.0))], &HashSet::new());
        assert_eq!(b.tracks.len(), 3, "{:?}", b.tracks);
        assert!(tangent(&b, 2.0), "{:?}", b.tracks);
        assert!(b.tracks.iter().any(|t| t.a.same(p(-1.0, 10.0)) && t.b.same(p(-1.0, 2.0))), "{:?}", b.tracks);
        assert!(b.tracks.iter().any(|t| t.mid.is_none() && t.a.same(p(1.0, 0.0)) && t.b.same(p(10.0, 0.0))), "{:?}", b.tracks);
    }

    #[test]
    fn an_arc_at_the_pad_moves_with_it_and_stays_tangent() {
        // Pad at the origin: a 2 mm arc east-then-north up to (2, 2), up to
        // (2, 10), then east to (8, 10).
        let s = 2f64.sqrt();
        let mut b = Board::default();
        add(&mut b, p(0.0, 0.0), p(2.0, 2.0), Some(p(s, 2.0 - s)));
        add(&mut b, p(2.0, 2.0), p(2.0, 10.0), None);
        add(&mut b, p(2.0, 10.0), p(8.0, 10.0), None);
        follow_moves(&mut b, &[(p(0.0, 0.0), p(1.0, 0.0))], &HashSet::new());
        assert!(tangent(&b, 2.0), "{:?}", b.tracks);
        assert!(b.tracks.iter().any(|t| t.a.same(p(3.0, 2.0)) && t.b.same(p(3.0, 10.0))), "{:?}", b.tracks);
        assert!(b.tracks.iter().any(|t| t.a.same(p(3.0, 10.0)) && t.b.same(p(8.0, 10.0))), "{:?}", b.tracks);
    }

    #[test]
    fn with_nowhere_to_slide_the_leg_at_the_arc_keeps_its_direction() {
        // As above, but the vertical track ends loose at (2, 10).
        let s = 2f64.sqrt();
        let mut b = Board::default();
        add(&mut b, p(0.0, 0.0), p(2.0, 2.0), Some(p(s, 2.0 - s)));
        add(&mut b, p(2.0, 2.0), p(2.0, 10.0), None);
        follow_moves(&mut b, &[(p(0.0, 0.0), p(1.0, 0.0))], &HashSet::new());
        assert!(tangent(&b, 2.0), "{:?}", b.tracks);
        assert!(b.tracks.iter().all(|t| t.mid.is_some() || octilinear(t.b.sub(t.a))));
    }
}

#[cfg(test)]
mod vias_push_on_other_layers {
    use super::*;
    use crate::pcb::drc;
    use crate::pcb::router::{Conflict, plan};

    fn pad(b: &mut Board, at: P, net: u32) {
        let id = b.new_id();
        b.footprints.push(Footprint {
            id,
            def: "TH".into(),
            refdes: format!("P{id}"),
            value: String::new(),
            at,
            rot: 0.0,
            side: Side::Top,
            pads: vec![Pad { num: "1".into(), at: p(0.0, 0.0), size: p(1.0, 1.0), rot: 0.0, shape: PadShape::Circle, drill: 0.5, net }],
            graphics: vec![],
            label_at: P::default(),
            locked: false,
        });
    }

    fn track(b: &mut Board, s: (f64, f64), e: (f64, f64), layer: u8, net: u32) {
        let id = b.new_id();
        b.tracks.push(Track { mid: None, id, a: p(s.0, s.1), b: p(e.0, e.1), w: 0.25, layer, net });
    }

    #[test]
    fn a_via_pushed_on_top_pushes_tracks_on_the_bottom() {
        // Net B: along the top to a via, on along the bottom. Net C runs along
        // the bottom just above the via, with room for it but no more.
        let mut b = Board { outline: rect_outline(p(-8.0, -8.0), p(28.0, 8.0)), ..Board::default() };
        let (a, nb, nc) = (b.net("A"), b.net("B"), b.net("C"));
        pad(&mut b, p(0.0, 0.0), nb);
        pad(&mut b, p(20.0, 0.0), nb);
        pad(&mut b, p(-4.0, 0.95), nc);
        pad(&mut b, p(24.0, 0.95), nc);
        track(&mut b, (0.0, 0.0), (10.0, 0.0), TOP, nb);
        let v = b.new_id();
        b.vias.push(Via { id: v, at: p(10.0, 0.0), dia: 0.6, drill: 0.3, net: nb });
        track(&mut b, (10.0, 0.0), (20.0, 0.0), BOTTOM, nb);
        track(&mut b, (-4.0, 0.95), (24.0, 0.95), BOTTOM, nc);
        let clean = |b: &Board| {
            drc::check(b, &analyse(b)).into_iter().filter(|p| ["Too close", "Short circuit"].contains(&p.title.as_str())).map(|p| p.detail).collect::<Vec<_>>()
        };
        assert!(clean(&b).is_empty());
        // A route on the top comes up at the via from below.
        let mut scene = Scene::new(&b, &analyse(&b), TOP, a);
        scene.via_push = true;
        let head = plan(&scene, p(10.0, -6.0), p(10.0, -0.25), 0.25, Corner::D45, false, Conflict::Push, b.outline_bbox(), &[]);
        assert_eq!(head.vias.len(), 1, "the via is pushed: {head:?}");
        // Settle: the route stands in as fixed while the other layers make room.
        let mut after = b.clone();
        apply_pushed(&mut after, &head.pushed, &head.vias);
        let first = after.next_id;
        for s in head.pts.windows(2) {
            let id = after.new_id();
            after.tracks.push(Track { mid: None, id, a: s[0], b: s[1], w: 0.25, layer: TOP, net: a });
        }
        let settled = push_after_move(&b, after, true).expect("the bottom makes room");
        assert!(clean(&settled).is_empty(), "{:?}", clean(&settled));
        let moved_to = settled.via(v).unwrap().at;
        assert!(moved_to.y > 0.0, "the via went up, towards C: {moved_to:?}");
        assert!(settled.tracks.iter().filter(|t| t.net == nc).count() > 1, "C was pushed on the bottom");
        assert!(settled.tracks.iter().filter(|t| t.id < first).all(|t| t.mid.is_some() || octilinear(t.b.sub(t.a))));
    }
}

#[cfg(test)]
mod timing {
    use super::*;
    use crate::pcb::router::{Conflict, plan};
    use std::time::Instant;

    /// 30 nets of track side by side across a 70 mm board, each with a dog-leg,
    /// pads at both ends, a via in each.
    pub fn dense() -> Board {
        let mut b = Board { outline: rect_outline(p(-5.0, -5.0), p(75.0, 40.0)), ..Board::default() };
        for k in 0..30 {
            let n = b.net(&format!("N{k}"));
            let y = k as f64 * 1.0;
            for x in [0.0, 70.0] {
                let id = b.new_id();
                b.footprints.push(Footprint {
                    id,
                    def: "TP".into(),
                    refdes: format!("TP{id}"),
                    value: String::new(),
                    at: p(x, y),
                    rot: 0.0,
                    side: Side::Top,
                    pads: vec![Pad { num: "1".into(), at: p(0.0, 0.0), size: p(0.6, 0.6), rot: 0.0, shape: PadShape::Circle, drill: 0.0, net: n }],
                    graphics: vec![],
                    label_at: P::default(),
                    locked: false,
                });
            }
            let pts = [p(0.0, y), p(20.0, y), p(22.0, y + 2.0), p(45.0, y + 2.0), p(47.0, y), p(70.0, y)];
            for s in pts.windows(2) {
                let id = b.new_id();
                b.tracks.push(Track { mid: None, id, a: s[0], b: s[1], w: 0.2, layer: TOP, net: n });
            }
        }
        b
    }

    #[test]
    #[ignore]
    fn how_long_things_take() {
        let b = dense();
        let t = Instant::now();
        let c = analyse(&b);
        let found = crate::pcb::drc::check(&b, &c);
        eprintln!("refresh (connectivity + checks): {:?}, {} problems, {} copper items", t.elapsed(), found.len(), c.items.len());
        let net = b.net_by_name("N3").unwrap();
        let mut scene = Scene::new(&b, &c, TOP, 0);
        eprintln!("scene: {:?}", t.elapsed());
        scene.net = net;
        for mode in [Conflict::Walkaround, Conflict::Push, Conflict::HugPush] {
            let mut worst = 0.0f64;
            for to in [p(33.0, 20.0), p(33.0, 35.0), p(10.0, 33.0), p(60.0, -3.0)] {
                let s = Instant::now();
                let _ = plan(&Scene::new(&b, &c, TOP, net), p(0.0, 3.0), to, 0.2, Corner::D45, false, mode, b.outline_bbox(), &[]);
                worst = worst.max(s.elapsed().as_secs_f64());
            }
            eprintln!("plan {mode:?}: worst {:.0} ms", worst * 1e3);
        }
        // Drag a long straight piece across the bundle, as a slide would.
        let id = b.tracks.iter().find(|t| t.net == net && (t.a.y - 5.0).abs() < 1e-9 && t.a.x > 21.0).unwrap().id;
        let mut worst = 0.0f64;
        for d in [0.5, 1.0, 3.0, 6.0, 12.0] {
            let s = Instant::now();
            let mut moved = b.clone();
            if slide(&mut moved, id, p(0.0, d)) {
                let _ = push_after_move(&b, moved, true);
            }
            worst = worst.max(s.elapsed().as_secs_f64());
        }
        eprintln!("slide + push, worst: {:.0} ms", worst * 1e3);
        // Loop removal on a net with many tracks.
        let mut big = b.clone();
        let n0 = big.net_by_name("N0").unwrap();
        for k in 0..40 {
            let id = big.new_id();
            let x = 0.5 * k as f64;
            big.tracks.push(Track { mid: None, id, a: p(x, -1.0), b: p(x + 0.5, -1.0), w: 0.2, layer: TOP, net: n0 });
        }
        let s = Instant::now();
        let spare = crate::pcb::router::spare_tracks(&big, n0);
        eprintln!("spare_tracks on a 45-track net: {:?} ({} spare)", s.elapsed(), spare.len());
    }
}
