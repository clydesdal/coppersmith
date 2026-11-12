//! How the window is laid out: the panels either side and their tabs, what
//! can be picked (the select filter), and what of this is remembered between
//! runs. The menus, the ribbon and the panels themselves are drawn below.

use crate::ui::app::Sel;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The pages of the left panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum LeftTab {
    /// The project's schematics, boards and libraries.
    Project,
    Layers,
    Parts,
    Select,
}

impl LeftTab {
    pub const ALL: [LeftTab; 4] = [LeftTab::Project, LeftTab::Layers, LeftTab::Parts, LeftTab::Select];

    pub fn name(self) -> &'static str {
        match self {
            LeftTab::Project => "Project",
            LeftTab::Layers => "Layers",
            LeftTab::Parts => "Parts",
            LeftTab::Select => "Select",
        }
    }
}

/// What a menu asks of whichever editor (library or schematic) is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditorCmd {
    /// A new footprint, symbol or sheet.
    New,
    Open,
    Save,
    SaveAs,
    /// Footprints into the folder the Parts list reads.
    SaveToParts,
    Undo,
    Redo,
    Delete,
    Turn,
    Mirror,
    Fit,
    /// A tool by its place in the toolbar: 0 select, then the drawing tools.
    Tool(u8),
}

/// The pages of the left panel of a schematic or library tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditorLeft {
    Project,
    /// The editor's own list: symbols to place, or the library's footprints or symbols.
    Items,
}

/// The pages of the right panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum RightTab {
    Properties,
    Checks,
    Nets,
}

impl RightTab {
    pub const ALL: [RightTab; 3] = [RightTab::Properties, RightTab::Checks, RightTab::Nets];

    pub fn name(self) -> &'static str {
        match self {
            RightTab::Properties => "Properties",
            RightTab::Checks => "Checks",
            RightTab::Nets => "Nets",
        }
    }
}

/// A kind of thing the select filter lets through or holds back.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Pick {
    Parts,
    /// A part's name on the silkscreen.
    Names,
    Tracks,
    Vias,
    Pours,
    Fills,
    Keepouts,
}

impl Pick {
    pub const ALL: [Pick; 7] = [Pick::Parts, Pick::Names, Pick::Tracks, Pick::Vias, Pick::Pours, Pick::Fills, Pick::Keepouts];

    pub fn name(self) -> &'static str {
        match self {
            Pick::Parts => "Parts",
            Pick::Names => "Names",
            Pick::Tracks => "Tracks",
            Pick::Vias => "Vias",
            Pick::Pours => "Pours",
            Pick::Fills => "Fills",
            Pick::Keepouts => "Keepouts",
        }
    }

    /// What's said of one when it's not the kind being asked for.
    pub fn one(self) -> &'static str {
        match self {
            Pick::Parts => "a part",
            Pick::Names => "a name",
            Pick::Tracks => "a track",
            Pick::Vias => "a via",
            Pick::Pours => "a pour",
            Pick::Fills => "a fill",
            Pick::Keepouts => "a keepout",
        }
    }

    pub fn of(sel: Sel) -> Pick {
        match sel {
            Sel::Fp(_) => Pick::Parts,
            Sel::Name(_) => Pick::Names,
            Sel::Track(_) => Pick::Tracks,
            Sel::Via(_) => Pick::Vias,
            Sel::Pour(_) => Pick::Pours,
            Sel::Fill(_) => Pick::Fills,
            Sel::Keepout(_) => Pick::Keepouts,
        }
    }
}

/// Which kinds of thing can be picked by a click, a box or Select all.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    pub parts: bool,
    pub names: bool,
    pub tracks: bool,
    pub vias: bool,
    pub pours: bool,
    pub fills: bool,
    pub keepouts: bool,
}

impl Default for Filter {
    /// Everything.
    fn default() -> Self {
        Filter { parts: true, names: true, tracks: true, vias: true, pours: true, fills: true, keepouts: true }
    }
}

impl Filter {
    pub fn none() -> Filter {
        Filter { parts: false, names: false, tracks: false, vias: false, pours: false, fills: false, keepouts: false }
    }

    /// Just one kind.
    pub fn only(kind: Pick) -> Filter {
        let mut f = Filter::none();
        f.set(kind, true);
        f
    }

    /// The copper that carries signals: tracks, vias, pours and fills.
    pub fn copper() -> Filter {
        Filter { tracks: true, vias: true, pours: true, fills: true, ..Filter::none() }
    }

    /// Parts, and their names.
    pub fn parts() -> Filter {
        Filter { parts: true, names: true, ..Filter::none() }
    }

    pub fn on(&self, kind: Pick) -> bool {
        match kind {
            Pick::Parts => self.parts,
            Pick::Names => self.names,
            Pick::Tracks => self.tracks,
            Pick::Vias => self.vias,
            Pick::Pours => self.pours,
            Pick::Fills => self.fills,
            Pick::Keepouts => self.keepouts,
        }
    }

    pub fn set(&mut self, kind: Pick, on: bool) {
        match kind {
            Pick::Parts => self.parts = on,
            Pick::Names => self.names = on,
            Pick::Tracks => self.tracks = on,
            Pick::Vias => self.vias = on,
            Pick::Pours => self.pours = on,
            Pick::Fills => self.fills = on,
            Pick::Keepouts => self.keepouts = on,
        }
    }

    pub fn allows(&self, sel: Sel) -> bool {
        self.on(Pick::of(sel))
    }

    /// Does it let everything through?
    pub fn is_all(&self) -> bool {
        *self == Filter::default()
    }

    pub fn is_none(&self) -> bool {
        *self == Filter::none()
    }

    /// The kinds it lets through, in order.
    pub fn kinds(&self) -> Vec<Pick> {
        Pick::ALL.into_iter().filter(|k| self.on(*k)).collect()
    }

    /// A few words for a chip: "All", "Tracks, Vias", "5 kinds", "Nothing".
    pub fn summary(&self) -> String {
        let kinds = self.kinds();
        match kinds.len() {
            7 => "All".into(),
            0 => "Nothing".into(),
            1 | 2 => kinds.iter().map(|k| k.name()).collect::<Vec<_>>().join(", "),
            n => format!("{n} kinds"),
        }
    }
}

/// The first of `hits`, which come in the order a click should reach them,
/// that the filter lets through: a click goes past what it holds back.
pub fn first_allowed(hits: &[(u8, f64, Sel)], filter: &Filter) -> Option<Sel> {
    hits.iter().map(|h| h.2).find(|s| filter.allows(*s))
}

/// What the filter is holding back under a point, when there's nothing there
/// it lets through: the kind of the first thing, to say why nothing was picked.
pub fn held_back_kind(hits: &[(u8, f64, Sel)], filter: &Filter) -> Option<Pick> {
    if first_allowed(hits, filter).is_some() { None } else { hits.first().map(|h| Pick::of(h.2)) }
}

/// What of the window's layout is kept from one run to the next.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub show_left: bool,
    pub show_right: bool,
    pub left_tab: LeftTab,
    pub right_tab: RightTab,
    /// The strip of controls under the menus.
    pub ribbon: bool,
    pub filter: Filter,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { show_left: true, show_right: true, left_tab: LeftTab::Layers, right_tab: RightTab::Properties, ribbon: false, filter: Filter::default() }
    }
}

impl Prefs {
    pub fn path() -> Option<PathBuf> {
        Some(crate::platform::data_dir()?.join("window.json"))
    }

    /// As written, or the usual layout where it can't be read (a file from a
    /// version that had less, or a damaged one, still opens the window).
    pub fn from_json(text: &str) -> Prefs {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Sets the layout from words, as `--layout` gives them: `ribbon`,
    /// `no-ribbon`, `no-left`, `left`, `no-right`, `right`, `left:select`,
    /// `right:checks`, `filter:tracks+vias`, `filter:all`, `filter:none`,
    /// `filter:copper`.
    pub fn apply_spec(&mut self, spec: &str) -> Result<(), String> {
        for word in spec.split([',', ';', ' ']).map(str::trim).filter(|w| !w.is_empty()) {
            let (key, value) = word.split_once(':').unwrap_or((word, ""));
            let value = value.to_lowercase();
            match key.to_lowercase().as_str() {
                "ribbon" => self.ribbon = true,
                "no-ribbon" => self.ribbon = false,
                "left" if value.is_empty() => self.show_left = true,
                "no-left" => self.show_left = false,
                "right" if value.is_empty() => self.show_right = true,
                "no-right" => self.show_right = false,
                "left" => {
                    self.left_tab = LeftTab::ALL.into_iter().find(|t| t.name().to_lowercase() == value).ok_or(format!("There's no left tab called {value}"))?;
                    self.show_left = true;
                }
                "right" => {
                    self.right_tab = RightTab::ALL.into_iter().find(|t| t.name().to_lowercase() == value).ok_or(format!("There's no right tab called {value}"))?;
                    self.show_right = true;
                }
                "filter" => {
                    self.filter = match value.as_str() {
                        "all" | "" => Filter::default(),
                        "none" => Filter::none(),
                        "copper" => Filter::copper(),
                        kinds => {
                            let mut f = Filter::none();
                            for name in kinds.split('+') {
                                let kind = Pick::ALL.into_iter().find(|k| k.name().to_lowercase() == name).ok_or(format!("There's no kind of thing called {name} to pick"))?;
                                f.set(kind, true);
                            }
                            f
                        }
                    };
                }
                other => return Err(format!("I don't know the layout word {other}")),
            }
        }
        Ok(())
    }

    pub fn load() -> Prefs {
        Prefs::path().map(|p| Prefs::load_from(&p)).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(path) = Prefs::path() {
            self.save_to(&path);
        }
    }

    /// What's in a file, or the usual layout where there's none or it's no good.
    pub fn load_from(path: &std::path::Path) -> Prefs {
        std::fs::read_to_string(path).map(|t| Prefs::from_json(&t)).unwrap_or_default()
    }

    /// Writes it down, making the folder if need be; a failure is no reason to stop.
    pub fn save_to(&self, path: &std::path::Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.to_json());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_filter_lets_everything_through() {
        let f = Filter::default();
        assert!(f.is_all() && !f.is_none());
        for sel in [Sel::Fp(1), Sel::Name(1), Sel::Track(1), Sel::Via(1), Sel::Pour(1), Sel::Fill(1), Sel::Keepout(1)] {
            assert!(f.allows(sel), "{sel:?}");
        }
        assert_eq!(f.summary(), "All");
    }

    #[test]
    fn every_kind_of_selection_has_its_own_switch() {
        let all = [(Sel::Fp(1), Pick::Parts), (Sel::Name(1), Pick::Names), (Sel::Track(1), Pick::Tracks), (Sel::Via(1), Pick::Vias), (Sel::Pour(1), Pick::Pours), (Sel::Fill(1), Pick::Fills), (Sel::Keepout(1), Pick::Keepouts)];
        for (sel, kind) in all {
            assert_eq!(Pick::of(sel), kind);
            // Only that kind lets it through, and with just it off, only it is held back.
            let only = Filter::only(kind);
            assert!(only.allows(sel));
            for (other, k) in all.iter().filter(|(_, k)| *k != kind) {
                assert!(!only.allows(*other), "{k:?} through a filter for {kind:?}");
            }
            let mut without = Filter::default();
            without.set(kind, false);
            assert!(!without.allows(sel));
            assert!(all.iter().filter(|(_, k)| *k != kind).all(|(o, _)| without.allows(*o)));
        }
        assert_eq!(Pick::ALL.len(), 7);
    }

    #[test]
    fn a_click_goes_past_what_the_filter_holds_back() {
        // A track on top of a part, as a click reaches them.
        let hits = [(1, 0.0, Sel::Track(7)), (2, 0.0, Sel::Via(3)), (3, 9.0, Sel::Fp(2))];
        assert_eq!(first_allowed(&hits, &Filter::default()), Some(Sel::Track(7)));
        let mut no_tracks = Filter::default();
        no_tracks.set(Pick::Tracks, false);
        assert_eq!(first_allowed(&hits, &no_tracks), Some(Sel::Via(3)), "past the track to the via");
        assert_eq!(first_allowed(&hits, &Filter::parts()), Some(Sel::Fp(2)), "past the track and the via to the part");
        assert_eq!(held_back_kind(&hits, &Filter::parts()), None, "something was picked, so nothing to explain");
        // With nothing let through, there's a reason to give: the thing that would have been picked.
        assert_eq!(first_allowed(&hits, &Filter::only(Pick::Pours)), None);
        assert_eq!(held_back_kind(&hits, &Filter::only(Pick::Pours)), Some(Pick::Tracks));
        assert_eq!(held_back_kind(&hits, &Filter::none()), Some(Pick::Tracks));
        // Nothing under the point at all: nothing to say.
        assert_eq!(first_allowed(&[], &Filter::default()), None);
        assert_eq!(held_back_kind(&[], &Filter::none()), None);
    }

    #[test]
    fn presets_and_summaries_say_what_they_let_through() {
        assert_eq!(Filter::none().summary(), "Nothing");
        assert_eq!(Filter::only(Pick::Vias).summary(), "Vias");
        assert_eq!(Filter::parts().summary(), "Parts, Names");
        assert_eq!(Filter::copper().summary(), "4 kinds");
        assert_eq!(Filter::copper().kinds(), vec![Pick::Tracks, Pick::Vias, Pick::Pours, Pick::Fills]);
        let mut f = Filter::default();
        f.set(Pick::Names, false);
        assert_eq!(f.summary(), "6 kinds");
        assert!(!f.is_all());
    }

    #[test]
    fn the_layout_is_remembered_and_a_missing_or_damaged_file_gives_the_usual_one() {
        let p = Prefs { show_left: false, show_right: true, left_tab: LeftTab::Select, right_tab: RightTab::Checks, ribbon: true, filter: Filter::only(Pick::Tracks) };
        assert_eq!(Prefs::from_json(&p.to_json()), p);
        assert_eq!(Prefs::from_json(""), Prefs::default());
        assert_eq!(Prefs::from_json("not json {"), Prefs::default());
        // A file from a version that kept less: what's there is taken, the rest is usual.
        let old = Prefs::from_json(r#"{"show_left": false, "something_new": 3}"#);
        assert!(!old.show_left && old.show_right && !old.ribbon && old.filter.is_all());
        // A tab it doesn't know is the usual layout, rather than a window that won't open.
        assert_eq!(Prefs::from_json(r#"{"left_tab": "Nonsense"}"#), Prefs::default());
        // A filter with a switch it hasn't got, and one that lacks some.
        assert!(Prefs::from_json(r#"{"filter": {"tracks": false}}"#).filter.vias);
    }

    #[test]
    fn the_layout_goes_to_a_file_and_comes_back() {
        let dir = std::env::temp_dir().join(format!("coppersmith-prefs-test-{}", std::process::id()));
        let path = dir.join("deeper").join("window.json");
        let _ = std::fs::remove_dir_all(&dir);
        // Nothing there yet.
        assert_eq!(Prefs::load_from(&path), Prefs::default());
        let p = Prefs { show_left: false, show_right: true, left_tab: LeftTab::Parts, right_tab: RightTab::Nets, ribbon: true, filter: Filter::only(Pick::Vias) };
        p.save_to(&path);
        assert!(path.exists(), "the folder was made");
        assert_eq!(Prefs::load_from(&path), p);
        // Written over, and a damaged file gives the usual layout.
        let q = Prefs { ribbon: false, ..p.clone() };
        q.save_to(&path);
        assert_eq!(Prefs::load_from(&path), q);
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Prefs::load_from(&path), Prefs::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_layout_can_be_given_in_words() {
        let mut p = Prefs::default();
        p.apply_spec("ribbon, no-left right:checks filter:tracks+vias").unwrap();
        assert!(p.ribbon && !p.show_left && p.show_right);
        assert_eq!(p.right_tab, RightTab::Checks);
        assert_eq!(p.filter.kinds(), vec![Pick::Tracks, Pick::Vias]);
        p.apply_spec("left:select;filter:all;no-ribbon").unwrap();
        assert!(p.show_left && p.left_tab == LeftTab::Select && p.filter.is_all() && !p.ribbon);
        p.apply_spec("filter:none").unwrap();
        assert!(p.filter.is_none());
        p.apply_spec("filter:copper").unwrap();
        assert_eq!(p.filter, Filter::copper());
        for bad in ["left:nowhere", "right:x", "filter:tracks+wires", "sideways"] {
            assert!(Prefs::default().apply_spec(bad).is_err(), "{bad}");
        }
    }
}
