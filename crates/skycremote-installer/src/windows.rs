//! The parts of the installation that are Windows' own.
//!
//! Three things happen here: a folder is put on the PATH of the
//! person, an entry is written so that the program shows up in
//! "Apps and Features", and a shortcut is made for the Start menu.
//!
//! Everything here is written for the person who is installing, not
//! for the whole machine: the registry key is the one of the current
//! user.  That is why the installer does not need an administrator
//! password, and why it cannot install for other people — which is
//! what a program that only talks to the machine it is on should do.

use std::path::{Path, PathBuf};

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{LPARAM, WIN32_ERROR, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

/// The key of the current user that holds the environment.
const ENVIRONMENT_KEY: &str = "Environment";

/// The key under which the program is listed in "Apps and Features".
const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\SkycRemote";

/// A wide string with a zero at the end, which is what Windows wants.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The PATH of the current user, as it is written down.
pub fn user_path() -> Result<String, String> {
    let key = open_environment()?;
    let result = read_string(key, "Path");
    let _ = unsafe { RegCloseKey(key) };
    result
}

/// Put a folder on the PATH of the current user.
///
/// The PATH is read, the folder is added at the end if it is not
/// already there, and it is written back.  The programs that are
/// already running are told about it, so that a terminal that is
/// already open sees the new PATH too.
pub fn add_to_user_path(dir: &Path) -> Result<(), String> {
    let wanted = dir.to_string_lossy().to_string();
    let key = open_environment()?;
    let current = read_string(key, "Path").unwrap_or_default();
    let mut value = current.clone();

    let already = current
        .split(';')
        .map(|part| part.trim().trim_matches('"'))
        .any(|part| part.eq_ignore_ascii_case(&wanted));
    if !already {
        if !value.is_empty() && !value.ends_with(';') {
            value.push(';');
        }
        value.push_str(&wanted);
        write_string(key, "Path", &value, REG_EXPAND_SZ)?;
    }
    let _ = unsafe { RegCloseKey(key) };
    broadcast_path_change();
    Ok(())
}

/// Take a folder off the PATH of the current user.
///
/// It is used when the program is uninstalled: a folder that is not
/// there any more must not stay on the PATH.
pub fn remove_from_user_path(dir: &Path) -> Result<(), String> {
    let wanted = dir.to_string_lossy().to_string();
    let key = open_environment()?;
    let current = read_string(key, "Path").unwrap_or_default();
    let kept: Vec<&str> = current
        .split(';')
        .filter(|part| {
            let part = part.trim().trim_matches('"');
            !part.eq_ignore_ascii_case(&wanted)
        })
        .collect();
    let value = kept.join(";");
    write_string(key, "Path", &value, REG_EXPAND_SZ)?;
    let _ = unsafe { RegCloseKey(key) };
    broadcast_path_change();
    Ok(())
}

/// Write the entry that makes the program show up in "Apps and
/// Features", with the program that takes it away again.
pub fn write_uninstall_entry(install_dir: &Path, uninstaller: &Path) -> Result<(), String> {
    let key = create_key(UNINSTALL_KEY)?;
    let write =
        |name: &str, value: &str| -> Result<(), String> { write_string(key, name, value, REG_SZ) };
    write("DisplayName", "SkycRemote")?;
    write("Publisher", "Skyc8266")?;
    write("DisplayVersion", env!("CARGO_PKG_VERSION"))?;
    write("InstallLocation", &install_dir.to_string_lossy())?;
    write("UninstallString", &uninstaller.to_string_lossy())?;
    write(
        "QuietUninstallString",
        &format!("{} /uninstall", uninstaller.to_string_lossy()),
    )?;
    write("DisplayIcon", &uninstaller.to_string_lossy())?;
    write("NoModify", "1")?;
    write("NoRepair", "1")?;
    let _ = unsafe { RegCloseKey(key) };
    Ok(())
}

/// Take the entry out of "Apps and Features".
pub fn remove_uninstall_entry() -> Result<(), String> {
    unsafe {
        let status = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(wide(UNINSTALL_KEY).as_ptr()));
        if status != WIN32_ERROR(0) {
            return Err(format!("the entry could not be taken away: {}", status.0));
        }
    }
    Ok(())
}

/// Make a shortcut that points at a program.
///
/// It goes into the Start menu and onto the desktop.  A shortcut that
/// cannot be made is not a reason to stop the installation, so the
/// error is only given back to be written in the log.
pub fn create_shortcut(target: &Path, link: &Path) -> Result<(), String> {
    unsafe {
        // The apartment is set up once per thread: the shell needs
        // one, and the error of "already set up" is not a failure.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("the shortcut could not be made: {error}"))?;
        let target_wide = wide(&target.to_string_lossy());
        shell_link
            .SetPath(PCWSTR(target_wide.as_ptr()))
            .map_err(|error| format!("the shortcut could not be made: {error}"))?;
        if let Some(folder) = target.parent() {
            let folder_wide = wide(&folder.to_string_lossy());
            let _ = shell_link.SetWorkingDirectory(PCWSTR(folder_wide.as_ptr()));
        }
        let description = wide("SkycRemote");
        let _ = shell_link.SetDescription(PCWSTR(description.as_ptr()));

        let persist: IPersistFile = shell_link
            .cast()
            .map_err(|error| format!("the shortcut could not be made: {error}"))?;
        if let Some(folder) = link.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let link_wide = wide(&link.to_string_lossy());
        persist
            .Save(PCWSTR(link_wide.as_ptr()), true)
            .map_err(|error| format!("the shortcut could not be saved: {error}"))?;
    }
    Ok(())
}

/// The folders where the shortcuts go.
pub fn shortcut_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        folders.push(
            PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }
    if let Some(desktop) = std::env::var_os("USERPROFILE") {
        folders.push(PathBuf::from(desktop).join("Desktop"));
    }
    folders
}

/// Where the program was installed, as the entry in "Apps and
/// Features" says.
pub fn install_location() -> Option<PathBuf> {
    let key = open_key(UNINSTALL_KEY).ok()?;
    let result = read_string(key, "InstallLocation").ok();
    let _ = unsafe { RegCloseKey(key) };
    let text = result?;
    if text.trim().is_empty() {
        None
    } else {
        Some(PathBuf::from(text))
    }
}

/// Open a key of the current user, or make it when it is not there.
fn open_key(path: &str) -> Result<HKEY, String> {
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wide(path).as_ptr()),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if status != WIN32_ERROR(0) {
        return Err(format!("the key could not be opened: {}", status.0));
    }
    Ok(key)
}

/// Open the key of the current user that holds the environment.
fn open_environment() -> Result<HKEY, String> {
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wide(ENVIRONMENT_KEY).as_ptr()),
            None,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if status != WIN32_ERROR(0) {
        return Err(format!("the environment could not be opened: {}", status.0));
    }
    Ok(key)
}

/// Make a key, or open it when it is already there.
fn create_key(path: &str) -> Result<HKEY, String> {
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wide(path).as_ptr()),
            None,
            PCWSTR::null(),
            windows::Win32::System::Registry::REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    if status != WIN32_ERROR(0) {
        return Err(format!("the key could not be made: {}", status.0));
    }
    Ok(key)
}

/// Read a string out of a key.
fn read_string(key: HKEY, name: &str) -> Result<String, String> {
    let mut size: u32 = 0;
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(wide(name).as_ptr()),
            None,
            None,
            None,
            Some(&mut size),
        )
    };
    if status != WIN32_ERROR(0) || size == 0 {
        return Err(format!("the value {name} could not be read: {}", status.0));
    }
    let mut buffer = vec![0u8; size as usize];
    let mut read = size;
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(wide(name).as_ptr()),
            None,
            None,
            Some(buffer.as_mut_ptr()),
            Some(&mut read),
        )
    };
    if status != WIN32_ERROR(0) {
        return Err(format!("the value {name} could not be read: {}", status.0));
    }
    // The value is a string of two-byte characters, the way Windows
    // stores text.
    let (pairs, _) = buffer[..read as usize].as_chunks::<2>();
    let words: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    let text = String::from_utf16_lossy(&words);
    Ok(text.trim_end_matches(' ').to_string())
}

/// Write a string into a key.
fn write_string(
    key: HKEY,
    name: &str,
    value: &str,
    kind: windows::Win32::System::Registry::REG_VALUE_TYPE,
) -> Result<(), String> {
    let mut data: Vec<u8> = value
        .encode_utf16()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    data.push(0);
    data.push(0);
    let status =
        unsafe { RegSetValueExW(key, PCWSTR(wide(name).as_ptr()), None, kind, Some(&data)) };
    if status != WIN32_ERROR(0) {
        return Err(format!(
            "the value {name} could not be written: {}",
            status.0
        ));
    }
    Ok(())
}

/// Take a value out of a key.
fn delete_value(key: HKEY, name: &str) -> Result<(), String> {
    let status = unsafe { RegDeleteValueW(key, PCWSTR(wide(name).as_ptr())) };
    if status != WIN32_ERROR(0) {
        return Err(format!(
            "the value {name} could not be taken away: {}",
            status.0
        ));
    }
    Ok(())
}

/// Tell the programs that are running that the PATH has changed.
///
/// Without this, a terminal window that is already open keeps the old
/// PATH until it is closed.
fn broadcast_path_change() {
    let text = wide("Environment");
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(text.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            5000,
            None,
        );
    }
}

/// Remove the value of the given name from the environment key.
///
/// It is used when the program is uninstalled.
pub fn remove_from_environment(name: &str) -> Result<(), String> {
    let key = open_environment()?;
    let result = delete_value(key, name);
    let _ = unsafe { RegCloseKey(key) };
    result
}
