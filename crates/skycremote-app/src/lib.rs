//! The parts of SkycRemote that the window and the command line
//! tool both use.
//!
//! There are two shapes of this program: the window, which is what a
//! person opens, and the command line tool, which is what goes onto
//! the PATH.  They are the same program, so the parts they share live
//! here.

pub mod autostart;
pub mod backend;
pub mod trust;
pub mod ui;

#[cfg(not(target_os = "linux"))]
pub mod tray;
