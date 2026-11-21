//! The files that are installed are folded into the installer.
//!
//! The installer is one file that a person double-clicks, so the
//! program and the command line tool have to be inside it.  They are
//! put there by the build: the folder that holds them is named by the
//! environment variable `SKYCREMOTE_PAYLOAD`, and it is copied into
//! the build folder so that the compiler can find it.
//!
//! When the variable is not set, the installer is still made, but it
//! has nothing to install.  That is what happens when the crate is
//! built on its own, and it is better than a build that fails.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=SKYCREMOTE_PAYLOAD");
    println!("cargo:rerun-if-changed=build.rs");

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let payload = out.join("payload");
    let _ = std::fs::remove_dir_all(&payload);
    std::fs::create_dir_all(&payload).expect("the build folder could not be made");

    match std::env::var("SKYCREMOTE_PAYLOAD") {
        Ok(source) => {
            let source = PathBuf::from(source);
            if source.is_dir() {
                copy_tree(&source, &payload);
            } else {
                std::fs::write(
                    payload.join("README.txt"),
                    "the folder that holds the program was not there\n",
                )
                .expect("the note could not be written");
            }
        }
        Err(_) => {
            std::fs::write(
                payload.join("README.txt"),
                "this installer was built without the program in it\n",
            )
            .expect("the note could not be written");
        }
    }

    println!(
        "cargo:rustc-env=SKYCREMOTE_PAYLOAD_DIR={}",
        payload.display()
    );
}

/// Copy a folder, with everything in it.
fn copy_tree(from: &Path, to: &Path) {
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let target = to.join(&name);
        if path.is_dir() {
            std::fs::create_dir_all(&target).expect("the folder could not be made");
            copy_tree(&path, &target);
        } else if let Ok(bytes) = std::fs::read(&path) {
            std::fs::write(&target, bytes).expect("the file could not be copied");
        }
    }
}
