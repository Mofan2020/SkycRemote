//! Putting SkycRemote on the machine.
//!
//! Two things get installed: the program itself, and the command line
//! tool that goes with it.  The program is the window; the tool is the
//! same program seen from a terminal, and it is what a person uses on
//! a machine with no screen of its own.
//!
//! The tool is installed into a directory that is already on the PATH,
//! so that `skycremote` just works in a new terminal window.  Nothing
//! here asks for a password: a program that pops up a password box on
//! its own is a program people stop running.

pub mod cli;

#[cfg(target_os = "windows")]
pub mod windows;

/// The name of the program, the way it is called everywhere.
pub const APP_NAME: &str = "SkycRemote";

/// The name of the command line tool.
pub const CLI_NAME: &str = "skycremote";

/// The version of the program, taken from the crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
