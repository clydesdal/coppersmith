//! A small made-up board to start with: a 555 timer blinking an LED, with a
//! few tracks laid and the rest still to route.

use crate::pcb::geom::*;
use crate::pcb::library;
use crate::pcb::model::*;

fn put(b: &mut Board, name: &str, refdes: &str, value: &str, at: P, rot: f64, nets: &[(&str, &str)]) {
    let defs = library::built_in();
    let d = defs.iter().find(|d| d.name == name).expect("a built-in footprint");
    let id = b.new_id();
    let mut f = d.instantiate(id, refdes.into(), at);
    f.value = value.into();
    f.rot = rot;
    for (pad, net) in nets {
        let n = b.net(net);
        for pd in f.pads.iter_mut().filter(|pd| pd.num == *pad) {
            pd.net = n;
        }
    }
    b.footprints.push(f);
}

fn track(b: &mut Board, pts: &[(f64, f64)], layer: u8, net: &str) {
    let n = b.net(net);
    for w in pts.windows(2) {
        let id = b.new_id();
        b.tracks.push(Track { mid: None, id, a: p(w[0].0, w[0].1), b: p(w[1].0, w[1].1), w: 0.3, layer, net: n });
    }
}

pub fn board() -> Board {
    let mut b = Board { name: "Blinky 555".into(), outline: rect_outline(p(0.0, 0.0), p(50.0, 35.0)), ..Board::default() };
    put(
        &mut b,
        "SOIC-8",
        "U1",
        "NE555",
        p(25.0, 18.0),
        0.0,
        &[("1", "GND"), ("2", "THR"), ("3", "OUT"), ("4", "VCC"), ("5", "CTRL"), ("6", "THR"), ("7", "DIS"), ("8", "VCC")],
    );
    put(&mut b, "R_0805", "R1", "10k", p(34.0, 24.0), 0.0, &[("1", "VCC"), ("2", "DIS")]);
    put(&mut b, "R_0805", "R2", "47k", p(34.0, 18.0), 0.0, &[("1", "DIS"), ("2", "THR")]);
    put(&mut b, "C_0805", "C1", "10uF", p(34.0, 12.0), 0.0, &[("1", "THR"), ("2", "GND")]);
    put(&mut b, "C_0805", "C2", "10nF", p(29.0, 10.0), 90.0, &[("1", "CTRL"), ("2", "GND")]);
    put(&mut b, "R_0805", "R3", "470", p(16.0, 22.0), 0.0, &[("1", "OUT"), ("2", "LED")]);
    put(&mut b, "LED_0805", "D1", "Red", p(16.0, 27.0), 0.0, &[("1", "GND"), ("2", "LED")]);
    put(&mut b, "PinHeader_1x02", "J1", "Power", p(6.0, 17.5), 0.0, &[("1", "VCC"), ("2", "GND")]);
    for (i, (x, y)) in [(3.5, 3.5), (46.5, 3.5), (46.5, 31.5), (3.5, 31.5)].iter().enumerate() {
        put(&mut b, "MountingHole_M3", &format!("H{}", i + 1), "M3", p(*x, *y), 0.0, &[]);
    }
    track(&mut b, &[(27.475, 18.635), (29.5, 18.635), (30.135, 18.0), (33.0875, 18.0)], TOP, "DIS");
    track(&mut b, &[(16.9125, 22.0), (16.9125, 27.0)], TOP, "LED");
    track(&mut b, &[(22.525, 19.905), (20.5, 19.905)], TOP, "GND");
    track(&mut b, &[(20.5, 19.905), (9.675, 19.905), (6.0, 16.23)], BOTTOM, "GND");
    let gnd = b.net("GND");
    let id = b.new_id();
    b.vias.push(Via { id, at: p(20.5, 19.905), dia: 0.6, drill: 0.3, net: gnd });
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_has_no_errors_only_things_to_route() {
        let b = board();
        let c = crate::pcb::copper::analyse(&b);
        let found = crate::pcb::drc::check(&b, &c);
        let errors: Vec<_> = found.iter().filter(|p| p.severity == crate::pcb::drc::Severity::Error).collect();
        assert!(errors.is_empty(), "{errors:#?}");
        assert!(!c.rats.is_empty());
        // The ground track and via join U1 to J1.
        let u1 = b.footprints.iter().find(|f| f.refdes == "U1").unwrap().id;
        let j1 = b.footprints.iter().find(|f| f.refdes == "J1").unwrap().id;
        assert!(c.connected(crate::pcb::copper::Item::Pad(u1, 0)).contains(&crate::pcb::copper::Item::Pad(j1, 1)));
    }
}
