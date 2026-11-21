//! The command line tool, and where it is put.
//!
//! The tool is a second shape of the same program.  It is put into a
//! directory that is already on the PATH, so that a person can type
//! `skycremote` and it is found.
//!
//! Two rules shape this:
//!
//! * **No password.**  A program that asks for an administrator
//!   password on its own is a program people stop running.  So the
//!   directories are tried in the order of how likely the person is to
//!   be allowed to write in them, and the first one that works is
//!   used.
//! * **Only when it is installed.**  The tool is only put on the PATH
//!   when the program is running from an installed place.  A build in
//!   a build folder is not something to put on the PATH of the
//!   machine.

use std::path::{Path, PathBuf};

/// What happened to the command line tool.
#[derive(Debug, Clone)]
pub struct CliInstall {
    /// Where the tool is now.
    pub path: Option<PathBuf>,
    /// Whether it is there.
    pub ok: bool,
    /// What to tell the person, in words.
    pub message: String,
}

impl CliInstall {
    /// A tool that could not be put anywhere.
    fn failed(message: impl Into<String>) -> Self {
        Self {
            path: None,
            ok: false,
            message: message.into(),
        }
    }
}

/// Put the command line tool on the PATH.
///
/// It is called when the program starts.  It does nothing when the
/// program is not running from an installed place, and it does nothing
/// when the tool is already there.
pub fn install() -> CliInstall {
    match imp::install() {
        Ok(report) => report,
        Err(error) => CliInstall::failed(error),
    }
}

/// Where the command line tool is put, in the order it is tried.
pub fn candidate_dirs() -> Vec<PathBuf> {
    imp::candidate_dirs()
}

/// The path of the tool that goes on the PATH.
///
/// It is called `skycremote` on every system: that is the name the
/// person types.
pub fn tool_path(dir: &Path) -> PathBuf {
    dir.join(imp::tool_file_name())
}

/// Copy a file and make it executable.
///
/// The copy goes through a temporary file that is renamed into place,
/// so that a tool that is being run right now is not overwritten
/// halfway.
pub fn copy_executable(from: &Path, to: &Path) -> std::io::Result<()> {
    let parent = to.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = to.with_extension("tmp-install");
    std::fs::copy(from, &temporary)?;
    make_executable(&temporary)?;
    std::fs::rename(&temporary, to)?;
    Ok(())
}

/// Make a file executable for everybody who may run it.
#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
}

/// On Windows the executable bit does not exist; a copied file is
/// already runnable.
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Whether a directory can be written in.
///
/// It is tested by writing a file in it and taking it away again: the
/// permission bits say who may write, but the file system and the
/// sandbox may say otherwise.
pub fn is_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(".skycremote-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The directories that are tried, and the rules of each system.
#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    /// The tool inside the application bundle.
    ///
    /// The program lives in `SkycRemote.app/Contents/MacOS/`, and the
    /// tool is put next to it when the bundle is made.
    pub fn bundled_tool() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let folder = exe.parent()?;
        // Only a program that is running from an installed bundle may
        // put the tool on the PATH of the machine.
        if !folder.to_string_lossy().contains(".app/Contents/MacOS") {
            return None;
        }
        let tool = folder.join("skycremote-cli");
        if tool.is_file() {
            Some(tool)
        } else {
            None
        }
    }

    pub fn candidate_dirs() -> Vec<PathBuf> {
        let mut dirs = vec![
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".local").join("bin"));
        }
        dirs
    }

    pub fn tool_file_name() -> &'static str {
        "skycremote"
    }

    pub fn install() -> Result<CliInstall, String> {
        let Some(source) = bundled_tool() else {
            return Ok(CliInstall {
                path: None,
                ok: false,
                message: "这个程序不是从安装好的位置运行的，没有改动 PATH".to_string(),
            });
        };

        let mut last_error = String::from("没有一个目录是可以写的");
        for dir in candidate_dirs() {
            // The folder has to be there before it can be written in,
            // but it is only made when it is missing on purpose.
            if !dir.is_dir() && std::fs::create_dir_all(&dir).is_err() {
                continue;
            }
            if !super::is_writable(&dir) {
                last_error = format!("{} 不能写入", dir.display());
                continue;
            }

            let target = super::tool_path(&dir);
            // Nothing to do when the same tool is already there.
            if let Ok(existing) = std::fs::metadata(&target) {
                if existing.len() == std::fs::metadata(&source).map(|m| m.len()).unwrap_or(0) {
                    return Ok(CliInstall {
                        path: Some(target),
                        ok: true,
                        message: format!("命令行工具已经在 {}", dir.display()),
                    });
                }
            }

            if super::copy_executable(&source, &target).is_err() {
                last_error = format!("{} 不能写入", dir.display());
                continue;
            }

            // A folder that is not on the PATH by itself has to be
            // put there, or the name would not be found.
            if dir.starts_with(
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default(),
            ) {
                add_to_shell_profile(&dir);
            }

            return Ok(CliInstall {
                path: Some(target),
                ok: true,
                message: format!("命令行工具已经装到 {}", dir.display()),
            });
        }

        Ok(CliInstall::failed(format!(
            "命令行工具没有装上：{last_error}。可以手动把 {} 复制到 PATH 里的任何一个目录。",
            source.display()
        )))
    }

    /// Put a folder on the PATH of the shells that are started later.
    ///
    /// The file is only touched when the line is not in it already, so
    /// that starting the program twice does not make the file longer.
    fn add_to_shell_profile(dir: &Path) {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let home = PathBuf::from(home);
        let line = format!("export PATH=\"{}:$PATH\"", dir.display());
        for name in [".zshrc", ".bash_profile", ".profile"] {
            let file = home.join(name);
            if let Ok(text) = std::fs::read_to_string(&file) {
                if text.lines().any(|row| row.trim() == line) {
                    continue;
                }
                let mut text = text;
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&line);
                text.push('\n');
                let _ = std::fs::write(&file, text);
            }
        }
    }
}

/// Windows: the tool goes into a folder of its own next to the
/// program, and that folder is put on the PATH of the person.
#[cfg(target_os = "windows")]
mod imp {
    use super::*;

    /// The tool that came with the program.
    ///
    /// The installer puts the program in one folder and the tool in a
    /// folder of its own next to it, so that the folder on the PATH
    /// holds nothing but the tool.
    pub fn bundled_tool() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let folder = exe.parent()?;
        for name in ["skycremote.exe", "skycremote-cli.exe"] {
            let tool = folder.join("cli").join(name);
            if tool.is_file() {
                return Some(tool);
            }
        }
        None
    }

    pub fn candidate_dirs() -> Vec<PathBuf> {
        let Some(exe) = std::env::current_exe().ok() else {
            return Vec::new();
        };
        let Some(folder) = exe.parent() else {
            return Vec::new();
        };
        vec![folder.join("cli")]
    }

    pub fn tool_file_name() -> &'static str {
        "skycremote.exe"
    }

    pub fn install() -> Result<CliInstall, String> {
        let Some(source) = bundled_tool() else {
            return Ok(CliInstall {
                path: None,
                ok: false,
                message: "这个程序不是从安装好的位置运行的，没有改动 PATH".to_string(),
            });
        };

        // There is one folder to put it in: the one next to the
        // program.  It is made here if it is not there yet.
        let Some(dir) = candidate_dirs().into_iter().next() else {
            return Err("找不到装命令行工具的目录".to_string());
        };
        if let Err(error) = std::fs::create_dir_all(&dir) {
            return Err(format!("{} 建不出来：{error}", dir.display()));
        }

        let target = super::tool_path(&dir);
        // The installer puts the tool where it belongs already, so
        // there is nothing to copy when it is already there.
        if source != target && super::copy_executable(&source, &target).is_err() {
            return Err(format!("{} 不能写入", dir.display()));
        }
        // The folder goes on the PATH of the person.  When the program
        // was installed for everybody, the installer already put it on
        // the machine's PATH, and this program has no administrator
        // rights to write there — so it only checks whether it is
        // already there.
        match crate::windows::add_to_path(crate::windows::Scope::User, &dir) {
            Ok(()) => Ok(CliInstall {
                path: Some(target),
                ok: true,
                message: format!("命令行工具已经装到 {}", dir.display()),
            }),
            Err(error) => {
                if crate::windows::is_on_path(crate::windows::Scope::AllUsers, &dir) {
                    Ok(CliInstall {
                        path: Some(target),
                        ok: true,
                        message: format!(
                            "命令行工具已经在 {}，机器上的每个账户都能用",
                            dir.display()
                        ),
                    })
                } else {
                    Err(format!("PATH 没有改成：{error}"))
                }
            }
        }
    }
}

/// The other systems: nothing is installed, and nothing is said about
/// it either.  The program is not packaged for them yet.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    use super::*;

    pub fn candidate_dirs() -> Vec<PathBuf> {
        Vec::new()
    }

    pub fn tool_file_name() -> &'static str {
        "skycremote"
    }

    pub fn install() -> Result<CliInstall, String> {
        Ok(CliInstall {
            path: None,
            ok: false,
            message: "这个系统还没有打包，命令行工具没有安装".to_string(),
        })
    }
}
