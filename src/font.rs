//! A plain stroke font for silkscreen names. The same strokes are drawn on
//! screen and written to the Gerbers, so what you see is what gets printed.
//!
//! Each glyph sits on a 4 × 6 grid. A stroke is a run of digit pairs (x then
//! y); strokes are separated by spaces.

use crate::geom::*;

fn glyph(c: char) -> &'static str {
    match c.to_ascii_uppercase() {
        'A' => "002640 1333",
        'B' => "00063645443303 3342413000",
        'C' => "4536160501103041",
        'D' => "00062644422000",
        'E' => "46060040 0333",
        'F' => "460600 0333",
        'G' => "45361605011030414323",
        'H' => "0006 4046 0343",
        'I' => "1636 2620 1030",
        'J' => "4641301001",
        'K' => "0006 4602 1340",
        'L' => "060040",
        'M' => "0006234640",
        'N' => "00064046",
        'O' => "100105163645413010",
        '0' => "100105163645413010 0145",
        'P' => "00063645443303",
        'Q' => "100105163645413010 2240",
        'R' => "00063645443303 2340",
        'S' => "453616050413334241301001",
        'T' => "0646 2620",
        'U' => "060110304146",
        'V' => "062046",
        'W' => "0610233046",
        'X' => "0046 0640",
        'Y' => "0623 4623 2320",
        'Z' => "06464000",
        '1' => "042620 0040",
        '2' => "05163645440040",
        '3' => "0516364544334241301001 1333",
        '4' => "30360242",
        '5' => "4606033342413000",
        '6' => "4536160501103041423303",
        '7' => "064610",
        '8' => "13040516364544331302011030414233",
        '9' => "0110304145361605041343",
        '-' => "1333",
        '+' => "1333 2224",
        '.' => "2021",
        ',' => "2110",
        '_' => "0040",
        '/' => "0046",
        '(' => "36252130",
        ')' => "16252110",
        ':' => "2425 2021",
        '=' => "0444 0242",
        '#' => "1015 3035 0242 0444",
        ' ' => "",
        _ => "0516364544232222 2021",
    }
}

const ADVANCE: f64 = 6.0;

/// The strokes of a line of text, centred on `at`, `height` tall.
pub fn strokes(text: &str, at: P, height: f64, rot: f64, mirror: bool) -> Vec<Vec<P>> {
    let k = height / 6.0;
    let n = text.chars().count() as f64;
    let width = (n * ADVANCE - 2.0).max(0.0);
    let mut out = Vec::new();
    for (i, c) in text.chars().enumerate() {
        for stroke in glyph(c).split_whitespace() {
            let digits: Vec<f64> = stroke.bytes().map(|b| (b - b'0') as f64).collect();
            let pts: Vec<P> = digits
                .chunks(2)
                .map(|xy| {
                    let local = p((i as f64 * ADVANCE + xy[0] - width / 2.0) * k, (xy[1] - 3.0) * k);
                    let local = if mirror { p(-local.x, local.y) } else { local };
                    at.add(local.rot(rot))
                })
                .collect();
            out.push(pts);
        }
    }
    out
}

/// How wide a line of text is.
#[allow(dead_code)]
pub fn width(text: &str, height: f64) -> f64 {
    (text.chars().count() as f64 * ADVANCE - 2.0).max(0.0) * height / 6.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_whole_pairs_on_the_grid() {
        for c in (b'A'..=b'Z').chain(b'0'..=b'9').map(|b| b as char).chain("-+._/():=#?".chars()) {
            for s in glyph(c).split_whitespace() {
                assert!(s.len() % 2 == 0 && s.len() >= 4, "{c}: {s}");
                for (i, b) in s.bytes().enumerate() {
                    let v = b - b'0';
                    assert!(if i % 2 == 0 { v <= 4 } else { v <= 6 }, "{c}: {s}");
                }
            }
        }
    }

    #[test]
    fn text_is_centred() {
        let s = strokes("R1", p(10.0, 5.0), 1.2, 0.0, false);
        let b = BBox::of(&s.concat());
        assert!((b.center().x - 10.0).abs() < 0.2 && (b.center().y - 5.0).abs() < 0.01);
        assert!((b.h() - 1.2).abs() < 1e-9);
    }
}
