//! The design rules a board can be held to: Altium's, by the names its files use,
//! and which of them Coppersmith checks, applies, or only reads and shows.

use serde::{Deserialize, Serialize};

/// A design rule that can be switched on and off, and that a problem can break.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RuleId {
    // Electrical
    Clearance,
    ShortCircuit,
    UnRoutedNet,
    UnconnectedPin,
    // Routing
    Width,
    NetClassWidth,
    RoutingVias,
    RoutingLayers,
    RoutingCorners,
    RoutingTopology,
    RoutingPriority,
    FanoutControl,
    DiffPairsRouting,
    // Surface mount
    SmdToCorner,
    SmdToPlane,
    SmdNeckDown,
    // Solder and paste mask
    SolderMaskExpansion,
    PasteMaskExpansion,
    MinimumSolderMaskSliver,
    SilkToSolderMaskClearance,
    // Planes and pours
    PlaneConnect,
    PolygonConnect,
    PlaneClearance,
    UnpouredPolygon,
    // Manufacturing
    MinimumAnnularRing,
    AcuteAngle,
    HoleSize,
    HoleToHoleClearance,
    BoardOutlineClearance,
    SilkToSilkClearance,
    SilkToBoardRegionClearance,
    NetAntennae,
    LayerPairs,
    Keepout,
    // High speed
    MatchedLengths,
    Length,
    ParallelSegment,
    DaisyChainStubLength,
    ViasUnderSmd,
    MaxViaCount,
    // Placement
    ComponentClearance,
    ComponentOrientations,
    PermittedLayers,
    NetsToIgnore,
    Height,
    // Test points
    FabricationTestpoint,
    FabricationTestpointUsage,
    AssemblyTestpoint,
    AssemblyTestpointUsage,
    // Supplies
    SupplyNets,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Electrical,
    Routing,
    SurfaceMount,
    Mask,
    Planes,
    Manufacturing,
    HighSpeed,
    Placement,
    Testpoints,
    Supplies,
}

impl Group {
    pub const ALL: [Group; 10] = [
        Group::Electrical,
        Group::Routing,
        Group::HighSpeed,
        Group::Manufacturing,
        Group::Placement,
        Group::Planes,
        Group::Mask,
        Group::SurfaceMount,
        Group::Testpoints,
        Group::Supplies,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Group::Electrical => "Electrical",
            Group::Routing => "Routing",
            Group::SurfaceMount => "Surface mount",
            Group::Mask => "Solder and paste mask",
            Group::Planes => "Planes and pours",
            Group::Manufacturing => "Manufacturing",
            Group::HighSpeed => "High speed",
            Group::Placement => "Placement",
            Group::Testpoints => "Test points",
            Group::Supplies => "Supplies",
        }
    }
}

/// What Coppersmith does with a rule.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Support {
    /// Looked for in every check: breaking it makes a problem.
    Checked,
    /// Used when the board is made (pours, Gerbers), not a check of its own.
    Applied,
    /// Read from Altium files and shown, so nothing is lost; not checked yet.
    Listed,
}

impl Support {
    pub fn name(self) -> &'static str {
        match self {
            Support::Checked => "Checked",
            Support::Applied => "Applied",
            Support::Listed => "Shown only",
        }
    }
}

pub struct RuleInfo {
    pub id: RuleId,
    pub name: &'static str,
    pub group: Group,
    /// What Altium calls it in a file (`RULEKIND`); empty for rules that are Coppersmith's own.
    pub altium: &'static str,
    pub support: Support,
    /// What it asks, in a line.
    pub what: &'static str,
}

macro_rules! rules {
    ($(($id:ident, $name:expr, $group:ident, $altium:expr, $support:ident, $what:expr)),* $(,)?) => {
        pub const CATALOG: &[RuleInfo] = &[$(RuleInfo { id: RuleId::$id, name: $name, group: Group::$group, altium: $altium, support: Support::$support, what: $what }),*];
    };
}

rules! {
    (Clearance, "Clearance", Electrical, "Clearance", Checked, "The least gap between copper of different nets: tracks, pads, vias, pours."),
    (ShortCircuit, "Short circuit", Electrical, "ShortCircuit", Checked, "Copper of different nets must not touch."),
    (UnRoutedNet, "Un-routed net", Electrical, "UnRoutedNet", Checked, "Every connection the pads call for has to be routed."),
    (UnconnectedPin, "Un-connected pin", Electrical, "UnconnectedPin", Listed, "A pin with a net must be joined to something."),
    (Width, "Width", Routing, "Width", Checked, "How thin and how wide a track may be, and the width a new one starts at; per layer where the file says so."),
    (NetClassWidth, "Width of a net class", Routing, "Width (for a net class)", Checked, "Widths asked of the nets in a class, such as a 50 ohm line: the limits of each copper layer."),
    (RoutingVias, "Routing via style", Routing, "RoutingVias", Checked, "The pad and hole a via may have, and the one a new via starts as. Altium holds only through vias to it; Coppersmith can't yet tell blind and buried vias from through ones, so this check starts off."),
    (RoutingLayers, "Routing layers", Routing, "RoutingLayers", Listed, "Which copper layers may carry tracks."),
    (RoutingCorners, "Routing corners", Routing, "RoutingCorners", Listed, "The style of a bend (45°, 90°, round) and how long its setback is."),
    (RoutingTopology, "Routing topology", Routing, "RoutingTopology", Listed, "How the pins of a net are joined: shortest, daisy chain, star."),
    (RoutingPriority, "Routing priority", Routing, "RoutingPriority", Listed, "Which nets an autorouter does first."),
    (FanoutControl, "Fanout control", Routing, "FanoutControl", Listed, "How an autorouter fans pins of chips out with vias."),
    (DiffPairsRouting, "Differential pairs routing", Routing, "DiffPairsRouting", Listed, "Width and gap for the two tracks of a pair, and how long they may run uncoupled."),
    (SmdToCorner, "SMD to corner", SurfaceMount, "SMDToCorner", Listed, "How far a track turns from a surface pad."),
    (SmdToPlane, "SMD to plane", SurfaceMount, "SMDToPlane", Listed, "How far a surface pad is from the via that takes it to a plane."),
    (SmdNeckDown, "SMD neck-down", SurfaceMount, "SMDNeckDown", Listed, "How much a track may narrow to enter a surface pad."),
    (SolderMaskExpansion, "Solder mask expansion", Mask, "SolderMaskExpansion", Applied, "How far the mask opening is out from each pad; used when the Gerbers are written."),
    (PasteMaskExpansion, "Paste mask expansion", Mask, "PasteMaskExpansion", Listed, "How far the paste opening is in from each pad."),
    (MinimumSolderMaskSliver, "Minimum solder mask sliver", Mask, "MinimumSolderMaskSliver", Listed, "The narrowest strip of mask left between two openings."),
    (SilkToSolderMaskClearance, "Silkscreen to solder mask", Mask, "SilkToSolderMaskClearance", Listed, "The gap between silkscreen and exposed copper."),
    (PlaneConnect, "Plane connect style", Planes, "PlaneConnect", Applied, "How holes join a power plane: direct, or with relief spokes."),
    (PolygonConnect, "Polygon connect style", Planes, "PolygonConnect", Applied, "How pads and vias join a pour: direct, or with relief spokes."),
    (PlaneClearance, "Plane clearance", Planes, "PlaneClearance", Applied, "How far a power plane keeps from holes of other nets."),
    (UnpouredPolygon, "Un-poured polygon", Planes, "UnpouredPolygon", Checked, "A pour must have copper once it's poured."),
    (MinimumAnnularRing, "Minimum annular ring", Manufacturing, "MinimumAnnularRing", Checked, "The copper left round a drilled hole."),
    (AcuteAngle, "Acute angle", Manufacturing, "AcuteAngle", Checked, "Two tracks of a net may not meet at less than this angle (acid traps)."),
    (HoleSize, "Hole size", Manufacturing, "HoleSize", Checked, "The smallest and largest drill a hole may have."),
    (HoleToHoleClearance, "Hole to hole clearance", Manufacturing, "HoleToHoleClearance", Checked, "The least gap between the edges of two drilled holes."),
    (BoardOutlineClearance, "Board outline clearance", Manufacturing, "BoardOutlineClearance", Checked, "How far copper keeps from the board's edge, and stays inside it."),
    (SilkToSilkClearance, "Silkscreen to silkscreen", Manufacturing, "SilkToSilkClearance", Listed, "The gap between silkscreen of different parts."),
    (SilkToBoardRegionClearance, "Silkscreen to board region", Manufacturing, "SilkToBoardRegionClearance", Listed, "The gap between silkscreen and cut-outs or the edge."),
    (NetAntennae, "Net antennae", Manufacturing, "NetAntennae", Listed, "A stub of track joined at only one end."),
    (LayerPairs, "Layer pairs", Manufacturing, "LayerPairs", Listed, "Which layers a via may join."),
    (Keepout, "Keep-out", Manufacturing, "", Checked, "Copper kept out of a keepout, as each keepout is set to restrict."),
    (MatchedLengths, "Matched net lengths", HighSpeed, "MatchedLengths", Checked, "How much longer one net of a group may be than the others; for pairs and buses."),
    (Length, "Length", HighSpeed, "Length", Checked, "The shortest and longest a net's tracks may add up to."),
    (ParallelSegment, "Parallel segment", HighSpeed, "ParallelSegment", Listed, "How long two tracks may run side by side."),
    (DaisyChainStubLength, "Daisy chain stub length", HighSpeed, "DaisyChainStubLength", Listed, "The longest stub on a daisy-chained net."),
    (ViasUnderSmd, "Vias under SMD", HighSpeed, "ViasUnderSMD", Listed, "Whether a via may be under a surface pad."),
    (MaxViaCount, "Maximum via count", HighSpeed, "MaximumViaCount", Listed, "The most vias a net may have."),
    (ComponentClearance, "Component clearance", Placement, "ComponentClearance", Checked, "The least gap between the courtyards of two parts."),
    (ComponentOrientations, "Component orientations", Placement, "ComponentOrientations", Listed, "Which turns a part may have."),
    (PermittedLayers, "Permitted layers", Placement, "PermittedLayers", Listed, "Which sides of the board parts may go on."),
    (NetsToIgnore, "Nets to ignore", Placement, "NetsToIgnore", Listed, "Nets left out of placement."),
    (Height, "Height", Placement, "Height", Listed, "How tall a part may stand."),
    (FabricationTestpoint, "Fabrication testpoint style", Testpoints, "FabricationTestpoint", Listed, "Size, hole and grid for the test points a bare-board test uses."),
    (FabricationTestpointUsage, "Fabrication testpoint usage", Testpoints, "FabricationTestPointUsage", Listed, "How many test points each net must have for it."),
    (AssemblyTestpoint, "Assembly testpoint style", Testpoints, "AssemblyTestpoint", Listed, "Size, hole and grid for the test points a loaded-board test uses."),
    (AssemblyTestpointUsage, "Assembly testpoint usage", Testpoints, "AssemblyTestPointUsage", Listed, "How many test points each net must have for it."),
    (SupplyNets, "Supply nets", Supplies, "SupplyNets", Listed, "Which nets are supplies, and their voltage."),
}

impl RuleId {
    pub fn info(self) -> &'static RuleInfo {
        CATALOG.iter().find(|r| r.id == self).expect("every rule is in the catalog")
    }

    /// The rule an Altium `RULEKIND` is, if it's one Coppersmith knows.
    pub fn from_altium(kind: &str) -> Option<RuleId> {
        CATALOG.iter().find(|r| !r.altium.is_empty() && !r.altium.contains(' ') && r.altium.eq_ignore_ascii_case(kind)).map(|r| r.id)
    }
}

/// What the file says of one rule: where it applies, whether it's on, and its settings
/// (bar the per-layer ones and the bookkeeping).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SeenRule {
    pub rule: Option<RuleId>,
    /// Altium's own name for the kind, for ones Coppersmith doesn't know.
    pub kind: String,
    pub name: String,
    pub scope: String,
    pub enabled: bool,
    pub priority: i32,
    pub params: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_rule_is_listed_once() {
        let ids: HashSet<RuleId> = CATALOG.iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), CATALOG.len(), "a rule listed twice");
        for r in CATALOG {
            assert!(!r.what.is_empty() && !r.name.is_empty());
            assert_eq!(r.id.info().name, r.name);
        }
        for g in Group::ALL {
            assert!(CATALOG.iter().any(|r| r.group == g), "{} has no rules", g.name());
        }
    }

    #[test]
    fn altium_names_find_their_rules() {
        assert_eq!(RuleId::from_altium("HoleToHoleClearance"), Some(RuleId::HoleToHoleClearance));
        assert_eq!(RuleId::from_altium("holesize"), Some(RuleId::HoleSize));
        assert_eq!(RuleId::from_altium("AssemblyTestPointUsage"), Some(RuleId::AssemblyTestpointUsage));
        assert_eq!(RuleId::from_altium("FabricationTestPointUsage"), Some(RuleId::FabricationTestpointUsage));
        assert_eq!(RuleId::from_altium("SomethingNew"), None);
        // Every kind in a real Altium board (EZDisplay) is one Coppersmith knows.
        for kind in [
            "AssemblyTestPointUsage", "AssemblyTestpoint", "BoardOutlineClearance", "Clearance", "ComponentClearance", "DiffPairsRouting", "FabricationTestPointUsage", "FabricationTestpoint", "FanoutControl", "Height", "HoleSize",
            "HoleToHoleClearance", "LayerPairs", "Length", "MatchedLengths", "MinimumAnnularRing", "MinimumSolderMaskSliver", "NetAntennae", "PasteMaskExpansion", "PlaneClearance", "PlaneConnect", "PolygonConnect", "RoutingCorners",
            "RoutingLayers", "RoutingPriority", "RoutingTopology", "RoutingVias", "ShortCircuit", "SilkToBoardRegionClearance", "SilkToSilkClearance", "SilkToSolderMaskClearance", "SolderMaskExpansion", "SupplyNets", "UnRoutedNet",
            "UnpouredPolygon", "Width",
        ] {
            assert!(RuleId::from_altium(kind).is_some(), "{kind}");
        }
    }
}
