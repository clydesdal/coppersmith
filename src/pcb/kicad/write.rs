//! Writing a board as a KiCad board (`.kicad_pcb`, in KiCad 8's format, which 9 and 10 read)
//! and the project file (`.kicad_pro`) that holds its rules and net classes.
//!
//! What KiCad can't carry is collected as the board is written, so the person saving can
//! be told before anything is written.

use crate::pcb::geom::*;
use crate::pcb::model::*;
use crate::pcb::rulebook::RuleId;
use std::collections::HashMap;
use std::fmt::Write;

/// A board as KiCad files, and what can't go.
pub struct Written {
    pub pcb: String,
    pub project: String,
    /// What would be lost or changed, in a line each; empty if nothing.
    pub notes: Vec<String>,
}

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"))
}

/// A number without trailing zeros.
fn n(v: f64) -> String {
    let s = format!("{:.6}", if v.abs() < 5e-7 { 0.0 } else { v });
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// KiCad's name for a copper layer.
fn copper_name(l: u8) -> String {
    match l {
        TOP => "F.Cu".into(),
        BOTTOM => "B.Cu".into(),
        n => format!("In{n}.Cu"),
    }
}

/// KiCad's name for a layer a part draws on, as the board sees it.
fn drawing_layer(l: Layer) -> Option<&'static str> {
    Some(match l {
        Layer::Silk(Side::Top) => "F.SilkS",
        Layer::Silk(Side::Bottom) => "B.SilkS",
        Layer::Court(Side::Top) => "F.CrtYd",
        Layer::Court(Side::Bottom) => "B.CrtYd",
        Layer::Fab(Side::Top) => "F.Fab",
        Layer::Fab(Side::Bottom) => "B.Fab",
        _ => return None,
    })
}

/// Item numbers that look like KiCad's own, one after another, so every item has its own.
struct Ids(u64);

impl Ids {
    fn next(&mut self) -> String {
        self.0 += 1;
        format!("(uuid \"00000000-0000-4000-8000-{:012x}\")", self.0)
    }
}

/// Where KiCad's page is: a board is moved in whole millimetres, if need be, to sit on it.
fn shift_of(b: &Board) -> (f64, f64) {
    let mut bb = BBox::of(&b.outline);
    if bb.is_empty() {
        for f in &b.footprints {
            bb.add(f.at);
        }
    }
    if bb.is_empty() {
        return (0.0, 0.0);
    }
    // KiCad's y runs down, so the board's top is its largest y.
    let (min_x, min_y) = (bb.min.x, -bb.max.y);
    let dx = if min_x < 5.0 { (20.0 - min_x).ceil() } else { 0.0 };
    let dy = if min_y < 5.0 { (20.0 - min_y).ceil() } else { 0.0 };
    (dx, dy)
}

pub fn board(b: &Board, name: &str) -> Written {
    let mut notes: Vec<String> = Vec::new();
    let (dx, dy) = shift_of(b);
    if dx != 0.0 || dy != 0.0 {
        notes.push(format!("The board is moved {} mm across and {} mm down so it sits on KiCad's page (its coordinates were off it).", n(dx), n(dy)));
    }
    let at = |p: P| (p.x + dx, -p.y + dy);
    let mut ids = Ids(0);
    let mut out = String::new();

    // Nets: numbered from 1, in the order the board lists them.
    let net_index: HashMap<u32, usize> = b.nets.iter().enumerate().map(|(i, n)| (n.id, i + 1)).collect();
    let net = |id: u32| -> String {
        match net_index.get(&id) {
            Some(i) if id != 0 => format!("(net {i} {})", q(b.net_name(id))),
            _ => String::new(),
        }
    };
    let net_id = |id: u32| -> usize { if id == 0 { 0 } else { net_index.get(&id).copied().unwrap_or(0) } };

    let _ = writeln!(out, "(kicad_pcb\n\t(version 20240108)\n\t(generator \"coppersmith\")\n\t(generator_version \"{}\")", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "\t(general\n\t\t(thickness 1.6)\n\t\t(legacy_teardrops no)\n\t)\n\t(paper \"A3\")");
    let _ = writeln!(out, "\t(title_block\n\t\t(title {})\n\t)", q(name));
    // The layers, numbered as KiCad 8 has them.
    let _ = writeln!(out, "\t(layers\n\t\t(0 \"F.Cu\" signal)");
    for l in b.copper_layers().into_iter().filter(|l| *l != TOP && *l != BOTTOM) {
        let _ = writeln!(out, "\t\t({l} \"In{l}.Cu\" signal)");
    }
    out.push_str(concat!(
        "\t\t(31 \"B.Cu\" signal)\n\t\t(32 \"B.Adhes\" user \"B.Adhesive\")\n\t\t(33 \"F.Adhes\" user \"F.Adhesive\")\n\t\t(34 \"B.Paste\" user)\n\t\t(35 \"F.Paste\" user)\n",
        "\t\t(36 \"B.SilkS\" user \"B.Silkscreen\")\n\t\t(37 \"F.SilkS\" user \"F.Silkscreen\")\n\t\t(38 \"B.Mask\" user)\n\t\t(39 \"F.Mask\" user)\n",
        "\t\t(40 \"Dwgs.User\" user \"User.Drawings\")\n\t\t(41 \"Cmts.User\" user \"User.Comments\")\n\t\t(42 \"Eco1.User\" user \"User.Eco1\")\n\t\t(43 \"Eco2.User\" user \"User.Eco2\")\n",
        "\t\t(44 \"Edge.Cuts\" user)\n\t\t(45 \"Margin\" user)\n\t\t(46 \"B.CrtYd\" user \"B.Courtyard\")\n\t\t(47 \"F.CrtYd\" user \"F.Courtyard\")\n\t\t(48 \"B.Fab\" user)\n\t\t(49 \"F.Fab\" user)\n\t)\n"
    ));
    let _ = writeln!(out, "\t(setup\n\t\t(pad_to_mask_clearance {})\n\t)", n(b.rules.mask_margin));
    out.push_str("\t(net 0 \"\")\n");
    for (i, nt) in b.nets.iter().enumerate() {
        let _ = writeln!(out, "\t(net {} {})", i + 1, q(&nt.name));
    }

    // Parts.
    for f in &b.footprints {
        let bottom = f.side == Side::Bottom;
        let (fx, fy) = at(f.at);
        let layer = if bottom { "B.Cu" } else { "F.Cu" };
        let lib_id = format!("Coppersmith:{}", if f.def.is_empty() { &f.refdes } else { &f.def });
        let all_through = !f.pads.is_empty() && f.pads.iter().all(|p| p.through());
        let any_through = f.pads.iter().any(|p| p.through());
        let rot = if f.rot.rem_euclid(360.0) == 0.0 { String::new() } else { format!(" {}", n(f.rot)) };
        let _ = writeln!(out, "\t(footprint {}\n\t\t(layer \"{layer}\")\n\t\t{}\n\t\t(at {} {}{rot})", q(&lib_id), ids.next(), n(fx), n(fy));
        if f.locked {
            out.push_str("\t\t(locked yes)\n");
        }
        // Local coordinates: from a part underneath, as it looks from below.
        let lk = |p: P| -> (f64, f64) { if bottom { (-p.x, -p.y) } else { (p.x, -p.y) } };
        let size = f.label_height.unwrap_or(b.rules.text_height);
        let (silk, fab) = if bottom { ("B.SilkS", "B.Fab") } else { ("F.SilkS", "F.Fab") };
        let mirror = if bottom { " (justify mirror)" } else { "" };
        let (lx, ly) = lk(f.label_at);
        let _ = writeln!(
            out,
            "\t\t(property \"Reference\" {}\n\t\t\t(at {} {} {})\n\t\t\t(layer \"{silk}\")\n\t\t\t{}\n\t\t\t(effects (font (size {} {}) (thickness {})){mirror})\n\t\t)",
            q(&f.refdes), n(lx), n(ly), n(f.rot.rem_euclid(360.0)), ids.next(), n(size), n(size), n(size * 0.15)
        );
        let _ = writeln!(
            out,
            "\t\t(property \"Value\" {}\n\t\t\t(at 0 {} {})\n\t\t\t(layer \"{fab}\")\n\t\t\t{}\n\t\t\t(effects (font (size {} {}) (thickness {})){mirror})\n\t\t)",
            q(&f.value), n(size * 1.8), n(f.rot.rem_euclid(360.0)), ids.next(), n(size), n(size), n(size * 0.15)
        );
        let _ = writeln!(out, "\t\t(attr {})", if any_through && !all_through || all_through { "through_hole" } else { "smd" });
        for g in &f.graphics {
            // Stored in the part's own terms: a part underneath has its sides swapped back.
            let Some(name) = drawing_layer(if bottom { g.layer().flipped() } else { g.layer() }) else { continue };
            match g {
                Graphic::Line { a, b: e, w, .. } => {
                    let (ax, ay) = lk(*a);
                    let (ex, ey) = lk(*e);
                    let _ = writeln!(out, "\t\t(fp_line (start {} {}) (end {} {}) (stroke (width {}) (type solid)) (layer \"{name}\") {})", n(ax), n(ay), n(ex), n(ey), n(*w), ids.next());
                }
                Graphic::Circle { c, r, w, fill, .. } => {
                    let (cx, cy) = lk(*c);
                    let _ = writeln!(out, "\t\t(fp_circle (center {} {}) (end {} {}) (stroke (width {}) (type solid)) (fill {}) (layer \"{name}\") {})", n(cx), n(cy), n(cx + r), n(cy), n(*w), if *fill { "solid" } else { "none" }, ids.next());
                }
            }
        }
        for pad in &f.pads {
            let (px, py) = lk(pad.at);
            // The pad's angle on the board: the part's and its own, the part's own turning the other way underneath.
            let abs = if bottom { f.rot - pad.rot } else { f.rot + pad.rot }.rem_euclid(360.0);
            let angle = if abs == 0.0 { String::new() } else { format!(" {}", n(abs)) };
            let (kind, shape, layers) = if pad.bare_hole() {
                ("np_thru_hole", "circle".to_string(), "\"*.Cu\" \"*.Mask\"".to_string())
            } else if pad.through() {
                ("thru_hole", shape_name(pad), "\"*.Cu\" \"*.Mask\"".to_string())
            } else if bottom {
                ("smd", shape_name(pad), "\"B.Cu\" \"B.Mask\" \"B.Paste\"".to_string())
            } else {
                ("smd", shape_name(pad), "\"F.Cu\" \"F.Mask\" \"F.Paste\"".to_string())
            };
            let (w, h) = if matches!(pad.shape, PadShape::Circle) || pad.bare_hole() { (pad.size.x.max(pad.drill), pad.size.x.max(pad.drill)) } else { (pad.size.x, pad.size.y) };
            let rratio = match pad.shape {
                PadShape::RoundRect(k) if !pad.bare_hole() => format!(" (roundrect_rratio {})", n(k)),
                _ => String::new(),
            };
            let drill = if pad.drill > 0.0 { format!(" (drill {})", n(pad.drill)) } else { String::new() };
            let _ = writeln!(out, "\t\t(pad {} {kind} {shape}\n\t\t\t(at {} {}{angle})\n\t\t\t(size {} {})\n\t\t\t(layers {layers}){drill}{rratio}\n\t\t\t{}\n\t\t\t{}\n\t\t)", q(&pad.num), n(px), n(py), n(w), n(h), net(pad.net), ids.next());
        }
        out.push_str("\t)\n");
    }

    // Tracks, arcs and vias.
    for t in &b.tracks {
        let ((ax, ay), (ex, ey)) = (at(t.a), at(t.b));
        match t.mid {
            Some(m) => {
                let (mx, my) = at(m);
                let _ = writeln!(out, "\t(arc (start {} {}) (mid {} {}) (end {} {}) (width {}) (layer \"{}\") (net {}) {})", n(ax), n(ay), n(mx), n(my), n(ex), n(ey), n(t.w), copper_name(t.layer), net_id(t.net), ids.next());
            }
            None => {
                let _ = writeln!(out, "\t(segment (start {} {}) (end {} {}) (width {}) (layer \"{}\") (net {}) {})", n(ax), n(ay), n(ex), n(ey), n(t.w), copper_name(t.layer), net_id(t.net), ids.next());
            }
        }
    }
    for v in &b.vias {
        let (x, y) = at(v.at);
        let _ = writeln!(out, "\t(via (at {} {}) (size {}) (drill {}) (layers \"F.Cu\" \"B.Cu\") (net {}) {})", n(x), n(y), n(v.dia), n(v.drill), net_id(v.net), ids.next());
    }

    // Zones: pours, then copper fills, then the areas kept free.
    let ring_pts = |ring: &[P]| -> String { ring.iter().map(|p| { let (x, y) = at(*p); format!("(xy {} {})", n(x), n(y)) }).collect::<Vec<_>>().join(" ") };
    let mut order: Vec<usize> = (0..b.pours.len()).collect();
    order.sort_by_key(|i| b.pours[*i].order);
    let priority: HashMap<u32, usize> = order.iter().enumerate().map(|(rank, i)| (b.pours[*i].id, order.len() - 1 - rank)).collect();
    // A pour keeps from each kind of copper the more of its own clearance and the table's gap for the pair;
    // KiCad has one number for a zone, so it gets the most, and no pour comes closer to copper than it did.
    let table_gap = [Kind::Track, Kind::SmdPad, Kind::ThPad, Kind::Via, Kind::Region, Kind::Pour].iter().map(|k| b.rules.gap(Kind::Pour, *k)).fold(0.0, f64::max);
    for pour in &b.pours {
        let ring = pour.area(b);
        if ring.len() < 3 {
            continue;
        }
        let prio = priority.get(&pour.id).copied().unwrap_or(0);
        let prio = if prio > 0 { format!("\n\t\t(priority {prio})") } else { String::new() };
        let connect = if pour.connect == Connect::Direct { "yes " } else { "" };
        // Islands: all kept, or those under an area taken out, or every one that's cut off.
        let islands = if pour.remove_islands {
            String::new()
        } else if pour.min_area > 0.0 {
            format!(" (island_removal_mode 2) (island_area_min {})", n(pour.min_area))
        } else {
            " (island_removal_mode 1)".to_string()
        };
        let clearance = pour.clearance.max(table_gap);
        // KiCad has no power planes; a zone's name is free text, so a plane says so there.
        let plane = if pour.plane { "\n\t\t(name \"Power plane\")" } else { "" };
        let _ = writeln!(
            out,
            "\t(zone\n\t\t(net {})\n\t\t(net_name {})\n\t\t(layer \"{}\")\n\t\t{}{plane}\n\t\t(hatch edge 0.5){prio}\n\t\t(connect_pads {connect}(clearance {}))\n\t\t(min_thickness {})\n\t\t(filled_areas_thickness no)\n\t\t(fill{islands} (thermal_gap {}) (thermal_bridge_width {}))\n\t\t(polygon (pts {}))\n\t)",
            net_id(pour.net), q(b.net_name(pour.net)), copper_name(pour.layer), ids.next(), n(clearance), n(if pour.neck > 0.0 { pour.neck } else { 0.2 }), n(pour.gap), n(pour.spoke), ring_pts(ring)
        );
    }
    for fill in &b.fills {
        if fill.pts.len() < 3 {
            continue;
        }
        let _ = writeln!(
            out,
            "\t(zone\n\t\t(net {})\n\t\t(net_name {})\n\t\t(layer \"{}\")\n\t\t{}\n\t\t(hatch edge 0.5)\n\t\t(priority 100)\n\t\t(connect_pads yes (clearance 0))\n\t\t(min_thickness 0.1)\n\t\t(filled_areas_thickness no)\n\t\t(fill (thermal_gap 0.5) (thermal_bridge_width 0.5))\n\t\t(polygon (pts {}))\n\t)",
            net_id(fill.net), q(b.net_name(fill.net)), copper_name(fill.layer), ids.next(), ring_pts(&fill.pts)
        );
    }
    let keepouts = b.placed_keepouts();
    for k in &keepouts {
        if k.pts.len() < 3 {
            continue;
        }
        let layers = match k.layer {
            Some(l) => format!("(layer \"{}\")", copper_name(l)),
            None => format!("(layers {})", b.copper_layers().iter().map(|l| format!("\"{}\"", copper_name(*l))).collect::<Vec<_>>().join(" ")),
        };
        let r = &k.restrict;
        let allow = |on: bool| if on { "not_allowed" } else { "allowed" };
        let _ = writeln!(
            out,
            "\t(zone\n\t\t(net 0)\n\t\t(net_name \"\")\n\t\t{layers}\n\t\t{}\n\t\t(hatch edge 0.5)\n\t\t(connect_pads (clearance 0))\n\t\t(min_thickness 0.25)\n\t\t(filled_areas_thickness no)\n\t\t(keepout (tracks {}) (vias {}) (pads {}) (copperpour {}) (footprints allowed))\n\t\t(fill (thermal_gap 0.5) (thermal_bridge_width 0.5))\n\t\t(polygon (pts {}))\n\t)",
            ids.next(), allow(r.tracks), allow(r.vias), allow(r.smd_pads || r.th_pads), allow(r.pours), ring_pts(&k.pts)
        );
    }

    // The edge.
    let m = b.outline.len();
    for i in 0..m {
        let ((ax, ay), (ex, ey)) = (at(b.outline[i]), at(b.outline[(i + 1) % m]));
        let _ = writeln!(out, "\t(gr_line (start {} {}) (end {} {}) (stroke (width 0.1) (type solid)) (layer \"Edge.Cuts\") {})", n(ax), n(ay), n(ex), n(ey), ids.next());
    }
    out.push_str(")\n");

    // What KiCad can't hold.
    losses(b, &keepouts, &mut notes);
    let project = project_file(b, name);
    Written { pcb: out, project, notes }
}

fn shape_name(pad: &Pad) -> String {
    match pad.shape {
        PadShape::Circle => "circle",
        PadShape::Rect => "rect",
        PadShape::RoundRect(_) => "roundrect",
        PadShape::Oval => "oval",
    }
    .to_string()
}

/// A line each for what KiCad can't carry of this board.
fn losses(b: &Board, keepouts: &[PlacedKeepout], notes: &mut Vec<String>) {
    let pours = b.pours.len();
    if pours > 0 {
        notes.push(format!("{pours} copper pour{} written as zone{} without their copper: KiCad fills zones itself, so press B in KiCad to fill {}.", s(pours), s(pours), if pours == 1 { "it" } else { "them" }));
    }
    let planes = b.pours.iter().filter(|p| p.plane).count();
    if planes > 0 {
        notes.push(format!("{planes} power plane{} become ordinary zones, named \"Power plane\" (KiCad has no plane layers, and fills them by its own rules: a plane's holes may be connected differently).", s(planes)));
    }
    let pullbacks = b.pours.iter().filter(|p| (p.edge - b.rules.edge_clearance).abs() > 1e-6).count();
    if pullbacks > 0 {
        notes.push(format!("{pullbacks} pour{} keep back from the board edge by their own amount: KiCad uses its one copper-to-edge clearance ({} mm) for every zone.", s(pullbacks), n(b.rules.edge_clearance)));
    }
    if !b.fills.is_empty() {
        notes.push(format!("{} solid copper region{} become zones, which keep KiCad's clearance from other nets (a region keeps none here).", b.fills.len(), s(b.fills.len())));
    }
    if b.footprints.iter().any(|f| !f.keepouts.is_empty()) {
        let k = keepouts.iter().filter(|k| k.owner.is_some()).count();
        notes.push(format!("{k} keepout{} on parts become board-level rule areas: they no longer move with the part.", s(k)));
    }
    if !b.footprints.is_empty() {
        notes.push("Parts keep their pads and drawing but are named \"Coppersmith:…\", a library KiCad doesn't have: it will say they aren't in a library when you update from libraries.".into());
        notes.push("Solder mask and paste openings are KiCad's own for each pad (only the mask margin is carried); pad numbers, shapes, holes and nets are kept. No 3D models.".into());
    }
    if !b.rules.gaps.is_empty() {
        let most = [Kind::Track, Kind::SmdPad, Kind::ThPad, Kind::Via, Kind::Region, Kind::Pour].iter().map(|k| b.rules.gap(Kind::Pour, *k)).fold(0.0, f64::max);
        notes.push(format!(
            "{} special clearance{} for pairs of kinds of copper (track to pad, via to track…) aren't carried: KiCad has one clearance per net class, so the general {} mm is used{}.",
            b.rules.gaps.len(), s(b.rules.gaps.len()), n(b.rules.clearance),
            if most > b.rules.clearance + 1e-9 { format!(", and each zone is given the most a pour kept from anything ({} mm) so it stays as far from copper as it was", n(most)) } else { String::new() }
        ));
    }
    let class_rules = b.class_rules.iter().filter(|r| !matches!(r.kind, ClassRuleKind::Width) || r.layers.iter().any(|l| l.layer != u8::MAX)).count();
    if class_rules > 0 {
        notes.push(format!("{class_rules} net-class rule{} (widths per layer, differential pair routing, matched lengths) aren't carried; KiCad's net classes keep one preferred track width each.", s(class_rules)));
    }
    if !b.diff_pairs.is_empty() {
        notes.push(format!("{} differential pair{} aren't carried as pairs: KiCad finds them by net name (_P/_N, +/−).", b.diff_pairs.len(), s(b.diff_pairs.len())));
    }
    let unmatched = b.rules.seen.iter().filter(|r| !matches!(r.rule, Some(RuleId::Clearance | RuleId::Width | RuleId::RoutingVias | RuleId::HoleSize | RuleId::MinimumAnnularRing | RuleId::BoardOutlineClearance | RuleId::HoleToHoleClearance | RuleId::SolderMaskExpansion))).count();
    if unmatched > 0 {
        notes.push(format!("{unmatched} other design rules read from the Altium file (testpoints, fanout, routing layers, silkscreen, mask slivers, supplies…) aren't carried."));
    }
    let mut off = b.rules.off.clone();
    let mut usual = Rules::default().off;
    off.sort();
    usual.sort();
    if off != usual {
        notes.push("Which rule checks are switched on or off isn't carried: KiCad sets those in its own board setup.".into());
    }
    if b.tracks.iter().any(|t| t.layer != TOP && t.layer != BOTTOM && t.layer >= b.copper.saturating_sub(1)) {
        notes.push("Tracks on layers the board doesn't have are written to inner layers KiCad's file doesn't list.".into());
    }
    if b.copper > 2 {
        notes.push("The layer stack-up (thicknesses, materials) isn't carried: KiCad starts from its default.".into());
    }
    if b.outline.len() < 3 {
        notes.push("The board has no outline, so there is no Edge.Cuts: KiCad will need one drawn.".into());
    }
}

fn s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The project file: the rules and the net classes, in the form KiCad's own settings have.
fn project_file(b: &Board, name: &str) -> String {
    use serde_json::json;
    let r = &b.rules;
    let class = |name: &str, track: f64| {
        json!({
            "bus_width": 12, "clearance": r.clearance, "diff_pair_gap": 0.25, "diff_pair_via_gap": 0.25, "diff_pair_width": 0.2, "line_style": 0,
            "microvia_diameter": 0.3, "microvia_drill": 0.1, "name": name, "pcb_color": "rgba(0, 0, 0, 0.000)", "schematic_color": "rgba(0, 0, 0, 0.000)",
            "track_width": track, "via_diameter": r.via_dia, "via_drill": r.via_drill, "wire_width": 6
        })
    };
    let mut classes = vec![class("Default", r.track)];
    let mut assignments = serde_json::Map::new();
    for c in &b.net_classes {
        // The width its rule prefers, if it has one.
        let width = b.class_rules.iter().find(|x| matches!(&x.scope, ClassScope::NetClass(n) if n == &c.name) && x.kind == ClassRuleKind::Width).and_then(|x| x.layers.iter().find_map(|l| l.pref_width)).unwrap_or(r.track);
        classes.push(class(&c.name, width));
        for id in &c.nets {
            assignments.insert(b.net_name(*id).to_string(), json!(c.name));
        }
    }
    let v = json!({
        "board": { "design_settings": { "rules": {
            "min_clearance": r.clearance, "min_track_width": r.min_track, "min_through_hole_diameter": r.min_drill, "min_via_annular_width": r.min_ring,
            "min_copper_edge_clearance": r.edge_clearance, "min_hole_to_hole": r.hole_gap, "min_via_diameter": if r.via_min_dia > 0.0 { r.via_min_dia } else { r.via_dia.min(r.via_drill + 2.0 * r.min_ring) }
        } } },
        "meta": { "filename": format!("{name}.kicad_pro"), "version": 1 },
        "net_settings": { "classes": classes, "meta": { "version": 3 }, "net_colors": null, "netclass_assignments": assignments, "netclass_patterns": [] }
    });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::pcb::copper::analyse;
    use crate::pcb::drc;
    use crate::pcb::kicad::pcb::read;

    /// A board as KiCad files and back again.
    fn there_and_back(b: &Board) -> (Board, Written) {
        let w = board(b, "x");
        let back = read(&w.pcb, "x", Some(&w.project)).unwrap_or_else(|e| panic!("what was written can't be read: {e}")).board;
        (back, w)
    }

    /// How far the board was moved to sit on KiCad's page.
    fn moved(a: &Board, b: &Board) -> P {
        BBox::of(&b.outline).min.sub(BBox::of(&a.outline).min)
    }

    #[test]
    fn a_board_written_for_kicad_reads_back_part_for_part() {
        let a = read(crate::pcb::kicad::pcb::tests::SMALL, "small", None).unwrap().board;
        let (b, w) = there_and_back(&a);
        let d = moved(&a, &b);
        assert_eq!((b.copper, b.footprints.len(), b.tracks.len(), b.vias.len(), b.pours.len()), (a.copper, a.footprints.len(), a.tracks.len(), a.vias.len(), a.pours.len()));
        for (x, y) in a.footprints.iter().zip(&b.footprints) {
            assert_eq!((x.refdes.as_str(), x.value.as_str(), x.side, x.rot), (y.refdes.as_str(), y.value.as_str(), y.side, y.rot));
            assert!(x.at.add(d).dist(y.at) < 1e-6);
            assert_eq!(x.pads.len(), y.pads.len());
            for i in 0..x.pads.len() {
                assert!(x.pad_center(i).add(d).dist(y.pad_center(i)) < 1e-6, "{} pad {i}: {:?} vs {:?}", x.refdes, x.pad_center(i).add(d), y.pad_center(i));
                assert_eq!(a.net_name(x.pads[i].net), b.net_name(y.pads[i].net));
                // The copper is the same size and turned the same way.
                let (bx, by) = (x.pad_blob(i, 0.0).bbox(), y.pad_blob(i, 0.0).bbox());
                assert!((bx.w() - by.w()).abs() < 1e-6 && (bx.h() - by.h()).abs() < 1e-6, "{} pad {i} shape", x.refdes);
            }
            assert_eq!(x.graphics.len(), y.graphics.len());
            // The silkscreen is still on its own side of the board.
            for (g, h) in x.graphics.iter().zip(&y.graphics) {
                assert_eq!(g.layer().on(x.side), h.layer().on(y.side));
            }
        }
        for (x, y) in a.tracks.iter().zip(&b.tracks) {
            assert!(x.a.add(d).dist(y.a) < 1e-6 && x.b.add(d).dist(y.b) < 1e-6 && x.w == y.w && x.layer == y.layer);
            assert_eq!((x.mid.is_some(), a.net_name(x.net)), (y.mid.is_some(), b.net_name(y.net)));
            if let (Some(m), Some(n2)) = (x.mid, y.mid) {
                assert!(m.add(d).dist(n2) < 1e-6);
            }
        }
        assert!(a.vias[0].at.add(d).dist(b.vias[0].at) < 1e-6 && a.vias[0].dia == b.vias[0].dia && a.vias[0].drill == b.vias[0].drill);
        assert!((ring_area(&a.outline) - ring_area(&b.outline)).abs() < 1e-6);
        let (p, q2) = (&a.pours[0], &b.pours[0]);
        assert_eq!((p.layer, a.net_name(p.net), p.clearance, p.neck, p.gap, p.spoke, p.connect), (q2.layer, b.net_name(q2.net), q2.clearance, q2.neck, q2.gap, q2.spoke, q2.connect));
        // The board was moved onto KiCad's page and the notes say so.
        assert!(w.notes.iter().any(|n| n.contains("moved")), "{:?}", w.notes);
        assert!(w.notes.iter().any(|n| n.contains("press B")), "the pour is unfilled: {:?}", w.notes);
    }

    #[test]
    fn the_rules_and_net_classes_go_in_the_project_file_and_come_back() {
        let mut a = read(crate::pcb::kicad::pcb::tests::SMALL, "small", None).unwrap().board;
        a.rules.clearance = 0.15;
        a.rules.min_track = 0.12;
        a.rules.min_drill = 0.25;
        a.rules.min_ring = 0.09;
        a.rules.edge_clearance = 0.4;
        a.rules.track = 0.3;
        let gnd = a.net_by_name("GND").unwrap();
        a.net_classes.push(NetClass { name: "pwr".into(), nets: vec![gnd] });
        a.class_rules.push(ClassRule { name: "Width_pwr".into(), kind: ClassRuleKind::Width, scope: ClassScope::NetClass("pwr".into()), layers: vec![LayerLimit { layer: u8::MAX, pref_width: Some(0.8), ..LayerLimit::default() }], tolerance: None, max_uncoupled: None, within_pairs: true, between_pairs: false });
        let (b, w) = there_and_back(&a);
        assert_eq!((b.rules.clearance, b.rules.min_track, b.rules.min_drill, b.rules.min_ring, b.rules.edge_clearance, b.rules.track), (0.15, 0.12, 0.25, 0.09, 0.4, 0.3));
        assert_eq!(b.net_classes.len(), 1);
        assert_eq!(b.net_classes[0].nets.len(), 1);
        assert_eq!(b.net_name(b.net_classes[0].nets[0]), "GND");
        assert!(b.class_rules.iter().any(|c| c.layers[0].pref_width == Some(0.8)));
        // A class with only a preferred width loses nothing that KiCad can hold.
        assert!(!w.notes.iter().any(|n| n.contains("net-class")), "{:?}", w.notes);
    }

    #[test]
    fn what_kicad_cannot_hold_is_listed_for_a_board_that_has_it() {
        let mut a = read(crate::pcb::kicad::pcb::tests::SMALL, "small", None).unwrap().board;
        a.rules.gaps.push(Gap { a: Kind::Track, b: Kind::Via, mm: 0.3 });
        a.class_rules.push(ClassRule { name: "Match".into(), kind: ClassRuleKind::MatchedLengths, scope: ClassScope::PairClass("BUS".into()), layers: vec![], tolerance: Some(0.1), max_uncoupled: None, within_pairs: true, between_pairs: false });
        a.diff_pairs.push(DiffPair { name: "D".into(), pos: 1, neg: 2 });
        let id = a.new_id();
        a.fills.push(Fill { id, layer: TOP, net: 1, pts: rect_outline(p(1.0, 1.0), p(5.0, 5.0)) });
        let w = board(&a, "x");
        for want in ["special clearance", "net-class rule", "differential pair", "solid copper region", "press B"] {
            assert!(w.notes.iter().any(|n| n.contains(want)), "no note about {want}: {:?}", w.notes);
        }
        // A bare board has next to nothing to lose.
        let bare = board(&Board::default(), "x");
        assert!(bare.notes.iter().all(|n| n.contains("outline") || n.contains("moved")) || bare.notes.is_empty(), "{:?}", bare.notes);
    }

    #[test]
    fn kicads_demo_boards_survive_a_round_trip_connected() {
        for name in ["pic.kicad_pcb", "video.kicad_pcb"] {
            let dir = std::env::var_os("COPPERSMITH_KICAD_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| "/Users/dal/Documents/examplelayouts/kicad".into());
            let Some(path) = Some(dir.join(name)).filter(|p| p.exists()) else { continue };
            let a = crate::pcb::kicad::read_board(&path).unwrap().board;
            let (mut b, _) = there_and_back(&a);
            assert_eq!((b.footprints.len(), b.tracks.len(), b.vias.len(), b.pours.len(), b.copper), (a.footprints.len(), a.tracks.len(), a.vias.len(), a.pours.len(), a.copper), "{name}");
            b.repour();
            let c = analyse(&b);
            assert!(c.rats.is_empty(), "{name}: {} connections broken by the round trip", c.rats.len());
            let errors: Vec<String> = drc::check(&b, &c).into_iter().filter(|p| p.severity == drc::Severity::Error).map(|p| format!("{}: {}", p.title, p.detail)).collect();
            assert!(errors.is_empty(), "{name}: {errors:?}");
        }
    }

    #[test]
    fn an_altium_board_goes_to_kicad_with_its_copper_exactly_and_says_what_changes() {
        let path = std::path::Path::new("/Users/dal/Layouts/EZDISP1V9_LAYOUTS/EZDisplay.PcbDoc");
        if !path.exists() {
            return;
        }
        let a = crate::pcb::altium::read_board(path).unwrap().board;
        let (mut b, w) = there_and_back(&a);
        assert_eq!((b.footprints.len(), b.tracks.len(), b.vias.len(), b.copper), (a.footprints.len(), a.tracks.len(), a.vias.len(), a.copper));
        // Each solid copper region of the Altium board comes back as a zone.
        assert_eq!(b.pours.len(), a.pours.len() + a.fills.iter().filter(|f| f.pts.len() >= 3).count());
        b.repour();
        let c = analyse(&b);
        let shorts = drc::check(&b, &c).into_iter().filter(|p| p.title == "Short circuit").count();
        assert_eq!(shorts, 0, "the copper hasn't moved relative to itself");

        // Without pours and regions, which KiCad fills for itself, the copper comes through exactly: the same pads, tracks
        // and vias join the same things, so the same connections are open (the pours are what closes the rest).
        let rats = |b: &Board| analyse(b).rats.len();
        let mut bare = a.clone();
        bare.pours.clear();
        bare.fills.clear();
        let (back, _) = there_and_back(&bare);
        assert_eq!(rats(&back), rats(&bare), "tracks, vias and pads translate exactly");

        // The translation says what it loses: this board has rules KiCad can't hold.
        for want in ["net-class rule", "other design rules", "differential pair", "press B", "special clearance", "most a pour kept"] {
            assert!(w.notes.iter().any(|n| n.contains(want)), "no note about {want}: {:?}", w.notes);
        }
    }

    #[test]
    fn numbers_are_written_plainly() {
        assert_eq!(n(0.0), "0");
        assert_eq!(n(-0.0), "0");
        assert_eq!(n(1.5), "1.5");
        assert_eq!(n(10.0), "10");
        assert_eq!(n(0.1234567), "0.123457");
        assert_eq!(n(-3.25), "-3.25");
        assert_eq!(q("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
