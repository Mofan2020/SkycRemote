//! SkycRemote: the program you open.
//!
//! Two ends, one direct connection, no server in the middle.  The
//! tracker only says where the other end is; every byte of the session
//! goes straight from one machine to the other.
//!
//! This is the window.  The command line tool is the same program seen
//! from a terminal, and it lives in `cli.rs`.
//!
//! Starting the program opens the window.  There is nothing to type
//! first, and there is no command line to learn: the window is where
//! the work is done.  The command line tool is for the machines that
//! have no screen of their own.

use skycremote_app::ui;

fn main() -> anyhow::Result<()> {
    // The log goes to the error stream, which on macOS is the system
    // log and on Windows is nowhere unless something is watching.
    // A person who reports "it does not work" needs something to show.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    ui::run(ui::Options::default())
}
