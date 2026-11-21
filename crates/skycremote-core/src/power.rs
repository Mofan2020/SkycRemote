//! Making the machine do something to itself.
//!
//! The command comes from the other end of a session, so it is only
//! carried out when the two ends already know each other.  The caller
//! is expected to have asked the person first; this module only does
//! what it is told.

use std::process::Command;

/// What the other end can ask this machine to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    /// Turn the machine off.
    Shutdown,
    /// Turn the machine off and on again.
    Restart,
    /// Put the machine to sleep.
    Sleep,
    /// Log the person out, leaving the machine on.
    LogOff,
}

impl PowerAction {
    /// Read an action out of the word that came over the wire.
    ///
    /// The words are the ones the interface sends, plus a couple of
    /// aliases so that a hand written message still lands.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "shutdown" | "poweroff" | "power_off" | "关机" => Some(Self::Shutdown),
            "restart" | "reboot" => Some(Self::Restart),
            "sleep" | "suspend" => Some(Self::Sleep),
            "logoff" | "log_out" | "signout" | "注销" => Some(Self::LogOff),
            _ => None,
        }
    }

    /// The word this action is sent as.
    pub fn as_wire_word(&self) -> &'static str {
        match self {
            Self::Shutdown => "shutdown",
            Self::Restart => "restart",
            Self::Sleep => "sleep",
            Self::LogOff => "logoff",
        }
    }
}

/// Do what the action says.
///
/// It says what went wrong, in words, when the machine would not do it.
/// The machine may still be shutting down when this returns: the
/// command is handed over and the session is over.
pub fn perform(action: PowerAction) -> Result<(), String> {
    platform::perform(action)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    fn osascript(statement: &str) -> Result<(), String> {
        let output = Command::new("osascript")
            .arg("-e")
            .arg(statement)
            .output()
            .map_err(|error| format!("the machine could not be asked to do that: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if text.is_empty() {
            "the machine said no".to_string()
        } else {
            text
        })
    }

    pub fn perform(action: PowerAction) -> Result<(), String> {
        match action {
            PowerAction::Shutdown => osascript("tell application \"System Events\" to shut down"),
            PowerAction::Restart => osascript("tell application \"System Events\" to restart"),
            PowerAction::Sleep => osascript("tell application \"System Events\" to sleep"),
            PowerAction::LogOff => osascript("tell application \"System Events\" to log out"),
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;

    pub fn perform(action: PowerAction) -> Result<(), String> {
        let (program, args): (&str, Vec<&str>) = match action {
            PowerAction::Shutdown => ("shutdown", vec!["/s", "/t", "0"]),
            PowerAction::Restart => ("shutdown", vec!["/r", "/t", "0"]),
            PowerAction::Sleep => (
                "rundll32.exe",
                vec!["powrprof.dll,SetSuspendState", "0,1,0"],
            ),
            PowerAction::LogOff => ("shutdown", vec!["/l"]),
        };
        let output = Command::new(program)
            .args(&args)
            .output()
            .map_err(|error| format!("the machine could not be asked to do that: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if text.is_empty() {
            "the machine said no".to_string()
        } else {
            text
        })
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    pub fn perform(action: PowerAction) -> Result<(), String> {
        let (program, args): (&str, Vec<String>) = match action {
            PowerAction::Shutdown => ("systemctl", vec!["poweroff".to_string()]),
            PowerAction::Restart => ("systemctl", vec!["reboot".to_string()]),
            PowerAction::Sleep => ("systemctl", vec!["suspend".to_string()]),
            PowerAction::LogOff => (
                "loginctl",
                vec![
                    "terminate-user".to_string(),
                    std::env::var("USER").unwrap_or_else(|_| "root".to_string()),
                ],
            ),
        };
        let output = Command::new(program)
            .args(&args)
            .output()
            .map_err(|error| format!("the machine could not be asked to do that: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if text.is_empty() {
            "the machine said no".to_string()
        } else {
            text
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod platform {
    use super::*;

    pub fn perform(_action: PowerAction) -> Result<(), String> {
        Err("this system has no way to do that".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_read_back() {
        assert_eq!(PowerAction::parse("shutdown"), Some(PowerAction::Shutdown));
        assert_eq!(PowerAction::parse("Reboot"), Some(PowerAction::Restart));
        assert_eq!(PowerAction::parse(" sleep "), Some(PowerAction::Sleep));
        assert_eq!(PowerAction::parse("logoff"), Some(PowerAction::LogOff));
        assert_eq!(PowerAction::parse("nonsense"), None);
    }

    #[test]
    fn every_action_has_a_word_of_its_own() {
        let actions = [
            PowerAction::Shutdown,
            PowerAction::Restart,
            PowerAction::Sleep,
            PowerAction::LogOff,
        ];
        for action in actions {
            assert_eq!(PowerAction::parse(action.as_wire_word()), Some(action));
        }
    }
}
