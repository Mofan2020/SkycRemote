//! SkycRemote: the command line tool.
//!
//! This is what goes onto the PATH when the program is installed.  It
//! is the same program as the window, seen from a terminal:
//!
//! * `skycremote host` shares the screen of a machine that has no
//!   screen of its own — a server, a box under the desk.
//! * `skycremote control` opens the window and connects to a device.
//! * `skycremote tracker` runs the address book that the two ends
//!   use to find each other.
//!
//! The window itself is `skycremote`, the program that is installed
//! as an application.

use clap::{Parser, Subcommand};

use skycremote_app::ui;

#[derive(Parser)]
#[command(
    name = "skycremote",
    version,
    about = "SkycRemote: a peer-to-peer remote desktop"
)]
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
    /// Share the screen of this machine, for a machine without a screen.
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
        /// How many pictures a second to send.
        #[arg(long, default_value_t = 30)]
        fps: u32,
        /// How many megabits a second the pictures may use.
        #[arg(long, default_value_t = 8)]
        bitrate: u32,
    },
    /// Open the window and connect to a device.
    Control {
        /// The id of the device to connect to.
        #[arg(long)]
        id: String,
        /// The tracker to talk to.
        #[arg(long, default_value = "http://127.0.0.1:21115")]
        tracker: String,
    },
    /// Open the window without connecting to anything.
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
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        // The window wants the main thread to itself, because that is
        // where the system draws it.  So it gets no runtime of its own
        // here; it makes one on a thread of its own.
        Command::Ui {
            width,
            height,
            probe,
        } => {
            ui::run(ui::Options {
                width,
                height,
                probe,
                ..ui::Options::default()
            })?;
        }
        Command::Control { id, tracker } => {
            // The window is the one that controls: it opens and goes
            // straight to the device that was asked for.
            ui::run(ui::Options {
                connect_to: Some(id),
                tracker: Some(tracker),
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
                    Command::Host {
                        id,
                        name,
                        tracker,
                        port,
                        fps,
                        bitrate,
                    } => {
                        let display_name = if name.is_empty() {
                            id.clone()
                        } else {
                            name.clone()
                        };
                        eprintln!("skycremote: sharing {id} as {display_name} on port {port}");
                        let service = skycremote_core::host::HostService::new(
                            skycremote_core::host::HostOptions {
                                tracker,
                                device_id: id,
                                device_name: display_name,
                                port,
                                fps,
                                bitrate_bps: bitrate.saturating_mul(1_000_000),
                            },
                        );
                        // The events are only words on the terminal
                        // here: the window is not there to show them.
                        let (events, mut receiver) =
                            tokio::sync::mpsc::unbounded_channel::<skycremote_core::host::HostEvent>();
                        tokio::spawn(async move {
                            while let Some(event) = receiver.recv().await {
                                match event {
                                    skycremote_core::host::HostEvent::Waiting => {
                                        eprintln!("skycremote: waiting for the other end");
                                    }
                                    skycremote_core::host::HostEvent::PeerJoined { peer, screen } => {
                                        eprintln!(
                                            "skycremote: the other end is here ({peer}), sharing {screen}"
                                        );
                                    }
                                    skycremote_core::host::HostEvent::PeerLeft => {
                                        eprintln!("skycremote: the other end left");
                                    }
                                    skycremote_core::host::HostEvent::Warning { message } => {
                                        eprintln!("skycremote: {message}");
                                    }
                                }
                            }
                        });
                        service.run(events).await?;
                    }
                    Command::Control { .. } | Command::Ui { .. } => {
                        unreachable!("the window has its own path")
                    }
                }
                Ok::<(), anyhow::Error>(())
            })?;
        }
    }
    Ok(())
}
