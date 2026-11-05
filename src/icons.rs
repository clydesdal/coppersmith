//! Small line icons, drawn rather than stored so the program stays tiny.

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, vec2};

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Icon {
    Eye,
    EyeOff,
    Folder,
    Zip,
    Ruler,
    Cursor,
    Fit,
    Flip,
    Grid,
    Copy,
    Trash,
    Up,
    Down,
    Left,
    Right,
    Close,
    Picture,
    Pin,
    Filter,
    Pen,
    Undo,
    Cube,
    Redo,
    Route,
    Via,
    Outline,
    Chip,
    Save,
    New,
    Rotate,
    Export,
    Check,
    Plus,
    Search,
    Warning,
    Lock,
}

fn line(p: &Painter, at: &dyn Fn(f32, f32) -> Pos2, pts: &[(f32, f32)], s: Stroke) {
    p.add(Shape::line(pts.iter().map(|q| at(q.0, q.1)).collect(), s));
}

/// Draws an icon inside a square, on a 16 by 16 grid.
pub fn draw(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let k = rect.width().min(rect.height()) / 16.0;
    let o = rect.center() - Vec2::splat(8.0 * k);
    let at = move |x: f32, y: f32| o + vec2(x * k, y * k);
    let s = Stroke::new(1.3, color);
    let l = |pts: &[(f32, f32)]| line(p, &at, pts, s);
    match icon {
        Icon::Eye | Icon::EyeOff => {
            let eye: Vec<(f32, f32)> = (0..=24)
                .map(|i| {
                    let t = i as f32 / 24.0 * std::f32::consts::TAU;
                    (8.0 + 6.5 * t.cos(), 8.0 + 3.8 * t.sin() * (0.6 + 0.4 * t.cos().abs().powi(0)))
                })
                .collect();
            l(&eye);
            p.circle_stroke(at(8.0, 8.0), 1.9 * k, s);
            if icon == Icon::EyeOff {
                l(&[(2.8, 13.2), (13.2, 2.8)]);
            }
        }
        Icon::Folder => {
            l(&[(1.8, 12.8), (1.8, 3.4), (6.2, 3.4), (7.8, 5.2), (14.2, 5.2), (14.2, 12.8), (1.8, 12.8)]);
        }
        Icon::Zip => {
            l(&[(2.4, 2.8), (13.6, 2.8), (13.6, 13.2), (2.4, 13.2), (2.4, 2.8)]);
            l(&[(8.0, 2.8), (8.0, 4.4)]);
            l(&[(8.0, 5.6), (8.0, 7.0)]);
            l(&[(6.6, 8.2), (9.4, 8.2), (9.4, 10.8), (6.6, 10.8), (6.6, 8.2)]);
        }
        Icon::Ruler => {
            l(&[(1.6, 11.0), (11.0, 1.6), (14.4, 5.0), (5.0, 14.4), (1.6, 11.0)]);
            for (x, y) in [(4.6, 8.0), (7.0, 5.6), (9.4, 3.2)] {
                l(&[(x, y), (x + 1.6, y + 1.6)]);
            }
        }
        Icon::Cursor => {
            l(&[(3.2, 2.4), (3.2, 12.8), (6.2, 10.0), (8.2, 14.0), (10.2, 13.0), (8.2, 9.2), (12.2, 9.0), (3.2, 2.4)]);
        }
        Icon::Fit => {
            l(&[(2.2, 5.6), (2.2, 2.2), (5.6, 2.2)]);
            l(&[(10.4, 2.2), (13.8, 2.2), (13.8, 5.6)]);
            l(&[(13.8, 10.4), (13.8, 13.8), (10.4, 13.8)]);
            l(&[(5.6, 13.8), (2.2, 13.8), (2.2, 10.4)]);
        }
        Icon::Flip => {
            l(&[(8.0, 1.6), (8.0, 14.4)]);
            l(&[(5.8, 4.2), (1.8, 8.0), (5.8, 11.8), (5.8, 4.2)]);
            l(&[(10.2, 4.2), (14.2, 8.0), (10.2, 11.8), (10.2, 4.2)]);
        }
        Icon::Grid => {
            l(&[(2.2, 2.2), (13.8, 2.2), (13.8, 13.8), (2.2, 13.8), (2.2, 2.2)]);
            l(&[(6.1, 2.2), (6.1, 13.8)]);
            l(&[(9.9, 2.2), (9.9, 13.8)]);
            l(&[(2.2, 6.1), (13.8, 6.1)]);
            l(&[(2.2, 9.9), (13.8, 9.9)]);
        }
        Icon::Copy => {
            l(&[(5.6, 5.6), (13.6, 5.6), (13.6, 13.6), (5.6, 13.6), (5.6, 5.6)]);
            l(&[(10.4, 5.6), (10.4, 2.4), (2.4, 2.4), (2.4, 10.4), (5.6, 10.4)]);
        }
        Icon::Trash => {
            l(&[(2.4, 4.2), (13.6, 4.2)]);
            l(&[(6.0, 4.2), (6.0, 2.2), (10.0, 2.2), (10.0, 4.2)]);
            l(&[(3.8, 4.2), (4.6, 13.8), (11.4, 13.8), (12.2, 4.2)]);
        }
        Icon::Up => l(&[(3.4, 10.4), (8.0, 5.6), (12.6, 10.4)]),
        Icon::Down => l(&[(3.4, 5.6), (8.0, 10.4), (12.6, 5.6)]),
        Icon::Left => l(&[(10.4, 3.4), (5.6, 8.0), (10.4, 12.6)]),
        Icon::Right => l(&[(5.6, 3.4), (10.4, 8.0), (5.6, 12.6)]),
        Icon::Close => {
            l(&[(3.6, 3.6), (12.4, 12.4)]);
            l(&[(12.4, 3.6), (3.6, 12.4)]);
        }
        Icon::Picture => {
            l(&[(1.8, 2.8), (14.2, 2.8), (14.2, 13.2), (1.8, 13.2), (1.8, 2.8)]);
            l(&[(1.8, 11.2), (6.0, 7.2), (9.2, 10.2), (11.0, 8.6), (14.2, 11.4)]);
            p.circle_stroke(at(11.0, 5.8), 1.0 * k, s);
        }
        Icon::Cube => {
            l(&[(8.0, 1.8), (13.6, 4.8), (13.6, 11.2), (8.0, 14.2), (2.4, 11.2), (2.4, 4.8), (8.0, 1.8)]);
            l(&[(2.4, 4.8), (8.0, 7.8), (13.6, 4.8)]);
            l(&[(8.0, 7.8), (8.0, 14.2)]);
        }
        Icon::Pen => {
            l(&[(10.8, 2.6), (13.4, 5.2), (5.4, 13.2), (2.2, 13.8), (2.8, 10.6), (10.8, 2.6)]);
            l(&[(9.0, 4.4), (11.6, 7.0)]);
        }
        Icon::Undo => {
            l(&[(5.6, 2.6), (2.4, 5.8), (5.6, 9.0)]);
            l(&[(2.4, 5.8), (10.0, 5.8), (12.0, 6.6), (13.4, 8.6), (13.4, 10.2), (12.0, 12.2), (10.0, 13.0), (6.0, 13.0)]);
        }
        Icon::Redo => {
            l(&[(10.4, 2.6), (13.6, 5.8), (10.4, 9.0)]);
            l(&[(13.6, 5.8), (6.0, 5.8), (4.0, 6.6), (2.6, 8.6), (2.6, 10.2), (4.0, 12.2), (6.0, 13.0), (10.0, 13.0)]);
        }
        Icon::Route => {
            l(&[(2.4, 12.6), (6.4, 12.6), (11.0, 8.0), (11.0, 3.4)]);
            p.circle_stroke(at(2.4, 12.6), 1.5 * k, s);
            p.circle_stroke(at(11.0, 3.4), 1.5 * k, s);
        }
        Icon::Via => {
            p.circle_stroke(at(8.0, 8.0), 5.6 * k, s);
            p.circle_stroke(at(8.0, 8.0), 2.2 * k, s);
        }
        Icon::Outline => {
            l(&[(2.2, 4.6), (9.0, 2.2), (13.8, 6.4), (12.4, 13.6), (3.6, 12.6), (2.2, 4.6)]);
            for (x, y) in [(2.2, 4.6), (9.0, 2.2), (13.8, 6.4), (12.4, 13.6), (3.6, 12.6)] {
                p.circle_filled(at(x, y), 1.3 * k, color);
            }
        }
        Icon::Chip => {
            l(&[(4.4, 3.0), (11.6, 3.0), (11.6, 13.0), (4.4, 13.0), (4.4, 3.0)]);
            for y in [5.0, 8.0, 11.0] {
                l(&[(1.8, y), (4.4, y)]);
                l(&[(11.6, y), (14.2, y)]);
            }
        }
        Icon::Save => {
            l(&[(2.4, 2.4), (11.4, 2.4), (13.6, 4.6), (13.6, 13.6), (2.4, 13.6), (2.4, 2.4)]);
            l(&[(5.0, 2.4), (5.0, 6.0), (10.4, 6.0), (10.4, 2.4)]);
            l(&[(4.6, 13.6), (4.6, 9.4), (11.4, 9.4), (11.4, 13.6)]);
        }
        Icon::New => {
            l(&[(3.2, 1.8), (9.6, 1.8), (12.8, 5.0), (12.8, 14.2), (3.2, 14.2), (3.2, 1.8)]);
            l(&[(9.6, 1.8), (9.6, 5.0), (12.8, 5.0)]);
        }
        Icon::Rotate => {
            let arc: Vec<(f32, f32)> = (0..=18)
                .map(|i| {
                    let t = (-60.0 + i as f32 * 15.0f32).to_radians();
                    (8.0 + 5.4 * t.cos(), 8.0 + 5.4 * t.sin())
                })
                .collect();
            l(&arc);
            let end = arc[0];
            l(&[(end.0 - 3.0, end.1 - 0.4), end, (end.0 - 0.2, end.1 + 3.0)]);
        }
        Icon::Export => {
            l(&[(8.0, 10.4), (8.0, 1.8)]);
            l(&[(4.6, 5.0), (8.0, 1.8), (11.4, 5.0)]);
            l(&[(2.4, 9.0), (2.4, 14.0), (13.6, 14.0), (13.6, 9.0)]);
        }
        Icon::Check => {
            l(&[(2.6, 8.4), (6.2, 12.0), (13.4, 4.0)]);
        }
        Icon::Plus => {
            l(&[(8.0, 2.8), (8.0, 13.2)]);
            l(&[(2.8, 8.0), (13.2, 8.0)]);
        }
        Icon::Search => {
            p.circle_stroke(at(6.8, 6.8), 4.4 * k, s);
            l(&[(10.0, 10.0), (14.0, 14.0)]);
        }
        Icon::Warning => {
            l(&[(8.0, 2.0), (14.4, 13.6), (1.6, 13.6), (8.0, 2.0)]);
            l(&[(8.0, 6.4), (8.0, 9.8)]);
            p.circle_filled(at(8.0, 11.8), 0.8 * k, color);
        }
        Icon::Lock => {
            l(&[(3.4, 7.4), (12.6, 7.4), (12.6, 14.0), (3.4, 14.0), (3.4, 7.4)]);
            l(&[(5.4, 7.4), (5.4, 4.8), (6.6, 2.8), (9.4, 2.8), (10.6, 4.8), (10.6, 7.4)]);
        }
        Icon::Filter => {
            l(&[(2.0, 3.0), (14.0, 3.0), (9.4, 8.6), (9.4, 13.6), (6.6, 12.0), (6.6, 8.6), (2.0, 3.0)]);
        }
        Icon::Pin => {
            l(&[(6.0, 2.0), (10.0, 2.0), (9.4, 7.0), (11.4, 9.4), (4.6, 9.4), (6.6, 7.0), (6.0, 2.0)]);
            l(&[(8.0, 9.4), (8.0, 14.4)]);
        }
    }
}

/// The app's picture, drawn from assets/icon.svg by assets/make-icons.sh.
pub const PICTURES: [(u32, &[u8]); 8] = [
    (16, include_bytes!("../assets/icons/16.png")),
    (24, include_bytes!("../assets/icons/24.png")),
    (32, include_bytes!("../assets/icons/32.png")),
    (48, include_bytes!("../assets/icons/48.png")),
    (64, include_bytes!("../assets/icons/64.png")),
    (128, include_bytes!("../assets/icons/128.png")),
    (256, include_bytes!("../assets/icons/256.png")),
    (512, include_bytes!("../assets/icons/512.png")),
];
pub const SCALABLE: &str = include_str!("../assets/icon.svg");

/// The app's picture as plain pixels, for the window and the top bar.
pub fn picture_pixels(size: u32) -> Option<(Vec<u8>, u32)> {
    let (_, bytes) = PICTURES.iter().find(|(s, _)| *s == size)?;
    let mut reader = png::Decoder::new(std::io::Cursor::new(*bytes)).read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    (info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight).then_some((pixels, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_size_of_the_picture_is_there_and_reads() {
        for (size, _) in PICTURES {
            let (pixels, side) = picture_pixels(size).expect("a picture");
            assert_eq!(pixels.len(), (side * side * 4) as usize);
            assert!(pixels.chunks(4).any(|p| p[3] == 255), "not blank at {size}");
            assert_eq!(pixels[3], 0, "the corners are see-through at {size}");
        }
    }
}
