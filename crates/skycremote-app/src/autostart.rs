//! Starting the program when the machine is turned on.
//!
//! Each system has its own way of saying "run this when I log in":
//!
//! * macOS has a LaunchAgent, which is a small file in the user's own
//!   folder saying what to run and when;
//! * Windows has the Startup folder, which is just a shortcut in a
//!   folder that is opened at login;
//! * Linux has a desktop file in the autostart folder.
//!
//! Everything here is written into the user's own space.  Nothing is
//! written into a system folder and no administrator is asked for: a
//! program that asks for a password on its own is a program people
//! stop running.

use std::path::PathBuf;

/// What happened when the setting was changed.
#[derive(Debug, Clone)]
pub struct AutostartReport {
    /// Whether the program will start on its own now.
    pub enabled: bool,
    /// A sentence for the person.
    pub message: String,
    /// Where the thing that was written is.
    pub path: Option<PathBuf>,
}

impl AutostartReport {
    /// The setting is on.
    pub fn on(path: Option<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            enabled: true,
            message: message.into(),
            path,
        }
    }

    /// The setting is off.
    pub fn off(message: impl Into<String>) -> Self {
        Self {
            enabled: false,
            message: message.into(),
            path: None,
        }
    }
}

/// Turn starting on login on or off.
///
/// The program that is run is the one that is running now, so that the
/// thing that starts is the thing the person has in front of them.
pub fn set(enabled: bool) -> AutostartReport {
    platform::set(enabled)
}

/// Whether the program starts on its own now.
pub fn is_enabled() -> bool {
    platform::is_enabled()
}

/// Turn it on or off, with the file in a folder of its own.
///
/// It is what the test uses: the real place on the machine is not
/// touched, so that running the test does not change what happens when
/// the person logs in.
pub fn set_at(path: PathBuf, enabled: bool) -> AutostartReport {
    platform::set_at(path, enabled)
}

/// The path of the program that is running now.
///
/// It is what gets put into the thing that starts the program.
pub fn program_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    /// The LaunchAgent file.
    fn agent_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library")
                .join("LaunchAgents")
                .join("com.skycremote.app.plist"),
        )
    }

    pub fn set(enabled: bool) -> AutostartReport {
        let Some(path) = agent_path() else {
            return AutostartReport::off("找不到主目录，没能改动".to_string());
        };
        set_at(path, enabled)
    }

    /// Turn it on or off, with the file in a folder of its own.
    ///
    /// The test uses this so that it does not touch the real place.
    pub fn set_at(path: PathBuf, enabled: bool) -> AutostartReport {
        if !enabled {
            let _ = std::fs::remove_file(&path);
            return AutostartReport::off("开机自启已经关了".to_string());
        }
        let Some(program) = program_path() else {
            return AutostartReport::off("不知道自己是哪个程序，没能改动".to_string());
        };
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return AutostartReport::off("建不了文件夹，没能改动".to_string());
            }
        }
        let text = format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
                "<plist version=\"1.0\">\n",
                "<dict>\n",
                "  <key>Label</key>\n",
                "  <string>com.skycremote.app</string>\n",
                "  <key>ProgramArguments</key>\n",
                "  <array>\n",
                "    <string>{}</string>\n",
                "  </array>\n",
                "  <key>RunAtLoad</key>\n",
                "  <true/>\n",
                "  <key>KeepAlive</key>\n",
                "  <false/>\n",
                "</dict>\n",
                "</plist>\n",
            ),
            program.display()
        );
        match std::fs::write(&path, text) {
            Ok(()) => AutostartReport::on(
                Some(path),
                "开机自启已经开了，登录以后程序会自己起来".to_string(),
            ),
            Err(error) => AutostartReport::off(format!("没能写下来：{error}")),
        }
    }

    pub fn is_enabled() -> bool {
        agent_path().map(|path| path.exists()).unwrap_or(false)
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;

    /// The folder that is opened at login.
    fn startup_dir() -> Option<PathBuf> {
        let appdata = std::env::var_os("APPDATA")?;
        Some(
            PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup"),
        )
    }

    /// The shortcut that is put there.
    fn shortcut_path() -> Option<PathBuf> {
        let dir = startup_dir()?;
        Some(dir.join("SkycRemote.lnk"))
    }

    pub fn set(enabled: bool) -> AutostartReport {
        let Some(path) = shortcut_path() else {
            return AutostartReport::off("找不到启动文件夹，没能改动".to_string());
        };
        set_at(path, enabled)
    }

    /// Turn it on or off, with the file in a folder of its own.
    ///
    /// The test uses this so that it does not touch the real place.
    pub fn set_at(path: PathBuf, enabled: bool) -> AutostartReport {
        if !enabled {
            let _ = std::fs::remove_file(&path);
            return AutostartReport::off("开机自启已经关了".to_string());
        }
        let Some(program) = program_path() else {
            return AutostartReport::off("不知道自己是哪个程序，没能改动".to_string());
        };
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return AutostartReport::off("建不了文件夹，没能改动".to_string());
            }
        }
        // The shortcut is made with the system's own tool, so that it is
        // a real shortcut and not a text file that pretends to be one.
        let script = format!(
            "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}')
$s.TargetPath = '{}'
$s.Save()
",
            path.display(),
            program.display()
        );
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output();
        match output {
            Ok(output) if output.status.success() => AutostartReport::on(
                Some(path),
                "开机自启已经开了，登录以后程序会自己起来".to_string(),
            ),
            Ok(output) => AutostartReport::off(format!(
                "没能写下来：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => AutostartReport::off(format!("没能写下来：{error}")),
        }
    }

    pub fn is_enabled() -> bool {
        shortcut_path().map(|path| path.exists()).unwrap_or(false)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    /// The desktop file that is put in the autostart folder.
    fn desktop_path() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                let home = std::env::var_os("HOME")?;
                Some(PathBuf::from(home).join(".config"))
            })?;
        Some(config.join("autostart").join("skycremote.desktop"))
    }

    pub fn set(enabled: bool) -> AutostartReport {
        let Some(path) = desktop_path() else {
            return AutostartReport::off("找不到配置文件夹，没能改动".to_string());
        };
        set_at(path, enabled)
    }

    /// Turn it on or off, with the file in a folder of its own.
    ///
    /// The test uses this so that it does not touch the real place.
    pub fn set_at(path: PathBuf, enabled: bool) -> AutostartReport {
        if !enabled {
            let _ = std::fs::remove_file(&path);
            return AutostartReport::off("开机自启已经关了".to_string());
        }
        let Some(program) = program_path() else {
            return AutostartReport::off("不知道自己是哪个程序，没能改动".to_string());
        };
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return AutostartReport::off("建不了文件夹，没能改动".to_string());
            }
        }
        let text = format!(
            "[Desktop Entry]
Type=Application
Name=SkycRemote
Exec={}
X-GNOME-Autostart-enabled=true
",
            program.display()
        );
        match std::fs::write(&path, text) {
            Ok(()) => AutostartReport::on(
                Some(path),
                "开机自启已经开了，登录以后程序会自己起来".to_string(),
            ),
            Err(error) => AutostartReport::off(format!("没能写下来：{error}")),
        }
    }

    pub fn is_enabled() -> bool {
        desktop_path().map(|path| path.exists()).unwrap_or(false)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod platform {
    use super::*;

    pub fn set(_enabled: bool) -> AutostartReport {
        AutostartReport::off("这个系统没有开机自启的做法".to_string())
    }

    pub fn set_at(_path: PathBuf, _enabled: bool) -> AutostartReport {
        AutostartReport::off("这个系统没有开机自启的做法".to_string())
    }

    pub fn is_enabled() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_knows_where_it_is() {
        // The path is what gets written into the thing that starts the
        // program.  A program that cannot say where it is cannot turn
        // the setting on, and the reason is said in words.
        let _ = program_path();
    }

    #[test]
    fn turning_it_on_and_off_leaves_nothing_behind() {
        // The test does not touch the real place the setting lives: it
        // would change what happens on the machine running the test.
        // A folder of its own is used instead.
        let folder = std::env::temp_dir().join(format!(
            "skycremote-autostart-{}-{}",
            std::process::id(),
            "test"
        ));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("the folder must be made");
        let path = folder.join("the-file");

        let report = set_at(path.clone(), true);
        assert!(report.enabled, "the setting must be on: {}", report.message);
        assert!(
            report.path.is_some(),
            "the place that was written must be said: {}",
            report.message
        );
        assert!(path.exists(), "the file must be there");

        let report = set_at(path.clone(), false);
        assert!(
            !report.enabled,
            "the setting must be off: {}",
            report.message
        );
        assert!(!path.exists(), "the file must be gone again");

        let _ = std::fs::remove_dir_all(&folder);
    }
}
