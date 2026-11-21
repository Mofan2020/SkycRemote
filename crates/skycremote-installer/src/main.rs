//! The installer.
//!
//! On Windows this is the program a person double-clicks: it puts
//! SkycRemote on the machine, puts the command line tool on the PATH,
//! makes the shortcuts and writes the entry that shows up in "Apps and
//! Features".
//!
//! It is written from scratch.  There is no NSIS here and no other
//! packer: the files that are installed are folded into this program
//! when it is built, and it puts them where they belong itself.
//!
//! The installer works for the person who runs it, not for the whole
//! machine.  Everything it writes is under the current user's own
//! account, so it never asks for an administrator password.

#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use include_dir::include_dir;

/// The files that are installed, folded into this program.
#[cfg(target_os = "windows")]
static PAYLOAD: include_dir::Dir<'static> = include_dir!("$SKYCREMOTE_PAYLOAD_DIR");

/// The name of the program that is installed.
#[cfg(target_os = "windows")]
const APP_FILE: &str = "SkycRemote.exe";

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let asked_for = |name: &str| {
        arguments
            .iter()
            .any(|argument| argument.eq_ignore_ascii_case(name))
    };

    // The helper that runs from the temporary folder: it takes the
    // program away.
    if asked_for("--remove") {
        #[cfg(target_os = "windows")]
        {
            std::process::exit(remove_installed());
        }
        #[cfg(not(target_os = "windows"))]
        {
            std::process::exit(1);
        }
    }

    // The way "Apps and Features" takes the program away.
    if asked_for("--uninstall") || asked_for("/uninstall") {
        std::process::exit(uninstall());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::exit(windows::install());
    }

    #[cfg(not(target_os = "windows"))]
    {
        eprintln!(
            "这个安装程序只能在 Windows 上运行。\n\
             macOS 用 SkycRemote.dmg：把程序拖进「应用程序」就行。"
        );
        std::process::exit(1);
    }
}

/// The folder the program is installed into, unless the person says
/// otherwise.
///
/// It is in the user's own area, so that installing does not need an
/// administrator password.
#[cfg(target_os = "windows")]
fn default_install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Programs").join("SkycRemote")
}

/// Put the program on the machine.
#[cfg(target_os = "windows")]
fn install_to(dir: &Path) -> Result<String, String> {
    if let Err(error) = std::fs::create_dir_all(dir) {
        return Err(format!("{} 建不出来：{error}", dir.display()));
    }

    // The program itself and the command line tool.
    for file in PAYLOAD.files() {
        let relative = file.path();
        let target = dir.join(relative);
        if let Some(folder) = target.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let bytes = file.contents();
        if let Err(error) = std::fs::write(&target, bytes) {
            return Err(format!("{} 写不进去：{error}", target.display()));
        }
    }

    let program = dir.join(APP_FILE);
    if !program.is_file() {
        return Err(format!("{} 不在里面", program.display()));
    }

    // The command line tool goes on the PATH of the person.
    let tool_dir = dir.join("cli");
    let _ = std::fs::create_dir_all(&tool_dir);
    skycremote_installer::windows::add_to_user_path(&tool_dir)
        .map_err(|error| format!("PATH 没有改：{error}"))?;

    // The entry that shows the program in "Apps and Features".
    let uninstaller = dir.join("SkycRemote-Setup.exe");
    if let Ok(bytes) = std::fs::read(std::env::current_exe().unwrap_or_default()) {
        let _ = std::fs::write(&uninstaller, bytes);
    }
    skycremote_installer::windows::write_uninstall_entry(dir, &uninstaller)?;

    // The shortcuts.  A shortcut that cannot be made is not a reason
    // to call the installation a failure.
    for folder in skycremote_installer::windows::shortcut_folders() {
        let _ = skycremote_installer::windows::create_shortcut(
            &program,
            &folder.join("SkycRemote.lnk"),
        );
    }

    Ok(dir.to_string_lossy().to_string())
}

/// Take the program off the machine.
///
/// The program that is running cannot take itself away, so the work is
/// handed to a copy of this program in the temporary folder: it waits a
/// moment, takes everything away, and then goes.
fn uninstall() -> i32 {
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("这个安装程序只能在 Windows 上运行。");
        std::process::exit(1);
    }

    #[cfg(target_os = "windows")]
    {
        let location = skycremote_installer::windows::install_location();
        let Some(dir) = location else {
            // Nothing is installed, or the entry is gone already.
            return 0;
        };

        // A copy in the temporary folder does the work, so that the
        // folder can be taken away while this program is still in it.
        let helper =
            std::env::temp_dir().join(format!("SkycRemote-uninstall-{}.exe", std::process::id()));
        let Ok(bytes) = std::fs::read(std::env::current_exe().unwrap_or_default()) else {
            std::process::exit(1);
        };
        if std::fs::write(&helper, bytes).is_err() {
            return 1;
        }
        let mut command = std::process::Command::new(&helper);
        command.arg("--remove");
        if command.spawn().is_err() {
            return 1;
        }
        let _ = dir;
        0
    }
}

/// The part of the removal that runs from the temporary folder.
#[cfg(target_os = "windows")]
fn remove_installed() -> i32 {
    let Some(dir) = skycremote_installer::windows::install_location() else {
        return 0;
    };
    let tool_dir = dir.join("cli");
    let _ = skycremote_installer::windows::remove_from_user_path(&tool_dir);
    let _ = skycremote_installer::windows::remove_uninstall_entry();

    // Wait a moment: the program that started this may still be
    // holding a file open.
    std::thread::sleep(std::time::Duration::from_millis(800));
    let _ = std::fs::remove_dir_all(&dir);

    // This copy takes itself away last.
    let helper = std::env::current_exe().unwrap_or_default();
    if !helper.to_string_lossy().contains("SkycRemote-uninstall-") {
        return 0;
    }
    let _ = std::process::Command::new("cmd")
        .args([
            "/C",
            "ping",
            "-n",
            "3",
            "127.0.0.1",
            ">",
            "nul",
            "&",
            "del",
            "/F",
            "/Q",
        ])
        .arg(&helper)
        .spawn();
    0
}

#[cfg(target_os = "windows")]
mod windows {
    //! The window of the installer.

    use std::path::PathBuf;
    use std::sync::mpsc::TryRecvError;
    use std::time::{Duration, Instant};

    use include_dir::include_dir;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoop};
    use tao::window::WindowBuilder;
    use wry::WebViewBuilder;

    /// The page of the installer, folded into the program.
    static PAGE: include_dir::Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/ui");

    /// How often the window looks for the answer.
    const POLL: Duration = Duration::from_millis(30);

    /// Open the window and install what the person asks for.
    pub fn install() -> i32 {
        let event_loop = EventLoop::new();
        let window = match WindowBuilder::new()
            .with_title("安装 SkycRemote")
            .with_inner_size(tao::dpi::LogicalSize::new(560.0, 460.0))
            .with_resizable(true)
            .build(&event_loop)
        {
            Ok(window) => window,
            Err(error) => {
                eprintln!("窗口开不出来：{error}");
                return 1;
            }
        };

        let (command_tx, command_rx) = std::sync::mpsc::channel::<(u64, String, String)>();
        let builder = WebViewBuilder::new().with_ipc_handler(move |request| {
            let body = request.body().to_string();
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
                let id = value.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                let command = value
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let payload = value
                    .get("payload")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}")
                    .to_string();
                let _ = command_tx.send((id, command, payload));
            }
        });

        let webview = match builder.build_as_child(&window) {
            Ok(webview) => webview,
            Err(error) => {
                eprintln!("安装界面开不出来：{error}");
                return 1;
            }
        };

        let page = read_page();
        let _ = webview.load_html(&page);

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::WaitUntil(Instant::now() + POLL);

            if let Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } = event
            {
                *control_flow = ControlFlow::Exit;
                return;
            }

            loop {
                match command_rx.try_recv() {
                    Ok((id, command, payload)) => {
                        let value = match command.as_str() {
                            "install" => {
                                let path = serde_json::from_str::<serde_json::Value>(&payload)
                                    .ok()
                                    .and_then(|value| {
                                        value.get("path").and_then(|v| v.as_str()).map(String::from)
                                    })
                                    .unwrap_or_default();
                                let dir = PathBuf::from(path.trim());
                                if dir.as_os_str().is_empty() {
                                    serde_json::json!({ "ok": false, "message": "没有安装位置" })
                                } else {
                                    match super::install_to(&dir) {
                                        Ok(where_to) => serde_json::json!({
                                            "ok": true,
                                            "path": where_to,
                                            "message": "安装完成"
                                        }),
                                        Err(error) => serde_json::json!({
                                            "ok": false,
                                            "message": error
                                        }),
                                    }
                                }
                            }
                            "defaultPath" => serde_json::json!({
                                "ok": true,
                                "path": super::default_install_dir().to_string_lossy(),
                                "version": env!("CARGO_PKG_VERSION"),
                            }),
                            "close" => {
                                *control_flow = ControlFlow::Exit;
                                serde_json::json!({ "ok": true })
                            }
                            other => serde_json::json!({
                                "ok": false,
                                "message": format!("不知道的命令：{other}")
                            }),
                        };
                        let script =
                            format!("window.__skycremote_resolve__({}, {}, {})", id, true, value);
                        let _ = webview.evaluate_script(&script);
                    }
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                }
            }
        })
    }

    /// The page of the installer, with the stylesheet folded into it.
    fn read_page() -> String {
        let mut page = String::from_utf8_lossy(
            PAGE.get_file("index.html")
                .map(|file| file.contents())
                .unwrap_or(&[]),
        )
        .to_string();
        if let Some(style) = PAGE.get_file("style.css") {
            let style = String::from_utf8_lossy(style.contents());
            page = page.replace(
                r#"<link rel="stylesheet" href="style.css">"#,
                &format!("<style>\n{style}\n</style>"),
            );
        }
        page
    }
}
