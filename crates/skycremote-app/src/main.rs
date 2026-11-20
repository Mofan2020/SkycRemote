//! SkycRemote: a peer-to-peer remote desktop.
//!
//! Two ends, one direct connection, no server in the middle.  The tracker
//! only says where the other end is; every byte of the session goes
//! straight from one machine to the other.

mod backend;
mod ui;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "skycremote", version, about = "A peer-to-peer remote desktop")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the tracker: it hands out addresses and never carries data.
    Tracker {
        /// The port to listen on.
        #[arg(long, default_value_t = 21115)]
        port: u16,
    },
    /// Run the end that is being controlled.
    Host {
        /// The id of this device.
        #[arg(long)]
        id: String,
        /// The name to show for this device.
        #[arg(long, default_value = "")]
        name: String,
        /// The tracker to talk to.
        #[arg(long, default_value = "http://127.0.0.1:21115")]
        tracker: String,
        /// The port to listen on for the direct connection.
        #[arg(long, default_value_t = 21116)]
        port: u16,
    },
    /// Run the end that does the controlling.
    Control {
        /// The id of the device to connect to.
        #[arg(long)]
        id: String,
        /// The tracker to talk to.
        #[arg(long, default_value = "http://127.0.0.1:21115")]
        tracker: String,
    },
    /// Open the window.
    Ui {
        /// The width of the window in points.
        #[arg(long, default_value_t = 1280.0)]
        width: f64,
        /// The height of the window in points.
        #[arg(long, default_value_t = 820.0)]
        height: f64,
        /// Check that the whole chain works, then close.
        #[arg(long)]
        probe: bool,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        // The window wants the main thread to itself, because that is
        // where the system draws it.  So it gets no runtime of its own
        // here; it makes one on a thread of its own.
        Command::Ui { width, height, probe } => {
            ui::run(ui::Options {
                width,
                height,
                probe,
                ..ui::Options::default()
            })?;
        }
        command => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async move {
                match command {
                    Command::Tracker { port } => {
                        skycremote_tracker::serve(port).await?;
                    }
                    Command::Host { id, name, tracker, port } => {
                        eprintln!("host {id} on {port}, tracker {tracker}");
                    }
                    Command::Control { id, tracker } => {
                        eprintln!("control {id}, tracker {tracker}");
                    }
                    Command::Ui { .. } => unreachable!("the window has its own path"),
                }
                Ok::<(), anyhow::Error>(())
            })?;
        }
    }
    Ok(())
}
