//! The window: the editor's state and edits (app), the board view (view),
//! the panels and bars round it (panels), how it's laid out and what can be
//! picked (dock), and the Copperview look (theme, icons). `play` acts out scripted input for picture checks.

pub mod app;
pub mod chrome;
pub mod dock;
pub mod icons;
pub mod libedit;
pub mod libview;
pub mod panels;
pub mod play;
pub mod routing;
pub mod theme;
pub mod view;
