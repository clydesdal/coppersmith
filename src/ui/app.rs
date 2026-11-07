//! The open board, what's selected, what the editor is in the middle of, and
//! the edits themselves. Drawing and clicking are in view.rs, the panels in
//! panels.rs.

use crate::pcb::copper::{self, Copper, Item};
use crate::pcb::drc::{self, Problem};
use crate::pcb::geom::*;
use crate::pcb::library::{FootprintDef, Library};
use crate::pcb::model::*;
use crate::platform::{self, Pinch};
use crate::ui::dock::{Filter, LeftTab, Prefs, RightTab};
use crate::ui::theme;
use eframe::egui::{self, ColorImage, Key, Panel, Rect, Ui, vec2};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Route,
    Via,
    Outline,
    Pour,
    /// An area kept free of copper pouring in (and more, if it says).
    Keepout,
    Place,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Sel {
    Fp(u32),
    Track(u32),
    Via(u32),
    Pour(u32),
    Fill(u32),
    Keepout(u32),
    /// The name of a part, drawn on the silkscreen.
    Name(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Mm,
    Mil,
    Inch,
}

impl Unit {
    pub fn name(self) -> &'static str {
        match self {
            Unit::Mm => "mm",
            Unit::Mil => "mil",
            Unit::Inch => "in",
        }
    }
    pub fn from_mm(self, mm: f64) -> f64 {
        match self {
            Unit::Mm => mm,
            Unit::Mil => mm / 0.0254,
            Unit::Inch => mm / 25.4,
        }
    }
    pub fn to_mm(self, v: f64) -> f64 {
        match self {
            Unit::Mm => v,
            Unit::Mil => v * 0.0254,
            Unit::Inch => v * 25.4,
        }
    }
    pub fn decimals(self) -> usize {
        match self {
            Unit::Mm => 3,
            Unit::Mil => 1,
            Unit::Inch => 4,
        }
    }
    pub fn num(self, mm: f64) -> String {
        format!("{:.*}", self.decimals(), self.from_mm(mm))
    }
    pub fn len(self, mm: f64) -> String {
        format!("{} {}", self.num(mm), self.name())
    }
}

pub use crate::ui::routing::Route;
use crate::ui::routing::{ObstacleCache, PlanCache, RouteMode};

#[derive(Clone, Debug)]
pub enum DragKind {
    Move,
    /// One end of a track; `true` for the far end.
    End(u32, bool),
}

#[derive(Clone, Debug)]
pub struct Drag {
    pub kind: DragKind,
    pub before: Board,
    pub grab: P,
    /// The point that lands on the grid.
    pub reference: P,
    pub moved: bool,
    /// Ctrl held: moved without pushing.
    pub free: bool,
    /// Quarter turns (in degrees) given to the dragged parts with Space or R.
    pub turns: f64,
    /// The dragged parts were put on the other side with L.
    pub flipped: bool,
    /// Where the pointer last was, to redo the drag after a turn.
    pub last: P,
}

#[derive(Default)]
pub struct Launch {
    pub path: Option<PathBuf>,
    pub screenshot: Option<PathBuf>,
    pub play: Option<String>,
    pub zoom: Option<(P, f64)>,
    pub select: Option<String>,
    /// The window's layout in words, for `--layout`.
    pub layout: Option<String>,
    /// A library to open in the library editor, for `--library`.
    pub library: Option<PathBuf>,
    /// A schematic to open in the schematic editor, for `--schematic`.
    pub schematic: Option<PathBuf>,
    /// Open the 3D view, for `--3d`.
    pub view3d: bool,
}

/// Which tab is on show once tab `closed` is gone: the same one (its place
/// may have moved up), or, if it was the one closed, the tab before it (the
/// board when it was the first).
pub fn active_after_close(active: Option<usize>, closed: usize) -> Option<usize> {
    match active {
        Some(a) if a == closed => closed.checked_sub(1),
        Some(a) if a > closed => Some(a - 1),
        other => other,
    }
}

/// A document open in a tab beside the board.
pub enum Doc {
    Library(Box<crate::ui::libview::LibWindow>),
    Schematic(Box<crate::ui::schview::SchWindow>),
    /// The board as a solid, to turn round.
    View3d(Box<crate::ui::view3d::View3d>),
}

impl Doc {
    fn file_name(p: Option<&std::path::Path>) -> Option<String> {
        p.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string())
    }

    /// What the tab says: the file's name, or the document's own.
    pub fn title(&self) -> String {
        match self {
            Doc::Library(w) => Self::file_name(w.ed.path().or(w.ed.fp_path.as_deref()).or(w.ed.sym_path.as_deref())).unwrap_or_else(|| w.ed.name.clone()),
            Doc::Schematic(w) => Self::file_name(w.path.as_deref()).unwrap_or_else(|| w.sch.name.clone()),
            Doc::View3d(_) => "3D view".to_string(),
        }
    }

    pub fn dirty(&self) -> bool {
        match self {
            Doc::Library(w) => w.ed.dirty,
            Doc::Schematic(w) => w.dirty,
            Doc::View3d(_) => false,
        }
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        match self {
            Doc::Library(w) => w.ed.fp_path.iter().chain(w.ed.sym_path.iter()).cloned().collect(),
            Doc::Schematic(w) => w.path.iter().cloned().collect(),
            Doc::View3d(_) => Vec::new(),
        }
    }

    pub fn mode(&self) -> Mode {
        match self {
            Doc::Library(_) => Mode::Library,
            Doc::Schematic(_) => Mode::Schematic,
            Doc::View3d(_) => Mode::View3d,
        }
    }
}

/// Which window is on show: the board, or one of the editors in a tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Board,
    Library,
    Schematic,
    View3d,
}

pub enum Asked {
    AddLibrary(PathBuf),
    Project(PathBuf),
    Open(PathBuf),
    SaveAs(PathBuf),
    Export(PathBuf),
}

/// What waits on the "save changes?" question.
#[derive(Clone, Debug)]
pub enum Then {
    New,
    Open(PathBuf),
    Quit,
}

pub const GRIDS: [f64; 8] = [0.05, 0.1, 0.25, 0.5, 1.0, 0.635, 1.27, 2.54];
/// While something is dragged or routed, the rule check runs this often at most.
/// The check only redoes what changed, so this is just a stop against a very
/// large move making every step wait for it.
const CHECK_EVERY: std::time::Duration = std::time::Duration::from_millis(50);
pub const WIDTHS: [f64; 9] = [0.15, 0.2, 0.25, 0.3, 0.4, 0.5, 0.8, 1.0, 1.5];

pub struct App {
    pub doc: Board,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    undo: Vec<Board>,
    redo: Vec<Board>,
    /// Goes up with every change, so what's worked out from the board is redone.
    pub rev: u64,
    pub copper: Copper,
    copper_rev: u64,
    /// The board change the pours were last poured for.
    poured_rev: u64,
    /// The board change the rule check last ran for.
    checked_rev: u64,
    last_check: std::time::Instant,
    /// What the last rule check found out, so the next looks only at what changed.
    check_cache: Option<drc::Cache>,
    /// Whether pours are filled. Off, they keep their areas and settings but
    /// have no copper, so routing and moving run unimpeded and quickly;
    /// switching it on pours them again.
    pub flood: bool,
    pub problems: Vec<Problem>,
    pub lib: Library,
    pub lib_filter: String,

    pub center: P,
    pub scale: f64,
    pub mirror: bool,
    pub fit_pending: bool,
    pub fitted: bool,
    pub canvas_rect: Rect,

    pub tool: Tool,
    pub layer: u8,
    /// A silkscreen layer being worked on, where names can be picked and moved;
    /// none when it's the copper layer above.
    pub silk: Option<Side>,
    pub hidden: HashSet<Layer>,
    pub show_rats: bool,
    pub show_problems: bool,
    pub sel: Vec<Sel>,
    pub hover: Option<Sel>,
    pub hover_q: Option<P>,
    pub hover_text: Option<String>,
    pub cursor: Option<P>,
    pub drag: Option<Drag>,
    pub box_from: Option<egui::Pos2>,
    pub route: Option<Route>,
    pub route_mode: RouteMode,
    pub plan: Option<PlanCache>,
    pub obstacles: ObstacleCache,
    /// Where the pointer lands in the route tool, for Enter to click there.
    pub route_target: Option<(P, Option<Item>)>,
    pub outline: Vec<P>,
    pub placing: Option<(Footprint, String)>,
    pub grid: f64,
    pub show_grid: bool,
    pub unit: Unit,
    pub width: f64,
    pub highlight: Option<u32>,
    pub focus: Option<usize>,
    pub notice: Option<String>,
    pub exported: Option<PathBuf>,
    pub new_net: String,
    pub show_left: bool,
    pub show_right: bool,
    /// The pages of the two panels, each chosen from tabs along its bottom.
    pub left_tab: LeftTab,
    pub right_tab: RightTab,
    /// The strip of controls under the menus.
    pub ribbon: bool,
    /// What can be picked.
    pub filter: Filter,
    /// The layout as last written down, so it's written again only when it changes.
    prefs_saved: Option<Prefs>,
    /// The keyboard shortcuts window.
    pub show_keys: bool,
    /// The window of net classes, pairs and the rules asked of them.
    pub show_classes: bool,
    /// The symbols read from the project's library files, so they're read once.
    pub symbol_cache: std::collections::HashMap<PathBuf, Vec<crate::pcb::altium::schlib::Symbol>>,
    /// The schematics and libraries open in tabs beside the board.
    pub docs: Vec<Doc>,
    /// Which tab is on show: one of `docs`, or the board (None).
    pub active: Option<usize>,
    /// A tab with unsaved changes that has been asked to close once.
    pub pending_close: Option<usize>,
    /// Silkscreen layers hidden by an inner-layer quick pick, to bring back with the next other pick.
    pub silk_hid: Vec<Layer>,
    /// What each inner copper layer is for, found out when the board changed last (and which change).
    pub layer_roles: (u64, Vec<(u8, crate::pcb::model::LayerRole)>),
    /// The graphics card's side of the 3D view, and what it said went wrong.
    pub gpu3d: crate::ui::gpu3d::Shared,
    pub gpu3d_error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// The 3D tab has closed: let go of what was drawn for it.
    gpu3d_release: bool,
    /// The schematics, boards and libraries that go together.
    pub project: Option<crate::pcb::project::Project>,
    /// Which page of an editor tab's left dock is showing.
    pub editor_tab: crate::ui::dock::EditorLeft,
    /// The file picked (once) in the project panel.
    pub project_sel: Option<PathBuf>,
    pub last_field_edit: f64,
    pub confirm: Option<Then>,
    after_save: Option<Then>,
    may_close: bool,

    pub pinch: Pinch,
    pub logo: Option<egui::TextureHandle>,
    pub asked_tx: mpsc::Sender<Asked>,
    asked_rx: mpsc::Receiver<Asked>,
    pub launch: Launch,
    pub play: Vec<crate::ui::play::Step>,
    pub frames: u32,
    pub shot_sent: bool,
    applied_launch: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, launch: Launch) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        // The look is its own dark one, whatever the system's light or dark setting is:
        // both themes get it, and the dark one is always used.
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, theme::visuals());
        cc.egui_ctx.set_visuals_of(egui::Theme::Light, theme::visuals());
        cc.egui_ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(10.0, 5.0);
            s.spacing.interact_size.y = 24.0;
            s.visuals.handle_shape = egui::style::HandleShape::Circle;
            s.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
            s.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
            s.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        });
        // The window as it was left; a scripted run starts from the usual layout so its
        // pictures don't depend on whoever ran it, and never writes one down.
        let mut prefs = if launch.screenshot.is_some() { Prefs::default() } else { Prefs::load() };
        let layout_problem = launch.layout.as_deref().and_then(|spec| prefs.apply_spec(spec).err());
        let (asked_tx, asked_rx) = mpsc::channel();
        let doc = crate::pcb::sample::board();
        let mut hidden = HashSet::new();
        hidden.insert(Layer::Fab(Side::Top));
        hidden.insert(Layer::Fab(Side::Bottom));
        let mut app = App {
            width: doc.rules.track,
            doc,
            path: None,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            rev: 1,
            copper: Copper::default(),
            copper_rev: 0,
            poured_rev: u64::MAX,
            checked_rev: 0,
            check_cache: None,
            last_check: std::time::Instant::now(),
            flood: true,
            problems: Vec::new(),
            lib: Library::load(),
            lib_filter: String::new(),
            center: P::default(),
            scale: 10.0,
            mirror: false,
            fit_pending: true,
            fitted: true,
            canvas_rect: Rect::NOTHING,
            tool: Tool::Select,
            layer: TOP,
            silk: None,
            hidden,
            show_rats: true,
            show_problems: true,
            sel: Vec::new(),
            hover: None,
            hover_q: None,
            hover_text: None,
            cursor: None,
            drag: None,
            box_from: None,
            route: None,
            route_mode: RouteMode::default(),
            plan: None,
            obstacles: ObstacleCache::default(),
            route_target: None,
            outline: Vec::new(),
            placing: None,
            grid: 0.25,
            show_grid: true,
            unit: Unit::Mm,
            highlight: None,
            focus: None,
            notice: None,
            exported: None,
            new_net: String::new(),
            show_left: prefs.show_left,
            show_right: prefs.show_right,
            left_tab: prefs.left_tab,
            right_tab: prefs.right_tab,
            ribbon: prefs.ribbon,
            filter: prefs.filter,
            prefs_saved: None,
            show_keys: false,
            show_classes: false,
            symbol_cache: std::collections::HashMap::new(),
            docs: Vec::new(),
            active: None,
            pending_close: None,
            silk_hid: Vec::new(),
            layer_roles: (u64::MAX, Vec::new()),
            gpu3d: std::sync::Arc::new(std::sync::Mutex::new(None)),
            gpu3d_error: std::sync::Arc::new(std::sync::Mutex::new(None)),
            gpu3d_release: false,
            project: None,
            editor_tab: crate::ui::dock::EditorLeft::Items,
            project_sel: None,
            last_field_edit: -10.0,
            confirm: None,
            after_save: None,
            may_close: false,
            pinch: Pinch::default(),
            logo: crate::ui::icons::picture_pixels(64).map(|(pixels, side)| {
                let side = side as usize;
                let picture = ColorImage::from_rgba_unmultiplied([side, side], &pixels);
                cc.egui_ctx.load_texture("logo", picture, egui::TextureOptions::LINEAR)
            }),
            asked_tx,
            asked_rx,
            play: crate::ui::play::parse(launch.play.as_deref().unwrap_or("")),
            launch,
            frames: 0,
            shot_sent: false,
            applied_launch: false,
        };
        platform::listen_for_pinches(&mut app.pinch, cc);
        if let Some(path) = app.launch.path.take() {
            app.open(path);
        }
        if let Some(problem) = app.lib.problems.first() {
            app.notice = Some(format!("A footprint file couldn't be read: {problem}"));
        }
        if let Some(problem) = layout_problem {
            app.notice = Some(problem);
        }
        app
    }

    /// The layout as it is, to be kept.
    pub fn prefs(&self) -> Prefs {
        Prefs { show_left: self.show_left, show_right: self.show_right, left_tab: self.left_tab, right_tab: self.right_tab, ribbon: self.ribbon, filter: self.filter }
    }

    /// Writes the layout down if it has changed since it last was. Not in a
    /// scripted run, which would write over what the person set.
    pub fn keep_layout(&mut self) {
        if self.launch.screenshot.is_some() {
            return;
        }
        let now = self.prefs();
        if self.prefs_saved.as_ref() != Some(&now) {
            now.save();
            self.prefs_saved = Some(now);
        }
    }

    // -- What's worked out from the board ------------------------------------

    /// Brings the copper groups and the checks up to date with the board.
    pub fn refresh(&mut self) {
        // Pours pour again once a change is finished, not on every step of
        // a drag or route; until then they keep their old shape.
        let busy = self.drag.is_some() || self.route.is_some() || self.placing.is_some();
        // Pours switched off stay empty, whatever undo or a drag brings back.
        if !self.flood && self.doc.clear_fills() {
            self.rev += 1;
        }
        let repour = self.flood && !busy && self.poured_rev != self.rev;
        if repour {
            let started = std::time::Instant::now();
            self.doc.repour();
            self.poured_rev = self.rev;
            note_slow("pouring", started);
        }
        let changed = self.copper_rev != self.rev || repour;
        if changed {
            let started = std::time::Instant::now();
            self.copper = copper::analyse_again(&self.doc, &self.copper);
            self.copper_rev = self.rev;
            note_slow("connections", started);
        }
        // The rule check looks at the whole board, so while something is being
        // dragged or routed it runs a few times a second rather than at every
        // step, and once more as soon as it's let go.
        let due = !busy || self.last_check.elapsed() >= CHECK_EVERY;
        if (self.checked_rev != self.rev || repour) && due {
            let started = std::time::Instant::now();
            let (problems, cache) = drc::check_again(&self.doc, &self.copper, self.check_cache.as_ref());
            self.problems = problems;
            self.check_cache = Some(cache);
            self.checked_rev = self.rev;
            self.last_check = std::time::Instant::now();
            note_slow("checks", started);
            if self.focus.is_some_and(|i| i >= self.problems.len()) {
                self.focus = None;
            }
        }
    }

    /// Switches the pours' copper on or off; on, they're poured again.
    pub fn set_flood(&mut self, on: bool) {
        self.flood = on;
        if on {
            self.poured_rev = u64::MAX;
        }
        self.notice = Some(if on {
            "Pours filled again.".into()
        } else {
            "Pours emptied: their areas stay, so tracks can go anywhere. Switch Pour fills back on to pour them.".into()
        });
    }

    pub fn errors(&self) -> usize {
        self.problems.iter().filter(|p| p.severity == drc::Severity::Error).count()
    }

    // -- Changing the board ----------------------------------------------------

    /// Keeps the board as it was, so the change can be undone.
    pub fn remember(&mut self, before: Board) {
        self.undo.push(before);
        if self.undo.len() > 300 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.touch();
    }

    /// The board changed, without a new undo step.
    pub fn touch(&mut self) {
        self.rev += 1;
        self.dirty = true;
    }

    pub fn edit(&mut self, f: impl FnOnce(&mut Board)) {
        let before = self.doc.clone();
        f(&mut self.doc);
        if self.doc != before {
            self.remember(before);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        self.cancel();
        if let Some(b) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.doc, b));
            self.after_history();
        }
    }

    pub fn redo(&mut self) {
        self.cancel();
        if let Some(b) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.doc, b));
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        self.touch();
        let doc = &self.doc;
        self.sel.retain(|s| match *s {
            Sel::Fp(id) => doc.fp(id).is_some(),
            Sel::Track(id) => doc.track(id).is_some(),
            Sel::Via(id) => doc.via(id).is_some(),
            Sel::Pour(id) => doc.pour(id).is_some(),
            Sel::Fill(id) => doc.fill(id).is_some(),
            Sel::Keepout(id) => doc.keepout(id).is_some(),
            Sel::Name(id) => doc.fp(id).is_some(),
        });
    }

    /// Stops whatever is half done: a route, an outline, a drag.
    pub fn cancel(&mut self) {
        if let Some(d) = self.drag.take() {
            self.doc = d.before;
            self.touch();
        }
        // A route in progress may have pushed tracks aside: put them back.
        if let Some(before) = self.route.take().and_then(|mut r| r.take_before())
            && before != self.doc
        {
            self.doc = before;
            self.touch();
        }
        self.plan = None;
        self.outline.clear();
        self.box_from = None;
    }

    pub fn set_tool(&mut self, tool: Tool) {
        if tool != Tool::Route {
            self.finish_route();
        }
        self.outline.clear();
        if tool != Tool::Place {
            self.placing = None;
        }
        self.tool = tool;
    }

    pub fn start_placing(&mut self, def: &FootprintDef) {
        self.finish_route();
        let prefix = def.prefix.clone();
        let mut f = def.instantiate(0, self.doc.next_refdes(&prefix), self.cursor.unwrap_or(self.center));
        f.side = if self.layer == BOTTOM { Side::Bottom } else { Side::Top };
        self.placing = Some((f, prefix));
        self.tool = Tool::Place;
        self.sel.clear();
    }

    pub fn place_here(&mut self) {
        let Some((mut f, prefix)) = self.placing.clone() else { return };
        let before = self.doc.clone();
        f.id = self.doc.new_id();
        f.refdes = self.doc.next_refdes(&prefix);
        let id = f.id;
        self.doc.footprints.push(f);
        self.remember(before);
        self.sel = vec![Sel::Fp(id)];
        if let Some((ghost, _)) = &mut self.placing {
            ghost.refdes = self.doc.next_refdes(&prefix);
        }
    }

    /// Moves what's selected, dragging the ends of tracks that were joined to it.
    pub fn move_selection(board: &mut Board, sel: &[Sel], d: P) {
        if d.len() == 0.0 {
            return;
        }
        // One straight track on its own slides sideways, keeping its corners.
        if let [Sel::Track(id)] = sel
            && board.track(*id).is_some_and(|t| t.mid.is_none())
        {
            let mut tried = board.clone();
            if crate::pcb::slide::slide(&mut tried, *id, d) {
                *board = tried;
            }
            return;
        }
        let mut anchors: Vec<P> = Vec::new();
        let picked: HashSet<Sel> = sel.iter().copied().collect();
        for f in board.footprints.iter_mut().filter(|f| picked.contains(&Sel::Fp(f.id)) && !f.locked) {
            anchors.extend((0..f.pads.len()).map(|i| f.pad_center(i)));
            f.at = f.at.add(d).rounded();
        }
        for v in board.vias.iter_mut().filter(|v| picked.contains(&Sel::Via(v.id))) {
            anchors.push(v.at);
            v.at = v.at.add(d).rounded();
        }
        // Pours and fills move whole; a pour's copper moves with it until it pours again.
        let shift = |q: &P| q.add(d).rounded();
        for v in board.pours.iter_mut().filter(|v| picked.contains(&Sel::Pour(v.id)) && v.outline.len() >= 3) {
            v.outline = v.outline.iter().map(shift).collect();
            for island in &mut v.fill {
                island.rings = island.rings.iter().map(|r| r.iter().map(|q| q.add(d)).collect()).collect();
                island.tris = island.tris.iter().map(|t| t.map(|q| q.add(d))).collect();
            }
        }
        for v in board.fills.iter_mut().filter(|v| picked.contains(&Sel::Fill(v.id))) {
            v.pts = v.pts.iter().map(shift).collect();
        }
        for v in board.keepouts.iter_mut().filter(|v| picked.contains(&Sel::Keepout(v.id))) {
            v.pts = v.pts.iter().map(shift).collect();
        }
        // A name moves by itself, unless its part moves too (then it goes with it).
        for f in board.footprints.iter_mut().filter(|f| picked.contains(&Sel::Name(f.id)) && !picked.contains(&Sel::Fp(f.id))) {
            f.move_label(d);
        }
        for t in board.tracks.iter_mut().filter(|t| picked.contains(&Sel::Track(t.id))) {
            anchors.push(t.a);
            anchors.push(t.b);
            t.a = t.a.add(d).rounded();
            t.b = t.b.add(d).rounded();
            t.mid = t.mid.map(|m| m.add(d).rounded());
        }
        // The tracks joined to what moved come along, keeping their corners.
        let moves: Vec<(P, P)> = anchors.iter().map(|q| (*q, q.add(d).rounded())).collect();
        let moved: HashSet<u32> = sel.iter().filter_map(|s| if let Sel::Track(id) = s { Some(*id) } else { None }).collect();
        crate::pcb::slide::follow_moves(board, &moves, &moved);
    }

    /// Moves one end of a track, and any other track ends or vias sitting on it.
    pub fn move_end(board: &mut Board, id: u32, far: bool, to: P) {
        let Some(t) = board.track(id) else { return };
        let from = if far { t.b } else { t.a };
        crate::pcb::slide::drag_joint(board, from, to);
    }

    /// Pushes what's in the way of what an edit just moved; if it can't, the
    /// edit stands and the rule check shows what's too close.
    fn push_after_edit(&mut self, before: &Board) {
        if self.doc == *before {
            return;
        }
        let changed = crate::pcb::slide::changed_tracks(before, &self.doc);
        let min = self.doc.rules.min_diag;
        crate::pcb::slide::fix_corners(&mut self.doc, &changed, min);
        match crate::pcb::slide::push_after_move(before, self.doc.clone(), self.route_mode.via_push) {
            Some(b) => {
                if b != self.doc {
                    self.doc = b;
                    self.touch();
                }
            }
            None => self.notice = Some("Couldn't push everything out of the way; the checks show what's too close.".into()),
        }
    }

    pub fn rotate_selection(&mut self, deg: f64) {
        if let Some((f, _)) = &mut self.placing {
            f.rot = (f.rot + deg).rem_euclid(360.0);
            return;
        }
        // In the middle of dragging, the turn goes with the drag.
        if self.turn_while_dragging(deg, false) {
            return;
        }
        let fps = self.picked_parts();
        let before = self.doc.clone();
        self.edit(|b| App::turn_parts(b, &fps, deg, false));
        self.push_after_edit(&before);
    }

    pub fn picked_parts(&self) -> Vec<u32> {
        self.sel.iter().filter_map(|s| if let Sel::Fp(id) = s { Some(*id) } else { None }).collect()
    }

    /// Turns parts round their own origins by `deg`, and puts them on the
    /// other side if `flip`; the tracks on their pads follow, keeping their
    /// corners. Locked parts stay as they are.
    pub fn turn_parts(b: &mut Board, ids: &[u32], deg: f64, flip: bool) {
        for id in ids {
            let Some(f) = b.fp(*id) else { continue };
            if f.locked {
                continue;
            }
            let before: Vec<P> = (0..f.pads.len()).map(|i| f.pad_center(i)).collect();
            let f = b.fp_mut(*id).unwrap();
            f.rot = (f.rot + deg).rem_euclid(360.0);
            if flip {
                f.side = f.side.other();
            }
            let after: Vec<P> = (0..f.pads.len()).map(|i| f.pad_center(i)).collect();
            let moves: Vec<(P, P)> = before.into_iter().zip(after).filter(|(a, b)| !a.same(*b)).collect();
            crate::pcb::slide::follow_moves(b, &moves, &HashSet::new());
        }
    }

    /// Space, R or L while a part is being dragged: turns (or flips) it
    /// there and then, and the drag carries on with it turned. False when
    /// no part is being dragged.
    pub fn turn_while_dragging(&mut self, deg: f64, flip: bool) -> bool {
        let parts = !self.picked_parts().is_empty();
        let Some(drag) = &mut self.drag else { return false };
        if !matches!(drag.kind, DragKind::Move) || !parts {
            return false;
        }
        drag.turns = (drag.turns + deg).rem_euclid(360.0);
        drag.flipped ^= flip;
        let (q, free) = (drag.last, drag.free);
        self.continue_drag(q, free);
        true
    }

    pub fn flip_selection(&mut self) {
        if let Some((f, _)) = &mut self.placing {
            f.side = f.side.other();
            return;
        }
        if self.turn_while_dragging(0.0, true) {
            return;
        }
        let sel = self.sel.clone();
        let before = self.doc.clone();
        self.edit(|b| {
            for s in &sel {
                if let Sel::Fp(id) = s
                    && let Some(f) = b.fp_mut(*id)
                    && !f.locked
                {
                    f.side = f.side.other();
                }
                if let Sel::Track(id) = s
                    && let Some(t) = b.tracks.iter_mut().find(|t| t.id == *id)
                {
                    t.layer = if t.layer == TOP { BOTTOM } else { TOP };
                }
            }
        });
        self.push_after_edit(&before);
    }

    pub fn delete_selection(&mut self) {
        let sel: HashSet<Sel> = self.sel.drain(..).collect();
        self.edit(|b| {
            b.footprints.retain(|f| !sel.contains(&Sel::Fp(f.id)) || f.locked);
            b.tracks.retain(|t| !sel.contains(&Sel::Track(t.id)));
            b.vias.retain(|v| !sel.contains(&Sel::Via(v.id)));
            b.pours.retain(|v| !sel.contains(&Sel::Pour(v.id)));
            b.fills.retain(|v| !sel.contains(&Sel::Fill(v.id)));
            b.keepouts.retain(|v| !sel.contains(&Sel::Keepout(v.id)));
        });
    }

    pub fn duplicate_selection(&mut self) {
        let sel = self.sel.clone();
        let step = (self.grid * 8.0).max(1.0);
        let mut made = Vec::new();
        self.edit(|b| {
            for s in &sel {
                match *s {
                    Sel::Fp(id) => {
                        let Some(mut f) = b.fp(id).cloned() else { continue };
                        let prefix: String = f.refdes.chars().take_while(|c| c.is_alphabetic()).collect();
                        f.id = b.new_id();
                        f.refdes = b.next_refdes(if prefix.is_empty() { "U" } else { &prefix });
                        f.at = f.at.add(p(step, -step)).rounded();
                        f.locked = false;
                        made.push(Sel::Fp(f.id));
                        b.footprints.push(f);
                    }
                    Sel::Track(id) => {
                        let Some(mut t) = b.track(id).cloned() else { continue };
                        t.id = b.new_id();
                        t.a = t.a.add(p(step, -step)).rounded();
                        t.b = t.b.add(p(step, -step)).rounded();
                        made.push(Sel::Track(t.id));
                        b.tracks.push(t);
                    }
                    Sel::Via(id) => {
                        let Some(mut v) = b.via(id).cloned() else { continue };
                        v.id = b.new_id();
                        v.at = v.at.add(p(step, -step)).rounded();
                        made.push(Sel::Via(v.id));
                        b.vias.push(v);
                    }
                    Sel::Pour(id) => {
                        let Some(mut v) = b.pour(id).cloned() else { continue };
                        v.id = b.new_id();
                        v.order = v.id;
                        v.outline = v.outline.iter().map(|q| q.add(p(step, -step)).rounded()).collect();
                        made.push(Sel::Pour(v.id));
                        b.pours.push(v);
                    }
                    Sel::Fill(id) => {
                        let Some(mut v) = b.fill(id).cloned() else { continue };
                        v.id = b.new_id();
                        v.pts = v.pts.iter().map(|q| q.add(p(step, -step)).rounded()).collect();
                        made.push(Sel::Fill(v.id));
                        b.fills.push(v);
                    }
                    // A name is its part's: there's one of each.
                    Sel::Name(_) => {}
                    Sel::Keepout(id) => {
                        let Some(mut v) = b.keepout(id).cloned() else { continue };
                        v.id = b.new_id();
                        v.pts = v.pts.iter().map(|q| q.add(p(step, -step)).rounded()).collect();
                        made.push(Sel::Keepout(v.id));
                        b.keepouts.push(v);
                    }
                }
            }
        });
        if !made.is_empty() {
            self.sel = made;
        }
    }

    pub fn select_all(&mut self) {
        let filter = self.filter;
        self.sel = self
            .doc
            .footprints
            .iter()
            .map(|f| Sel::Fp(f.id))
            .chain(self.doc.tracks.iter().map(|t| Sel::Track(t.id)))
            .chain(self.doc.vias.iter().map(|v| Sel::Via(v.id)))
            .filter(|s| filter.allows(*s))
            .collect();
    }

    pub fn place_via(&mut self, q: P, tol: f64) {
        let mut at = q.snap(self.grid);
        let mut net = 0;
        for layer in self.doc.copper_layers() {
            if let Some(cu) = self.copper.at(q, layer, tol).first() {
                net = cu.net;
                if matches!(cu.item, Item::Pad(..) | Item::Via(_)) {
                    at = cu.anchors[0];
                } else if !matches!(cu.item, Item::Track(_)) {
                    // A via dropped on a pour or fill joins it where it lands.
                } else {
                    at = dist_to_segment(q, cu.anchors[0], cu.anchors[1]).1.rounded();
                }
                break;
            }
        }
        let rules = self.doc.rules.clone();
        if self.doc.vias.iter().any(|v| v.at.same(at)) {
            return;
        }
        let add = move |b: &mut Board| {
            let id = b.new_id();
            b.vias.push(Via { id, at, dia: rules.via_dia, drill: rules.via_drill, net });
            vec![Item::Via(id)]
        };
        // Only where it keeps its clearance on every layer.
        if let Some(why) = self.clash_with(add.clone()) {
            self.notice = Some(format!("No room for a via here: {why}"));
            return;
        }
        self.edit(|b| {
            add(b);
        });
    }

    /// Makes a pour of the corners clicked, on the layer in use. Its net is
    /// the lit-up one, or the net of what's under its first corner, or GND.
    pub fn close_pour(&mut self) {
        if self.outline.len() < 3 {
            self.notice = Some("A pour needs three corners or more.".into());
            return;
        }
        let mut ring = std::mem::take(&mut self.outline);
        if ring_area(&ring) < 0.0 {
            ring.reverse();
        }
        let layer = self.layer;
        let under = self.copper.at(ring[0], layer, 0.0).into_iter().map(|c| c.net).find(|n| *n != 0);
        let net = self.highlight.or(under).or(self.doc.net_by_name("GND")).unwrap_or(0);
        let mut made = 0;
        self.edit(|b| {
            let id = b.new_id();
            let pour = Pour::new(id, layer, net, ring, &b.rules);
            b.pours.push(pour);
            made = id;
        });
        self.sel = vec![Sel::Pour(made)];
        let name = self.doc.net_name(net).to_string();
        self.notice = Some(format!("Poured {name} on {}; change its net and settings on the right", Layer::Cu(layer).name().to_lowercase()));
    }

    /// Closes the keepout being drawn: on the layer in use, keeping pours out.
    /// Its card on the right says what else it keeps out, and on which layers.
    pub fn close_keepout(&mut self) {
        if self.outline.len() < 3 {
            self.notice = Some("A keepout needs three corners or more.".into());
            return;
        }
        let mut ring = std::mem::take(&mut self.outline);
        if ring_area(&ring) < 0.0 {
            ring.reverse();
        }
        let layer = self.layer;
        let mut made = 0;
        self.edit(|b| {
            let id = b.new_id();
            b.keepouts.push(Keepout { id, layer: Some(layer), pts: ring, restrict: Restrict::pours_only() });
            made = id;
        });
        self.sel = vec![Sel::Keepout(made)];
        self.notice = Some(format!(
            "Keepout on {}: pours keep out of it. Change what it keeps out, and its layer, on the right",
            Layer::Cu(layer).name().to_lowercase()
        ));
    }

    pub fn close_outline(&mut self) {
        if self.outline.len() < 3 {
            self.notice = Some("An outline needs three corners or more.".into());
            return;
        }
        let mut ring = std::mem::take(&mut self.outline);
        if ring_area(&ring) < 0.0 {
            ring.reverse();
        }
        self.edit(|b| b.outline = ring);
        self.fit_pending = true;
    }

    // -- Viewing -------------------------------------------------------------

    pub fn fit(&mut self, rect: Rect) {
        const SIDES: f32 = 40.0;
        const ABOVE: f32 = 40.0;
        const BELOW: f32 = 70.0;
        let b = self.doc.extent();
        let sx = (rect.width() - SIDES).max(50.0) as f64 / b.w().max(0.1);
        let sy = (rect.height() - ABOVE - BELOW).max(50.0) as f64 / b.h().max(0.1);
        self.scale = sx.min(sy);
        self.center = b.center();
        self.center.y -= ((BELOW - ABOVE) / 2.0) as f64 / self.scale;
        self.fitted = true;
    }

    pub fn zoom_to(&mut self, at: P, width: f64) {
        let room = self.canvas_rect.width().min(self.canvas_rect.height()).max(200.0) as f64;
        self.scale = (room / width.max(0.5)).clamp(2.0, 2000.0);
        self.center = at;
        self.fitted = false;
    }

    pub fn active_side(&self) -> Side {
        if self.layer == BOTTOM { Side::Bottom } else { Side::Top }
    }

    pub fn next_layer(&mut self, step: i32) {
        // The copper layers, then the two silkscreens, round and round.
        let mut all: Vec<Layer> = self.doc.copper_layers().into_iter().map(Layer::Cu).collect();
        all.extend([Layer::Silk(Side::Top), Layer::Silk(Side::Bottom)]);
        let i = all.iter().position(|l| *l == self.working()).unwrap_or(0) as i32;
        match all[(i + step).rem_euclid(all.len() as i32) as usize] {
            Layer::Cu(l) => self.switch_layer(l),
            Layer::Silk(side) => self.work_on_silk(side),
            _ => {}
        }
    }

    pub fn switch_layer(&mut self, layer: u8) {
        // Changing layer mid-route drops a via where the route has got to.
        if self.route.as_ref().is_some_and(|r| r.layer != layer) {
            self.route_via(Some(layer));
        }
        self.layer = layer;
        self.silk = None;
        self.hidden.remove(&Layer::Cu(layer));
    }

    /// Works on a silkscreen layer: its names can be picked and dragged.
    pub fn work_on_silk(&mut self, side: Side) {
        if self.route.is_some() {
            self.finish_route();
        }
        self.silk = Some(side);
        self.hidden.remove(&Layer::Silk(side));
    }

    /// The layer being worked on.
    pub fn working(&self) -> Layer {
        match self.silk {
            Some(side) => Layer::Silk(side),
            None => Layer::Cu(self.layer),
        }
    }

    // -- Files ---------------------------------------------------------------

    /// Every file open in a tab, with whether it's the one on show.
    pub fn open_paths(&self) -> Vec<(PathBuf, bool)> {
        let mut out: Vec<(PathBuf, bool)> = self.path.iter().map(|p| (p.clone(), self.active.is_none())).collect();
        for (i, d) in self.docs.iter().enumerate() {
            out.extend(d.paths().into_iter().map(|p| (p, self.active == Some(i))));
        }
        out
    }

    pub fn title(&self) -> String {
        let name = match &self.path {
            Some(p) => p.file_name().unwrap_or_default().to_string_lossy().to_string(),
            None => self.doc.name.clone(),
        };
        if self.dirty { format!("{name}  •") } else { name }
    }

    pub fn new_board(&mut self) {
        if self.dirty {
            self.confirm = Some(Then::New);
            return;
        }
        let before = self.doc.clone();
        self.cancel();
        self.doc = Board::default();
        self.path = None;
        self.sel.clear();
        self.highlight = None;
        self.remember(before);
        self.dirty = false;
        self.fit_pending = true;
    }

    pub fn open(&mut self, path: PathBuf) {
        if crate::pcb::project::is_project_file(&path) {
            self.open_project(&path);
            return;
        }
        // Opening a board shows the board.
        self.activate(None);
        self.note_file(&path);
        // An Altium or Circuit Studio board opens as a new, unsaved board.
        if crate::pcb::altium::is_board(&path) {
            match crate::pcb::altium::read_board(&path) {
                Ok(got) => {
                    let before = std::mem::replace(&mut self.doc, got.board);
                    self.cancel();
                    self.remember(before);
                    self.path = None;
                    self.sel.clear();
                    self.highlight = None;
                    self.fit_pending = true;
                    let n = got.notes.len();
                    self.notice = Some(if n == 0 {
                        format!("Read {}; save it to keep it as a Coppersmith board", path.display())
                    } else {
                        format!("Read {} — {n} notes: {}", path.display(), got.notes.join("; "))
                    });
                }
                Err(e) => self.notice = Some(e),
            }
            return;
        }
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Board::load(&t)) {
            Ok(b) => {
                let before = std::mem::replace(&mut self.doc, b);
                self.cancel();
                self.remember(before);
                self.dirty = false;
                self.path = Some(path);
                self.sel.clear();
                self.highlight = None;
                self.width = self.doc.rules.track;
                self.fit_pending = true;
            }
            Err(e) => self.notice = Some(format!("{} couldn't be opened: {e}", path.display())),
        }
    }

    pub fn save_to(&mut self, path: PathBuf) {
        let path = if path.extension().is_none() { path.with_extension("cspcb") } else { path };
        match std::fs::write(&path, self.doc.save()) {
            Ok(()) => {
                self.notice = Some(format!("Saved {}", path.display()));
                self.path = Some(path);
                self.dirty = false;
            }
            Err(e) => self.notice = Some(format!("{} couldn't be saved: {e}", path.display())),
        }
    }

    pub fn save(&mut self, ctx: &egui::Context) {
        match self.path.clone() {
            Some(path) => self.save_to(path),
            None => self.ask_save_as(ctx),
        }
    }

    pub fn ask_save_as(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.asked_tx.clone(), ctx.clone());
        let name = format!("{}.cspcb", self.doc.name);
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new()
                .set_title("Save the board")
                .set_file_name(&name)
                .add_filter("Coppersmith board", &["cspcb"]);
            if let Some(path) = dialog.save_file() {
                let _ = tx.send(Asked::SaveAs(path));
                ctx.request_repaint();
            }
        });
    }

    pub fn ask_open(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.asked_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            use crate::pcb::altium::with_cases as cases;
            // Boards first; projects, schematics and libraries open in their own places.
            let dialog = rfd::FileDialog::new()
                .set_title("Open")
                .add_filter("Everything Coppersmith opens", &cases(&["cspcb", "json", "PcbDoc", "CSPcbDoc", "SchDoc", "CSSchDoc", "cssch", "PcbLib", "SchLib", "IntLib", "PrjPcb", "CSPrjPcb"]))
                .add_filter("Projects", &cases(&["PrjPcb", "CSPrjPcb"]))
                .add_filter("Boards", &cases(&["cspcb", "json", "PcbDoc", "CSPcbDoc"]))
                .add_filter("Schematics", &cases(&["SchDoc", "CSSchDoc", "cssch"]))
                .add_filter("Libraries", &cases(&["PcbLib", "SchLib", "IntLib"]));
            if let Some(path) = dialog.pick_file() {
                let _ = tx.send(Asked::Open(path));
                ctx.request_repaint();
            }
        });
    }

    pub fn ask_add_library(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.asked_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new()
                .set_title("Add an Altium library")
                .add_filter("Altium footprint library", &crate::pcb::altium::with_cases(&["PcbLib", "IntLib"]));
            if let Some(path) = dialog.pick_file() {
                let _ = tx.send(Asked::AddLibrary(path));
                ctx.request_repaint();
            }
        });
    }

    /// Copies an Altium library into the footprint folder, so it's there
    /// from now on, and reads the library again.
    pub fn add_library(&mut self, path: PathBuf) {
        let got = match crate::pcb::altium::read(&path) {
            Ok(got) => got,
            Err(e) => {
                self.notice = Some(format!("That library couldn't be read: {e}"));
                return;
            }
        };
        let Some(dir) = crate::pcb::library::user_folder() else { return };
        let _ = std::fs::create_dir_all(&dir);
        let to = dir.join(path.file_name().unwrap_or_default());
        if path != to
            && let Err(e) = std::fs::copy(&path, &to)
        {
            self.notice = Some(format!("{} couldn't be copied: {e}", to.display()));
            return;
        }
        self.lib = crate::pcb::library::Library::load();
        let left = if got.notes.is_empty() { String::new() } else { format!(" ({} notes on what was left out: see coppersmith --read-altium)", got.notes.len()) };
        self.notice = Some(format!("Added {} footprints from {}{left}", got.footprints.len(), to.file_name().unwrap_or_default().to_string_lossy()));
    }

    pub fn ask_export(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.asked_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new().set_title("Choose a folder for the Gerber files").pick_folder() {
                let _ = tx.send(Asked::Export(path));
                ctx.request_repaint();
            }
        });
    }

    pub fn export_to(&mut self, dir: PathBuf) {
        // Gerbers of a board whose pours are empty or out of date would be wrong.
        if !self.flood {
            self.flood = true;
            self.poured_rev = u64::MAX;
        }
        if self.poured_rev != self.rev {
            self.doc.repour();
            self.poured_rev = self.rev;
        }
        self.refresh();
        match crate::pcb::gerber::write(&self.doc, &dir) {
            Ok(files) => {
                let errors = self.errors();
                let warn = if errors > 0 { format!(" The checks found {errors} problems.") } else { String::new() };
                self.notice = Some(format!("Wrote {} files to {}.{warn}", files.len(), dir.display()));
                self.exported = Some(dir);
            }
            Err(e) => self.notice = Some(e),
        }
    }

    /// Does what was waiting, now the changes are saved or let go.
    fn go_on(&mut self, then: Then) {
        self.dirty = false;
        match then {
            Then::New => self.new_board(),
            Then::Open(path) => self.open(path),
            Then::Quit => self.may_close = true,
        }
    }

    fn save_changes_dialog(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty && !self.may_close && self.launch.screenshot.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Then::Quit);
        }
        if self.may_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let Some(then) = self.confirm.clone() else { return };
        let frame = egui::Frame::new()
            .fill(theme::RAISED)
            .stroke(egui::Stroke::new(1.0, theme::LINE))
            .corner_radius(egui::CornerRadius::same(theme::CARD_RADIUS))
            .inner_margin(egui::Margin::same(20));
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::unique("save-changes")).frame(frame).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(egui::RichText::new(format!("Save the changes to {}?", self.doc.name)).font(theme::semibold(16.0)));
            ui.add_space(4.0);
            ui.label(egui::RichText::new("They'll be lost if they aren't saved.").color(theme::DIM));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(0);
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(1);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(2);
                }
            });
        });
        if modal.should_close() && choice.is_none() {
            choice = Some(2);
        }
        match choice {
            Some(0) => {
                self.confirm = None;
                match self.path.clone() {
                    Some(path) => {
                        self.save_to(path);
                        if !self.dirty {
                            self.go_on(then);
                        }
                    }
                    None => {
                        self.after_save = Some(then);
                        self.ask_save_as(ctx);
                    }
                }
            }
            Some(1) => {
                self.confirm = None;
                self.go_on(then);
            }
            Some(2) => self.confirm = None,
            _ => {}
        }
    }

    pub fn copperview() -> Option<PathBuf> {
        platform::find_program("copperview")
    }

    pub fn view_in_copperview(&mut self) {
        let (Some(dir), Some(program)) = (self.exported.clone(), App::copperview()) else { return };
        if let Err(e) = std::process::Command::new(program).arg(&dir).spawn() {
            self.notice = Some(format!("Copperview couldn't start: {e}"));
        }
    }

    fn poll(&mut self) {
        while let Ok(asked) = self.asked_rx.try_recv() {
            match asked {
                Asked::AddLibrary(path) => self.add_library(path),
                Asked::Project(path) => self.open_project(&path),
                // A project file shows the project; schematics and libraries open in their editors.
                Asked::Open(path) if crate::pcb::project::is_project_file(&path) => self.open_project(&path),
                Asked::Open(path) if crate::pcb::altium::is_schematic(&path) => self.open_schematic_editor(Some(&path)),
                Asked::Open(path) if crate::pcb::altium::is_library(&path) => self.open_library_editor(Some(&path)),
                Asked::Open(path) if self.dirty => self.confirm = Some(Then::Open(path)),
                Asked::Open(path) => self.open(path),
                Asked::SaveAs(path) => {
                    self.save_to(path);
                    if !self.dirty
                        && let Some(then) = self.after_save.take()
                    {
                        self.go_on(then);
                    }
                }
                Asked::Export(path) => self.export_to(path),
            }
        }
    }

    // -- Keys ----------------------------------------------------------------

    /// Altium's keys while the route tool is in hand. True when the key was
    /// the router's, so nothing else acts on it.
    fn routing_keys(&mut self, ctx: &egui::Context, presses: &[(Key, egui::Modifiers)]) -> bool {
        let hit = |k: Key, shift: bool| presses.iter().any(|(key, m)| *key == k && !m.command && m.shift == shift);
        let ctrl = |k: Key| presses.iter().any(|(key, m)| *key == k && m.command);
        let routing = self.route.is_some();
        let mut used = true;
        if hit(Key::Space, true) {
            self.route_mode.corner = self.route_mode.corner.next();
            self.notice = Some(format!("Corners: {}", self.route_mode.corner.name()));
        } else if hit(Key::Space, false) || hit(Key::Slash, false) {
            if let Some(r) = &mut self.route {
                r.flip = !r.flip;
            }
        } else if hit(Key::R, true) {
            self.route_mode.conflict = self.route_mode.conflict.next();
            self.notice = Some(format!("When something's in the way: {}", self.route_mode.conflict.name()));
        } else if hit(Key::D, true) {
            self.route_mode.remove_loops = !self.route_mode.remove_loops;
            let on = if self.route_mode.remove_loops { "on" } else { "off" };
            self.notice = Some(format!("Taking out old paths a new route replaces: {on}"));
        } else if ctrl(Key::W) {
            self.route_mode.show_clearance = !self.route_mode.show_clearance;
        } else if hit(Key::Comma, false) || hit(Key::Comma, true) || hit(Key::Period, false) || hit(Key::Period, true) {
            // Altium's arc radius keys: 1 mil a step, 10 with Shift.
            let step = if hit(Key::Comma, true) || hit(Key::Period, true) { 0.254 } else { 0.0254 };
            let sign = if hit(Key::Comma, false) || hit(Key::Comma, true) { -1.0 } else { 1.0 };
            let r = &mut self.route_mode.arc_radius;
            *r = nm(*r + sign * step).clamp(0.05, 50.0);
            self.notice = Some(format!("Arc radius {}", self.unit.len(*r)));
        } else if routing && hit(Key::Num1, false) {
            self.route_mode.look_ahead = !self.route_mode.look_ahead;
            let on = if self.route_mode.look_ahead { "on" } else { "off" };
            self.notice = Some(format!("Look-ahead segment: {on}"));
        } else if routing && hit(Key::Num2, false) {
            self.route_via(None);
        } else if hit(Key::Num3, false) {
            self.cycle_width_source();
        } else if routing && (hit(Key::Enter, false)) {
            if let Some((to, item)) = self.route_target {
                self.update_plan(to, item);
                self.route_click();
            }
        } else if routing && hit(Key::Backspace, false) {
            self.route_back();
        } else if routing && hit(Key::Escape, false) {
            self.finish_route();
        } else if routing && hit(Key::V, false) {
            self.route_via_other_side();
        } else if routing && (hit(Key::Num4, false) || hit(Key::Num5, false) || hit(Key::Num6, false)) {
            // 1–3 are the router's while routing; layers go by + − PgUp PgDn.
        } else {
            used = false;
        }
        let _ = ctx;
        used
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        // Each press is matched with the keys held with it, so Ctrl+S never
        // also counts as S.
        let presses: Vec<(Key, egui::Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        let command = presses.iter().any(|(_, m)| m.command);
        let shifted = presses.iter().any(|(_, m)| m.shift);
        let pressed = |k: Key| presses.iter().any(|(key, m)| *key == k && m.command == command);
        // A key with Shift held or not, and no Ctrl.
        let hit = |k: Key, shift: bool| presses.iter().any(|(key, m)| *key == k && !m.command && m.shift == shift);
        let m = egui::Modifiers { shift: shifted, command, ..Default::default() };
        if self.tool == Tool::Route && self.routing_keys(ctx, &presses) {
            return;
        }
        if pressed(Key::F1) {
            self.show_keys = !self.show_keys;
        }
        if m.command {
            // The panels and the ribbon.
            if pressed(Key::OpenBracket) {
                self.show_left = !self.show_left;
            }
            if pressed(Key::CloseBracket) {
                self.show_right = !self.show_right;
            }
            if m.shift && pressed(Key::R) {
                self.ribbon = !self.ribbon;
            }
            if pressed(Key::Z) {
                if m.shift { self.redo() } else { self.undo() }
            }
            if pressed(Key::Y) {
                self.redo();
            }
            if pressed(Key::S) {
                if m.shift { self.ask_save_as(ctx) } else { self.save(ctx) }
            }
            if pressed(Key::O) {
                self.ask_open(ctx);
            }
            if pressed(Key::N) {
                self.new_board();
            }
            if pressed(Key::E) {
                self.ask_export(ctx);
            }
            if pressed(Key::A) {
                self.select_all();
            }
            if pressed(Key::D) {
                self.duplicate_selection();
            }
            return;
        }
        if pressed(Key::Escape) {
            if self.route.is_some() {
                self.finish_route();
            } else if !self.outline.is_empty() || self.drag.is_some() {
                self.cancel();
            } else if self.tool != Tool::Select {
                self.set_tool(Tool::Select);
            } else if self.highlight.is_some() {
                self.highlight = None;
            } else {
                self.sel.clear();
                self.focus = None;
            }
        }
        if pressed(Key::Enter) {
            if self.tool == Tool::Outline {
                self.close_outline();
            } else if self.tool == Tool::Pour {
                self.close_pour();
            } else if self.tool == Tool::Keepout {
                self.close_keepout();
            }
        }
        if pressed(Key::Backspace) {
            if self.route.is_some() {
                self.route_back();
            } else if !self.outline.is_empty() {
                self.outline.pop();
            } else {
                self.delete_selection();
            }
        }
        if pressed(Key::Delete) {
            self.delete_selection();
        }
        if pressed(Key::S) {
            self.set_tool(Tool::Select);
        }
        if pressed(Key::W) {
            self.set_tool(Tool::Route);
        }
        if pressed(Key::V) {
            self.set_tool(Tool::Via);
        }
        if pressed(Key::O) {
            self.set_tool(Tool::Outline);
        }
        if pressed(Key::P) {
            self.set_tool(Tool::Pour);
        }
        if pressed(Key::K) {
            self.set_tool(Tool::Keepout);
        }
        // Space turns a part being dragged or placed, as R does.
        if hit(Key::Space, false) || hit(Key::Space, true) {
            if self.drag.is_some() || self.placing.is_some() {
                self.rotate_selection(if m.shift { -90.0 } else { 90.0 });
            }
        }
        if hit(Key::R, false) || hit(Key::R, true) {
            self.rotate_selection(if m.shift { -90.0 } else { 90.0 });
        }
        if pressed(Key::L) {
            self.flip_selection();
        }
        if pressed(Key::F) {
            self.fit_pending = true;
        }
        if pressed(Key::X) {
            self.mirror = !self.mirror;
        }
        if pressed(Key::G) {
            self.show_grid = !self.show_grid;
        }
        if pressed(Key::U) {
            self.unit = match self.unit {
                Unit::Mm => Unit::Mil,
                Unit::Mil => Unit::Inch,
                Unit::Inch => Unit::Mm,
            };
        }
        if pressed(Key::N) {
            self.highlight = match (self.highlight, self.hover_net()) {
                (Some(h), Some(n)) if h == n => None,
                (_, Some(n)) => Some(n),
                _ => None,
            };
        }
        if pressed(Key::PageUp) {
            self.switch_layer(TOP);
        }
        if pressed(Key::PageDown) {
            self.switch_layer(BOTTOM);
        }
        if pressed(Key::Plus) || pressed(Key::Equals) {
            self.next_layer(1);
        }
        if pressed(Key::Minus) {
            self.next_layer(-1);
        }
        let layers = self.doc.copper_layers();
        for (i, key) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6].iter().enumerate() {
            if pressed(*key) && i < layers.len() {
                self.switch_layer(layers[i]);
            }
        }
    }

    pub fn hover_net(&self) -> Option<u32> {
        let net = |item: Item| self.copper.index.get(&item).map(|&i| self.copper.items[i].net);
        match self.hover? {
            Sel::Track(id) => net(Item::Track(id)),
            Sel::Via(id) => net(Item::Via(id)),
            Sel::Pour(id) => self.doc.pour(id).map(|p| p.net),
            Sel::Fill(id) => self.doc.fill(id).map(|p| p.net),
            Sel::Keepout(_) | Sel::Name(_) => None,
            Sel::Fp(id) => {
                let q = self.hover_q?;
                let f = self.doc.fp(id)?;
                (0..f.pads.len()).find(|&i| f.pad_blob(i, 0.0).dist_to(q) <= 0.0).map(|i| f.pads[i].net)
            }
        }
        .filter(|n| *n != 0)
    }

    /// The tab with this file open, if there is one.
    fn find_doc(&self, path: &std::path::Path) -> Option<usize> {
        self.docs.iter().position(|d| d.paths().iter().any(|p| crate::pcb::project::same_file(p, path)))
    }

    /// The libraries of symbols a sheet can place from: the project's symbol libraries
    /// (read once, and kept), the one open in a tab standing for its file so a sheet
    /// sees its edits, and any other library open in a tab.
    pub fn symbol_libraries(&mut self) -> Vec<crate::ui::schview::SymbolLibrary> {
        use crate::pcb::project::{FileKind, same_file};
        use crate::ui::schview::SymbolLibrary;
        let is_symbols = |p: &std::path::Path| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("schlib") || e.eq_ignore_ascii_case("intlib"));
        let stem = |p: &std::path::Path| p.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let mut out: Vec<SymbolLibrary> = Vec::new();
        let files: Vec<PathBuf> = self.project.iter().flat_map(|p| p.files.iter()).filter(|f| f.kind == FileKind::Library && !f.missing && is_symbols(&f.path)).map(|f| f.path.clone()).collect();
        for path in files {
            let open = self.docs.iter().find_map(|d| match d {
                Doc::Library(w) if w.ed.sym_path.as_deref().is_some_and(|q| same_file(q, &path)) => Some(w.ed.symbols.clone()),
                _ => None,
            });
            let symbols = match open {
                Some(s) => s,
                None => self.symbol_cache.entry(path.clone()).or_insert_with(|| crate::pcb::altium::read(&path).map(|g| g.symbols).unwrap_or_default()).clone(),
            };
            if !symbols.is_empty() {
                out.push(SymbolLibrary { name: stem(&path), path: Some(path), symbols });
            }
        }
        for d in &self.docs {
            if let Doc::Library(w) = d
                && !w.ed.symbols.is_empty()
                && !w.ed.sym_path.as_deref().is_some_and(|q| out.iter().any(|l| l.path.as_deref().is_some_and(|p| same_file(p, q))))
            {
                let name = w.ed.sym_path.as_deref().map(stem).unwrap_or_else(|| w.ed.name.clone());
                out.push(SymbolLibrary { name, path: w.ed.sym_path.clone(), symbols: w.ed.symbols.clone() });
            }
        }
        out
    }

    /// Gives the sheet on show the libraries on offer.
    fn feed_symbols(&mut self) {
        let Some(i) = self.active else { return };
        if !matches!(self.docs.get(i), Some(Doc::Schematic(_))) {
            return;
        }
        let libs = self.symbol_libraries();
        if let Some(Doc::Schematic(w)) = self.docs.get_mut(i) {
            w.set_libraries(libs);
        }
    }

    /// Shows a tab, or the board (None).
    pub fn activate(&mut self, tab: Option<usize>) {
        self.active = tab.filter(|i| *i < self.docs.len());
        self.pending_close = None;
        self.feed_symbols();
    }

    /// Closes a tab. One with unsaved changes asks to be closed twice.
    pub fn close_doc(&mut self, i: usize) {
        let Some(d) = self.docs.get(i) else { return };
        if d.dirty() && self.pending_close != Some(i) {
            self.notice = Some(format!("{} has unsaved changes: save it (Ctrl+S), or close it again to leave them", d.title()));
            self.pending_close = Some(i);
            return;
        }
        if matches!(self.docs[i], Doc::View3d(_)) {
            self.gpu3d_release = true;
        }
        self.docs.remove(i);
        self.pending_close = None;
        self.activate(active_after_close(self.active, i));
    }

    /// Opens a library in a tab, on a file if given (or on the tab that has it).
    pub fn open_library_editor(&mut self, path: Option<&std::path::Path>) {
        if let Some(path) = path {
            if let Some(i) = self.find_doc(path) {
                self.activate(Some(i));
                return;
            }
            self.note_file(path);
        }
        let kind = match path.and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_lowercase()) {
            Some(e) if e == "schlib" => crate::ui::libedit::Kind::Symbols,
            _ => crate::ui::libedit::Kind::Footprints,
        };
        let mut win = crate::ui::libview::LibWindow::new(kind);
        if let Some(path) = path {
            self.notice = win.open_file(path);
        }
        self.docs.push(Doc::Library(Box::new(win)));
        self.activate(Some(self.docs.len() - 1));
    }

    /// Opens the library a footprint is in, in a tab, on that footprint. Saving
    /// goes back to the library's own file if it has one in the footprint folder.
    pub fn edit_footprint(&mut self, def: &FootprintDef) {
        let existing = self.docs.iter().position(|d| matches!(d, Doc::Library(w) if w.ed.name == def.category));
        let i = match existing {
            Some(i) => i,
            None => {
                let defs: Vec<FootprintDef> = self.lib.defs.iter().filter(|d| d.category == def.category).cloned().collect();
                let file = crate::pcb::library::user_folder().map(|d| d.join(format!("{}.PcbLib", def.category))).filter(|p| p.exists());
                let mut win = crate::ui::libview::LibWindow::new(crate::ui::libedit::Kind::Footprints);
                win.load_footprints(&def.category, defs, &def.name, file);
                self.docs.push(Doc::Library(Box::new(win)));
                self.docs.len() - 1
            }
        };
        if existing.is_some()
            && let Doc::Library(w) = &mut self.docs[i]
        {
            w.ed.set_kind(crate::ui::libedit::Kind::Footprints);
            if let Some(k) = w.ed.footprints.iter().position(|f| f.name == def.name) {
                w.ed.choose(k);
            }
        }
        self.activate(Some(i));
        self.notice = Some(if def.source.starts_with("Built in") {
            "Built-in footprints: Save as… to keep your changes in a PcbLib of your own".into()
        } else {
            format!("Editing {}", def.category)
        });
    }

    /// Opens a schematic in a tab, on a file if given (or on the tab that has it).
    pub fn open_schematic_editor(&mut self, path: Option<&std::path::Path>) {
        if let Some(path) = path {
            if let Some(i) = self.find_doc(path) {
                self.activate(Some(i));
                return;
            }
            self.note_file(path);
        }
        let mut win = crate::ui::schview::SchWindow::new();
        if let Some(path) = path {
            self.notice = Some(win.open_file(path));
        }
        self.docs.push(Doc::Schematic(Box::new(win)));
        self.activate(Some(self.docs.len() - 1));
    }

    /// Which window is on show.
    pub fn mode(&self) -> Mode {
        self.active.and_then(|i| self.docs.get(i)).map(|d| d.mode()).unwrap_or(Mode::Board)
    }

    /// The library editor on show, if it is the one.
    pub fn lib_active(&self) -> Option<&crate::ui::libview::LibWindow> {
        match self.active.and_then(|i| self.docs.get(i)) {
            Some(Doc::Library(w)) => Some(w),
            _ => None,
        }
    }

    /// The schematic editor on show, if it is the one.
    pub fn sch_active(&self) -> Option<&crate::ui::schview::SchWindow> {
        match self.active.and_then(|i| self.docs.get(i)) {
            Some(Doc::Schematic(w)) => Some(w),
            _ => None,
        }
    }

    /// Runs a menu command on the editor on show, and takes in what it says.
    pub fn editor_command(&mut self, ctx: &egui::Context, cmd: crate::ui::dock::EditorCmd) {
        let Some(i) = self.active else { return };
        let (mut notice, mut reload) = (None, false);
        match self.docs.get_mut(i) {
            Some(Doc::Library(w)) => {
                let mut out = crate::ui::libview::Outcome::default();
                w.command(cmd, ctx, &mut out);
                (notice, reload) = (out.notice, out.reload_parts);
            }
            Some(Doc::Schematic(w)) => {
                let mut out = crate::ui::schview::Outcome::default();
                w.command(cmd, ctx, &mut out);
                notice = out.notice;
            }
            Some(Doc::View3d(v)) => {
                use crate::ui::dock::EditorCmd;
                match cmd {
                    EditorCmd::Fit => v.refit(),
                    EditorCmd::Tool(0) => v.top(),
                    EditorCmd::Tool(1) => v.bottom(),
                    EditorCmd::Tool(2) => v.angle(),
                    _ => {}
                }
            }
            None => {}
        }
        if notice.is_some() {
            self.notice = notice;
        }
        if reload {
            self.lib = Library::load();
        }
    }

    /// Shows the board in 3D, in a tab of its own (the one already open if there is one).
    pub fn open_3d(&mut self) {
        let i = match self.docs.iter().position(|d| matches!(d, Doc::View3d(_))) {
            Some(i) => i,
            None => {
                self.docs.push(Doc::View3d(Box::new(crate::ui::view3d::View3d::new(None))));
                self.docs.len() - 1
            }
        };
        self.activate(Some(i));
    }

    /// Makes the solid again when the board has changed since (and its pours have caught up).
    fn refresh_3d(&mut self) {
        let current = !self.flood || self.poured_rev == self.rev;
        let rev = self.rev;
        let active = self.active;
        let doc = &self.doc;
        let Some(Doc::View3d(v)) = active.and_then(|i| self.docs.get_mut(i)) else { return };
        if current && v.solid.as_ref().is_none_or(|s| s.rev != rev) {
            let solid = crate::pcb::solid::build(doc, rev);
            let plain = solid.as_ref().is_some_and(|s| !s.outlined);
            v.set_solid(solid);
            if plain {
                self.notice = Some("The board has no outline, so the 3D view uses a plain slab round its parts".into());
            }
        }
    }

    /// Remembers which project a file is in, when it isn't the one on show.
    pub fn note_file(&mut self, path: &std::path::Path) {
        if self.project.as_ref().is_some_and(|p| p.contains(path)) || path.parent().is_none() {
            return;
        }
        self.project = Some(crate::pcb::project::Project::around(path));
    }

    /// Takes a project file as the project, and shows its files.
    pub fn open_project(&mut self, path: &std::path::Path) {
        match crate::pcb::project::Project::from_file(path) {
            Ok(p) => {
                self.notice = Some(format!("Project {}: {} files", p.name, p.files.len()));
                self.project = Some(p);
                self.left_tab = LeftTab::Project;
                self.editor_tab = crate::ui::dock::EditorLeft::Project;
                self.show_left = true;
                self.feed_symbols();
            }
            Err(e) => self.notice = Some(e),
        }
    }

    pub fn ask_open_project(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.asked_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new().set_title("Open a project").add_filter("Altium or Circuit Studio project", &crate::pcb::altium::with_cases(&["PrjPcb", "CSPrjPcb"]));
            if let Some(path) = dialog.pick_file() {
                let _ = tx.send(Asked::Project(path));
                ctx.request_repaint();
            }
        });
    }

    /// The editor in the tab on show.
    fn show_editor(&mut self, ui: &mut Ui) {
        self.refresh_3d();
        let Some(i) = self.active else { return };
        let (gpu, report) = (self.gpu3d.clone(), self.gpu3d_error.clone());
        let known: Vec<String> = self.lib.defs.iter().map(|d| d.name.clone()).collect();
        let (mut notice, mut reload) = (None, false);
        match self.docs.get_mut(i) {
            Some(Doc::Library(w)) => {
                let out = w.show(ui, &known);
                (notice, reload) = (out.notice, out.reload_parts);
            }
            Some(Doc::Schematic(w)) => {
                let out = w.show(ui, &known);
                notice = out.notice;
                // A tool that has its list in the left panel brings it forward.
                if out.show_symbols {
                    self.editor_tab = crate::ui::dock::EditorLeft::Items;
                    self.show_left = true;
                }
            }
            Some(Doc::View3d(v)) => v.show(ui, &gpu, &report),
            None => {}
        }
        if let Ok(mut said) = report.lock()
            && let Some(e) = said.take()
        {
            notice = Some(format!("The 3D view couldn't be drawn: {e}"));
        }
        if notice.is_some() {
            self.notice = notice;
        }
        if reload {
            self.lib = Library::load();
        }
    }

    /// The bar under an editor: just what was last said.
    fn library_status(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.set_height(18.0);
            match self.notice.clone() {
                Some(n) => {
                    if crate::ui::panels::small_icon(ui, crate::ui::icons::Icon::Close, "Dismiss", true).clicked() {
                        self.notice = None;
                    }
                    ui.add(egui::Label::new(egui::RichText::new(n).size(12.5).color(theme::YELLOW)).truncate());
                }
                None => {
                    let what = if self.mode() == Mode::View3d {
                        "3D view · drag to turn, right-drag to move, scroll to zoom · the board's copper, mask and silk as they would be made"
                    } else if self.mode() == Mode::Schematic { "Schematic editor · symbols from Altium libraries; saves as SchDoc or Coppersmith's .cssch" } else { "Library editor · footprints (PcbLib) and symbols (SchLib), as Altium reads them" };
                    ui.label(egui::RichText::new(what).size(12.0).color(theme::FAINT));
                }
            }
        });
    }

    fn apply_launch(&mut self) {
        if !self.applied_launch && self.frames >= 8 && let Some(path) = self.launch.library.take() {
            self.open_library_editor(Some(&path));
        }
        if !self.applied_launch && self.frames >= 8 && std::mem::take(&mut self.launch.view3d) {
            self.open_3d();
        }
        if !self.applied_launch && self.frames >= 8 && self.launch.schematic.is_some() {
            let path = self.launch.schematic.take();
            self.open_schematic_editor(path.as_deref().filter(|p| p.exists()));
        }
        if self.applied_launch || self.frames < 8 || self.canvas_rect == Rect::NOTHING {
            return;
        }
        self.applied_launch = true;
        if let Some((at, w)) = self.launch.zoom {
            self.zoom_to(at, w);
        }
        if let Some(name) = self.launch.select.clone()
            && let Some(f) = self.doc.footprints.iter().find(|f| f.refdes == name)
        {
            self.sel = vec![Sel::Fp(f.id)];
        }
    }
}

/// Writes a line to `slow.log` in Coppersmith's data folder when something
/// interactive took longer than a quarter of a second, so a stall can be
/// traced afterwards.
pub fn note_slow(what: &str, started: std::time::Instant) {
    let ms = started.elapsed().as_millis();
    if ms < 250 {
        return;
    }
    let Some(dir) = crate::platform::data_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = format!("{when} {what} took {ms} ms\n");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("slow.log")) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Whether a track from `from` to `mid` and another from `mid` to `to` carry on
/// in one straight line, going the same way: the joint lies within half a
/// micron of the line from end to end. (Boards from Altium are rounded to a
/// nanometre or so, so the same line is never exact to the last digit.)
fn carries_on(from: P, mid: P, to: P) -> bool {
    let (d1, d2, whole) = (mid.sub(from), to.sub(mid), to.sub(from));
    if d1.len() < 1e-9 || d2.len() < 1e-9 || d1.dot(d2) <= 0.0 {
        return false;
    }
    whole.cross(mid.sub(from)).abs() / whole.len() <= 5e-4
}

/// Joins a track to the next when they carry straight on, so a route laid in
/// clicks doesn't leave a string of short pieces.
pub(crate) fn merge_straight(b: &mut Board) {
    use std::collections::HashMap;
    // Points are the same within 0.0001, so cells of 0.001 and a look at the
    // nine round a point find everything that's there.
    let cell = |q: P| ((q.x * 1000.0).floor() as i64, (q.y * 1000.0).floor() as i64);
    let round = |q: P| {
        let (cx, cy) = cell(q);
        [(-1, -1), (0, -1), (1, -1), (-1, 0), (0, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].map(|(dx, dy)| (cx + dx, cy + dy))
    };
    // The tracks with an end in each cell, and the vias in each.
    let mut ends: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, t) in b.tracks.iter().enumerate() {
        let (ca, cb) = (cell(t.a), cell(t.b));
        ends.entry(ca).or_default().push(i);
        if cb != ca {
            ends.entry(cb).or_default().push(i);
        }
    }
    let mut vias: HashMap<(i64, i64), Vec<P>> = HashMap::new();
    for v in &b.vias {
        vias.entry(cell(v.at)).or_default().push(v.at);
    }
    let mut alive = vec![true; b.tracks.len()];
    for i in 0..b.tracks.len() {
        // Joins on whatever carries straight on from either end, again and
        // again, however each piece happens to be laid: two that meet head on
        // (both running towards the joint) are as much one run as head to tail.
        while alive[i] && b.tracks[i].mid.is_none() {
            let mut joined = false;
            for at_b in [true, false] {
                let (mid, far_t) = if at_b { (b.tracks[i].b, b.tracks[i].a) } else { (b.tracks[i].a, b.tracks[i].b) };
                // What's joined here: tracks with an end here, and vias.
                let mut at: Vec<usize> = Vec::new();
                for c in round(mid) {
                    for &k in ends.get(&c).into_iter().flatten() {
                        if alive[k] && (b.tracks[k].a.same(mid) || b.tracks[k].b.same(mid)) && !at.contains(&k) {
                            at.push(k);
                        }
                    }
                }
                let via_count = round(mid).iter().filter_map(|c| vias.get(c)).flatten().filter(|v| v.same(mid)).count();
                // Something else joining in the middle keeps the corner.
                if at.len() + via_count != 2 {
                    continue;
                }
                let Some(&j) = at.iter().find(|&&k| k != i) else { continue };
                let (t, u) = (&b.tracks[i], &b.tracks[j]);
                if u.mid.is_some() || t.layer != u.layer || t.net != u.net || (t.w - u.w).abs() > 1e-9 {
                    continue;
                }
                let far_u = if u.a.same(mid) { u.b } else { u.a };
                if !carries_on(far_t, mid, far_u) {
                    continue;
                }
                if at_b {
                    b.tracks[i].b = far_u;
                } else {
                    b.tracks[i].a = far_u;
                }
                alive[j] = false;
                ends.entry(cell(far_u)).or_default().push(i);
                joined = true;
                break;
            }
            if !joined {
                break;
            }
        }
    }
    let mut keep = alive.into_iter();
    b.tracks.retain(|_| keep.next().unwrap_or(true));
}

/// What two straight tracks make when they lie on one line and overlap: the
/// track that spans both, running the way `t` does. `None` if they're not
/// parallel, not on one line, or only touch.
fn overlap_union(t: &Track, u: &Track) -> Option<(P, P)> {
    let d = t.b.sub(t.a);
    let e = u.b.sub(u.a);
    let (len, elen) = (d.len(), e.len());
    if len < 1e-9 || elen < 1e-9 || d.cross(e).abs() > 1e-6 * len * elen {
        return None;
    }
    let n = d.scale(1.0 / len);
    let off = |q: P| q.sub(t.a).cross(n).abs();
    if off(u.a) > 1e-4 || off(u.b) > 1e-4 {
        return None;
    }
    let s = |q: P| q.sub(t.a).dot(n);
    let (ulo, uhi) = (s(u.a).min(s(u.b)), s(u.a).max(s(u.b)));
    if ulo.max(0.0) + 1e-6 >= uhi.min(len) {
        return None;
    }
    let ends = [(t.a, 0.0), (t.b, len), (u.a, s(u.a)), (u.b, s(u.b))];
    let lo = ends.iter().min_by(|x, y| x.1.total_cmp(&y.1)).map(|x| x.0)?;
    let hi = ends.iter().max_by(|x, y| x.1.total_cmp(&y.1)).map(|x| x.0)?;
    Some((lo, hi))
}

/// Joins straight tracks that lie on top of one another (same layer, net and
/// width, on one line, overlapping) into one, as happens when a trace is moved
/// along another. Of two, the one in `prefer` (what's picked) keeps its number.
/// Returns how many were taken out.
pub(crate) fn merge_overlapping(b: &mut Board, prefer: &std::collections::HashSet<u32>) -> usize {
    use std::collections::HashMap;
    let mut groups: HashMap<(u8, u32, i64), Vec<usize>> = HashMap::new();
    for (i, t) in b.tracks.iter().enumerate() {
        if t.mid.is_none() && !t.a.same(t.b) {
            groups.entry((t.layer, t.net, (t.w * 1e6).round() as i64)).or_default().push(i);
        }
    }
    let mut dead = vec![false; b.tracks.len()];
    let mut merged = 0;
    for mut idx in groups.into_values().filter(|g| g.len() >= 2) {
        // Left to right, so a track is only compared with those that can reach it;
        // one that grows leftwards upsets the order, so go round until nothing joins.
        loop {
            let mut changed = false;
            let (lo, hi) = (|t: &Track| t.a.x.min(t.b.x), |t: &Track| t.a.x.max(t.b.x));
            idx.sort_by(|&p, &q| lo(&b.tracks[p]).total_cmp(&lo(&b.tracks[q])));
            for ai in 0..idx.len() {
                let i = idx[ai];
                if dead[i] {
                    continue;
                }
                for bi in ai + 1..idx.len() {
                    let j = idx[bi];
                    if dead[j] {
                        continue;
                    }
                    if lo(&b.tracks[j]) > hi(&b.tracks[i]) + 1e-4 {
                        break;
                    }
                    let Some((a, e)) = overlap_union(&b.tracks[i], &b.tracks[j]) else { continue };
                    let (keep, gone) = if prefer.contains(&b.tracks[j].id) && !prefer.contains(&b.tracks[i].id) { (j, i) } else { (i, j) };
                    b.tracks[keep].a = a;
                    b.tracks[keep].b = e;
                    dead[gone] = true;
                    merged += 1;
                    changed = true;
                    if gone == i {
                        break;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
    if merged > 0 {
        let mut keep = dead.into_iter().map(|d| !d);
        b.tracks.retain(|_| keep.next().unwrap_or(true));
    }
    merged
}

/// Tidies tracks after a move or a route: those on top of one another become
/// one, and straight runs in pieces join up.
pub(crate) fn tidy_tracks(b: &mut Board, prefer: &std::collections::HashSet<u32>) {
    merge_overlapping(b, prefer);
    merge_straight(b);
}

/// What `merge_straight` was before it used an index: the same joins, found by
/// comparing every pair of tracks. Kept to check the quick one against.
#[cfg(test)]
pub(crate) fn merge_straight_slow(b: &mut Board) {
    loop {
        let mut joined = false;
        'outer: for i in 0..b.tracks.len() {
            for j in 0..b.tracks.len() {
                if i == j {
                    continue;
                }
                let (t, u) = (&b.tracks[i], &b.tracks[j]);
                if t.mid.is_some() || u.mid.is_some() || t.layer != u.layer || t.net != u.net || (t.w - u.w).abs() > 1e-9 {
                    continue;
                }
                // Each end of one against each end of the other.
                for at_b in [true, false] {
                    let (mid, far_t) = if at_b { (t.b, t.a) } else { (t.a, t.b) };
                    if !(u.a.same(mid) || u.b.same(mid)) {
                        continue;
                    }
                    let far_u = if u.a.same(mid) { u.b } else { u.a };
                    if !carries_on(far_t, mid, far_u) {
                        continue;
                    }
                    // Something else joining in the middle keeps the corner.
                    let others = b.tracks.iter().filter(|o| o.a.same(mid) || o.b.same(mid)).count()
                        + b.vias.iter().filter(|v| v.at.same(mid)).count();
                    if others != 2 {
                        continue;
                    }
                    if at_b {
                        b.tracks[i].b = far_u;
                    } else {
                        b.tracks[i].a = far_u;
                    }
                    b.tracks.remove(j);
                    joined = true;
                    break 'outer;
                }
            }
        }
        if !joined {
            break;
        }
    }
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.pinch.stop();
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.play_step(raw_input);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        self.save_changes_dialog(&ctx);
        if self.confirm.is_none() && self.mode() == Mode::Board {
            self.keys(&ctx);
        }
        // Ctrl+W closes the tab on show (the board stays).
        if self.confirm.is_none()
            && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::W))
            && let Some(i) = self.active
        {
            self.close_doc(i);
        }
        self.refresh();
        self.drive_screenshot(&ctx);
        self.apply_launch();
        let width = ctx.content_rect().width();
        // The menus, then the tabs of what's open; under them, on the board, the ribbon if it's switched on.
        let top_frame = egui::Frame::new().fill(theme::WELL).inner_margin(egui::Margin::symmetric(12, 4));
        Panel::top("menus").resizable(false).frame(top_frame).show(ui, |ui| self.menu_bar(ui));
        let tab_frame = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 4));
        Panel::top("doc_tabs").resizable(false).frame(tab_frame).show(ui, |ui| self.tab_bar(ui));
        // A closed 3D tab lets go of its pictures, from inside a draw so the card's context is current.
        if self.gpu3d_release && self.mode() != Mode::View3d {
            let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0));
            ui.painter().add(crate::ui::gpu3d::release(rect, self.gpu3d.clone()));
            self.gpu3d_release = false;
            ctx.request_repaint();
        }
        if self.mode() != Mode::Board {
            let status_frame = egui::Frame::new().fill(theme::WELL).inner_margin(egui::Margin::symmetric(12, 5));
            Panel::bottom("status").resizable(false).frame(status_frame).show(ui, |ui| self.library_status(ui));
            if self.show_left && width >= 600.0 {
                let frame = egui::Frame::new().fill(theme::PANEL);
                Panel::left("editor_left").resizable(true).default_size(190.0).size_range(160.0..=310.0).frame(frame).show(ui, |ui| self.editor_left_dock(ui));
            }
            self.show_editor(ui);
            self.shortcuts_window(&ctx);
            return;
        }
        if self.ribbon {
            let ribbon_frame = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 6));
            Panel::top("ribbon").resizable(false).frame(ribbon_frame).show(ui, |ui| self.ribbon_bar(ui));
        }
        // The panels either side, each with its tabs along the bottom.
        if self.show_left && width >= 600.0 {
            let frame = egui::Frame::new().fill(theme::PANEL);
            Panel::left("left").resizable(true).default_size(196.0).size_range(160.0..=310.0).frame(frame).show(ui, |ui| self.left_dock(ui));
        }
        if self.show_right && width >= 900.0 {
            let frame = egui::Frame::new().fill(theme::PANEL);
            Panel::right("right").resizable(true).default_size(217.0).size_range(175.0..=336.0).frame(frame).show(ui, |ui| self.right_dock(ui));
        }
        let status_frame = egui::Frame::new().fill(theme::WELL).inner_margin(egui::Margin::symmetric(12, 5));
        Panel::bottom("status").resizable(false).frame(status_frame).show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::WELL)).show(ui, |ui| self.canvas(ui));
        self.shortcuts_window(&ctx);
        self.classes_window(&ctx);
        self.keep_layout();
        // A change made this frame shows its effect straight away.
        if self.copper_rev != self.rev || self.checked_rev != self.rev {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_a_tab_keeps_the_right_one_on_show() {
        // Tabs: the board (None), then docs 0, 1, 2.
        assert_eq!(active_after_close(Some(1), 1), Some(0), "closing the one on show shows the one before");
        assert_eq!(active_after_close(Some(0), 0), None, "the first doc's neighbour is the board");
        assert_eq!(active_after_close(Some(2), 0), Some(1), "one further along moves up");
        assert_eq!(active_after_close(Some(0), 2), Some(0), "one before it is unmoved");
        assert_eq!(active_after_close(None, 1), None, "the board stays on show");
    }

    #[test]
    fn legs_are_straight_then_diagonal() {
        use crate::pcb::router::{Corner, legs};
        let legs2 = legs(p(0.0, 0.0), p(5.0, 2.0), Corner::D45, false);
        assert_eq!(legs2, vec![p(0.0, 0.0), p(3.0, 0.0), p(5.0, 2.0)]);
        let legs = legs(p(0.0, 0.0), p(5.0, 2.0), Corner::D45, true);
        assert_eq!(legs, vec![p(0.0, 0.0), p(2.0, 2.0), p(5.0, 2.0)]);
    }

    #[test]
    fn moving_a_part_drags_its_tracks() {
        let mut b = crate::pcb::sample::board();
        let r3 = b.footprints.iter().find(|f| f.refdes == "R3").unwrap().id;
        App::move_selection(&mut b, &[Sel::Fp(r3)], p(0.0, -1.0));
        // Still one straight track from D1 to where R3's pad has gone.
        let led: Vec<&Track> = b.tracks.iter().filter(|t| t.net == b.net_by_name("LED").unwrap()).collect();
        assert_eq!(led.len(), 1);
        let ends = [led[0].a, led[0].b];
        assert!(ends.iter().any(|q| q.same(p(16.9125, 27.0))) && ends.iter().any(|q| q.same(p(16.9125, 21.0))), "{led:?}");
    }

    /// A board with a straight run left in pieces, and with dots of copper
    /// (tracks of no length, as an Altium board has) that a clumsy tidy would wipe.
    fn board_with_a_run_in_pieces() -> Board {
        let mut b = crate::pcb::sample::board();
        let n = b.net("SPLIT");
        for (a, e) in [((5.0, 5.0), (8.0, 5.0)), ((8.0, 5.0), (11.0, 5.0)), ((11.0, 5.0), (14.0, 5.0)), ((20.0, 8.0), (20.0, 8.0)), ((22.0, 8.0), (22.0, 8.0))] {
            let id = b.new_id();
            b.tracks.push(Track { mid: None, id, a: p(a.0, a.1), b: p(e.0, e.1), w: 0.4, layer: TOP, net: n });
        }
        b
    }

    #[test]
    fn moving_a_name_leaves_every_track_as_it_was() {
        let mut b = board_with_a_run_in_pieces();
        let r3 = b.footprints.iter().find(|f| f.refdes == "R3").unwrap().id;
        let (tracks, part, label) = (b.tracks.clone(), b.fp(r3).unwrap().at, b.fp(r3).unwrap().label_pos());
        App::move_selection(&mut b, &[Sel::Name(r3)], p(1.5, -0.5));
        assert_eq!(b.tracks, tracks, "no track was touched, or joined to another");
        assert_eq!(b.fp(r3).unwrap().at, part, "the part stays");
        assert!(b.fp(r3).unwrap().label_pos().dist(label.add(p(1.5, -0.5))) < 1e-3, "the name moves");
    }

    #[test]
    fn moving_a_keepout_or_a_pour_leaves_every_track_as_it_was_too() {
        let mut b = board_with_a_run_in_pieces();
        let k = b.new_id();
        b.keepouts.push(Keepout { id: k, layer: Some(TOP), pts: rect_outline(p(30.0, 30.0), p(34.0, 34.0)), restrict: Restrict::pours_only() });
        let f = b.new_id();
        b.fills.push(Fill { id: f, layer: TOP, net: 0, pts: rect_outline(p(20.0, 30.0), p(24.0, 34.0)) });
        let tracks = b.tracks.clone();
        App::move_selection(&mut b, &[Sel::Keepout(k), Sel::Fill(f)], p(1.0, 1.0));
        assert_eq!(b.tracks, tracks);
        assert!(b.keepout(k).unwrap().pts[0].same(p(31.0, 31.0)) && b.fill(f).unwrap().pts[0].same(p(21.0, 31.0)));
    }


    /// A board's tracks in a form that doesn't depend on their numbering or order.
    fn shape(b: &Board) -> Vec<(u8, u32, i64, [i64; 4])> {
        let k = |v: f64| (v * 10_000.0).round() as i64;
        // Whichever way round a track is laid, it's the same track.
        let mut out: Vec<_> = b
            .tracks
            .iter()
            .map(|t| {
                let (a, e) = ((k(t.a.x), k(t.a.y)), (k(t.b.x), k(t.b.y)));
                let (a, e) = if a <= e { (a, e) } else { (e, a) };
                (t.layer, t.net, k(t.w), [a.0, a.1, e.0, e.1])
            })
            .collect();
        out.sort();
        out
    }

    /// Boards of straight runs in pieces, shuffled: with vias and branches at
    /// some joints, other widths and nets, pieces laid the other way, arcs.
    fn random_runs(seed: u64) -> Board {
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut rnd = move |n: u64| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) % n
        };
        let mut b = Board::default();
        let nets = [b.net("A"), b.net("B")];
        let dirs = [(1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, -1.0), (-1.0, 0.0), (-1.0, 1.0)];
        let mut pieces: Vec<Track> = Vec::new();
        for run in 0..(3 + rnd(6)) {
            let (dx, dy) = dirs[rnd(dirs.len() as u64) as usize];
            let mut at = p(2.0 + rnd(40) as f64, 2.0 + rnd(30) as f64);
            let (layer, net, w) = (if rnd(4) == 0 { BOTTOM } else { TOP }, nets[rnd(2) as usize], [0.2, 0.25][rnd(2) as usize]);
            for k in 0..(1 + rnd(5)) {
                let len = 0.5 + rnd(6) as f64 * 0.5;
                let to = at.add(p(dx * len, dy * len));
                let mut next = to;
                let mut t = Track { mid: None, id: 0, a: at, b: to, w, layer, net };
                match rnd(14) {
                    // A different width or net: the run stops being one.
                    0 => t.w = 0.3,
                    1 => t.net = nets[1 - (net == nets[1]) as usize],
                    // Laid the other way round: it still carries on, meeting the last head on.
                    2 => std::mem::swap(&mut t.a, &mut t.b),
                    // Something joined in the middle.
                    3 => {
                        let id = b.new_id();
                        b.vias.push(Via { id, at: to, dia: 0.6, drill: 0.3, net });
                    }
                    4 => pieces.push(Track { mid: None, id: 0, a: to, b: to.add(p(0.0, 1.0)), w, layer, net }),
                    // An arc, which is never joined.
                    5 if k > 0 => t.mid = Some(at.lerp(to, 0.5).add(p(0.1, 0.1))),
                    // Not quite where the last ended, but within the tolerance.
                    6 => t.a = t.a.add(p(5e-5, 0.0)),
                    // On the other layer from here on, with no via: it doesn't carry on.
                    7 | 9 => t.layer = if layer == TOP { BOTTOM } else { TOP },
                    // Doubling back on itself.
                    8 | 10 => {
                        t.b = at.sub(p(dx * len, dy * len));
                        next = t.b;
                    }
                    _ => {}
                }
                pieces.push(t);
                at = next;
            }
            let _ = run;
        }
        // Shuffled, then numbered.
        for i in (1..pieces.len()).rev() {
            pieces.swap(i, rnd(i as u64 + 1) as usize);
        }
        for mut t in pieces {
            t.id = b.new_id();
            b.tracks.push(t);
        }
        b
    }

    #[test]
    fn the_quick_join_makes_the_joins_the_slow_one_did() {
        let mut joined_somewhere = 0;
        for seed in 0..300 {
            let b = random_runs(seed);
            let (mut quick, mut slow) = (b.clone(), b.clone());
            merge_straight(&mut quick);
            merge_straight_slow(&mut slow);
            assert_eq!(shape(&quick), shape(&slow), "seed {seed}");
            if quick.tracks.len() < b.tracks.len() {
                joined_somewhere += 1;
            }
        }
        assert!(joined_somewhere > 100, "{joined_somewhere} of 300 boards had something to join");
    }

    #[test]
    fn joining_leaves_dots_and_other_tracks_alone() {
        let mut b = board_with_a_run_in_pieces();
        let before = b.tracks.len();
        merge_straight(&mut b);
        assert_eq!(b.tracks.len(), before - 2, "the three pieces become one");
        let split = b.net_by_name("SPLIT").unwrap();
        let run: Vec<&Track> = b.tracks.iter().filter(|t| t.net == split && !t.a.same(t.b)).collect();
        assert_eq!(run.len(), 1);
        assert!(run[0].a.same(p(5.0, 5.0)) && run[0].b.same(p(14.0, 5.0)));
        assert_eq!(b.tracks.iter().filter(|t| t.a.same(t.b)).count(), 2, "the dots stand");
    }

    #[test]
    fn turning_a_part_brings_its_tracks_round() {
        let mut b = crate::pcb::sample::board();
        let r3 = b.footprints.iter().find(|f| f.refdes == "R3").unwrap().id;
        let led = b.net_by_name("LED").unwrap();
        App::turn_parts(&mut b, &[r3], 90.0, false);
        let f = b.fp(r3).unwrap();
        assert_eq!(f.rot, 90.0);
        // The LED track still ends on R3's pad 2, wherever that's gone.
        let pad2 = f.pad_center(1);
        assert!(b.tracks.iter().any(|t| t.net == led && (t.a.same(pad2) || t.b.same(pad2))), "{pad2:?}");
        assert!(b.tracks.iter().filter(|t| t.net == led).all(|t| crate::pcb::slide::octilinear(t.b.sub(t.a))));
    }

    fn track(id: u32, a: P, e: P, w: f64, net: u32) -> Track {
        Track { mid: None, id, a, b: e, w, layer: TOP, net }
    }

    #[test]
    fn tracks_lying_on_one_another_become_one() {
        let none = std::collections::HashSet::new();
        // Overlapping, one inside another, and a third that carries on past them.
        let mut b = Board::default();
        b.tracks = vec![
            track(1, p(0.0, 0.0), p(6.0, 0.0), 0.25, 1),
            track(2, p(4.0, 0.0), p(10.0, 0.0), 0.25, 1),
            track(3, p(2.0, 0.0), p(3.0, 0.0), 0.25, 1),
            track(4, p(9.0, 0.0), p(15.0, 0.0), 0.25, 1),
        ];
        assert_eq!(merge_overlapping(&mut b, &none), 3);
        assert_eq!(b.tracks.len(), 1);
        assert!(b.tracks[0].a.same(p(0.0, 0.0)) && b.tracks[0].b.same(p(15.0, 0.0)), "{:?}", b.tracks[0]);
        // Slanted ones, whichever way they run.
        let mut b = Board::default();
        b.tracks = vec![track(1, p(0.0, 0.0), p(4.0, 4.0), 0.25, 1), track(2, p(6.0, 6.0), p(2.0, 2.0), 0.25, 1)];
        assert_eq!(merge_overlapping(&mut b, &none), 1);
        assert!(b.tracks[0].a.same(p(0.0, 0.0)) && b.tracks[0].b.same(p(6.0, 6.0)));
    }

    #[test]
    fn tracks_that_differ_or_only_touch_are_left_alone() {
        let none = std::collections::HashSet::new();
        let mut b = Board::default();
        b.tracks = vec![
            track(1, p(0.0, 0.0), p(6.0, 0.0), 0.25, 1),
            track(2, p(4.0, 0.0), p(10.0, 0.0), 0.30, 1),
            track(3, p(4.0, 0.0), p(10.0, 0.0), 0.25, 2),
            track(4, p(4.0, 0.1), p(10.0, 0.1), 0.25, 1),
            track(5, p(4.0, 0.0), p(8.0, 1.0), 0.25, 1),
            track(6, p(6.0, 0.0), p(9.0, 0.0), 0.25, 1),
        ];
        b.tracks.push(Track { layer: BOTTOM, ..track(7, p(4.0, 0.0), p(10.0, 0.0), 0.25, 1) });
        assert_eq!(merge_overlapping(&mut b, &none), 0, "other width, net, layer, a gap sideways, another angle, and one only touching");
        assert_eq!(b.tracks.len(), 7);
        // The one that only touches is joined by the usual end-to-end merge.
        tidy_tracks(&mut b, &none);
        assert_eq!(b.tracks.len(), 6);
    }

    #[test]
    fn what_is_picked_keeps_its_number_when_it_merges() {
        let mut b = Board::default();
        b.tracks = vec![track(1, p(0.0, 0.0), p(6.0, 0.0), 0.25, 1), track(2, p(4.0, 0.0), p(10.0, 0.0), 0.25, 1)];
        let picked: std::collections::HashSet<u32> = [2].into_iter().collect();
        merge_overlapping(&mut b, &picked);
        assert_eq!(b.tracks.len(), 1);
        assert_eq!(b.tracks[0].id, 2);
        assert!(b.tracks[0].a.same(p(0.0, 0.0)) && b.tracks[0].b.same(p(10.0, 0.0)));
    }

    #[test]
    fn tracks_meeting_head_on_become_one_whichever_way_they_are_laid() {
        // Both running towards the joint, as a slide leaves them when the track between vanishes.
        let mut b = Board::default();
        b.tracks = vec![track(1, p(0.0, 0.0), p(5.0, 0.0), 0.25, 1), track(2, p(9.0, 0.0), p(5.0, 0.0), 0.25, 1)];
        merge_straight(&mut b);
        assert_eq!(b.tracks.len(), 1);
        let t = &b.tracks[0];
        let ends = [t.a, t.b];
        assert!(ends.iter().any(|q| q.same(p(0.0, 0.0))) && ends.iter().any(|q| q.same(p(9.0, 0.0))), "{t:?}");
        // Both running away from the joint.
        let mut b = Board::default();
        b.tracks = vec![track(1, p(5.0, 0.0), p(0.0, 0.0), 0.25, 1), track(2, p(5.0, 0.0), p(9.0, 3.0), 0.25, 1)];
        merge_straight(&mut b);
        assert_eq!(b.tracks.len(), 2, "a corner isn't a run");
        let mut b = Board::default();
        b.tracks = vec![track(1, p(5.0, 0.0), p(0.0, 0.0), 0.25, 1), track(2, p(5.0, 0.0), p(9.0, 0.0), 0.25, 1)];
        merge_straight(&mut b);
        assert_eq!(b.tracks.len(), 1);
        // A branch at the joint keeps it, as before.
        let mut b = Board::default();
        b.tracks = vec![
            track(1, p(0.0, 0.0), p(5.0, 0.0), 0.25, 1),
            track(2, p(9.0, 0.0), p(5.0, 0.0), 0.25, 1),
            track(3, p(5.0, 0.0), p(5.0, 4.0), 0.25, 1),
        ];
        merge_straight(&mut b);
        assert_eq!(b.tracks.len(), 3);
    }

    #[test]
    fn a_slid_track_that_vanishes_leaves_one_run() {
        // X ─ A ┐        sliding the middle track along its neighbours' line, so it
        //       │ T      shrinks to nothing, leaves X and Y meeting head on.
        //   Y ─ B ┘
        let mut b = Board::default();
        let n = b.net("N");
        // X runs right to the joint, the track between has shrunk to a point, and Y
        // runs left to it from the far side.
        b.tracks = vec![
            track(1, p(0.0, 0.0), p(5.0, 0.0), 0.25, n),
            track(2, p(5.0, 0.0), p(5.0, 0.0), 0.25, n),
            track(3, p(10.0, 0.0), p(5.0, 0.0), 0.25, n),
        ];
        let dots = std::collections::HashSet::new();
        crate::pcb::slide::tidy(&mut b, &dots);
        assert_eq!(b.tracks.len(), 2, "the vanished track is taken out");
        tidy_tracks(&mut b, &dots);
        assert_eq!(b.tracks.len(), 1, "{:?}", b.tracks);
        let t = &b.tracks[0];
        assert!([t.a, t.b].iter().any(|q| q.same(p(0.0, 0.0))) && [t.a, t.b].iter().any(|q| q.same(p(10.0, 0.0))));
    }

    #[test]
    fn straight_runs_become_one_track() {
        let mut b = Board::default();
        for (id, a, e) in [(1, p(0.0, 0.0), p(1.0, 0.0)), (2, p(1.0, 0.0), p(3.0, 0.0)), (3, p(3.0, 0.0), p(4.0, 1.0))] {
            b.tracks.push(Track { mid: None, id, a, b: e, w: 0.25, layer: TOP, net: 1 });
        }
        merge_straight(&mut b);
        assert_eq!(b.tracks.len(), 2);
        assert!(b.tracks.iter().any(|t| t.a.same(p(0.0, 0.0)) && t.b.same(p(3.0, 0.0))));
    }
}
