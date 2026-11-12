//! Draws the solid board with the graphics card. Copper, solder-mask openings,
//! silkscreen and drill are each drawn once into a picture the board wears, and
//! colour and light are worked out as it's shown, so changing the board's colour
//! is instant. This is Copperview's way of doing it (its `gpu3d`), with the
//! pictures made from Coppersmith's own board rather than from Gerbers.

use crate::pcb::solid::{STRIDE, Solid, Tris};
use eframe::egui;
use eframe::glow::{self, HasContext};
use std::sync::{Arc, Mutex};

const LONGEST_SIDE: i32 = 4096;
const BAKE_SAMPLES: i32 = 4;

const VS: &str = r"
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_normal;
layout(location = 2) in float a_stuff;
uniform mat4 u_view;
uniform float u_thick;
uniform vec4 u_area; // min x, min y, 1/width, 1/height
out vec3 v_pos;
out vec3 v_normal;
out vec2 v_uv;
out float v_stuff;
void main() {
    vec3 q = vec3(a_pos.xy, a_pos.z * u_thick);
    v_pos = q;
    v_normal = a_normal;
    v_uv = (a_pos.xy - u_area.xy) * u_area.zw;
    v_stuff = a_stuff;
    gl_Position = u_view * vec4(q, 1.0);
}";

const FS: &str = r"
in vec3 v_pos;
in vec3 v_normal;
in vec2 v_uv;
in float v_stuff;
out vec4 color;
uniform sampler2D u_copper;
uniform sampler2D u_mask;
uniform sampler2D u_silk;
uniform sampler2D u_drill;
uniform vec3 u_eye;
uniform vec3 u_board;
uniform vec3 u_has; // copper, mask, silk
uniform int u_part; // 0 a face, 1 a wall

const vec3 FR4 = vec3(0.70, 0.62, 0.42);
const vec3 GOLD = vec3(0.90, 0.68, 0.30);
const vec3 PLATED = vec3(0.82, 0.62, 0.32);
const vec3 INK = vec3(0.97, 0.97, 0.95);

vec3 light(vec3 n, vec3 v, vec3 base, float shine, float gloss) {
    vec3 sun = normalize(vec3(-0.35, 0.45, 0.82));
    vec3 fill = normalize(vec3(0.5, -0.4, -0.75));
    float wrap = max(dot(n, sun), 0.0) * 0.72 + max(dot(n, fill), 0.0) * 0.38 + 0.34;
    vec3 h = normalize(sun + v);
    float spec = pow(max(dot(n, h), 0.0), gloss) * shine;
    return base * wrap + vec3(spec);
}

void main() {
    vec3 n = normalize(v_normal);
    vec3 v = normalize(u_eye - v_pos);
    if (dot(n, v) < 0.0) n = -n;
    if (u_part == 1) {
        vec3 base = v_stuff > 0.5 ? PLATED : FR4 * 0.8;
        color = vec4(light(n, v, base, v_stuff > 0.5 ? 0.5 : 0.04, 24.0), 1.0);
        return;
    }
    if (texture(u_drill, v_uv).r > 0.5) discard;
    float cu = texture(u_copper, v_uv).r * u_has.x;
    float open = u_has.y > 0.5 ? texture(u_mask, v_uv).r : 1.0;
    float silk = texture(u_silk, v_uv).r * u_has.z;
    float covered = 1.0 - open;

    // Copper stands a little proud, and the mask shows it as a soft edge.
    float dx = textureOffset(u_copper, v_uv, ivec2(1, 0)).r - textureOffset(u_copper, v_uv, ivec2(-1, 0)).r;
    float dy = textureOffset(u_copper, v_uv, ivec2(0, 1)).r - textureOffset(u_copper, v_uv, ivec2(0, -1)).r;
    n = normalize(n + vec3(-dx, -dy, 0.0) * 0.55 * u_has.x);

    vec3 bare = mix(FR4, GOLD, cu);
    vec3 masked = mix(u_board * 0.82, u_board * 1.28 + vec3(0.02), cu);
    vec3 base = mix(bare, masked, covered);
    base = mix(base, INK, silk * (0.35 + 0.65 * covered));

    float shine = mix(mix(0.05, 0.7, cu), 0.28, covered);
    color = vec4(light(n, v, base, shine * (1.0 - silk * 0.8), mix(36.0, 70.0, covered)), 1.0);
}";

const FILL_VS: &str = r"
layout(location = 0) in vec3 a;
uniform vec4 u_xf;
out float v;
void main() {
    v = a.z;
    gl_Position = vec4(a.x * u_xf.x + u_xf.z, a.y * u_xf.y + u_xf.w, 0.0, 1.0);
}";

const FILL_FS: &str = r"
in float v;
out vec4 color;
void main() { color = vec4(v, 0.0, 0.0, 1.0); }";

unsafe fn program(gl: &glow::Context, vs: &str, fs: &str) -> Result<glow::Program, String> {
    unsafe {
        let head = if gl.version().is_embedded {
            "#version 300 es\nprecision highp float;\nprecision highp sampler2D;\n"
        } else {
            "#version 330 core\n"
        };
        let program = gl.create_program()?;
        for (kind, body) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, &format!("{head}{body}"));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                return Err(gl.get_shader_info_log(shader));
            }
            gl.attach_shader(program, shader);
            gl.delete_shader(shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            return Err(gl.get_program_info_log(program));
        }
        Ok(program)
    }
}

fn as_bytes<T: Copy>(values: &[T]) -> &[u8] {
    // Plain numbers laid out one after another, as the graphics card expects.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

/// One face's pictures.
struct Skin {
    copper: Option<glow::Texture>,
    mask: Option<glow::Texture>,
    silk: Option<glow::Texture>,
}

struct Scene {
    id: u64,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    top: Skin,
    bottom: Skin,
    drill: Option<glow::Texture>,
    blank: glow::Texture,
}

pub struct Gpu3d {
    fill: glow::Program,
    solid: glow::Program,
    resolve: glow::Framebuffer,
    samples: i32,
    scene: Option<Scene>,
}

/// What changes from frame to frame.
#[derive(Clone)]
pub struct Shot {
    pub view: [f32; 16],
    pub eye: [f32; 3],
    pub board: [f32; 3],
    pub thick: f32,
    /// Copper, mask and silk, shown or not, on each face.
    pub top_on: [bool; 3],
    pub bottom_on: [bool; 3],
}

fn picture_size(gl: &glow::Context, solid: &Solid) -> [i32; 2] {
    let most = unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) }.clamp(1024, LONGEST_SIDE);
    let (w, h) = (solid.max[0] - solid.min[0], solid.max[1] - solid.min[1]);
    let k = most as f32 / w.max(h);
    [((w * k) as i32).max(16), ((h * k) as i32).max(16)]
}

fn levels(size: [i32; 2]) -> i32 {
    32 - (size[0].max(size[1]) as u32).leading_zeros() as i32
}

unsafe fn gray_texture(gl: &glow::Context, size: [i32; 2], levels: i32) -> Result<glow::Texture, String> {
    unsafe {
        let tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_storage_2d(glow::TEXTURE_2D, levels, glow::R8, size[0], size[1]);
        let min = if levels > 1 { glow::LINEAR_MIPMAP_LINEAR } else { glow::LINEAR };
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        const MAX_ANISOTROPY: u32 = 0x84FE;
        if gl.supported_extensions().contains("GL_EXT_texture_filter_anisotropic") {
            gl.tex_parameter_f32(glow::TEXTURE_2D, MAX_ANISOTROPY, 8.0);
        }
        Ok(tex)
    }
}

impl Gpu3d {
    fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            Ok(Gpu3d {
                fill: program(gl, FILL_VS, FILL_FS)?,
                solid: program(gl, VS, FS)?,
                resolve: gl.create_framebuffer()?,
                samples: gl.get_parameter_i32(glow::MAX_SAMPLES).clamp(0, 8),
                scene: None,
            })
        }
    }

    /// Draws these triangles into one black-and-white picture of the whole
    /// board. With `always`, an empty set is still a (black) picture.
    fn bake(&self, gl: &glow::Context, sets: &[&Tris], solid: &Solid, size: [i32; 2], always: bool) -> Result<Option<glow::Texture>, String> {
        if sets.iter().all(|t| t.is_empty()) && !always {
            return Ok(None);
        }
        let (sx, sy) = (2.0 / (solid.max[0] - solid.min[0]), 2.0 / (solid.max[1] - solid.min[1]));
        let (tx, ty) = (-1.0 - solid.min[0] * sx, -1.0 - solid.min[1] * sy);
        unsafe {
            let tex = gray_texture(gl, size, levels(size))?;
            let (fbo, rb) = (gl.create_framebuffer()?, gl.create_renderbuffer()?);
            let samples = self.samples.min(BAKE_SAMPLES);
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(rb));
            gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples, glow::R8, size[0], size[1]);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::RENDERBUFFER, Some(rb));
            gl.viewport(0, 0, size[0], size[1]);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.color_mask(true, true, true, true);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(self.fill));
            let loc = gl.get_uniform_location(self.fill, "u_xf");
            gl.uniform_4_f32(loc.as_ref(), sx, sy, tx, ty);
            for set in sets.iter().filter(|t| !t.is_empty()) {
                let vao = gl.create_vertex_array()?;
                let vbo = gl.create_buffer()?;
                let ebo = gl.create_buffer()?;
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&set.verts), glow::STREAM_DRAW);
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
                gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(&set.idx), glow::STREAM_DRAW);
                gl.enable_vertex_attrib_array(0);
                gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 12, 0);
                gl.draw_elements(glow::TRIANGLES, set.idx.len() as i32, glow::UNSIGNED_INT, 0);
                gl.bind_vertex_array(None);
                gl.delete_vertex_array(vao);
                gl.delete_buffer(vbo);
                gl.delete_buffer(ebo);
            }
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.resolve));
            gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fbo));
            gl.blit_framebuffer(0, 0, size[0], size[1], 0, 0, size[0], size[1], glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.delete_framebuffer(fbo);
            gl.delete_renderbuffer(rb);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.generate_mipmap(glow::TEXTURE_2D);
            Ok(Some(tex))
        }
    }

    fn build_scene(&mut self, gl: &glow::Context, solid: &Solid) -> Result<(), String> {
        self.drop_scene(gl);
        let size = picture_size(gl, solid);
        let skin = |s: &Gpu3d, f: &crate::pcb::solid::Face| -> Result<Skin, String> {
            Ok(Skin {
                copper: s.bake(gl, &[&f.copper], solid, size, false)?,
                // A board always has mask: with no openings it's all covered.
                mask: s.bake(gl, &[&f.mask], solid, size, true)?,
                silk: s.bake(gl, &[&f.silk], solid, size, false)?,
            })
        };
        let top = skin(self, &solid.top)?;
        let bottom = skin(self, &solid.bottom)?;
        let drill = self.bake(gl, &[&solid.drill], solid, size, false)?;
        unsafe {
            let blank = gray_texture(gl, [1, 1], 1)?;
            gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, 1, 1, glow::RED, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&[0u8])));
            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            let ebo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&solid.verts), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(&solid.idx), glow::STATIC_DRAW);
            let stride = (STRIDE * 4) as i32;
            for (at, count, offset) in [(0, 3, 0), (1, 3, 12), (2, 1, 24)] {
                gl.enable_vertex_attrib_array(at);
                gl.vertex_attrib_pointer_f32(at, count, glow::FLOAT, false, stride, offset);
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
            self.scene = Some(Scene { id: solid.id, vao, vbo, ebo, top, bottom, drill, blank });
        }
        Ok(())
    }

    /// Lets go of everything drawn for the board.
    fn drop_scene(&mut self, gl: &glow::Context) {
        let Some(s) = self.scene.take() else { return };
        unsafe {
            gl.delete_vertex_array(s.vao);
            gl.delete_buffer(s.vbo);
            gl.delete_buffer(s.ebo);
            gl.delete_texture(s.blank);
            let all = [s.top.copper, s.top.mask, s.top.silk, s.bottom.copper, s.bottom.mask, s.bottom.silk, s.drill];
            for tex in all.into_iter().flatten() {
                gl.delete_texture(tex);
            }
        }
    }

    fn paint(&mut self, gl: &glow::Context, info: &egui::PaintCallbackInfo, fbo: Option<glow::Framebuffer>, solid: &Solid, shot: &Shot) -> Result<(), String> {
        if self.scene.as_ref().is_none_or(|s| s.id != solid.id) {
            self.build_scene(gl, solid)?;
        }
        let Some(scene) = self.scene.as_ref() else { return Ok(()) };
        let prog = self.solid;
        let vp = info.viewport_in_pixels();
        let clip = info.clip_rect_in_pixels();
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, fbo);
            gl.viewport(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(clip.left_px, clip.from_bottom_px, clip.width_px, clip.height_px);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.depth_mask(true);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.use_program(Some(prog));
            gl.bind_vertex_array(Some(scene.vao));
            let u = |name: &str| gl.get_uniform_location(prog, name);
            gl.uniform_matrix_4_f32_slice(u("u_view").as_ref(), false, &shot.view);
            gl.uniform_1_f32(u("u_thick").as_ref(), shot.thick);
            let (w, h) = (solid.max[0] - solid.min[0], solid.max[1] - solid.min[1]);
            gl.uniform_4_f32(u("u_area").as_ref(), solid.min[0], solid.min[1], 1.0 / w, 1.0 / h);
            gl.uniform_3_f32_slice(u("u_eye").as_ref(), &shot.eye);
            gl.uniform_3_f32_slice(u("u_board").as_ref(), &shot.board);
            for (unit, name) in ["u_copper", "u_mask", "u_silk", "u_drill"].iter().enumerate() {
                gl.uniform_1_i32(u(name).as_ref(), unit as i32);
            }
            let bind = |unit: u32, tex: Option<glow::Texture>| {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, Some(tex.unwrap_or(scene.blank)));
            };
            bind(3, scene.drill);
            let parts = [(&scene.top, shot.top_on, 0, solid.top_count), (&scene.bottom, shot.bottom_on, solid.top_count, solid.bottom_count)];
            gl.uniform_1_i32(u("u_part").as_ref(), 0);
            for (skin, on, start, count) in parts {
                bind(0, skin.copper);
                bind(1, skin.mask);
                bind(2, skin.silk);
                let flag = |shown: bool, tex: Option<glow::Texture>| if shown && tex.is_some() { 1.0 } else { 0.0 };
                gl.uniform_3_f32(u("u_has").as_ref(), flag(on[0], skin.copper), flag(on[1], skin.mask), flag(on[2], skin.silk));
                gl.draw_elements(glow::TRIANGLES, count, glow::UNSIGNED_INT, start * 4);
            }
            gl.uniform_1_i32(u("u_part").as_ref(), 1);
            let walls_at = solid.top_count + solid.bottom_count;
            gl.draw_elements(glow::TRIANGLES, solid.wall_count, glow::UNSIGNED_INT, walls_at * 4);
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.active_texture(glow::TEXTURE0);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::SCISSOR_TEST);
        }
        Ok(())
    }
}

/// The graphics card's side of the 3D view, made on first use.
pub type Shared = Arc<Mutex<Option<Result<Gpu3d, String>>>>;

/// A piece of the window that shows the solid board.
pub fn callback(rect: egui::Rect, gpu: Shared, solid: Arc<Solid>, shot: Shot, report: Arc<Mutex<Option<String>>>) -> egui::PaintCallback {
    let run = eframe::egui_glow::CallbackFn::new(move |info, painter| {
        let gl = painter.gl();
        let Ok(mut slot) = gpu.lock() else { return };
        let state = slot.get_or_insert_with(|| Gpu3d::new(gl));
        let outcome = match state {
            Ok(g) => g.paint(gl, &info, painter.intermediate_fbo(), &solid, &shot),
            Err(error) => Err(error.clone()),
        };
        if let (Err(error), Ok(mut report)) = (outcome, report.lock()) {
            *report = Some(error);
        }
    });
    egui::PaintCallback { rect, callback: Arc::new(run) }
}

/// Lets go of what the 3D view drew, once it's no longer shown.
pub fn release(rect: egui::Rect, gpu: Shared) -> egui::PaintCallback {
    let run = eframe::egui_glow::CallbackFn::new(move |_info, painter| {
        let Ok(mut slot) = gpu.lock() else { return };
        if let Some(Ok(g)) = slot.as_mut() {
            g.drop_scene(painter.gl());
        }
    });
    egui::PaintCallback { rect, callback: Arc::new(run) }
}
