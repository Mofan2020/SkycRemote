//! The messages that travel between the two ends.
//!
//! Everything a session sends is one of these.  The video and the sound
//! are not messages: they are separate streams that the transport carries
//! beside them, because a dropped frame must not cost the next one its
//! place.

use serde::{Deserialize, Serialize};

/// What a session can say to the other end.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    /// The first message.  It carries who is talking and what the two
    /// ends agree on.
    Hello(Hello),
    /// The answer to a hello.  It carries the session key material.
    HelloAck(HelloAck),
    /// A key event, a mouse event, a scroll, a paste.
    Input(InputEvent),
    /// The screen the control end is looking at.
    SwitchScreen {
        /// The index of the screen, in the order the capture end reports them.
        index: u32,
    },
    /// The control end asks for the list of screens.
    ListScreens,
    /// The answer: the screens the capture end has.
    Screens {
        /// One entry per screen.
        screens: Vec<ScreenInfo>,
    },
    /// The file transfer starts.
    FileStart {
        /// The name on the sending side.
        name: String,
        /// The size in bytes.
        size: u64,
        /// Whether this is a whole file or a folder that is being walked.
        is_dir: bool,
    },
    /// One chunk of a file.  The offset is where it belongs in the file.
    FileChunk {
        /// Where the chunk starts in the file.
        offset: u64,
        /// The bytes.
        data: Vec<u8>,
    },
    /// The file is complete.
    FileEnd,
    /// A line of the remote terminal.
    Terminal {
        /// The bytes of one line.
        data: Vec<u8>,
    },
    /// Something went wrong and the session cannot go on.
    Error {
        /// A machine-readable code.
        code: String,
        /// A sentence for the person.
        message: String,
    },
    /// Ping, so the two ends know the other is still there.
    Ping {
        /// The milliseconds since the epoch on the sending side.
        at: u64,
    },
    /// The answer to a ping.
    Pong {
        /// The milliseconds since the epoch on the sending side.
        at: u64,
    },
}

/// The first message of a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    /// The protocol version both ends must agree on.
    pub version: u16,
    /// The id of the device that is being controlled.
    pub device_id: String,
    /// The name to show for that device.
    pub device_name: String,
    /// The public key of the sending side, in raw bytes.
    pub public_key: Vec<u8>,
    /// The port the sending side listens on for the direct connection.
    pub port: u16,
}

/// The answer to a hello.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloAck {
    /// The protocol version the other end speaks.
    pub version: u16,
    /// The public key of the answering side, in raw bytes.
    pub public_key: Vec<u8>,
    /// Whether the account may control this device.  The device decides:
    /// an unknown account gets the screen and no keyboard.
    pub allowed: bool,
    /// Why the connection was refused, when it was.
    pub reason: String,
}

/// One screen of the machine that is being controlled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenInfo {
    /// The index of this screen.
    pub index: u32,
    /// The name the system gives it.
    pub name: String,
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The scale factor, so a Retina screen is not sent twice as big.
    pub scale_factor: f64,
    /// Whether this screen is the one that holds the main desktop.
    pub primary: bool,
}

/// What the control end does with the keyboard and the mouse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    /// A key went down.
    KeyDown {
        /// The platform-independent key code.
        key: u32,
    },
    /// A key came up.
    KeyUp {
        /// The platform-independent key code.
        key: u32,
    },
    /// Text was typed.  It is sent as text, not as keys, so that the layout
    /// of the control end does not matter.
    Text {
        /// The characters.
        text: String,
    },
    /// The mouse moved.
    MouseMove {
        /// The position in the captured screen's pixels.
        x: u32,
        /// The position in the captured screen's pixels.
        y: u32,
    },
    /// A mouse button went down.
    MouseDown {
        /// 0 is the left button, 1 the middle, 2 the right.
        button: u8,
        /// The position in the captured screen's pixels.
        x: u32,
        /// The position in the captured screen's pixels.
        y: u32,
    },
    /// A mouse button came up.
    MouseUp {
        /// 0 is the left button, 1 the middle, 2 the right.
        button: u8,
        /// The position in the captured screen's pixels.
        x: u32,
        /// The position in the captured screen's pixels.
        y: u32,
    },
    /// The wheel turned.
    Scroll {
        /// How far it turned horizontally.
        dx: f32,
        /// How far it turned vertically.
        dy: f32,
        /// The position in the captured screen's pixels.
        x: u32,
        /// The position in the captured screen's pixels.
        y: u32,
    },
    /// The clipboard of the control end changed.
    Clipboard {
        /// The bytes of the new clipboard.
        data: Vec<u8>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_survives_a_round_trip() {
        let message = Message::Hello(Hello {
            version: crate::PROTOCOL_VERSION,
            device_id: "abc".into(),
            device_name: "测试机器".into(),
            public_key: vec![1, 2, 3],
            port: 21116,
        });
        let text = serde_json::to_string(&message).expect("the message must be serialisable");
        let back: Message = serde_json::from_str(&text).expect("the message must come back");
        match back {
            Message::Hello(hello) => {
                assert_eq!(hello.device_name, "测试机器");
                assert_eq!(hello.public_key, vec![1, 2, 3]);
            }
            other => panic!("the wrong message came back: {other:?}"),
        }
    }

    #[test]
    fn the_kind_of_a_message_is_in_the_json() {
        // The webview reads the messages, so the shape has to be readable
        // from JavaScript as well as from Rust.
        let text = serde_json::to_string(&Message::ListScreens).expect("serialisable");
        assert!(text.contains("\"type\""), "{text}");
        assert!(text.contains("ListScreens"), "{text}");
    }
}
