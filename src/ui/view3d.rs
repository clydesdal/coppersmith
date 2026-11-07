//! The 3D view of the board: turning the solid board round, and its toolbar.
//! After Copperview's `view3d`; the drawing is in `gpu3d`, the solid in
//! `pcb::solid`.

use crate::pcb::solid::Solid;
use crate::ui::gpu3d::{self, Shared, Shot};
use crate::ui::panels::{chip, segmented};
use crate::ui::theme::{self, FAINT, FG, LINE};
use eframe::egui::{self, Align2, Color32, CornerRadius, CursorIcon, PointerButton, Rect, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2};
use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::{Arc, Mutex};

const FOV: f32 = 30.0 * PI / 180.0;
const TURN_PER_POINT: f32 = 0.008;

/// The solder mask's colours: green first, as it's the usual one.
pub const BOARD_COLORS: [(&str, [f32; 3]); 6] = [
    ("Green", [0.04, 0.30, 0.17]),
    ("Black", [0.05, 0.05, 0.06]),
    ("Blue", [0.05, 0.14, 0.42]),
    ("Red", [0.46, 0.06, 0.07]),
    ("Purple", [0.24, 0.09, 0.38]),
    ("White", [0.78, 0.78, 0.76]),
];
const THICKNESSES: [(f32, &str); 4] = [(0.8, "0.8"), (1.0, "1.0"), (1.6, "1.6"), (2.0, "2.0")];

type V3 = [f32; 3];
type M4 = [f32; 16];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt().max(1e-9);
    [a[0] / l, a[1] / l, a[2] / l]
}

/// The view matrix for an eye looking at a target (column-major).
fn look_at(eye: V3, target: V3, up: V3) -> M4 {
    let f = norm(sub(target, eye));
    let s = norm(cross(f, up));
    let u = cross(s, f);
    [s[0], u[0], -f[0], 0.0, s[1], u[1], -f[1], 0.0, s[2], u[2], -f[2], 0.0, -dot(s, eye), -dot(u, eye), dot(f, eye), 1.0]
}

fn perspective(aspect: f32, near: f32, far: f32) -> M4 {
    let t = 1.0 / (FOV * 0.5).tan();
    [t / aspect, 0.0, 0.0, 0.0, 0.0, t, 0.0, 0.0, 0.0, 0.0, (far + near) / (near - far), -1.0, 0.0, 0.0, 2.0 * far * near / (near - far), 0.0]
}

/// a × b, column-major.
fn mul(a: &M4, b: &M4) -> M4 {
    let mut out = [0.0; 16];
    for j in 0..4 {
        for r in 0..4 {
            out[j * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[j * 4 + k]).sum();
        }
    }
    out
}

fn apply(m: &M4, v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (r, o) in out.iter_mut().enumerate() {
        *o = (0..4).map(|k| m[k * 4 + r] * v[k]).sum();
    }
    out
}

pub struct View3d {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub target: V3,
    pub color: usize,
    pub thick: f32,
    /// Fill the room, and keep doing so as it changes, until zoomed or moved.
    pub auto_fit: bool,
    pub solid: Option<Arc<Solid>>,
    /// Copper, mask and silk, shown or not; the same on both faces.
    pub layers: [bool; 3],
    /// The room it was last drawn in, for fitting.
    room: Vec2,
    fit_pending: bool,
}

impl View3d {
    pub fn new(solid: Option<Solid>) -> View3d {
        View3d { yaw: -0.42, pitch: 0.92, dist: 100.0, target: [0.0; 3], color: 0, thick: 1.6, auto_fit: true, solid: solid.map(Arc::new), layers: [true; 3], room: vec2(1200.0, 800.0), fit_pending: true }
    }

    /// A new picture of the board, kept as it is turned.
    pub fn set_solid(&mut self, solid: Option<Solid>) {
        let first = self.solid.is_none();
        self.solid = solid.map(Arc::new);
        if first {
            self.auto_fit = true;
            self.fit_pending = true;
        }
    }

    fn eye(&self) -> V3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [self.target[0] + sy * cp * self.dist, self.target[1] - cy * cp * self.dist, self.target[2] + sp * self.dist]
    }

    fn up(&self) -> V3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [-sy * sp, cy * sp, cp]
    }

    fn matrix(&self, aspect: f32) -> M4 {
        mul(&perspective(aspect, self.dist * 0.02, self.dist * 12.0), &look_at(self.eye(), self.target, self.up()))
    }

    pub fn look(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.pitch = pitch;
        self.auto_fit = true;
        self.fit_pending = true;
    }

    pub fn top(&mut self) {
        self.look(0.0, FRAC_PI_2);
    }

    pub fn bottom(&mut self) {
        self.look(PI, -FRAC_PI_2);
    }

    pub fn angle(&mut self) {
        self.look(-0.42, 0.92);
    }

    pub fn refit(&mut self) {
        self.auto_fit = true;
        self.fit_pending = true;
    }

    /// Backs off until every corner of the board is inside the room, which at
    /// an angle depends on which way it's turned.
    fn fit(&mut self, room: Vec2) {
        let Some(solid) = self.solid.clone() else { return };
        let (min, max) = (solid.min, solid.max);
        self.target = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0, 0.0];
        let aspect = room.x / room.y.max(1.0);
        // The toolbar sits along the bottom, so less of the height is free.
        let (room_x, room_y) = (0.95, (1.0 - 130.0 / room.y.max(300.0)).max(0.5));
        self.dist = (max[0] - min[0]).max(max[1] - min[1]) * 2.0;
        let corners = [(min[0], min[1]), (max[0], min[1]), (max[0], max[1]), (min[0], max[1])];
        for _ in 0..6 {
            let view = self.matrix(aspect);
            let mut most: f32 = 0.0;
            for (x, y) in corners {
                for z in [-1.0, 1.0] {
                    let q = apply(&view, [x, y, z, 1.0]);
                    most = most.max((q[0] / q[3]).abs() / room_x).max((q[1] / q[3]).abs() / room_y);
                }
            }
            self.dist = (self.dist * most).clamp(1.0, 5000.0);
        }
    }

    /// The board, turned by dragging, and the toolbar.
    pub fn show(&mut self, ui: &mut Ui, gpu: &Shared, report: &Arc<Mutex<Option<String>>>) {
        let full = ui.available_rect_before_wrap();
        let response = ui.interact(full, ui.scope_id().with("board 3d"), Sense::click_and_drag());
        ui.painter().rect_filled(full, 0.0, theme::WELL);
        let Some(solid) = self.solid.clone() else {
            ui.painter().text(full.center(), Align2::CENTER_CENTER, "There's no board to show.", egui::FontId::proportional(15.0), FAINT);
            return;
        };
        self.room = full.size();
        if self.fit_pending {
            self.fit_pending = false;
            if self.auto_fit {
                self.fit(full.size());
            }
        }
        if response.dragged_by(PointerButton::Primary) {
            let d = response.drag_delta();
            self.yaw -= d.x * TURN_PER_POINT;
            self.pitch = (self.pitch + d.y * TURN_PER_POINT).clamp(-FRAC_PI_2, FRAC_PI_2);
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if response.dragged_by(PointerButton::Secondary) || response.dragged_by(PointerButton::Middle) {
            let d = response.drag_delta();
            let per_point = 2.0 * self.dist * (FOV * 0.5).tan() / full.height().max(1.0);
            let forward = norm(sub(self.target, self.eye()));
            let up = self.up();
            let right = cross(forward, up);
            for k in 0..3 {
                self.target[k] += (up[k] * d.y - right[k] * d.x) * per_point;
            }
            self.auto_fit = false;
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        if response.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = zoom * (scroll * 0.003).exp();
            if (factor - 1.0).abs() > 1e-4 {
                self.dist = (self.dist / factor).clamp(1.0, 5000.0);
                self.auto_fit = false;
            }
        }
        let view = self.matrix(full.width() / full.height().max(1.0));
        let shot = Shot { view, eye: self.eye(), board: BOARD_COLORS[self.color].1, thick: self.thick, top_on: self.layers, bottom_on: self.layers };
        ui.painter().add(gpu3d::callback(full, gpu.clone(), solid, shot, report.clone()));
        self.toolbar(ui, full);
    }

    fn toolbar(&mut self, ui: &mut Ui, full: Rect) {
        egui::Area::new(ui.scope_id().with("tools 3d"))
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(full.center().x, full.bottom() - 12.0))
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ui.ctx(), |ui| {
                egui::Frame::new().fill(theme::CARD.gamma_multiply(0.96)).stroke(Stroke::new(1.0, LINE)).corner_radius(CornerRadius::same(12)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    ui.horizontal(|ui| {
                        if chip(ui, "Top", false).on_hover_text("Look straight down").clicked() {
                            self.top();
                        }
                        if chip(ui, "Bottom", false).on_hover_text("Look at the underside").clicked() {
                            self.bottom();
                        }
                        if chip(ui, "Angle", false).on_hover_text("Look from an angle").clicked() {
                            self.angle();
                        }
                        if chip(ui, "Fit", false).on_hover_text("Make the board fill the room").clicked() {
                            self.refit();
                        }
                        ui.add_space(6.0);
                        for (k, name) in ["Copper", "Mask", "Silk"].into_iter().enumerate() {
                            if chip(ui, name, self.layers[k]).on_hover_text("Show or hide this layer").clicked() {
                                self.layers[k] = !self.layers[k];
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        for (i, (name, color)) in BOARD_COLORS.iter().enumerate() {
                            let (rect, response) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
                            let shown = color.map(|c| (c * 255.0).clamp(0.0, 255.0) as u8);
                            ui.painter().circle_filled(rect.center(), 8.5, Color32::from_rgb(shown[0], shown[1], shown[2]));
                            let on = self.color == i;
                            let edge = if on { FG } else if response.hovered() { FAINT } else { LINE };
                            ui.painter().circle_stroke(rect.center(), 10.0, Stroke::new(if on { 1.6 } else { 1.0 }, edge));
                            if response.on_hover_text(format!("{name} solder mask")).clicked() {
                                self.color = i;
                            }
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new("Thickness").size(13.0).color(FAINT));
                        segmented(ui, &mut self.thick, &THICKNESSES);
                        ui.label(RichText::new("mm").size(13.0).color(FAINT));
                    });
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_matrix_maths_agrees_with_itself() {
        let id = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let m = look_at([3.0, -4.0, 5.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert_eq!(mul(&m, &id), m);
        assert_eq!(mul(&id, &m), m);
        // The target is straight ahead: on the view's axis at the eye's distance.
        let q = apply(&m, [0.0, 0.0, 0.0, 1.0]);
        assert!(q[0].abs() < 1e-5 && q[1].abs() < 1e-5 && (q[2] + (50.0f32).sqrt()).abs() < 1e-4, "{q:?}");
    }

    #[test]
    fn a_board_is_fitted_inside_the_room_from_every_side() {
        let b = crate::pcb::sample::board();
        let mut v = View3d::new(crate::pcb::solid::build(&b, 1));
        for (yaw, pitch) in [(-0.42, 0.92), (0.0, FRAC_PI_2), (PI, -FRAC_PI_2)] {
            v.look(yaw, pitch);
            v.fit(vec2(1200.0, 800.0));
            let view = v.matrix(1200.0 / 800.0);
            let s = v.solid.clone().unwrap();
            for (x, y) in [(s.min[0], s.min[1]), (s.max[0], s.max[1])] {
                let q = apply(&view, [x, y, 0.0, 1.0]);
                assert!((q[0] / q[3]).abs() <= 1.0 && (q[1] / q[3]).abs() <= 1.0, "a corner is out of view at {yaw},{pitch}: {q:?}");
            }
        }
    }
}
