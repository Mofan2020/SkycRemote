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
//! There are two ways to install:
//!
//! * **Just for me** — everything in the person's own account.  No
//!   password is asked for.
//! * **For everybody** — the program goes into "Program Files" and the
//!   machine's own keys are written.  That needs administrator rights,
//!   so this program starts itself again with them, and Windows asks
//!   the person whether that is all right.

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

    // The run that has the administrator rights.  It has no window of
    // its own: it installs and says so.
    if asked_for("--all-users") {
        #[cfg(target_os = "windows")]
        {
            std::process::exit(install_for_everybody(&arguments));
        }
        #[cfg(not(target_os = "windows"))]
        {
            std::process::exit(1);
        }
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

/// Where the program is installed, unless the person says otherwise.
///
/// For one person it is in that person's own area, so that installing
/// does not need an administrator password.  For everybody it is where
/// programs that everybody uses go.
#[cfg(target_os = "windows")]
fn default_install_dir(scope: skycremote_installer::windows::Scope) -> PathBuf {
    match scope {
        skycremote_installer::windows::Scope::User => {
            let base = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            base.join("Programs").join("SkycRemote")
        }
        skycremote_installer::windows::Scope::AllUsers => {
            let base = std::env::var_os("ProgramFiles")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("C:\\Program Files"));
            base.join("SkycRemote")
        }
    }
}

/// Put the program on the machine.
#[cfg(target_os = "windows")]
fn install_to(dir: &Path, scope: skycremote_installer::windows::Scope) -> Result<String, String> {
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

    // The command line tool goes on the PATH.
    let tool_dir = dir.join("cli");
    let _ = std::fs::create_dir_all(&tool_dir);
    skycremote_installer::windows::add_to_path(scope, &tool_dir)
        .map_err(|error| format!("PATH 没有改：{error}"))?;

    // The entry that shows the program in "Apps and Features".
    let uninstaller = dir.join("SkycRemote-Setup.exe");
    if let Ok(bytes) = std::fs::read(std::env::current_exe().unwrap_or_default()) {
        let _ = std::fs::write(&uninstaller, bytes);
    }
    skycremote_installer::windows::write_uninstall_entry(scope, dir, &uninstaller)?;

    // The shortcuts.  A shortcut that cannot be made is not a reason
    // to call the installation a failure.
    for folder in shortcut_folders(scope) {
        let _ = skycremote_installer::windows::create_shortcut(
            &program,
            &folder.join("SkycRemote.lnk"),
        );
    }

    Ok(dir.to_string_lossy().to_string())
}

/// Where the shortcuts go.
///
/// For everybody it is the shared Start menu and the shared desktop,
/// so that every account on the machine sees them.
#[cfg(target_os = "windows")]
fn shortcut_folders(scope: skycremote_installer::windows::Scope) -> Vec<PathBuf> {
    match scope {
        skycremote_installer::windows::Scope::User => {
            skycremote_installer::windows::shortcut_folders()
        }
        skycremote_installer::windows::Scope::AllUsers => {
            let mut folders = Vec::new();
            if let Some(programdata) = std::env::var_os("ProgramData") {
                folders.push(
                    PathBuf::from(programdata)
                        .join("Microsoft")
                        .join("Windows")
                        .join("Start Menu")
                        .join("Programs"),
                );
            }
            if let Some(public) = std::env::var_os("PUBLIC") {
                folders.push(PathBuf::from(public).join("Desktop"));
            }
            folders
        }
    }
}

/// Install for everybody, in the run that has the administrator rights.
///
/// It has no window of its own, so it says what happened in a window
/// of the system's own.
#[cfg(target_os = "windows")]
fn install_for_everybody(arguments: &[String]) -> i32 {
    let scope = skycremote_installer::windows::Scope::AllUsers;

    // The path comes after the switch on the command line.
    let wanted = arguments
        .iter()
        .position(|argument| argument.eq_ignore_ascii_case("--all-users"))
        .and_then(|index| arguments.get(index + 1))
        .map(|path| PathBuf::from(path.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| default_install_dir(scope));

    match install_to(&wanted, scope) {
        Ok(where_to) => {
            skycremote_installer::windows::say(
                &format!(
                    "SkycRemote 已经装到 {where_to}。\n\n命令行工具 skycremote 也在 PATH 上了。"
                ),
                "安装完成",
            );
            0
        }
        Err(error) => {
            skycremote_installer::windows::say(&error, "安装没有成功");
            1
        }
    }
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
        // A copy in the temporary folder does the work, so that the
        // folder can be taken away while this program is still in it.
        let helper =
            std::env::temp_dir().join(format!("SkycRemote-uninstall-{}.exe", std::process::id()));
        let Ok(bytes) = std::fs::read(std::env::current_exe().unwrap_or_default()) else {
            std::process::exit(1);
        };
        if std::fs::write(&helper, bytes).is_err() {
            std::process::exit(1);
        }
        let mut command = std::process::Command::new(&helper);
        command.arg("--remove");
        if command.spawn().is_err() {
            std::process::exit(1);
        }
        0
    }
}

/// The part of the removal that runs from the temporary folder.
#[cfg(target_os = "windows")]
fn remove_installed() -> i32 {
    use skycremote_installer::windows::Scope;

    // The program may have been installed for one person or for
    // everybody, so both places are looked at.
    let scopes = [Scope::AllUsers, Scope::User];
    let mut found = None;
    for scope in scopes {
        if let Some(dir) = skycremote_installer::windows::install_location(scope) {
            found = Some((scope, dir));
            break;
        }
    }
    let Some((scope, dir)) = found else {
        // Nothing is installed, or the entry is gone already.
        return 0;
    };

    let tool_dir = dir.join("cli");
    let _ = skycremote_installer::windows::remove_from_path(scope, &tool_dir);
    let _ = skycremote_installer::windows::remove_uninstall_entry(scope);

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
    use std::time::{Duration, Instant};

    use include_dir::include_dir;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoop};
    use tao::window::WindowBuilder;
    use wry::WebViewBuilder;

    use skycremote_installer::windows::{is_elevated, relaunch_elevated, Scope};

    /// The page of the installer, folded into the program.
    static PAGE: include_dir::Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/ui");

    /// How often the window looks for the answer.
    const POLL: Duration = Duration::from_millis(30);

    /// Open the window and install what the person asks for.
    pub fn install() -> i32 {
        let event_loop = EventLoop::new();
        let window = match WindowBuilder::new()
            .with_title("安装 SkycRemote")
            .with_inner_size(tao::dpi::LogicalSize::new(560.0, 520.0))
            .with_resizable(true)
            .build(&event_loop)
        {
            Ok(window) => window,
            Err(error) => {
                eprintln!("窗口开不出来：{error}");
                return 1;
            }
        };

        // The page is a child of the window, so it has to be told
        // how big it is: without this it stays at 200 by 200 and sits
        // in the corner of the window.
        let scale_factor = window.scale_factor();
        let inner_size = window.inner_size().to_logical::<f64>(scale_factor);

        let (command_tx, command_rx) = std::sync::mpsc::channel::<(u64, String, String)>();
        let builder = WebViewBuilder::new()
            .with_bounds(wry::Rect {
                position: tao::dpi::LogicalPosition::new(0.0, 0.0).into(),
                size: inner_size.into(),
            })
            .with_ipc_handler(move |request| {
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

            // The window changed size: the page has to be told.
            if let Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } = event
            {
                let size = size.to_logical::<f64>(scale_factor);
                let _ = webview.set_bounds(wry::Rect {
                    position: tao::dpi::LogicalPosition::new(0.0, 0.0).into(),
                    size: size.into(),
                });
            }

            if let Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } = event
            {
                *control_flow = ControlFlow::Exit;
                return;
            }

            while let Ok((id, command, payload)) = command_rx.try_recv() {
                let value = match command.as_str() {
                    "install" => do_install(&payload, control_flow),
                    "defaultPath" => serde_json::json!({
                        "ok": true,
                        "user": super::default_install_dir(Scope::User).to_string_lossy(),
                        "allUsers": super::default_install_dir(Scope::AllUsers).to_string_lossy(),
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
                let script = format!("window.__skycremote_resolve__({}, {}, {})", id, true, value);
                let _ = webview.evaluate_script(&script);
            }
        })
    }

    /// Do what the page asked: put the program on the machine.
    fn do_install(payload: &str, control_flow: &mut ControlFlow) -> serde_json::Value {
        let parsed = serde_json::from_str::<serde_json::Value>(payload).ok();
        let path = parsed
            .as_ref()
            .and_then(|value| value.get("path"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let scope = match parsed
            .as_ref()
            .and_then(|value| value.get("scope"))
            .and_then(|value| value.as_str())
        {
            Some("allUsers") => Scope::AllUsers,
            _ => Scope::User,
        };

        if path.is_empty() {
            return serde_json::json!({ "ok": false, "message": "没有安装位置" });
        }
        let dir = PathBuf::from(&path);

        // Installing for everybody needs the machine's keys, and that
        // needs administrator rights.  Windows asks for them; this copy
        // of the program gets out of the way and the new one does the
        // work.
        if scope == Scope::AllUsers && !is_elevated() {
            return match relaunch_elevated(&format!("--all-users \"{}\"", dir.display())) {
                Ok(()) => {
                    *control_flow = ControlFlow::Exit;
                    serde_json::json!({
                        "ok": true,
                        "message": "正在请求管理员权限，装完会有一个提示窗口。"
                    })
                }
                Err(error) => serde_json::json!({ "ok": false, "message": error }),
            };
        }

        match super::install_to(&dir, scope) {
            Ok(where_to) => serde_json::json!({
                "ok": true,
                "path": where_to,
                "message": "安装完成",
            }),
            Err(error) => serde_json::json!({ "ok": false, "message": error }),
        }
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
