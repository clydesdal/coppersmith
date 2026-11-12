//! The window: the editor's state and edits (app), the board view (view),
//! the panels and bars round it (panels), how it's laid out and what can be
//! picked (dock), and the Copperview look (theme, icons). `play` acts out scripted input for picture checks.

pub mod app;
pub mod chrome;
pub mod dock;
pub mod gpu3d;
pub mod icons;
pub mod libedit;
pub mod libview;
pub mod schview;
pub mod panels;
pub mod recent;
pub mod play;
pub mod routing;
pub mod rulesview;
pub mod theme;
pub mod view;
pub mod view3d;
