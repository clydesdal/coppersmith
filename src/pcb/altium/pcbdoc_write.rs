//! Writing a board as an Altium `.PcbDoc`: the other half of `pcbdoc`.
//!
//! A board file is a compound file with a storage for each kind of thing in it
//! (see `pcbdoc`). This writes the ones a board needs: `Board6` (outline and
//! layer stack), `Nets6`, `Components6`, `Pads6`, `Vias6`, `Tracks6`, `Arcs6`,
//! `Regions6`, `Texts6` (each part's name), `Polygons6` (pours), `Rules6`,
//! `Classes6` and `DifferentialPairs6`, with the empty storages Altium's own
//! boards carry. The records whose full layout is mostly Altium's own business
//! (a via, a pad, a piece of text) start from records copied out of a real
//! board (see `templates`), with what Coppersmith knows patched in.
//!
//! It is checked by reading what it writes back with `pcbdoc::read` and
//! comparing, and the record sizes were checked against a real board. It has
//! **not been opened in Altium itself**, which is the only test that counts.

use super::templates;
use super::write::{Out, param_text};
use super::{UNIT, prims};
use crate::pcb::geom::*;
use crate::pcb::model::*;
use crate::pcb::rulebook::RuleId;
use std::collections::HashMap;
use std::io::{Cursor, Write};

/// What was written, and what it couldn't keep.
pub struct Written {
    pub bytes: Vec<u8>,
    pub notes: Vec<String>,
}

/// The mechanical layers the courtyard and assembly drawings go on.
const COURT: u8 = prims::MECHANICAL_1;
const FAB: u8 = prims::MECHANICAL_1 + 1;
/// Altium's "no limit".
const NO_LIMIT: &str = "99999mil";
/// A primitive's usual flags: not locked, tented.
const FLAGS: u16 = 0x000c;

/// A number with no more digits than it needs.
fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.into() }
}

/// A length as the board's text records give it.
fn mil(mm: f64) -> String {
    format!("{}mil", num(mm / 0.0254))
}

/// Altium's number for a copper layer of ours, if the board has it.
fn copper_layer(l: u8, inner: u8) -> Option<u8> {
    match l {
        TOP => Some(prims::TOP),
        BOTTOM => Some(prims::BOTTOM),
        k if k >= 1 && k <= inner => Some(k + 1),
        _ => None,
    }
}

/// What a text record calls a copper layer.
fn layer_word(l: u8) -> String {
    match l {
        TOP => "TOP".into(),
        BOTTOM => "BOTTOM".into(),
        k => format!("MID{k}"),
    }
}

/// Eight capital letters from a name, for the ids Altium keeps on its records.
fn unique_id(seed: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in seed.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    (0..8).map(|_| {
        let c = (b'A' + (h % 26) as u8) as char;
        h /= 26;
        c
    }).collect()
}

fn pairs(list: &[(&str, String)]) -> Vec<(String, String)> {
    list.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

/// A text record: four-byte length, then its `|KEY=VALUE…`.
fn text_record(list: &[(String, String)]) -> Vec<u8> {
    let mut o = Out::default();
    o.block(&param_text(list));
    o.0
}

/// The head every board primitive starts with.
fn head(o: &mut Out, layer: u8, flags: u16, net: u16, comp: u16) {
    o.u8(layer).u16(flags).u16(net).u16(prims::NONE).u16(comp);
    o.0.extend([0xff; 4]);
}

fn put_mm(buf: &mut [u8], at: usize, v: f64) {
    buf[at..at + 4].copy_from_slice(&((v / UNIT).round() as i32).to_le_bytes());
}

fn put_u16(buf: &mut [u8], at: usize, v: u16) {
    buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// Zeroes the first run of these bytes found: the ids in the copied records.
fn clear(buf: &mut [u8], what: &str) {
    let needle = templates::bytes(what);
    if let Some(i) = buf.windows(needle.len()).position(|w| w == needle.as_slice()) {
        buf[i..i + needle.len()].fill(0);
    }
}

fn track(a: P, b: P, w: f64, layer: u8, net: u16, comp: u16) -> Vec<u8> {
    let mut blk = Out::default();
    head(&mut blk, layer, FLAGS, net, comp);
    blk.mm(a.x).mm(a.y).mm(b.x).mm(b.y).mm(w).zeros(8).u8(6).u8(0).u8(3).u8(1).zeros(4);
    let mut o = Out::default();
    o.u8(4).block(&blk.0);
    o.0
}

fn arc(c: P, r: f64, start: f64, end: f64, w: f64, layer: u8, net: u16, comp: u16) -> Vec<u8> {
    let mut blk = Out::default();
    head(&mut blk, layer, FLAGS, net, comp);
    blk.mm(c.x).mm(c.y).mm(r).f64(start).f64(end).mm(w).zeros(7).u8(6).u8(0).u8(3).u8(1).zeros(4);
    let mut o = Out::default();
    o.u8(1).block(&blk.0);
    o.0
}

/// A track that curves (its ends and a point between) as an Altium arc: the circle
/// through them, and the way round it, counter-clockwise from where it starts.
fn arc_track(t: &Track, mid: P, layer: u8, net: u16) -> Vec<u8> {
    let Some((c, r)) = crate::pcb::kicad::pcb::circumcircle(t.a, mid, t.b) else {
        return track(t.a, t.b, t.w, layer, net, prims::NONE);
    };
    let ang = |q: P| (q.y - c.y).atan2(q.x - c.x).to_degrees().rem_euclid(360.0);
    let counter = mid.sub(t.a).cross(t.b.sub(mid)) > 0.0;
    let (start, end) = if counter { (ang(t.a), ang(t.b)) } else { (ang(t.b), ang(t.a)) };
    arc(c, r, start, end, t.w, layer, net, prims::NONE)
}

fn via(v: &Via, net: u16) -> Vec<u8> {
    let mut b = templates::bytes(templates::VIA);
    let old_dia = b[21..25].to_vec();
    b[0] = prims::MULTI_LAYER;
    put_u16(&mut b, 1, FLAGS);
    put_u16(&mut b, 3, net);
    b[5..13].fill(0xff);
    put_mm(&mut b, 13, v.at.x);
    put_mm(&mut b, 17, v.at.y);
    put_mm(&mut b, 21, v.dia);
    put_mm(&mut b, 25, v.drill);
    // The diameter again for each layer.
    let new_dia = b[21..25].to_vec();
    let mut i = 60;
    while i + 4 <= b.len() {
        if b[i..i + 4] == old_dia[..] {
            b[i..i + 4].copy_from_slice(&new_dia);
            i += 4;
        } else {
            i += 1;
        }
    }
    clear(&mut b, "397ffd4534d67249ab81f4f140f5a827");
    let mut o = Out::default();
    o.u8(3).block(&b);
    o.0
}

fn pad(pad: &Pad, layer: u8, at: P, rot: f64, net: u16, comp: u16) -> Vec<u8> {
    let through = pad.drill > 0.0;
    let plain_hole = through && (pad.size.x - pad.drill).abs() < 1e-6 && (pad.size.y - pad.drill).abs() < 1e-6;
    let shape = match pad.shape {
        PadShape::Circle | PadShape::Oval => 1,
        PadShape::Rect | PadShape::RoundRect(_) => 2,
    };
    let mut g = templates::bytes(templates::PAD);
    g[0] = layer;
    put_u16(&mut g, 1, FLAGS);
    put_u16(&mut g, 3, net);
    put_u16(&mut g, 5, prims::NONE);
    put_u16(&mut g, 7, comp);
    put_mm(&mut g, 13, at.x);
    put_mm(&mut g, 17, at.y);
    for k in 0..3 {
        put_mm(&mut g, 21 + 8 * k, pad.size.x);
        put_mm(&mut g, 25 + 8 * k, pad.size.y);
    }
    put_mm(&mut g, 45, if through { pad.drill } else { 0.0 });
    g[49..52].fill(shape);
    g[52..60].copy_from_slice(&rot.rem_euclid(360.0).to_le_bytes());
    g[60] = u8::from(!plain_hole);
    clear(&mut g, "342496db8d8074478be7637c17019138");
    clear(&mut g, "db629b48d523a74cbd83f4ea04bb6a75");
    // Where a rounded rectangle's corner is said.
    let mut s = Out::default();
    if let PadShape::RoundRect(k) = pad.shape {
        s.zeros(596);
        s.0[532] = 9;
        s.0[564] = (k * 200.0).round().clamp(0.0, 100.0) as u8;
    }
    let mut num = Out::default();
    num.pascal(&pad.num);
    let mut link = Out::default();
    link.pascal("|&|0");
    let mut o = Out::default();
    o.u8(2).block(&num.0).block(&[0]).block(&link.0).block(&[0]).block(&g).block(&s.0);
    o.0
}

/// A region: corners as pairs of doubles in Altium's unit, with the properties that say what it is.
fn region(pts: &[P], layer: u8, flags: u16, net: u16, comp: u16, props: &[(&str, String)]) -> Vec<u8> {
    let mut blk = Out::default();
    blk.u8(layer).u16(flags).u16(net).u16(prims::NONE).u16(comp);
    blk.0.extend([0xff, 0xff, 0xff, 0xff, 0]);
    blk.u16(0).zeros(2).block(&param_text(props));
    blk.u32(pts.len() as u32);
    for q in pts {
        blk.f64(q.x / UNIT).f64(q.y / UNIT);
    }
    let mut o = Out::default();
    o.u8(11).block(&blk.0);
    o.0
}

fn region_props(layer_word: &str, extra: &[(&str, String)]) -> Vec<(String, String)> {
    let mut p = pairs(&[
        ("V7_LAYER", layer_word.to_string()),
        ("NAME", " ".to_string()),
        ("KIND", "0".to_string()),
        ("SUBPOLYINDEX", "-1".to_string()),
        ("UNIONINDEX", "0".to_string()),
        ("ARCRESOLUTION", "0.5mil".to_string()),
        ("ISSHAPEBASED", "FALSE".to_string()),
        ("CAVITYHEIGHT", "0mil".to_string()),
    ]);
    p.extend(pairs(extra));
    p
}

/// A part's name, as text on its overlay layer: starting where its lower left is.
fn designator(text: &str, start: P, height: f64, layer: u8, mirror: bool, comp: u16) -> Vec<u8> {
    let mut b = templates::bytes(templates::TEXT);
    let old_at = b[13..21].to_vec();
    b[0] = layer;
    put_u16(&mut b, 1, FLAGS);
    put_u16(&mut b, 3, prims::NONE);
    put_u16(&mut b, 5, prims::NONE);
    put_u16(&mut b, 7, comp);
    put_mm(&mut b, 13, start.x);
    put_mm(&mut b, 17, start.y);
    put_mm(&mut b, 21, height);
    b[27..35].copy_from_slice(&0f64.to_le_bytes());
    b[35] = u8::from(mirror);
    // The start is said again further on.
    let new_at = b[13..21].to_vec();
    if let Some(i) = (36..b.len().saturating_sub(8)).find(|i| b[*i..*i + 8] == old_at[..]) {
        b[i..i + 8].copy_from_slice(&new_at);
    }
    let mut body = Out::default();
    body.pascal(text);
    let mut o = Out::default();
    o.u8(5).block(&b).block(&body.0);
    o.0
}

/// A vertex list as the text records give it (`VX0`, `VY0`, `KIND0`…).
fn vertices(ring: &[P]) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for (i, q) in ring.iter().enumerate() {
        v.push((format!("VX{i}"), mil(q.x)));
        v.push((format!("VY{i}"), mil(q.y)));
        v.push((format!("KIND{i}"), "0".into()));
        v.push((format!("CX{i}"), "0mil".into()));
        v.push((format!("CY{i}"), "0mil".into()));
        v.push((format!("SA{i}"), " 0.00000000000000E+0000".into()));
        v.push((format!("EA{i}"), " 0.00000000000000E+0000".into()));
        v.push((format!("R{i}"), "0mil".into()));
    }
    v
}

/// The layer stack and board settings.
fn board_record(b: &Board, inner: u8) -> Vec<(String, String)> {
    let mut v = pairs(&[
        ("RECORD", "Board".into()),
        ("KIND", "Protel_Advanced_PCB".into()),
        ("VERSION", "5.01".into()),
        ("ORIGINX", "0mil".into()),
        ("ORIGINY", "0mil".into()),
        ("DISPLAYUNIT", "1".into()),
        ("ARCRESOLUTION", "0.5mil".into()),
        ("SHOWSHEET", "FALSE".into()),
        ("SHEETX", "1000mil".into()),
        ("SHEETY", "1000mil".into()),
        ("SHEETWIDTH", "10000mil".into()),
        ("SHEETHEIGHT", "8000mil".into()),
        ("TRACKWIDTH", mil(b.rules.track)),
        ("MINPRIMLENGTH", "3mil".into()),
        ("TOPMATERIAL", "Solder Resist".into()),
        ("TOPHEIGHT", "0.6mil".into()),
        ("TOPCONST", "3.800".into()),
        ("TOPTYPE", "3".into()),
        ("BOTTOMMATERIAL", "Solder Resist".into()),
        ("BOTTOMHEIGHT", "0.6mil".into()),
        ("BOTTOMCONST", "3.800".into()),
        ("BOTTOMTYPE", "3".into()),
        ("POLYGONOUTLINE", "FALSE".into()),
        ("SELECTION", "FALSE".into()),
        ("LAYER", "TOP".into()),
    ]);
    v.extend(vertices(&b.outline));
    // The copper layers, top to bottom, linked.
    let mut stack: Vec<u8> = vec![prims::TOP];
    stack.extend((1..=inner).map(|k| k + 1));
    stack.push(prims::BOTTOM);
    for (i, l) in stack.iter().enumerate() {
        let name = match *l {
            prims::TOP => "Top Layer".to_string(),
            prims::BOTTOM => "Bottom Layer".to_string(),
            k => format!("Inner Layer {}", k - 1),
        };
        let bottom = i + 1 == stack.len();
        v.extend(pairs(&[
            (&format!("LAYER{l}NAME"), name),
            (&format!("LAYER{l}PREV"), if i == 0 { 0 } else { stack[i - 1] }.to_string()),
            (&format!("LAYER{l}NEXT"), if bottom { 0 } else { stack[i + 1] }.to_string()),
            (&format!("LAYER{l}COPTHICK"), "1.378mil".into()),
            (&format!("LAYER{l}DIELTYPE"), if bottom { "0" } else { "1" }.into()),
            (&format!("LAYER{l}DIELMATERIAL"), "FR-4".into()),
            (&format!("LAYER{l}DIELCONST"), "4.800".into()),
            (&format!("LAYER{l}DIELHEIGHT"), "12.6mil".into()),
            (&format!("LAYER{l}MECHENABLED"), "FALSE".into()),
        ]));
    }
    for (l, name) in [(COURT, "Courtyard"), (FAB, "Assembly")] {
        v.extend(pairs(&[(&format!("LAYER{l}NAME"), name.into()), (&format!("LAYER{l}MECHENABLED"), "TRUE".into())]));
    }
    v
}

fn flag_pairs(extra: Vec<(String, String)>, seed: &str) -> Vec<(String, String)> {
    let mut v = extra;
    v.extend(pairs(&[
        ("KEEPOUT", "FALSE".into()),
        ("LOCKED", "FALSE".into()),
        ("POLYGONOUTLINE", "FALSE".into()),
        ("SELECTION", "FALSE".into()),
        ("UNIONINDEX", "0".into()),
        ("UNIQUEID", unique_id(seed)),
        ("USERROUTED", "TRUE".into()),
    ]));
    v
}

/// One rule: its type code, then its settings.
fn rule(code: i16, kind: &str, name: &str, scope: &str, priority: usize, enabled: bool, own: Vec<(String, String)>) -> Vec<u8> {
    let mut v = pairs(&[
        ("SELECTION", "FALSE".into()),
        ("LAYER", "TOP".into()),
        ("LOCKED", "FALSE".into()),
        ("POLYGONOUTLINE", "FALSE".into()),
        ("USERROUTED", "TRUE".into()),
        ("KEEPOUT", "FALSE".into()),
        ("UNIONINDEX", "0".into()),
        ("RULEKIND", kind.into()),
        ("NETSCOPE", if kind == "Clearance" { "DifferentNets" } else { "AnyNet" }.into()),
        ("LAYERKIND", "SameLayer".into()),
        ("SCOPE1EXPRESSION", scope.into()),
        ("SCOPE2EXPRESSION", "All".into()),
        ("NAME", name.into()),
        ("ENABLED", if enabled { "TRUE" } else { "FALSE" }.into()),
        ("PRIORITY", priority.to_string()),
        ("COMMENT", String::new()),
        ("UNIQUEID", unique_id(&format!("{kind}{name}{scope}"))),
        ("DEFINEDBYLOGICALDOCUMENT", "FALSE".into()),
    ]);
    v.extend(own);
    let mut o = Out::default();
    o.u16(code as u16).block(&param_text(&v));
    o.0
}

/// Altium's table of gaps between kinds of copper: a pair may go by more than one name.
fn object_clearances(gaps: &[Gap]) -> String {
    let names = |k: Kind| -> &'static [&'static str] {
        match k {
            Kind::Track => &["Track", "Arc"],
            Kind::SmdPad => &["SMDPad"],
            Kind::ThPad => &["THPad"],
            Kind::Via => &["Via"],
            Kind::Region => &["Region", "Fill"],
            Kind::Pour => &["Poly"],
        }
    };
    let mut out = Vec::new();
    for g in gaps {
        for a in names(g.a) {
            for b in names(g.b) {
                out.push(format!("ClearanceObj_{a}-ClearanceObj_{b}:{}", (g.mm / UNIT).round() as i64));
            }
        }
    }
    out.join(";")
}

/// The limits a rule gives each layer, as its keys (`TOPLAYER_MINWIDTH`, `MIDLAYER3_MAXGAP`…).
fn layer_keys(l: &LayerLimit) -> Vec<(String, String)> {
    let prefix = match l.layer {
        TOP => "TOPLAYER_".to_string(),
        BOTTOM => "BOTTOMLAYER_".to_string(),
        k => format!("MIDLAYER{k}_"),
    };
    let mut v = Vec::new();
    for (key, val) in [("MINWIDTH", l.min_width), ("PREFWIDTH", l.pref_width), ("MAXWIDTH", l.max_width), ("MINGAP", l.min_gap), ("PREFGAP", l.pref_gap), ("MAXGAP", l.max_gap)] {
        if let Some(x) = val {
            v.push((format!("{prefix}{key}"), mil(x)));
        }
    }
    v
}

fn limit(v: f64) -> String {
    if v > 0.0 { mil(v) } else { NO_LIMIT.into() }
}

/// The board's rules and the rules asked of classes, as records; and how many general ones there were.
fn rule_records(b: &Board, pours: &[&Pour]) -> Vec<Vec<u8>> {
    let r = &b.rules;
    let class_count = |kind: ClassRuleKind| b.class_rules.iter().filter(|c| c.kind == kind).count();
    let general = |kind: ClassRuleKind| class_count(kind) + 1;
    let mut out = Vec::new();

    // The rules asked of a class, which win over the general ones.
    let mut seen: HashMap<&'static str, usize> = HashMap::new();
    for c in &b.class_rules {
        let (code, kind) = match c.kind {
            ClassRuleKind::Width => (2, "Width"),
            ClassRuleKind::PairRouting => (51, "DiffPairsRouting"),
            ClassRuleKind::MatchedLengths => (4, "MatchedLengths"),
        };
        let scope = match &c.scope {
            ClassScope::NetClass(n) => format!("InNetClass('{n}')"),
            ClassScope::PairClass(n) => format!("InDifferentialPairClass('{n}')"),
        };
        let n = seen.entry(kind).or_insert(0);
        *n += 1;
        let mut own: Vec<(String, String)> = Vec::new();
        for l in &c.layers {
            if l.layer == u8::MAX {
                for (key, val) in [("MINLIMIT", l.min_width), ("PREFEREDWIDTH", l.pref_width), ("MAXLIMIT", l.max_width), ("MINGAP", l.min_gap), ("MOSTFREQGAP", l.pref_gap), ("MAXGAP", l.max_gap)] {
                    if let Some(x) = val {
                        own.push((key.into(), mil(x)));
                    }
                }
            } else {
                own.extend(layer_keys(l));
            }
        }
        if let Some(t) = c.tolerance {
            own.push(("TOLERANCE".into(), mil(t)));
        }
        if let Some(u) = c.max_uncoupled {
            own.push(("MAXUNCOUPLEDLENGTH".into(), mil(u)));
        }
        if c.kind == ClassRuleKind::MatchedLengths {
            own.push(("CHECKNETSINDIFFPAIR".into(), if c.within_pairs { "TRUE" } else { "FALSE" }.into()));
            own.push(("CHECKDIFFPAIRVSDIFFPAIR".into(), if c.between_pairs { "TRUE" } else { "FALSE" }.into()));
        }
        out.push(rule(code, kind, &c.name, &scope, *n, true, own));
    }

    // The general rules: each for everything, after those.
    let on = |id: RuleId| r.is_on(id);
    let kv = |k: &str, v: String| (k.to_string(), v);
    let mut clearance = vec![kv("GAP", mil(r.clearance)), kv("GENERICCLEARANCE", mil(r.clearance)), kv("IGNOREPADTOPADCLEARANCEINFOOTPRINT", "TRUE".into())];
    if !r.gaps.is_empty() {
        clearance.push(kv("OBJECTCLEARANCES", object_clearances(&r.gaps)));
    }
    out.push(rule(0, "Clearance", "Clearance", "All", 2, on(RuleId::Clearance), clearance));
    out.push(rule(2, "Width", "Width", "All", general(ClassRuleKind::Width), on(RuleId::Width), vec![
        kv("MINLIMIT", mil(r.min_track)),
        kv("MAXLIMIT", limit(r.max_track)),
        kv("PREFEREDWIDTH", mil(r.track)),
    ]));
    out.push(rule(11, "RoutingVias", "RoutingVias", "All", 1, on(RuleId::RoutingVias), vec![
        kv("HOLEWIDTH", mil(r.via_drill)),
        kv("WIDTH", mil(r.via_dia)),
        kv("VIASTYLE", "Through Hole".into()),
        kv("MINHOLEWIDTH", if r.via_min_drill > 0.0 { mil(r.via_min_drill) } else { "0mil".into() }),
        kv("MINWIDTH", if r.via_min_dia > 0.0 { mil(r.via_min_dia) } else { "0mil".into() }),
        kv("MAXHOLEWIDTH", limit(r.via_max_drill)),
        kv("MAXWIDTH", limit(r.via_max_dia)),
    ]));
    out.push(rule(42, "HoleSize", "HoleSize", "All", 1, on(RuleId::HoleSize), vec![
        kv("ABSOLUTEVALUES", "TRUE".into()),
        kv("MINLIMIT", mil(r.min_drill)),
        kv("MAXLIMIT", limit(r.max_drill)),
        kv("MAXPERCENT", "80.000".into()),
        kv("MINPERCENT", "20.000".into()),
    ]));
    if r.min_ring > 0.0 {
        out.push(rule(19, "MinimumAnnularRing", "MinimumAnnularRing", "All", 1, on(RuleId::MinimumAnnularRing), vec![kv("MINIMUMRING", mil(r.min_ring))]));
    }
    out.push(rule(13, "SolderMaskExpansion", "SolderMaskExpansion", "All", 1, on(RuleId::SolderMaskExpansion), vec![kv("EXPANSION", mil(r.mask_margin))]));
    out.push(rule(63, "BoardOutlineClearance", "BoardOutlineClearance", "All", 1, on(RuleId::BoardOutlineClearance), vec![kv("GAP", mil(r.edge_clearance))]));
    out.push(rule(52, "HoleToHoleClearance", "HoleToHoleClearance", "All", 1, on(RuleId::HoleToHoleClearance), vec![kv("GAP", mil(r.hole_gap))]));
    out.push(rule(24, "ComponentClearance", "ComponentClearance", "All", 1, on(RuleId::ComponentClearance), vec![kv("GAP", mil(r.component_gap))]));
    out.push(rule(3, "Length", "Length", "All", 1, on(RuleId::Length), vec![kv("MINLIMIT", if r.min_net_length > 0.0 { mil(r.min_net_length) } else { "0mil".into() }), kv("MAXLIMIT", limit(r.max_net_length))]));
    out.push(rule(15, "ShortCircuit", "ShortCircuit", "All", 1, on(RuleId::ShortCircuit), vec![kv("ALLOWED", "FALSE".into())]));
    // How pours meet pads: the first pour's, for all of them.
    if let Some(p) = pours.iter().find(|p| !p.plane).or(pours.first()) {
        let style = if p.connect == Connect::Direct { "Direct" } else { "Relief" };
        let mut own = Vec::new();
        for what in ["THPAD", "SMDPAD"] {
            own.extend(vec![
                kv(&format!("{what}.CONNECTSTYLE"), style.into()),
                kv(&format!("{what}.RELIEFCONDUCTORWIDTH"), mil(p.spoke)),
                kv(&format!("{what}.RELIEFENTRIES"), "4".into()),
                kv(&format!("{what}.POLYGONRELIEFANGLE"), "90 Angle".into()),
                kv(&format!("{what}.AIRGAPWIDTH"), mil(p.gap)),
            ]);
        }
        own.extend(vec![kv("VIA.CONNECTSTYLE", "Direct".into()), kv("VIA.RELIEFCONDUCTORWIDTH", "5mil".into()), kv("VIA.RELIEFENTRIES", "4".into()), kv("VIA.POLYGONRELIEFANGLE", "90 Angle".into()), kv("VIA.AIRGAPWIDTH", "5mil".into())]);
        // Coppersmith's own way of saying it, which it reads back.
        own.extend(vec![kv("CONNECTSTYLE", style.into()), kv("RELIEFCONDUCTORWIDTH", mil(p.spoke)), kv("AIRGAPWIDTH", mil(p.gap))]);
        out.push(rule(20, "PolygonConnect", "PolygonConnect", "All", 2, on(RuleId::PolygonConnect), own));
        out.push(rule(12, "PlaneClearance", "PlaneClearance", "All", 1, on(RuleId::PlaneClearance), vec![kv("CLEARANCE", mil(pours.iter().find(|p| p.plane).map(|p| p.clearance).unwrap_or(0.508)))]));
    }
    out
}

/// A board as a `.PcbDoc`.
pub fn board(b: &Board) -> Result<Written, String> {
    let err = |e: std::io::Error| format!("couldn't write the board: {e}");
    let mut notes: Vec<String> = Vec::new();
    let inner = b.copper.saturating_sub(2).min(30);
    let net_index: HashMap<u32, u16> = b.nets.iter().enumerate().map(|(i, n)| (n.id, i as u16)).collect();
    let net = |id: u32| -> u16 { if id == 0 { prims::NONE } else { net_index.get(&id).copied().unwrap_or(prims::NONE) } };

    // -- Nets, parts, and everything that sits on a part --
    let mut nets = Vec::new();
    for n in &b.nets {
        nets.extend(text_record(&flag_pairs(pairs(&[("NAME", n.name.clone()), ("COLOR", "7709086".into()), ("LAYER", "TOP".into()), ("VISIBLE", "TRUE".into()), ("JUMPERSVISIBLE", "TRUE".into()), ("LOOPREMOVAL", "TRUE".into())]), &format!("net{}", n.name))));
    }
    let (mut comps, mut pads, mut tracks, mut arcs, mut regions, mut texts) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut n_pads, mut n_tracks, mut n_arcs, mut n_regions, mut n_texts) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let mut dropped_drawing = 0usize;
    let mut shared_mech = false;
    for (i, f) in b.footprints.iter().enumerate() {
        let comp = i as u16;
        let bottom = f.side == Side::Bottom;
        comps.extend(text_record(&flag_pairs(
            pairs(&[
                ("PATTERN", f.def.clone()),
                ("SOURCEDESIGNATOR", f.refdes.clone()),
                ("SOURCELIBREFERENCE", f.value.clone()),
                ("X", mil(f.at.x)),
                ("Y", mil(f.at.y)),
                ("ROTATION", format!("{:.6}", f.rot.rem_euclid(360.0))),
                ("LAYER", if bottom { "BOTTOM" } else { "TOP" }.into()),
                ("NAMEON", "TRUE".into()),
                ("COMMENTON", "FALSE".into()),
                ("NAMEAUTOPOSITION", "3".into()),
                ("COMMENTAUTOPOSITION", "3".into()),
                ("JUMPERSVISIBLE", "TRUE".into()),
                ("COUNT", "0".into()),
                ("GROUPNUM", "0".into()),
                ("CHANNELOFFSET", "0".into()),
                ("LOCKED", if f.locked { "TRUE" } else { "FALSE" }.into()),
            ]),
            &format!("part{}{}", f.refdes, f.id),
        )));
        // Pads, where they sit on the board.
        for p in &f.pads {
            let layer = if p.through() { prims::MULTI_LAYER } else if bottom { prims::BOTTOM } else { prims::TOP };
            let rot = if bottom { f.rot - p.rot } else { p.rot + f.rot };
            pads.extend(pad(p, layer, f.place(p.at), rot, net(p.net), comp));
            n_pads += 1;
        }
        // Its drawing.
        for g in &f.graphics {
            let layer = match g.layer().on(f.side) {
                Layer::Silk(Side::Top) => prims::TOP_OVERLAY,
                Layer::Silk(Side::Bottom) => prims::BOTTOM_OVERLAY,
                Layer::Court(s) | Layer::Fab(s) if s == Side::Bottom || bottom => {
                    shared_mech = true;
                    if matches!(g.layer(), Layer::Court(_)) { COURT } else { FAB }
                }
                Layer::Court(_) => COURT,
                Layer::Fab(_) => FAB,
                _ => {
                    dropped_drawing += 1;
                    continue;
                }
            };
            match g {
                Graphic::Line { a, b: e, w, .. } => {
                    tracks.extend(track(f.place(*a), f.place(*e), *w, layer, prims::NONE, comp));
                    n_tracks += 1;
                }
                Graphic::Circle { c, r, w, fill: false, .. } => {
                    arcs.extend(arc(f.place(*c), *r, 0.0, 360.0, *w, layer, prims::NONE, comp));
                    n_arcs += 1;
                }
                Graphic::Circle { c, r, fill: true, .. } => {
                    let ring: Vec<P> = (0..32).map(|k| f.place(*c).add(p((k as f64 * std::f64::consts::TAU / 32.0).cos(), (k as f64 * std::f64::consts::TAU / 32.0).sin()).scale(*r))).collect();
                    let name = if layer == prims::TOP_OVERLAY { "TOPOVERLAY" } else if layer == prims::BOTTOM_OVERLAY { "BOTTOMOVERLAY" } else { "MECHANICAL1" };
                    regions.extend(region(&ring, layer, FLAGS, prims::NONE, comp, &region_props(name, &[]).iter().map(|(k, v)| (k.as_str(), v.clone())).collect::<Vec<_>>()));
                    n_regions += 1;
                }
            }
        }
        // Its keepouts.
        for k in f.placed_keepouts() {
            let (layer, name) = match k.layer {
                None => (prims::KEEPOUT_LAYER, "KEEPOUT".to_string()),
                Some(l) => match copper_layer(l, inner) {
                    Some(a) => (a, layer_word(l)),
                    None => continue,
                },
            };
            let props = region_props(&name, &[("KEEPOUTRESTRICTIONS", k.restrict.to_altium().to_string())]);
            regions.extend(region(&k.pts, layer, FLAGS | prims::KEEPOUT_FLAG, prims::NONE, comp, &props.iter().map(|(k, v)| (k.as_str(), v.clone())).collect::<Vec<_>>()));
            n_regions += 1;
        }
        // Its name, where it is.
        let height = f.label_height.unwrap_or(b.rules.text_height);
        let w = crate::pcb::font::width(&f.refdes, height);
        let across = if bottom { -w / 2.0 } else { w / 2.0 };
        let start = f.label_pos().sub(p(across, height / 2.0));
        let layer = if bottom { prims::BOTTOM_OVERLAY } else { prims::TOP_OVERLAY };
        texts.extend(designator(".Designator", start, height, layer, bottom, comp));
        n_texts += 1;
    }

    // -- Copper that belongs to no part --
    let mut missing = 0usize;
    for t in &b.tracks {
        let Some(layer) = copper_layer(t.layer, inner) else {
            missing += 1;
            continue;
        };
        match t.mid {
            Some(m) => {
                arcs.extend(arc_track(t, m, layer, net(t.net)));
                n_arcs += 1;
            }
            None => {
                tracks.extend(track(t.a, t.b, t.w, layer, net(t.net), prims::NONE));
                n_tracks += 1;
            }
        }
    }
    let mut vias = Vec::new();
    for v in &b.vias {
        vias.extend(via(v, net(v.net)));
    }
    for f in &b.fills {
        let Some(layer) = copper_layer(f.layer, inner) else {
            missing += 1;
            continue;
        };
        let props = region_props(&layer_word(f.layer), &[]);
        regions.extend(region(&f.pts, layer, FLAGS, net(f.net), prims::NONE, &props.iter().map(|(k, v)| (k.as_str(), v.clone())).collect::<Vec<_>>()));
        n_regions += 1;
    }
    for k in &b.keepouts {
        let (layer, name) = match k.layer {
            None => (prims::KEEPOUT_LAYER, "KEEPOUT".to_string()),
            Some(l) => match copper_layer(l, inner) {
                Some(a) => (a, layer_word(l)),
                None => continue,
            },
        };
        let props = region_props(&name, &[("KEEPOUTRESTRICTIONS", k.restrict.to_altium().to_string())]);
        regions.extend(region(&k.pts, layer, FLAGS | prims::KEEPOUT_FLAG, prims::NONE, prims::NONE, &props.iter().map(|(k, v)| (k.as_str(), v.clone())).collect::<Vec<_>>()));
        n_regions += 1;
    }

    // -- Pours --
    let live: Vec<&Pour> = b.pours.iter().filter(|p| copper_layer(p.layer, inner).is_some()).collect();
    let mut polygons = Vec::new();
    for (i, p) in live.iter().enumerate() {
        let ring: Vec<P> = if p.outline.len() >= 3 { p.outline.clone() } else { b.outline.clone() };
        let name: String = format!("Polygon{}", i + 1).bytes().map(|c| c.to_string()).collect::<Vec<_>>().join(",");
        let mut v = pairs(&[
            ("NAME", name),
            ("LAYER", layer_word(p.layer)),
            ("NET", net(p.net).to_string()),
            ("POLYGONTYPE", "Polygon".into()),
            ("HATCHSTYLE", "Solid".into()),
            ("GRIDSIZE", "20mil".into()),
            ("TRACKWIDTH", mil(b.rules.track)),
            ("ARCRESOLUTION", "0.5mil".into()),
            ("MINPRIMLENGTH", "3mil".into()),
            ("POURINDEX", p.order.to_string()),
            ("POUROVER", "TRUE".into()),
            ("POUROVERSTYLE", "1".into()),
            ("OBEYPOLYGONCUTOUT", "TRUE".into()),
            ("OPTIMALVOIDROTATION", "TRUE".into()),
            ("USEOCTAGONS", "FALSE".into()),
            ("IGNOREVIOLATIONS", "FALSE".into()),
            ("SHELVED", "FALSE".into()),
            ("RESTORELAYER", "UNKNOWN".into()),
            ("RESTORENET", String::new()),
            ("PRIMITIVELOCK", "FALSE".into()),
            ("REMOVEDEAD", if p.remove_islands { "TRUE" } else { "FALSE" }.into()),
            ("REMOVEISLANDSBYAREA", if p.min_area > 0.0 { "TRUE" } else { "FALSE" }.into()),
            ("AREATHRESHOLD", format!("{:.6}", if p.min_area > 0.0 { p.min_area / (UNIT * UNIT) } else { 250_000_000_000.0 })),
            ("REMOVENECKS", if p.neck > 0.0 { "TRUE" } else { "FALSE" }.into()),
            ("NECKWIDTHTHRESHOLD", mil(if p.neck > 0.0 { p.neck } else { 0.127 })),
        ]);
        v.extend(vertices(&ring));
        polygons.extend(text_record(&flag_pairs(v, &format!("poly{}{}", i, p.id))));
    }

    // -- Rules, classes, pairs --
    let rules = rule_records(b, &live);
    let n_rules = rules.len() as u32;
    let rule_bytes: Vec<u8> = rules.into_iter().flatten().collect();
    let mut classes = Vec::new();
    let mut n_classes = 0u32;
    for c in &b.net_classes {
        let mut v = pairs(&[("NAME", c.name.clone()), ("KIND", "0".into()), ("LAYER", "MULTILAYER".into()), ("SUPERCLASS", "FALSE".into()), ("SCHAUTOGENERATEDCLUSTER", "FALSE".into()), ("SELECTED", "FALSE".into())]);
        for (i, n) in c.nets.iter().enumerate() {
            v.push((format!("M{i}"), b.net_name(*n).to_string()));
        }
        classes.extend(text_record(&flag_pairs(v, &format!("class{}", c.name))));
        n_classes += 1;
    }
    for c in &b.pair_classes {
        let mut v = pairs(&[("NAME", c.name.clone()), ("KIND", "6".into()), ("LAYER", "MULTILAYER".into()), ("SUPERCLASS", "FALSE".into()), ("SCHAUTOGENERATEDCLUSTER", "FALSE".into()), ("SELECTED", "FALSE".into())]);
        for (i, n) in c.pairs.iter().enumerate() {
            v.push((format!("M{i}"), n.clone()));
        }
        classes.extend(text_record(&flag_pairs(v, &format!("pairclass{}", c.name))));
        n_classes += 1;
    }
    let mut pair_recs = Vec::new();
    for d in &b.diff_pairs {
        let v = pairs(&[("NAME", d.name.clone()), ("POSITIVENETNAME", b.net_name(d.pos).to_string()), ("NEGATIVENETNAME", b.net_name(d.neg).to_string()), ("GATHERCONTROL", "FALSE".into()), ("LAYER", "TOP".into())]);
        pair_recs.extend(text_record(&flag_pairs(v, &format!("pair{}", d.name))));
    }

    // -- The file --
    let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).map_err(err)?;
    let put = |file: &mut cfb::CompoundFile<Cursor<Vec<u8>>>, path: &str, bytes: &[u8]| -> Result<(), String> { file.create_stream(path).map_err(err)?.write_all(bytes).map_err(err) };
    // The two headers, as the real board's are.
    let mut six = Out::default();
    six.block(&{
        let mut n = Out::default();
        n.pascal("PCB 6.0 Binary File");
        n.0
    });
    six.f64(5.01).block(&{
        let mut n = Out::default();
        n.pascal("{3F12D9DD-980A-4513-8E73-0AB4F0153C27}");
        n.0
    });
    put(&mut file, "/FileHeaderSix", &six.0)?;
    put(&mut file, "/FileHeader", &[0x13, 0, 0, 0, 0x50, 0, 0x43, 0, 0x42, 0, 0x20, 0, 0x35, 0, 0x2e, 0, 0x30, 0, 0x20, 0, 0x42, 0, 0x69, 0])?;
    let storages: Vec<(&str, Vec<u8>, u32)> = vec![
        ("Board6", text_record(&board_record(b, inner)), 1),
        ("Nets6", nets, b.nets.len() as u32),
        ("Components6", comps, b.footprints.len() as u32),
        ("Polygons6", polygons, live.len() as u32),
        ("Pads6", pads, n_pads),
        ("Vias6", vias, b.vias.len() as u32),
        ("Tracks6", tracks, n_tracks),
        ("Arcs6", arcs, n_arcs),
        ("Fills6", Vec::new(), 0),
        ("Regions6", regions, n_regions),
        ("Texts6", texts, n_texts),
        ("Rules6", rule_bytes, n_rules),
        ("Classes6", classes, n_classes),
        ("DifferentialPairs6", pair_recs, b.diff_pairs.len() as u32),
    ];
    for (name, data, count) in &storages {
        file.create_storage(format!("/{name}")).map_err(err)?;
        put(&mut file, &format!("/{name}/Data"), data)?;
        put(&mut file, &format!("/{name}/Header"), &count.to_le_bytes())?;
    }
    // The storages Altium's boards have that this has nothing for.
    for name in ["FromTos6", "Textures", "Embeddeds6", "Coordinates6", "ModelsNoEmbed", "PinPairsSection", "WaivedViolations", "EmbeddedBoards6", "PadViaLibraryLinks", "Dimensions6", "Connections6", "ComponentBodies6", "ShapeBasedComponentBodies6", "ShapeBasedRegions6", "Models"] {
        file.create_storage(format!("/{name}")).map_err(err)?;
        put(&mut file, &format!("/{name}/Data"), &[])?;
        put(&mut file, &format!("/{name}/Header"), &0u32.to_le_bytes())?;
    }

    // -- What didn't come across --
    let s = |n: usize| if n == 1 { "" } else { "s" };
    if !live.is_empty() {
        notes.push(format!("{} copper pour{} written as outlines only, with no copper poured: repour them in Altium (Tools ▸ Polygon Pours ▸ Repour All).", live.len(), s(live.len())));
    }
    let planes = live.iter().filter(|p| p.plane).count();
    if planes > 0 {
        notes.push(format!("{planes} power plane{} written as ordinary polygon pours on {}, not Altium's internal planes.", s(planes), if planes == 1 { "their layer" } else { "their layers" }));
    }
    if live.len() > 1 {
        let first = live.iter().find(|p| !p.plane).or(live.first()).unwrap();
        if live.iter().any(|p| p.connect != first.connect || (p.spoke - first.spoke).abs() > 1e-9 || (p.gap - first.gap).abs() > 1e-9) {
            notes.push("Pours that meet pads differently (thermal relief against solid, spoke and gap widths) all take the first one's: Altium has one polygon-connect rule.".into());
        }
        if live.iter().any(|p| (p.clearance - live[0].clearance).abs() > 1e-9) {
            notes.push("Each pour's own clearance isn't carried; Altium's clearance rules decide it.".into());
        }
    }
    if !b.rules.seen.is_empty() || !b.rules.off.is_empty() || b.rules.gaps.len() > 0 {
        notes.push("Only the board's general design rules are written (clearance and its table of gaps, width, vias, hole size, annular ring, solder mask, board edge, hole and part gaps, length, short circuit, polygon connect), and the width, pair and matched-length rules asked of classes. The rest of an Altium board's rules (about 70 kinds, and anything scoped by a query) aren't carried, nor are impedance profiles.".into());
    } else {
        notes.push("Only the board's general design rules are written, and the rules asked of classes; Altium fills in the rest with its own defaults.".into());
    }
    notes.push("Parts keep their footprint name, designator and value only: no link to a schematic or a component library, so Altium's updates from the schematic will show every part as different until they're matched.".into());
    notes.push("The only text written is each part's designator. No 3D bodies, dimensions, free text or other drawing is written.".into());
    notes.push("The layer stack is written with default materials and thicknesses (1.378 mil copper, 12.6 mil FR-4): set the real stack-up in Altium before ordering.".into());
    if shared_mech {
        notes.push("A part underneath has its courtyard and assembly drawing on the same mechanical layers (57 and 58) as the top ones: Altium would pair them as mirrored layers.".into());
    }
    if dropped_drawing > 0 {
        notes.push(format!("{dropped_drawing} piece{} of part drawing on solder mask, paste or copper layers weren't written.", s(dropped_drawing)));
    }
    if missing > 0 {
        notes.push(format!("{missing} track{}, fill{} or pour{} on layers the board doesn't have weren't written.", s(missing), s(missing), s(missing)));
    }
    notes.push("Every via is written as a through-hole via.".into());
    Ok(Written { bytes: file.into_inner().into_inner(), notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcb::copper::analyse;
    use crate::pcb::drc;
    use crate::pcb::kicad::pcb::read as read_kicad;

    fn there_and_back(a: &Board) -> (Board, Written) {
        let w = board(a).unwrap_or_else(|e| panic!("{e}"));
        let back = super::super::pcbdoc::read(&w.bytes, "x").unwrap_or_else(|e| panic!("what was written can't be read: {e}")).board;
        (back, w)
    }

    #[test]
    fn the_copied_records_are_the_sizes_a_real_board_has() {
        assert_eq!(templates::bytes(templates::VIA).len(), 321);
        assert_eq!(templates::bytes(templates::PAD).len(), 202);
        assert_eq!(templates::bytes(templates::TEXT).len(), 252);
        // Tracks and arcs are 49 and 60 bytes, as a real board's are.
        assert_eq!(track(p(0.0, 0.0), p(1.0, 0.0), 0.2, 1, 0, 0).len(), 1 + 4 + 49);
        assert_eq!(arc(p(0.0, 0.0), 1.0, 0.0, 360.0, 0.2, 1, 0, 0).len(), 1 + 4 + 60);
    }

    #[test]
    fn a_board_written_for_altium_reads_back_part_for_part() {
        let mut a = read_kicad(crate::pcb::kicad::pcb::tests::SMALL, "small", None).unwrap().board;
        a.net_classes.push(NetClass { name: "PWR".into(), nets: a.nets.iter().take(2).map(|n| n.id).collect() });
        let (b, w) = there_and_back(&a);
        assert_eq!((b.copper, b.footprints.len(), b.tracks.len(), b.vias.len(), b.pours.len()), (a.copper, a.footprints.len(), a.tracks.len(), a.vias.len(), a.pours.len()));
        for (x, y) in a.footprints.iter().zip(&b.footprints) {
            assert_eq!((x.refdes.as_str(), x.value.as_str(), x.def.as_str(), x.side, x.rot), (y.refdes.as_str(), y.value.as_str(), y.def.as_str(), y.side, y.rot));
            assert!(x.at.dist(y.at) < 1e-4, "{} at {:?} vs {:?}", x.refdes, x.at, y.at);
            assert_eq!(x.pads.len(), y.pads.len());
            for i in 0..x.pads.len() {
                assert!(x.pad_center(i).dist(y.pad_center(i)) < 1e-4, "{} pad {i}", x.refdes);
                assert_eq!(a.net_name(x.pads[i].net), b.net_name(y.pads[i].net));
                let (bx, by) = (x.pad_blob(i, 0.0).bbox(), y.pad_blob(i, 0.0).bbox());
                assert!((bx.w() - by.w()).abs() < 1e-4 && (bx.h() - by.h()).abs() < 1e-4, "{} pad {i} shape: {:?} vs {:?}", x.refdes, (bx.w(), bx.h()), (by.w(), by.h()));
            }
            // The silkscreen is still on its own side.
            let silk = |f: &Footprint| f.graphics.iter().filter(|g| matches!(g.layer(), Layer::Silk(_))).count();
            assert_eq!(silk(x), silk(y), "{}", x.refdes);
        }
        // Arcs are kept apart from straight tracks in the file: each kind in its own order.
        let ordered = |t: &[Track]| t.iter().filter(|t| t.mid.is_none()).chain(t.iter().filter(|t| t.mid.is_some())).cloned().collect::<Vec<_>>();
        assert_eq!(a.tracks.len(), b.tracks.len());
        for (x, y) in ordered(&a.tracks).iter().zip(ordered(&b.tracks).iter()) {
            assert_eq!((x.layer, x.mid.is_some(), a.net_name(x.net)), (y.layer, y.mid.is_some(), b.net_name(y.net)));
            // An arc comes back running counter-clockwise, which may be the other way round: the same copper.
            let same_way = x.a.dist(y.a) < 1e-4 && x.b.dist(y.b) < 1e-4;
            let other_way = x.a.dist(y.b) < 1e-4 && x.b.dist(y.a) < 1e-4 && x.mid.is_some();
            assert!((same_way || other_way) && (x.w - y.w).abs() < 1e-4, "{x:?} vs {y:?}");
            if let (Some(m), Some(n2)) = (x.mid, y.mid) {
                assert!(m.dist(n2) < 1e-3, "{m:?} vs {n2:?}");
            }
        }
        assert!(a.vias[0].at.dist(b.vias[0].at) < 1e-4 && (a.vias[0].dia - b.vias[0].dia).abs() < 1e-4 && (a.vias[0].drill - b.vias[0].drill).abs() < 1e-4);
        assert!((ring_area(&a.outline) - ring_area(&b.outline)).abs() < 1e-3);
        // The rules and the classes.
        assert!((a.rules.clearance - b.rules.clearance).abs() < 1e-4 && (a.rules.track - b.rules.track).abs() < 1e-4 && (a.rules.via_drill - b.rules.via_drill).abs() < 1e-4);
        assert_eq!(b.net_classes.len(), 1);
        assert_eq!(b.net_classes[0].nets.len(), a.net_classes[0].nets.len());
        let (p, q) = (&a.pours[0], &b.pours[0]);
        assert_eq!((p.layer, a.net_name(p.net), p.connect), (q.layer, b.net_name(q.net), q.connect));
        assert!((p.spoke - q.spoke).abs() < 1e-4 && (p.gap - q.gap).abs() < 1e-4);
        assert!(w.notes.iter().any(|n| n.contains("Repour All")), "{:?}", w.notes);
    }

    #[test]
    fn class_rules_pairs_and_gaps_survive() {
        let mut a = read_kicad(crate::pcb::kicad::pcb::tests::SMALL, "small", None).unwrap().board;
        let (n1, n2) = (a.net("DP_P"), a.net("DP_N"));
        a.diff_pairs.push(DiffPair { name: "DP".into(), pos: n1, neg: n2 });
        a.pair_classes.push(PairClass { name: "BUS".into(), pairs: vec!["DP".into()] });
        a.net_classes.push(NetClass { name: "RF".into(), nets: vec![n1] });
        a.class_rules.push(ClassRule {
            name: "W".into(),
            kind: ClassRuleKind::Width,
            scope: ClassScope::NetClass("RF".into()),
            layers: vec![LayerLimit { layer: TOP, min_width: Some(0.1), pref_width: Some(0.12), max_width: Some(0.15), ..LayerLimit::default() }],
            tolerance: None,
            max_uncoupled: None,
            within_pairs: true,
            between_pairs: false,
        });
        a.class_rules.push(ClassRule {
            name: "ML".into(),
            kind: ClassRuleKind::MatchedLengths,
            scope: ClassScope::PairClass("BUS".into()),
            layers: Vec::new(),
            tolerance: Some(0.127),
            max_uncoupled: None,
            within_pairs: true,
            between_pairs: true,
        });
        a.rules.gaps = vec![Gap { a: Kind::Track, b: Kind::Via, mm: 0.2 }, Gap { a: Kind::Pour, b: Kind::SmdPad, mm: 0.4 }];
        a.rules.set_on(RuleId::HoleToHoleClearance, true);
        a.rules.hole_gap = 0.3;
        let (b, _) = there_and_back(&a);
        assert_eq!(b.diff_pairs.len(), 1);
        assert_eq!(b.pair_classes[0].pairs, vec!["DP".to_string()]);
        assert_eq!(b.class_rules.len(), 2);
        let ant = b.net_by_name("DP_P").unwrap();
        let w = b.class_rules_for(ant).into_iter().find(|r| r.kind == ClassRuleKind::Width).expect("the width rule");
        let top = w.on(TOP).unwrap();
        assert!((top.min_width.unwrap() - 0.1).abs() < 1e-3 && (top.max_width.unwrap() - 0.15).abs() < 1e-3);
        let ml = b.class_rules_for(ant).into_iter().find(|r| r.kind == ClassRuleKind::MatchedLengths).expect("the matched lengths rule");
        assert!((ml.tolerance.unwrap() - 0.127).abs() < 1e-3 && ml.between_pairs);
        assert!((b.rules.pair(Kind::Track, Kind::Via).unwrap() - 0.2).abs() < 1e-4);
        assert!((b.rules.pair(Kind::SmdPad, Kind::Pour).unwrap() - 0.4).abs() < 1e-4);
        assert!(b.rules.is_on(RuleId::HoleToHoleClearance) && (b.rules.hole_gap - 0.3).abs() < 1e-4);
        assert!(!a.rules.is_on(RuleId::AcuteAngle) && !b.rules.is_on(RuleId::AcuteAngle));
    }

    #[test]
    fn an_altium_board_goes_to_altium_with_its_copper_exactly() {
        let path = std::path::Path::new("/Users/dal/Layouts/EZDISP1V9_LAYOUTS/EZDisplay.PcbDoc");
        if !path.exists() {
            return;
        }
        let a = crate::pcb::altium::read_board(path).unwrap().board;
        let rats = |b: &Board| analyse(b).rats.len();
        let mut bare = a.clone();
        bare.pours.clear();
        bare.fills.clear();
        let (back, w) = there_and_back(&bare);
        assert_eq!((back.footprints.len(), back.tracks.len(), back.vias.len(), back.copper, back.nets.len()), (a.footprints.len(), a.tracks.len(), a.vias.len(), a.copper, a.nets.len()));
        assert_eq!(rats(&back), rats(&bare), "tracks, vias and pads translate exactly");
        assert_eq!((back.diff_pairs.len(), back.net_classes.len(), back.class_rules.len()), (a.diff_pairs.len(), a.net_classes.len(), a.class_rules.len()));
        // With its pours it still pours and checks the same.
        let (mut whole, _) = there_and_back(&a);
        assert_eq!(whole.pours.len(), a.pours.len());
        whole.repour();
        let shorts = drc::check(&whole, &analyse(&whole)).into_iter().filter(|p| p.title == "Short circuit").count();
        assert_eq!(shorts, 0);
        assert!(w.notes.iter().any(|n| n.contains("about 70 kinds")));
    }
}
