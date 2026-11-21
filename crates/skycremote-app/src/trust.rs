//! The devices that are remembered.
//!
//! Remembering a machine means two different things, and both are kept
//! here:
//!
//!   - On the machine that is being controlled: the list of controlling
//!     machines that may come in without the access code.  What is kept
//!     is what the value each of them was given is turned into, so the
//!     value itself is nowhere on this disk.
//!
//!   - On the machine that is doing the controlling: the value each
//!     remembered device gave, so that the access code is not asked for
//!     again.
//!
//! The file is beside the settings file.  It is written the same way:
//! written next to the one it replaces and moved over it, so that a
//! crash in the middle does not leave half a file behind.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What one machine is remembered by.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Remembered {
    /// The id of the other machine.
    pub id: String,
    /// The salt that goes with the value, in base64.
    pub salt: String,
    /// The value itself, in base64.
    pub verifier: String,
}

/// Everything that is remembered.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Memory {
    /// The id this machine is known by when it is the one controlling.
    ///
    /// It is made once and then kept, so that a device that trusts
    /// this machine still trusts it after the program is started again.
    #[serde(default)]
    pub controller_id: String,
    /// The machines that may come in without the access code, by the id
    /// they are known by.
    #[serde(default)]
    pub machines: BTreeMap<String, Remembered>,
    /// The devices that gave this machine a value, by the id of the
    /// device.
    #[serde(default)]
    pub keys: BTreeMap<String, Remembered>,
}

/// The remembered machines, on disk and in the program.
#[derive(Debug, Clone)]
pub struct Store {
    /// Where the file is.
    path: PathBuf,
    /// What is remembered.
    memory: Memory,
}

impl Store {
    /// The store that belongs to this machine.
    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    /// The store that is kept in a file.
    pub fn load_from(path: &Path) -> Self {
        let memory = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path: path.to_path_buf(),
            memory,
        }
    }

    /// Where the file is.
    fn path() -> PathBuf {
        #[cfg(windows)]
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        #[cfg(not(windows))]
        let base = std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config"))
            .unwrap_or_else(|| PathBuf::from("."));

        base.join("skycremote").join("remembered.json")
    }

    /// Write it down.
    pub fn save(&self) -> Result<()> {
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder).with_context(|| {
                format!(
                    "the folder for the remembered devices could not be made: {}",
                    folder.display()
                )
            })?;
        }
        let text = serde_json::to_string_pretty(&self.memory)
            .context("the remembered devices could not be written")?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, text).with_context(|| {
            format!(
                "the remembered devices could not be written: {}",
                temporary.display()
            )
        })?;
        std::fs::rename(&temporary, &self.path).with_context(|| {
            format!(
                "the remembered devices could not be written: {}",
                self.path.display()
            )
        })?;
        Ok(())
    }

    /// The id this machine is known by when it is the one controlling.
    ///
    /// It is made the first time it is asked for and then kept.
    pub fn controller_id(&mut self) -> String {
        if self.memory.controller_id.trim().is_empty() {
            let mut bytes = [0u8; 16];
            rand::fill(&mut bytes);
            self.memory.controller_id = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        }
        self.memory.controller_id.clone()
    }

    /// The machines that may come in without the access code.
    pub fn machines(&self) -> Vec<Remembered> {
        self.memory.machines.values().cloned().collect()
    }

    /// The devices that gave this machine a value.
    pub fn keys(&self) -> Vec<Remembered> {
        self.memory.keys.values().cloned().collect()
    }

    /// The value one device gave, if it gave one.
    pub fn key_for(&self, device_id: &str) -> Option<Remembered> {
        self.memory.keys.get(device_id).cloned()
    }

    /// Remember a machine that may come in without the access code.
    pub fn remember_machine(&mut self, remembered: Remembered) {
        self.memory
            .machines
            .insert(remembered.id.clone(), remembered);
    }

    /// Forget a machine.
    pub fn forget_machine(&mut self, id: &str) {
        self.memory.machines.remove(id);
    }

    /// Remember the value a device gave.
    pub fn remember_key(&mut self, remembered: Remembered) {
        self.memory.keys.insert(remembered.id.clone(), remembered);
    }

    /// Forget the value a device gave.
    pub fn forget_key(&mut self, id: &str) {
        self.memory.keys.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine that is remembered is remembered the same way the next
    /// time the file is read.
    #[test]
    fn a_remembered_machine_survives_a_restart() {
        let folder = std::env::temp_dir().join(format!(
            "skycremote-remembered-{}-{}-{}",
            std::process::id(),
            "test",
            "keep"
        ));
        let path = folder.join("remembered.json");

        let mut store = Store::load_from(&path);
        store.remember_machine(Remembered {
            id: "the-controller".to_string(),
            salt: "c2FsdA".to_string(),
            verifier: "dmVyaWZpZXI=".to_string(),
        });
        store.save().expect("the file could not be written");

        let read = Store::load_from(&path);
        assert_eq!(read.machines().len(), 1);
        assert_eq!(read.machines()[0].id, "the-controller");

        let _ = std::fs::remove_dir_all(&folder);
    }

    /// A machine that is forgotten is gone the next time the file is
    /// read.
    #[test]
    fn a_forgotten_machine_is_gone() {
        let folder = std::env::temp_dir().join(format!(
            "skycremote-remembered-{}-{}-{}",
            std::process::id(),
            "test",
            "forget"
        ));
        let path = folder.join("remembered.json");

        let mut store = Store::load_from(&path);
        store.remember_machine(Remembered {
            id: "the-controller".to_string(),
            salt: "c2FsdA".to_string(),
            verifier: "dmVyaWZpZXI=".to_string(),
        });
        store.forget_machine("the-controller");
        store.save().expect("the file could not be written");

        let read = Store::load_from(&path);
        assert!(read.machines().is_empty());

        let _ = std::fs::remove_dir_all(&folder);
    }

    /// The id of this machine does not change from one moment to the
    /// next.
    #[test]
    fn the_id_of_this_machine_stays_the_same() {
        let folder = std::env::temp_dir().join(format!(
            "skycremote-remembered-{}-{}-{}",
            std::process::id(),
            "test",
            "id"
        ));
        let path = folder.join("remembered.json");

        let mut store = Store::load_from(&path);
        let first = store.controller_id();
        assert!(!first.is_empty());
        store.save().expect("the file could not be written");

        let mut read = Store::load_from(&path);
        assert_eq!(read.controller_id(), first);

        let _ = std::fs::remove_dir_all(&folder);
    }
}
