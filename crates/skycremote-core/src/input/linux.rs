//! Linux 的输入注入。
//!
//! 用的是 X11 的 XTEST 扩展：它让程序可以伪造键盘和鼠标事件，让它们
//! 走进 X 服务器的输入队列，和真人按的没有区别。
//!
//! **需要 X11。** Wayland 不给客户端这条路。XTEST 只能走到 XWayland
//! 那一半去，桌面上原生的那些窗口（GNOME、KDE 自己画的）收不到；事
//! 实上在 Wayland 会话里注入是半聋半哑的 —— 事件落进了没有人的地
//! 方，而用户只看到「点了没反应」。所以在一个 Wayland 会话里，这里
//! 明确地拒绝，而不是装作能做。
//!
//! **字符和按键走的是两条路。** 字符用 Unicode 送过去 —— 借一个没人
//! 用的键位，把它临时改成这个字符的键符，按下去，再改回来；这样跟
//! 被控端摆的是什么键盘没有关系。按键按物理键位送，只有方向键、回车
//! 这些没有字符的键才走这条路。

use super::dom_key;

#[cfg(target_os = "linux")]
use crate::capture::Screen;
#[cfg(target_os = "linux")]
use crate::protocol::InputEvent;

#[cfg(target_os = "linux")]
use super::{global_position, Injector, InputError};

/// X11 的键符（KeySym）。
///
/// 只列用得着的那些；完整的一张表在 X11 的 `keysymdef.h` 里。
mod keysym_code {
    /// 空格。
    pub const SPACE: u32 = 0x0020;
    /// 退格。
    pub const BACKSPACE: u32 = 0xFF08;
    /// 制表。
    pub const TAB: u32 = 0xFF09;
    /// 回车。
    pub const RETURN: u32 = 0xFF0D;
    /// 暂停。
    pub const PAUSE: u32 = 0xFF13;
    /// 滚动锁定。
    pub const SCROLL_LOCK: u32 = 0xFF14;
    /// 退出。
    pub const ESCAPE: u32 = 0xFF1B;
    /// 行首。
    pub const HOME: u32 = 0xFF50;
    /// 左箭头。
    pub const LEFT: u32 = 0xFF51;
    /// 上箭头。
    pub const UP: u32 = 0xFF52;
    /// 右箭头。
    pub const RIGHT: u32 = 0xFF53;
    /// 下箭头。
    pub const DOWN: u32 = 0xFF54;
    /// 向上翻页。
    pub const PRIOR: u32 = 0xFF55;
    /// 向下翻页。
    pub const NEXT: u32 = 0xFF56;
    /// 行尾。
    pub const END: u32 = 0xFF57;
    /// 插入。
    pub const INSERT: u32 = 0xFF63;
    /// 菜单键。
    pub const MENU: u32 = 0xFF67;
    /// 数字锁定。
    pub const NUM_LOCK: u32 = 0xFF7F;
    /// F1。
    pub const F1: u32 = 0xFFBE;
    /// F2。
    pub const F2: u32 = 0xFFBF;
    /// F3。
    pub const F3: u32 = 0xFFC0;
    /// F4。
    pub const F4: u32 = 0xFFC1;
    /// F5。
    pub const F5: u32 = 0xFFC2;
    /// F6。
    pub const F6: u32 = 0xFFC3;
    /// F7。
    pub const F7: u32 = 0xFFC4;
    /// F8。
    pub const F8: u32 = 0xFFC5;
    /// F9。
    pub const F9: u32 = 0xFFC6;
    /// F10。
    pub const F10: u32 = 0xFFC7;
    /// F11。
    pub const F11: u32 = 0xFFC8;
    /// F12。
    pub const F12: u32 = 0xFFC9;
    /// 左边的 Shift。
    pub const SHIFT_L: u32 = 0xFFE1;
    /// 左边的 Control。
    pub const CONTROL_L: u32 = 0xFFE3;
    /// 大写锁定。
    pub const CAPS_LOCK: u32 = 0xFFE5;
    /// 左边的 Alt。
    pub const ALT_L: u32 = 0xFFE9;
    /// 左边的 Super（Windows 键 / Command）。
    pub const SUPER_L: u32 = 0xFFEB;
}

/// Unicode 的键符从这一段开始。
///
/// X11 里，键符 0x20 到 0xFF 是拉丁一，再往上的字符要写成
/// 0x0100_0000 加上码位。
const UNICODE_KEYSYM_BASE: u32 = 0x0100_0000;

/// DOM 的键码 → X11 的键符。
///
/// 只有那些没有字符的键在这里。字母和数字走字符那条路，因为按物理
/// 键位会被对面键盘的布局影响 —— 被控端摆的是中文键盘的话，按 'a'
/// 就打不出 a。
pub fn keysym(code: u32) -> Option<u32> {
    Some(match code {
        dom_key::BACKSPACE => keysym_code::BACKSPACE,
        dom_key::TAB => keysym_code::TAB,
        dom_key::ENTER => keysym_code::RETURN,
        dom_key::SHIFT => keysym_code::SHIFT_L,
        dom_key::CONTROL => keysym_code::CONTROL_L,
        dom_key::ALT => keysym_code::ALT_L,
        dom_key::PAUSE => keysym_code::PAUSE,
        dom_key::CAPS_LOCK => keysym_code::CAPS_LOCK,
        dom_key::ESCAPE => keysym_code::ESCAPE,
        dom_key::SPACE => keysym_code::SPACE,
        dom_key::PAGE_UP => keysym_code::PRIOR,
        dom_key::PAGE_DOWN => keysym_code::NEXT,
        dom_key::END => keysym_code::END,
        dom_key::HOME => keysym_code::HOME,
        dom_key::ARROW_LEFT => keysym_code::LEFT,
        dom_key::ARROW_UP => keysym_code::UP,
        dom_key::ARROW_RIGHT => keysym_code::RIGHT,
        dom_key::ARROW_DOWN => keysym_code::DOWN,
        dom_key::INSERT => keysym_code::INSERT,
        dom_key::DELETE => 0xFFFF,
        dom_key::META => keysym_code::SUPER_L,
        dom_key::CONTEXT_MENU => keysym_code::MENU,
        dom_key::NUM_LOCK => keysym_code::NUM_LOCK,
        dom_key::SCROLL_LOCK => keysym_code::SCROLL_LOCK,
        dom_key::F1 => keysym_code::F1,
        dom_key::F2 => keysym_code::F2,
        dom_key::F3 => keysym_code::F3,
        dom_key::F4 => keysym_code::F4,
        dom_key::F5 => keysym_code::F5,
        dom_key::F6 => keysym_code::F6,
        dom_key::F7 => keysym_code::F7,
        dom_key::F8 => keysym_code::F8,
        dom_key::F9 => keysym_code::F9,
        dom_key::F10 => keysym_code::F10,
        dom_key::F11 => keysym_code::F11,
        dom_key::F12 => keysym_code::F12,
        _ => return None,
    })
}

/// 一个字符 → X11 的键符。
///
/// 拉丁一里的字符直接用自己的码位当键符；再往上的要走 Unicode 的
/// 那一段。
pub fn unicode_keysym(character: char) -> u32 {
    let code = character as u32;
    if code < 0x100 {
        code
    } else {
        UNICODE_KEYSYM_BASE | code
    }
}

/// 协议里的按钮号 → X11 的按钮号。
///
/// X11 从 1 开始数：1 是左键，2 是中键，3 是右键。
pub fn mouse_button(button: u8) -> Option<u32> {
    Some(match button {
        0 => 1,
        1 => 2,
        2 => 3,
        _ => return None,
    })
}

/// 线上来的滚轮量 → 滚了几格。
///
/// 线上走的是「行数」，这里把它变成整数格数；不到一格的部分不算，
/// 否则轻轻碰一下滚轮就会跳一格。
pub fn wheel_steps(value: f32) -> i32 {
    value.trunc() as i32
}

#[cfg(target_os = "linux")]
use std::ffi::c_void;
#[cfg(target_os = "linux")]
use std::os::raw::{c_int, c_uchar, c_uint};

#[cfg(target_os = "linux")]
use x11_dl::xlib::{Display, KeySym, XErrorEvent, Xlib};
#[cfg(target_os = "linux")]
// The crate calls this one `Xf86vmode` — a slip in the name it
// gives the XTEST bindings.  Here it is called what it is.
use x11_dl::xtest::Xf86vmode as XTest;

/// 一个空着的键位。
///
/// 打字的时候需要按一个「能打出这个字符」的键，而 X11 的键盘上未必
/// 有那么一个键位 —— 中文的键符就没有对应的键。所以要借一个没人用
/// 的键位，把它临时改成这个字符的键符，用完再改回去。
#[cfg(target_os = "linux")]
#[derive(Clone)]
struct SpareKey {
    /// 键位号。
    keycode: u8,
    /// 这个键位上有几个键符。
    per_keycode: i32,
    /// 它原来的键符。
    ///
    /// 键符是 `c_ulong`，所以这里跟着 X11 走，而不是自己定一个宽度。
    keysyms: Vec<KeySym>,
}

/// X11 出错的时候不要把进程杀了。
///
/// Xlib 自带的处理函数会把出错的消息打印出来，然后**结束整个进程**。
/// 一个因为一个键打不进去就把远控主机杀掉的程序，比一个说「这个键没
/// 打进去」的程序糟糕得多。
#[cfg(target_os = "linux")]
unsafe extern "C" fn quiet_error(_display: *mut Display, _event: *mut XErrorEvent) -> c_int {
    0
}

/// 在这台 Linux 上注入输入。
#[cfg(target_os = "linux")]
pub struct LinuxInjector {
    /// X11 的连接。
    display: *mut Display,
    /// X11 的函数。
    xlib: Xlib,
    /// XTEST 的函数。
    xtest: XTest,
    /// 专门用来打字的键位，以及它原来的键符。
    spare: Option<SpareKey>,
}

// The X11 connection is a plain pointer, so the compiler cannot know it
// is safe to move between threads.  It is: the session runs its input
// on whichever thread the network happens to be on, and only that one
// thread ever talks to the display.
#[cfg(target_os = "linux")]
unsafe impl Send for LinuxInjector {}

#[cfg(target_os = "linux")]
impl LinuxInjector {
    /// 做好一个注入器。
    pub fn new() -> Result<Self, InputError> {
        // XTEST is a library of its own and it is not installed on
        // every machine.  Saying so is better than a mysterious
        // failure later on.
        let xlib = Xlib::open().map_err(|error| {
            InputError::Refused(format!(
                "the X11 library could not be loaded ({error}); install libx11-6"
            ))
        })?;
        let xtest = XTest::open().map_err(|error| {
            InputError::Refused(format!(
                "the XTEST extension could not be loaded ({error}); install libxtst6"
            ))
        })?;

        // Wayland does not let a client in.  XTEST would reach the
        // XWayland applications and nothing else, which is the worst
        // kind of answer: it looks like it works and half of the
        // desktop never sees the mouse.
        if crate::capture::linux::detect_session_from_env()
            == crate::capture::linux::SessionKind::Wayland
        {
            return Err(InputError::Refused(
                "this is a Wayland session, and a Wayland session does not let a program \
                 fake the mouse and the keyboard: the XTEST events would only reach the \
                 XWayland applications. Log in with an X11 session to control this machine"
                    .to_string(),
            ));
        }

        // Xlib's own error handler ends the process when the server
        // refuses a request.
        unsafe { (xlib.XSetErrorHandler)(Some(quiet_error)) };

        let display = unsafe { (xlib.XOpenDisplay)(std::ptr::null()) };
        if display.is_null() {
            return Err(InputError::Refused(
                "there is no X display to inject into; the DISPLAY variable is not set, or \
                 points at a display server that is not running"
                    .to_string(),
            ));
        }

        Ok(Self {
            display,
            xlib,
            xtest,
            spare: None,
        })
    }

    /// 送一个键盘事件。
    fn fake_key(&self, keycode: u8, down: bool) -> Result<(), InputError> {
        let sent = unsafe {
            (self.xtest.XTestFakeKeyEvent)(
                self.display,
                c_uint::from(keycode),
                c_int::from(down),
                0,
            )
        };
        if sent == 0 {
            return Err(InputError::Refused(
                "the X server refused the key event".to_string(),
            ));
        }
        unsafe { (self.xlib.XFlush)(self.display) };
        Ok(())
    }

    /// 送一个鼠标按键事件。
    fn fake_button(&self, button: u32, down: bool) -> Result<(), InputError> {
        let sent = unsafe {
            (self.xtest.XTestFakeButtonEvent)(self.display, button, c_int::from(down), 0)
        };
        if sent == 0 {
            return Err(InputError::Refused(
                "the X server refused the button event".to_string(),
            ));
        }
        unsafe { (self.xlib.XFlush)(self.display) };
        Ok(())
    }

    /// 按下一个键，或者松开。
    fn post_key(&mut self, code: u32, down: bool) -> Result<(), InputError> {
        let symbol = keysym(code).ok_or(InputError::UnknownKey(code))?;
        let keycode = unsafe { (self.xlib.XKeysymToKeycode)(self.display, symbol as KeySym) };
        if keycode != 0 {
            return self.fake_key(keycode, down);
        }
        // The keyboard in front of the user has no key for this
        // symbol; borrow an empty one.
        self.post_with_borrowed_key(symbol as KeySym, down)
    }

    /// 借用一个空键位来按一个键符。
    ///
    /// 借出去之后不还，一直到这个注入器被丢掉的时候才还（见 `Drop`）。
    /// X 的 KeyPress 只带 keycode，字符是客户端在处理事件的那一刻才去
    /// 查键位表查出来的；按完就还原，正常节奏的客户端读到的都是空键符，
    /// 事件到了、字一个也出不来。所以借用期间键位表就这么留着，反正那
    /// 是一个没有物理键的空键位。
    fn post_with_borrowed_key(&mut self, symbol: KeySym, down: bool) -> Result<(), InputError> {
        let spare = self.spare_key()?;
        let mut borrowed = spare.keysyms.clone();
        borrowed[0] = symbol;
        self.set_keysyms(spare.keycode, spare.per_keycode, &borrowed)?;
        self.fake_key(spare.keycode, down)
    }

    /// 送一段字符过去。
    ///
    /// 字符送过去之后，对面摆的是什么键盘就不重要了。
    fn post_text(&mut self, text: &str) -> Result<(), InputError> {
        for character in text.chars() {
            self.post_character(character)?;
        }
        Ok(())
    }

    /// 送一个字符过去。
    fn post_character(&mut self, character: char) -> Result<(), InputError> {
        let symbol = unicode_keysym(character) as KeySym;
        self.post_with_borrowed_key(symbol, true)?;
        self.post_with_borrowed_key(symbol, false)
    }

    /// 换掉一个键位上的键符。
    ///
    /// `XChangeKeyboardMapping` 的返回值不能拿来判断成败：Xvfb 上它返回
    /// 0，键位表却真的被改了。按返回值判会把一个明明能打字的服务器判成
    /// 不能打字，所以这里不看返回值 —— 改完把键位表读回来，跟写进去的
    /// 那个键符比一比，比得上才算改成了。
    fn set_keysyms(
        &self,
        keycode: u8,
        per_keycode: i32,
        keysyms: &[KeySym],
    ) -> Result<(), InputError> {
        let mut buffer = keysyms.to_vec();
        buffer.resize(per_keycode as usize, 0);
        unsafe {
            (self.xlib.XChangeKeyboardMapping)(
                self.display,
                c_int::from(keycode),
                per_keycode,
                buffer.as_mut_ptr(),
                1,
            );
        }
        // The change has to reach the server before it is read back and
        // before the key is pressed.
        unsafe { (self.xlib.XSync)(self.display, 0) };

        let wanted = keysyms.first().copied().unwrap_or(0);
        let mut per_keycode_back: c_int = 0;
        let back = unsafe {
            (self.xlib.XGetKeyboardMapping)(self.display, keycode, 1, &mut per_keycode_back)
        };
        if back.is_null() || per_keycode_back <= 0 {
            return Err(InputError::Refused(
                "the X server would not say what the keyboard looks like".to_string(),
            ));
        }
        let actual = unsafe { *back };
        unsafe { (self.xlib.XFree)(back as *mut c_void) };
        if actual != wanted {
            return Err(InputError::Refused(format!(
                "the X server did not take the new key: it answers {actual:#x}, \
                 {wanted:#x} was written"
            )));
        }
        Ok(())
    }

    /// 找一个没人用的键位。
    ///
    /// X11 的键盘上总有几个键位是没有键符的，拿一个来打字，打完再改
    /// 回去。
    fn spare_key(&mut self) -> Result<SpareKey, InputError> {
        if let Some(spare) = &self.spare {
            return Ok(spare.clone());
        }

        let mut lowest: c_int = 0;
        let mut highest: c_int = 0;
        unsafe { (self.xlib.XDisplayKeycodes)(self.display, &mut lowest, &mut highest) };
        if highest < lowest {
            return Err(InputError::Refused(
                "the X server reports no keyboard".to_string(),
            ));
        }

        let count = highest - lowest + 1;
        let mut per_keycode: c_int = 0;
        let map = unsafe {
            (self.xlib.XGetKeyboardMapping)(
                self.display,
                lowest as c_uchar,
                count,
                &mut per_keycode,
            )
        };
        if map.is_null() || per_keycode <= 0 {
            return Err(InputError::Refused(
                "the X server would not give out the keyboard".to_string(),
            ));
        }

        let total = count as usize * per_keycode as usize;
        let keysyms = unsafe { std::slice::from_raw_parts(map, total) };
        let mut found = None;
        for index in 0..count as usize {
            let row = &keysyms[index * per_keycode as usize..(index + 1) * per_keycode as usize];
            if row.iter().all(|symbol| *symbol == 0) {
                found = Some(SpareKey {
                    keycode: (lowest as usize + index) as u8,
                    per_keycode,
                    keysyms: row.to_vec(),
                });
                break;
            }
        }
        unsafe { (self.xlib.XFree)(map as *mut c_void) };

        let spare = found.ok_or_else(|| {
            InputError::Refused("the keyboard has no empty key to type with".to_string())
        })?;
        self.spare = Some(spare.clone());
        Ok(spare)
    }

    /// 把指针挪到图里的那个位置。
    fn move_mouse(&self, screen: &Screen, x: u32, y: u32) -> Result<(), InputError> {
        let (px, py) = global_position(screen, x, y);
        // Screen 0 is the one that holds the whole desktop; the
        // monitors a RandR setup puts side by side are all inside it.
        let sent = unsafe {
            (self.xtest.XTestFakeMotionEvent)(self.display, 0, px as c_int, py as c_int, 0)
        };
        if sent == 0 {
            return Err(InputError::Refused(
                "the X server refused the pointer event".to_string(),
            ));
        }
        unsafe { (self.xlib.XFlush)(self.display) };
        Ok(())
    }

    /// 按下或者松开一个鼠标键。
    fn post_button(
        &self,
        button: u8,
        down: bool,
        screen: &Screen,
        x: u32,
        y: u32,
    ) -> Result<(), InputError> {
        let number = mouse_button(button).ok_or(InputError::UnknownButton(button))?;
        self.move_mouse(screen, x, y)?;
        self.fake_button(number, down)
    }

    /// 转一下滚轮。
    ///
    /// X11 把滚轮当成四个额外的按钮：4 是上，5 是下，6 是左，7 是右。
    fn post_scroll(
        &self,
        dx: f32,
        dy: f32,
        screen: &Screen,
        x: u32,
        y: u32,
    ) -> Result<(), InputError> {
        self.move_mouse(screen, x, y)?;
        let vertical = wheel_steps(dy);
        if vertical != 0 {
            let button = if vertical > 0 { 4 } else { 5 };
            for _ in 0..vertical.unsigned_abs() {
                self.fake_button(button, true)?;
                self.fake_button(button, false)?;
            }
        }
        let horizontal = wheel_steps(dx);
        if horizontal != 0 {
            let button = if horizontal > 0 { 7 } else { 6 };
            for _ in 0..horizontal.unsigned_abs() {
                self.fake_button(button, true)?;
                self.fake_button(button, false)?;
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Injector for LinuxInjector {
    fn inject(&mut self, event: &InputEvent, screen: &Screen) -> Result<(), InputError> {
        match event {
            InputEvent::KeyDown { key } => self.post_key(*key, true),
            InputEvent::KeyUp { key } => self.post_key(*key, false),
            InputEvent::Text { text } => self.post_text(text),
            InputEvent::MouseMove { x, y } => self.move_mouse(screen, *x, *y),
            InputEvent::MouseDown { button, x, y } => {
                self.post_button(*button, true, screen, *x, *y)
            }
            InputEvent::MouseUp { button, x, y } => {
                self.post_button(*button, false, screen, *x, *y)
            }
            InputEvent::Scroll { dx, dy, x, y } => self.post_scroll(*dx, *dy, screen, *x, *y),
            InputEvent::Clipboard { .. } => Err(InputError::Unsupported),
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for LinuxInjector {
    fn drop(&mut self) {
        // The borrowed key goes back now and not a moment sooner: a
        // client looks the character up when it handles the event, so
        // giving it back earlier would leave the character in the
        // mapping for nobody.
        if let Some(spare) = &self.spare {
            let _ = self.set_keysyms(spare.keycode, spare.per_keycode, &spare.keysyms);
        }
        unsafe {
            (self.xlib.XCloseDisplay)(self.display);
        }
    }
}

/// 这个进程能不能往系统里注入输入。
///
/// 判的是三件事：这是一个 X11 会话（Wayland 不行）、`DISPLAY` 指得到
/// 一个显示服务器、XTEST 和 X11 这两个库都在。
#[cfg(target_os = "linux")]
pub fn can_inject() -> bool {
    use crate::capture::linux::{detect_session_from_env, SessionKind};

    if detect_session_from_env() == SessionKind::Wayland {
        return false;
    }
    if std::env::var_os("DISPLAY").is_none() {
        return false;
    }
    x11_dl::xlib::Xlib::open().is_ok() && x11_dl::xtest::Xf86vmode::open().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个没有字符的键都要在表里。
    const SPECIAL_KEYS: [u32; 24] = [
        dom_key::BACKSPACE,
        dom_key::TAB,
        dom_key::ENTER,
        dom_key::SHIFT,
        dom_key::CONTROL,
        dom_key::ALT,
        dom_key::PAUSE,
        dom_key::CAPS_LOCK,
        dom_key::ESCAPE,
        dom_key::SPACE,
        dom_key::PAGE_UP,
        dom_key::PAGE_DOWN,
        dom_key::END,
        dom_key::HOME,
        dom_key::ARROW_LEFT,
        dom_key::ARROW_UP,
        dom_key::ARROW_RIGHT,
        dom_key::ARROW_DOWN,
        dom_key::INSERT,
        dom_key::DELETE,
        dom_key::META,
        dom_key::CONTEXT_MENU,
        dom_key::NUM_LOCK,
        dom_key::SCROLL_LOCK,
    ];

    #[test]
    fn the_special_keys_all_have_a_place_on_this_keyboard() {
        // Every key that has no character must be in the table, or it
        // would be dropped on the way in.
        for code in SPECIAL_KEYS {
            assert!(keysym(code).is_some(), "the key {code} is missing");
        }
    }

    #[test]
    fn the_function_keys_are_all_there() {
        for number in 1..=12u32 {
            let code = dom_key::F1 + number - 1;
            assert!(keysym(code).is_some(), "F{number} is missing");
        }
    }

    #[test]
    fn no_two_keys_share_a_place() {
        let mut seen = std::collections::HashMap::new();
        for code in SPECIAL_KEYS
            .iter()
            .copied()
            .chain(dom_key::F1..=dom_key::F12)
        {
            let symbol = keysym(code).expect("the key is in the table");
            if let Some(other) = seen.insert(symbol, code) {
                panic!("the key {code} and the key {other} share the place {symbol}");
            }
        }
    }

    #[test]
    fn a_key_that_is_not_in_the_table_is_said_so() {
        // A letter is a character, not a key: it goes the other way.
        assert!(keysym(65).is_none(), "a letter has no place here");
        assert!(keysym(48).is_none(), "a digit has no place here");
    }

    #[test]
    fn the_special_keys_land_on_their_own_places() {
        assert_eq!(keysym(dom_key::BACKSPACE), Some(0xFF08));
        assert_eq!(keysym(dom_key::ENTER), Some(0xFF0D));
        assert_eq!(keysym(dom_key::ESCAPE), Some(0xFF1B));
        assert_eq!(keysym(dom_key::SPACE), Some(0x0020));
        assert_eq!(keysym(dom_key::ARROW_DOWN), Some(0xFF54));
        assert_eq!(keysym(dom_key::DELETE), Some(0xFFFF));
        assert_eq!(keysym(dom_key::META), Some(0xFFEB));
        assert_eq!(keysym(dom_key::F1), Some(0xFFBE));
        assert_eq!(keysym(dom_key::F12), Some(0xFFC9));
    }

    #[test]
    fn a_character_becomes_a_keysym() {
        // The Latin-1 characters are their own code.
        assert_eq!(unicode_keysym('a'), 0x0061);
        assert_eq!(unicode_keysym('A'), 0x0041);
        assert_eq!(unicode_keysym(' '), 0x0020);
        // Anything above Latin-1 goes into the Unicode part.
        assert_eq!(unicode_keysym('中'), 0x0100_4E2D);
        assert_eq!(unicode_keysym('€'), 0x0100_20AC);
        assert_eq!(unicode_keysym('😀'), 0x0101_F600);
    }

    #[test]
    fn the_mouse_buttons_are_the_ones_x11_knows() {
        assert_eq!(mouse_button(0), Some(1));
        assert_eq!(mouse_button(1), Some(2));
        assert_eq!(mouse_button(2), Some(3));
        assert!(mouse_button(7).is_none(), "there is no seventh button");
    }

    #[test]
    fn the_wheel_turns_in_whole_steps() {
        assert_eq!(wheel_steps(3.7), 3);
        assert_eq!(wheel_steps(-3.7), -3);
        assert_eq!(wheel_steps(0.4), 0);
    }

    #[test]
    fn the_new_key_is_checked_by_reading_it_back() {
        // This contract was bitten by a real server: on Xvfb,
        // XChangeKeyboardMapping answers 0 and still changes the keyboard.
        // Judging it by that answer turns a machine that can type into one
        // that says it cannot, so the source is held to the read-back:
        // the call must not be taken as an answer, the mapping must be read
        // back, and the read-back must be freed.
        let source = include_str!("linux.rs");
        let start = source.find("fn set_keysyms").expect("set_keysyms is here");
        let rest = &source[start..];
        let end = rest.find("fn spare_key").expect("spare_key follows it");
        let body = &rest[..end];

        // One write, and it is not bound to a name: a bare call cannot be
        // mistaken for a verdict.
        assert_eq!(
            body.matches("XChangeKeyboardMapping").count(),
            1,
            "the mapping is written exactly once in set_keysyms"
        );
        assert!(
            !body.contains("let changed")
                && !body.contains("= unsafe {\n            (self.xlib.XChangeKeyboardMapping)"),
            "the answer of XChangeKeyboardMapping must not be taken for a verdict"
        );
        assert!(
            body.contains("XGetKeyboardMapping"),
            "the mapping has to be read back to see whether it took"
        );
        assert!(body.contains("XFree"), "the map read back has to be freed");
        assert!(
            body.contains("actual != wanted"),
            "the read-back has to be compared with what was written"
        );
    }

    #[test]
    fn a_borrowed_key_is_given_back_only_when_the_session_ends() {
        // X 的 KeyPress 只带 keycode，字符是客户端在处理事件那一刻才去
        // 查键位表查出来的。按完就还原的话，正常节奏的客户端读到的都是
        // 空键符 —— 事件到了，字一个也出不来。所以借用期间不许还，
        // 还的那一处只准出现在 Drop 里。
        let source = include_str!("linux.rs");
        let borrow_start = source
            .find("fn post_with_borrowed_key")
            .expect("the borrowing function is here");
        let rest = &source[borrow_start..];
        let borrow_end = rest.find("fn post_text").expect("post_text follows it");
        let borrow = &rest[..borrow_end];

        assert_eq!(
            borrow.matches("set_keysyms").count(),
            1,
            "the borrowing function writes the mapping exactly once"
        );
        assert_eq!(
            borrow.matches("&spare.keysyms").count(),
            0,
            "a borrowed key must not be given back while the session runs"
        );
        assert!(
            borrow.contains("fake_key"),
            "the borrowing function is the one that presses the key"
        );

        let drop_start = source
            .find("impl Drop for LinuxInjector")
            .expect("the injector is dropped somewhere");
        let drop_rest = &source[drop_start..];
        let drop_end = drop_rest.find("\n}\n").expect("the impl ends");
        let drop_body = &drop_rest[..drop_end];
        assert_eq!(
            drop_body.matches("&spare.keysyms").count(),
            1,
            "the borrowed key is given back when the session ends"
        );
    }
}
