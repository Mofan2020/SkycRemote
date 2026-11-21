//! The clipboard of this machine.
//!
//! Every platform has its own way of getting at the clipboard, and none
//! of them asks for a permission of its own:
//!
//! * macOS has pbcopy and pbpaste;
//! * Windows has the Get-Clipboard and Set-Clipboard commands of
//!   PowerShell;
//! * Linux has wl-paste / wl-copy on Wayland, xclip and xsel on X11.
//!
//! Small programs are used instead of the system libraries, because the
//! libraries differ more than the programs do.  A program that is not
//! there is simply not there: the machine says so and nothing breaks.

use std::io::Write;
use std::process::{Command, Stdio};

/// What can go wrong with the clipboard.
#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    /// The text could not be read.
    #[error("the clipboard could not be read: {0}")]
    Read(String),
    /// The text could not be written.
    #[error("the clipboard could not be written: {0}")]
    Write(String),
}

/// Read the text that is on the clipboard.
///
/// An empty clipboard is not an error: it is empty.
pub fn read_text() -> Result<String, ClipboardError> {
    read_platform()
}

/// Put text on the clipboard, replacing what was there.
pub fn write_text(text: &str) -> Result<(), ClipboardError> {
    write_platform(text)
}

/// Run a program and give back what it printed.
fn capture(program: &str, args: &[&str]) -> Option<(bool, String)> {
    let output = Command::new(program).args(args).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    Some((output.status.success(), text))
}

/// Give a program something on its input and wait for it to finish.
fn feed(program: &str, args: &[&str], input: &str) -> Option<bool> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    {
        let stdin = child.stdin.as_mut()?;
        if stdin.write_all(input.as_bytes()).is_err() {
            return Some(false);
        }
    }
    let status = child.wait().ok()?;
    Some(status.success())
}

#[cfg(target_os = "macos")]
fn read_platform() -> Result<String, ClipboardError> {
    match capture("pbpaste", &[]) {
        Some((_, text)) => Ok(text),
        None => Err(ClipboardError::Read(
            "pbpaste is not on this machine".to_string(),
        )),
    }
}

#[cfg(target_os = "macos")]
fn write_platform(text: &str) -> Result<(), ClipboardError> {
    match feed("pbcopy", &[], text) {
        Some(true) => Ok(()),
        Some(false) => Err(ClipboardError::Write("pbcopy refused the text".to_string())),
        None => Err(ClipboardError::Write(
            "pbcopy is not on this machine".to_string(),
        )),
    }
}

#[cfg(target_os = "windows")]
fn read_platform() -> Result<String, ClipboardError> {
    let script = "Get-Clipboard -Raw";
    match capture(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    ) {
        Some((true, text)) => Ok(text),
        Some((false, _)) => Err(ClipboardError::Read(
            "the clipboard could not be read".to_string(),
        )),
        None => Err(ClipboardError::Read(
            "powershell is not on this machine".to_string(),
        )),
    }
}

#[cfg(target_os = "windows")]
fn write_platform(text: &str) -> Result<(), ClipboardError> {
    let script = "Set-Clipboard -Value ([Console]::In.ReadToEnd())";
    match feed(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
        text,
    ) {
        Some(true) => Ok(()),
        Some(false) => Err(ClipboardError::Write(
            "the clipboard could not be written".to_string(),
        )),
        None => Err(ClipboardError::Write(
            "powershell is not on this machine".to_string(),
        )),
    }
}

#[cfg(target_os = "linux")]
fn read_platform() -> Result<String, ClipboardError> {
    // Wayland first, then X11.  A machine with both running uses the
    // one the session is on, which is the first one that answers.
    let candidates: Vec<(&str, Vec<&str>)> = vec![
        ("wl-paste", vec!["--no-newline"]),
        ("xclip", vec!["-selection", "clipboard", "-o"]),
        ("xsel", vec!["--clipboard", "--output"]),
    ];
    for (program, args) in candidates {
        let Some((ok, text)) = capture(program, &args) else {
            continue;
        };
        if ok {
            return Ok(text);
        }
    }
    Err(ClipboardError::Read(
        "no clipboard program was found (wl-paste, xclip, xsel)".to_string(),
    ))
}

#[cfg(target_os = "linux")]
fn write_platform(text: &str) -> Result<(), ClipboardError> {
    let candidates: Vec<(&str, Vec<&str>)> = vec![
        ("wl-copy", vec![]),
        ("xclip", vec!["-selection", "clipboard"]),
        ("xsel", vec!["--clipboard", "--input"]),
    ];
    let mut last = String::new();
    for (program, args) in candidates {
        match feed(program, &args, text) {
            Some(true) => return Ok(()),
            Some(false) => last = format!("{program} refused the text"),
            None => last = format!("{program} is not on this machine"),
        }
    }
    Err(ClipboardError::Write(last))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn read_platform() -> Result<String, ClipboardError> {
    Err(ClipboardError::Read(
        "this system has no clipboard the program knows".to_string(),
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn write_platform(_text: &str) -> Result<(), ClipboardError> {
    Err(ClipboardError::Write(
        "this system has no clipboard the program knows".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_the_clipboard_does_not_panic() {
        // The clipboard of the machine running the test is not written
        // to on purpose: that would change what the person at the
        // machine has copied.  What is checked here is that the call
        // answers with a result and not with a panic.
        let _ = read_text();
    }
}
