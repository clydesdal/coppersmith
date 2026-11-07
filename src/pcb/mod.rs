//! The board itself and everything worked out from it: geometry, the board
//! model and its file, footprints, connectivity, the rule check and the
//! Gerber writer. Nothing here knows about windows or operating systems, so
//! it builds and tests the same everywhere.

pub mod altium;
pub mod copper;
pub mod drc;
pub mod font;
pub mod geom;
pub mod gerber;
pub mod library;
pub mod model;
pub mod pour;
pub mod project;
pub mod router;
pub mod schematic;
pub mod sample;
pub mod slide;
pub mod solid;
