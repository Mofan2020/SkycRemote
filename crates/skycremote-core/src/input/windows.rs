//! Windows 的输入注入。
//!
//! 用的是 Win32 的 `SendInput`：系统自己的键盘和鼠标最终也是从这里
//! 进来的，所以注入进去的东西和真人按的没有区别，别的程序也分不出来。
//!
//! **字符和按键走的是两条路。** 字符用 Unicode 送过去
//! （`KEYEVENTF_UNICODE`），跟被控端摆的是什么键盘没有关系；按键按
//! 物理键位送，只有方向键、回车这些没有字符的键才走这条路。
//!
//! **权限。** Windows 没有 macOS 那种「辅助功能」开关，但有两道门：
//! 一道是 UIPI —— 普通权限的程序不能往管理员权限的窗口里塞输入；另
//! 一道是会话本身（跑在会话 0 里的服务看不见桌面）。这两道门挡着的
//! 时候 `SendInput` 一个事件也塞不进去，这里会把这件事说出来，而不
//! 是安静地什么也不做。
//!
//! **坐标。** Windows 的鼠标坐标是物理像素：屏幕在桌面上的位置
//! （`origin_x` / `origin_y`）和抓下来的那张图是同一个单位，所以这里
//! 不用像 macOS 和 X11 那样按缩放折算。

use super::dom_key;

#[cfg(target_os = "windows")]
use crate::capture::Screen;
#[cfg(target_os = "windows")]
use crate::protocol::InputEvent;

#[cfg(target_os = "windows")]
use super::{global_position_in_pixels, Injector, InputError};

/// Windows 的虚拟键码（VK_*）。
///
/// 只列用得着的那些；完整的一张表在 Windows 的头文件里。
mod vk {
    /// 退格。
    pub const BACK: u16 = 0x08;
    /// 制表。
    pub const TAB: u16 = 0x09;
    /// 回车。
    pub const RETURN: u16 = 0x0D;
    /// 任意一侧的 Shift。
    pub const SHIFT: u16 = 0x10;
    /// 任意一侧的 Control。
    pub const CONTROL: u16 = 0x11;
    /// 任意一侧的 Alt（Windows 叫它菜单键）。
    pub const MENU: u16 = 0x12;
    /// 暂停。
    pub const PAUSE: u16 = 0x13;
    /// 大写锁定。
    pub const CAPITAL: u16 = 0x14;
    /// 退出。
    pub const ESCAPE: u16 = 0x1B;
    /// 空格。
    pub const SPACE: u16 = 0x20;
    /// 向上翻页。
    pub const PRIOR: u16 = 0x21;
    /// 向下翻页。
    pub const NEXT: u16 = 0x22;
    /// 行尾。
    pub const END: u16 = 0x23;
    /// 行首。
    pub const HOME: u16 = 0x24;
    /// 左箭头。
    pub const LEFT: u16 = 0x25;
    /// 上箭头。
    pub const UP: u16 = 0x26;
    /// 右箭头。
    pub const RIGHT: u16 = 0x27;
    /// 下箭头。
    pub const DOWN: u16 = 0x28;
    /// 插入。
    pub const INSERT: u16 = 0x2D;
    /// 删除。
    pub const DELETE: u16 = 0x2E;
    /// 左边的 Windows 键。
    pub const LWIN: u16 = 0x5B;
    /// 菜单键。
    pub const APPS: u16 = 0x5D;
    /// 数字锁定。
    pub const NUMLOCK: u16 = 0x90;
    /// 滚动锁定。
    pub const SCROLL: u16 = 0x91;
    /// F1。
    pub const F1: u16 = 0x70;
    /// F2。
    pub const F2: u16 = 0x71;
    /// F3。
    pub const F3: u16 = 0x72;
    /// F4。
    pub const F4: u16 = 0x73;
    /// F5。
    pub const F5: u16 = 0x74;
    /// F6。
    pub const F6: u16 = 0x75;
    /// F7。
    pub const F7: u16 = 0x76;
    /// F8。
    pub const F8: u16 = 0x77;
    /// F9。
    pub const F9: u16 = 0x78;
    /// F10。
    pub const F10: u16 = 0x79;
    /// F11。
    pub const F11: u16 = 0x7A;
    /// F12。
    pub const F12: u16 = 0x7B;
}

/// `SendInput` 的标志位：鼠标的按键。
///
/// 这些数字是 Win32 定死的，写在这里是为了让它们的来龙去脉看得见。
mod flag {
    /// 鼠标：左键按下。
    pub const LEFT_DOWN: u32 = 0x0002;
    /// 鼠标：左键松开。
    pub const LEFT_UP: u32 = 0x0004;
    /// 鼠标：右键按下。
    pub const RIGHT_DOWN: u32 = 0x0008;
    /// 鼠标：右键松开。
    pub const RIGHT_UP: u32 = 0x0010;
    /// 鼠标：中键按下。
    pub const MIDDLE_DOWN: u32 = 0x0020;
    /// 鼠标：中键松开。
    pub const MIDDLE_UP: u32 = 0x0040;
}

/// `SendInput` 的标志位：键盘和指针。
///
/// 键盘的和鼠标的是两拨，数值不重叠。
#[cfg(target_os = "windows")]
mod win_flag {
    /// 键盘：这次是松开，不是按下。
    pub const KEY_UP: u32 = 0x0002;
    /// 键盘：这个事件带的是字符，不是键位。
    pub const UNICODE: u32 = 0x0004;
    /// 鼠标：指针在动。
    pub const MOVE: u32 = 0x0001;
    /// 鼠标：滚轮在转（竖着）。
    pub const WHEEL: u32 = 0x0800;
    /// 鼠标：滚轮在转（横着）。
    pub const HWHEEL: u32 = 0x1000;
    /// 鼠标：坐标是相对于整张桌面的，不是主屏幕。
    pub const VIRTUAL_DESK: u32 = 0x4000;
    /// 鼠标：坐标是 0..65535 的比例，不是像素。
    pub const ABSOLUTE: u32 = 0x8000;
}

/// 滚轮一格是这么多。
///
/// Windows 用 120 表示一格，和浏览器一次「滚一格」的 100 差不多。
const WHEEL_DELTA: i32 = 120;

/// DOM 的键码 → Windows 的虚拟键码。
///
/// 只有那些没有字符的键在这里。字母和数字走字符那条路，因为按物理
/// 键位会被对面键盘的布局影响 —— 被控端摆的是中文键盘的话，按 'a'
/// 就打不出 a。
///
/// 这些键码和 Windows 的虚拟键码是同一套数字：老式的 DOM 规范就是
/// 照着 Windows 抄的。但字母和数字也在那个范围里（'A' 就是 65），
/// 所以这里要一张白名单，不能直接拿键码当键码用。
pub fn virtual_key(code: u32) -> Option<u16> {
    Some(match code {
        dom_key::BACKSPACE => vk::BACK,
        dom_key::TAB => vk::TAB,
        dom_key::ENTER => vk::RETURN,
        dom_key::SHIFT => vk::SHIFT,
        dom_key::CONTROL => vk::CONTROL,
        dom_key::ALT => vk::MENU,
        dom_key::PAUSE => vk::PAUSE,
        dom_key::CAPS_LOCK => vk::CAPITAL,
        dom_key::ESCAPE => vk::ESCAPE,
        dom_key::SPACE => vk::SPACE,
        dom_key::PAGE_UP => vk::PRIOR,
        dom_key::PAGE_DOWN => vk::NEXT,
        dom_key::END => vk::END,
        dom_key::HOME => vk::HOME,
        dom_key::ARROW_LEFT => vk::LEFT,
        dom_key::ARROW_UP => vk::UP,
        dom_key::ARROW_RIGHT => vk::RIGHT,
        dom_key::ARROW_DOWN => vk::DOWN,
        dom_key::INSERT => vk::INSERT,
        dom_key::DELETE => vk::DELETE,
        dom_key::META => vk::LWIN,
        dom_key::CONTEXT_MENU => vk::APPS,
        dom_key::NUM_LOCK => vk::NUMLOCK,
        dom_key::SCROLL_LOCK => vk::SCROLL,
        dom_key::F1 => vk::F1,
        dom_key::F2 => vk::F2,
        dom_key::F3 => vk::F3,
        dom_key::F4 => vk::F4,
        dom_key::F5 => vk::F5,
        dom_key::F6 => vk::F6,
        dom_key::F7 => vk::F7,
        dom_key::F8 => vk::F8,
        dom_key::F9 => vk::F9,
        dom_key::F10 => vk::F10,
        dom_key::F11 => vk::F11,
        dom_key::F12 => vk::F12,
        _ => return None,
    })
}

/// 协议里的按钮号 → Windows 的「按下」标志位。
///
/// 协议里 0 是左键，1 是中键，2 是右键。
pub fn mouse_down_flag(button: u8) -> Option<u32> {
    Some(match button {
        0 => flag::LEFT_DOWN,
        1 => flag::MIDDLE_DOWN,
        2 => flag::RIGHT_DOWN,
        _ => return None,
    })
}

/// 协议里的按钮号 → Windows 的「松开」标志位。
pub fn mouse_up_flag(button: u8) -> Option<u32> {
    Some(match button {
        0 => flag::LEFT_UP,
        1 => flag::MIDDLE_UP,
        2 => flag::RIGHT_UP,
        _ => return None,
    })
}

/// 一段字符 → 要送出去的 UTF-16 码元。
///
/// Windows 的 Unicode 注入一次送一个码元，所以汉字是一个，emoji
/// （在基本多文种平面之外）是两个。
pub fn unicode_units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// 一整格滚轮折多少像素。
///
/// 浏览器一个滚轮刻度给 100 像素左右。X11 和 Windows 都只能按「格」发，
/// 所以这里要把像素折成格。
pub const PIXELS_PER_NOTCH: f32 = 100.0;

/// 攒下来的像素 + 这次的像素 → 这次发几格、还剩多少零头。
///
/// 零头必须留着：轻轻滚一下只有几十像素，直接截断成 0 格，慢慢滚就一点
/// 反应都没有。返回的第二个值是留给下一次的余量。
pub fn take_notches(remainder: f32, pixels: f32) -> (i32, f32) {
    let total = remainder + pixels;
    let steps = (total / PIXELS_PER_NOTCH).trunc();
    (steps as i32, total - steps * PIXELS_PER_NOTCH)
}

/// 滚了几格 → Windows 的滚轮单位。
///
/// 负数照样往里传：Windows 认的是有符号的数，位模式不变。
pub fn wheel_units(steps: i32) -> u32 {
    steps.wrapping_mul(WHEEL_DELTA) as u32
}

/// 桌面上的一个位置 → Windows 要的 0..65535。
///
/// Windows 的绝对坐标是把整张桌面（所有屏幕加起来）切成 65536 份，
/// 所以要先减掉桌面的左上角，位置才落在这一块屏幕上。
pub fn normalized_coordinate(value: f64, origin: f64, size: i32) -> i32 {
    if size <= 0 {
        return 0;
    }
    let scaled = (value - origin) / f64::from(size) * 65535.0;
    scaled.round().clamp(0.0, 65535.0) as i32
}

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::GetLastError;
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// 一个键盘事件。
#[cfg(target_os = "windows")]
fn keyboard_input(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: KEYBD_EVENT_FLAGS(flags),
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// 一个鼠标事件。
#[cfg(target_os = "windows")]
fn mouse_input(dx: i32, dy: i32, data: u32, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: MOUSE_EVENT_FLAGS(flags),
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// 在这台 Windows 上注入输入。
///
/// 这里不记任何状态：Windows 的修饰键就是真的按下去、松开来，
/// 系统自己知道现在按着哪些键。
#[cfg(target_os = "windows")]
pub struct WindowsInjector {
    /// 滚轮攒下的零头（像素），竖直方向。
    scroll_y: std::cell::Cell<f32>,
    /// 滚轮攒下的零头（像素），水平方向。
    scroll_x: std::cell::Cell<f32>,
}

#[cfg(target_os = "windows")]
impl WindowsInjector {
    /// 做好一个注入器。
    pub fn new() -> Result<Self, InputError> {
        Ok(Self {
            scroll_y: std::cell::Cell::new(0.0),
            scroll_x: std::cell::Cell::new(0.0),
        })
    }

    /// 把一组输入事件送进系统。
    ///
    /// `SendInput` 会说出它收下了几个。少了就是被系统挡了 —— 最常见
    /// 的原因是 UIPI：普通权限的程序不能往管理员权限的窗口里塞输入。
    fn send(&self, inputs: &[INPUT]) -> Result<(), InputError> {
        let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            return Err(InputError::Refused(format!(
                "the system took {sent} of {} input events (error {})",
                inputs.len(),
                unsafe { GetLastError() }.0
            )));
        }
        Ok(())
    }

    /// 按下一个键，或者松开。
    fn post_key(&self, code: u32, down: bool) -> Result<(), InputError> {
        let keycode = virtual_key(code).ok_or(InputError::UnknownKey(code))?;
        let flags = if down { 0 } else { win_flag::KEY_UP };
        self.send(&[keyboard_input(keycode, 0, flags)])
    }

    /// 送一段字符过去。
    ///
    /// 字符送过去之后，对面摆的是什么键盘就不重要了。
    fn post_text(&self, text: &str) -> Result<(), InputError> {
        let units = unicode_units(text);
        if units.is_empty() {
            return Ok(());
        }
        let mut inputs = Vec::with_capacity(units.len() * 2);
        for unit in units {
            // The key code is left at zero: there is no key that means
            // this character.  The characters ride along in the event
            // itself.
            inputs.push(keyboard_input(0, unit, win_flag::UNICODE));
            inputs.push(keyboard_input(
                0,
                unit,
                win_flag::UNICODE | win_flag::KEY_UP,
            ));
        }
        self.send(&inputs)
    }

    /// 把指针挪到图里的那个位置。
    fn move_mouse(&self, screen: &Screen, x: u32, y: u32) -> Result<(), InputError> {
        let (left, top, width, height) = Self::virtual_screen();
        if width <= 0 || height <= 0 {
            return Err(InputError::Refused(
                "the desktop reports no size, so the pointer cannot be placed".to_string(),
            ));
        }
        let (px, py) = global_position_in_pixels(screen, x, y);
        let dx = normalized_coordinate(px, left, width);
        let dy = normalized_coordinate(py, top, height);
        let flags = win_flag::MOVE | win_flag::ABSOLUTE | win_flag::VIRTUAL_DESK;
        self.send(&[mouse_input(dx, dy, 0, flags)])
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
        let flags = if down {
            mouse_down_flag(button)
        } else {
            mouse_up_flag(button)
        }
        .ok_or(InputError::UnknownButton(button))?;
        // The pointer has to be where the click is: a button event on
        // Windows carries no position of its own.
        self.move_mouse(screen, x, y)?;
        self.send(&[mouse_input(0, 0, 0, flags)])
    }

    /// 转一下滚轮。
    fn post_scroll(
        &self,
        dx: f32,
        dy: f32,
        screen: &Screen,
        x: u32,
        y: u32,
    ) -> Result<(), InputError> {
        self.move_mouse(screen, x, y)?;
        let mut inputs = Vec::new();
        let (vertical, rest_y) = take_notches(self.scroll_y.get(), dy);
        self.scroll_y.set(rest_y);
        if vertical != 0 {
            inputs.push(mouse_input(0, 0, wheel_units(vertical), win_flag::WHEEL));
        }
        let (horizontal, rest_x) = take_notches(self.scroll_x.get(), dx);
        self.scroll_x.set(rest_x);
        if horizontal != 0 {
            inputs.push(mouse_input(0, 0, wheel_units(horizontal), win_flag::HWHEEL));
        }
        if inputs.is_empty() {
            return Ok(());
        }
        self.send(&inputs)
    }

    /// 整张桌面的位置和大小。
    ///
    /// Windows 的绝对鼠标坐标是按整张桌面（所有屏幕加起来）算的，
    /// 所以要知道它有多大、从哪儿开始。
    fn virtual_screen() -> (f64, f64, i32, i32) {
        unsafe {
            (
                f64::from(GetSystemMetrics(SM_XVIRTUALSCREEN)),
                f64::from(GetSystemMetrics(SM_YVIRTUALSCREEN)),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        }
    }
}

#[cfg(target_os = "windows")]
impl Injector for WindowsInjector {
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
            assert!(virtual_key(code).is_some(), "the key {code} is missing");
        }
    }

    #[test]
    fn the_function_keys_are_all_there() {
        for number in 1..=12u32 {
            let code = dom_key::F1 + number - 1;
            assert!(virtual_key(code).is_some(), "F{number} is missing");
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
            let keycode = virtual_key(code).expect("the key is in the table");
            if let Some(other) = seen.insert(keycode, code) {
                panic!("the key {code} and the key {other} share the place {keycode}");
            }
        }
    }

    #[test]
    fn a_key_that_is_not_in_the_table_is_said_so() {
        // A letter is a character, not a key: it goes the other way.
        assert!(virtual_key(65).is_none(), "a letter has no place here");
        assert!(virtual_key(48).is_none(), "a digit has no place here");
    }

    #[test]
    fn the_special_keys_land_on_their_own_places() {
        // The DOM codes of these keys were copied from Windows long
        // ago, so the numbers agree; the table is what keeps the
        // letters out.
        assert_eq!(virtual_key(dom_key::BACKSPACE), Some(0x08));
        assert_eq!(virtual_key(dom_key::ENTER), Some(0x0D));
        assert_eq!(virtual_key(dom_key::ESCAPE), Some(0x1B));
        assert_eq!(virtual_key(dom_key::ARROW_DOWN), Some(0x28));
        assert_eq!(virtual_key(dom_key::META), Some(0x5B));
        assert_eq!(virtual_key(dom_key::F1), Some(0x70));
        assert_eq!(virtual_key(dom_key::F12), Some(0x7B));
    }

    #[test]
    fn a_character_travels_as_its_own_code_units() {
        assert_eq!(unicode_units("a"), vec![0x61]);
        assert_eq!(unicode_units("中"), vec![0x4E2D]);
        // An emoji lives outside the basic plane, so it is two code
        // units.
        assert_eq!(unicode_units("😀"), vec![0xD83D, 0xDE00]);
        assert!(unicode_units("").is_empty());
    }

    #[test]
    fn the_mouse_buttons_are_the_ones_windows_knows() {
        assert_eq!(mouse_down_flag(0), Some(flag::LEFT_DOWN));
        assert_eq!(mouse_down_flag(1), Some(flag::MIDDLE_DOWN));
        assert_eq!(mouse_down_flag(2), Some(flag::RIGHT_DOWN));
        assert_eq!(mouse_up_flag(0), Some(flag::LEFT_UP));
        assert_eq!(mouse_up_flag(1), Some(flag::MIDDLE_UP));
        assert_eq!(mouse_up_flag(2), Some(flag::RIGHT_UP));
        assert!(mouse_down_flag(7).is_none(), "there is no seventh button");
        assert!(mouse_up_flag(7).is_none(), "there is no seventh button");
    }

    #[test]
    fn a_position_on_the_desktop_becomes_a_number_the_desktop_understands() {
        // A 1920-wide screen: the left edge is 0 and the right edge is
        // as far as the number goes.
        assert_eq!(normalized_coordinate(0.0, 0.0, 1920), 0);
        assert_eq!(normalized_coordinate(960.0, 0.0, 1920), 32768);
        assert_eq!(normalized_coordinate(1920.0, 0.0, 1920), 65535);
        // A screen to the left of the desktop's origin starts below
        // zero, and the number still starts at 0 there.
        assert_eq!(normalized_coordinate(-1920.0, -1920.0, 1920), 0);
        // A desktop that reports no size has nowhere to point.
        assert_eq!(normalized_coordinate(100.0, 0.0, 0), 0);
    }

    #[test]
    fn the_wheel_turns_in_steps_of_a_hundred_and_twenty() {
        assert_eq!(wheel_units(1), 120);
        assert_eq!(wheel_units(0), 0);
        // Turning the other way is the same bits with the sign turned.
        assert_eq!(wheel_units(-1), u32::MAX - 119);
    }

    /// 浏览器一格滚轮约 100 像素，要变成一格，不是一百格。
    #[test]
    fn a_wheel_notch_becomes_one_step_not_a_hundred() {
        assert_eq!(take_notches(0.0, 100.0), (1, 0.0));
        assert_eq!(take_notches(0.0, -100.0), (-1, 0.0));
        assert_eq!(take_notches(0.0, 0.0), (0, 0.0));
    }

    /// 零头要留到下一次，不然轻轻滚一直凑不满一格，手感像卡住。
    #[test]
    fn a_small_scroll_is_carried_over_until_it_makes_a_step() {
        let (first, rest) = take_notches(0.0, 40.0);
        assert_eq!(first, 0);
        assert!((rest - 40.0).abs() < 0.001);

        let (second, rest) = take_notches(rest, 40.0);
        assert_eq!(second, 0);
        let (third, _) = take_notches(rest, 40.0);
        assert_eq!(third, 1, "三次 40 像素要凑出一格");
    }

    /// The table has to agree with the system it talks to.  This only
    /// runs where the system headers are, but that is exactly where
    /// the answer matters.
    #[cfg(target_os = "windows")]
    #[test]
    fn the_table_agrees_with_the_windows_headers() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
            MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DELETE,
            VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F12, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN,
            VK_MENU, VK_NEXT, VK_NUMLOCK, VK_PAUSE, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SCROLL,
            VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
        };

        assert_eq!(virtual_key(dom_key::BACKSPACE), Some(VK_BACK.0));
        assert_eq!(virtual_key(dom_key::TAB), Some(VK_TAB.0));
        assert_eq!(virtual_key(dom_key::ENTER), Some(VK_RETURN.0));
        assert_eq!(virtual_key(dom_key::SHIFT), Some(VK_SHIFT.0));
        assert_eq!(virtual_key(dom_key::CONTROL), Some(VK_CONTROL.0));
        assert_eq!(virtual_key(dom_key::ALT), Some(VK_MENU.0));
        assert_eq!(virtual_key(dom_key::PAUSE), Some(VK_PAUSE.0));
        assert_eq!(virtual_key(dom_key::CAPS_LOCK), Some(VK_CAPITAL.0));
        assert_eq!(virtual_key(dom_key::ESCAPE), Some(VK_ESCAPE.0));
        assert_eq!(virtual_key(dom_key::SPACE), Some(VK_SPACE.0));
        assert_eq!(virtual_key(dom_key::PAGE_UP), Some(VK_PRIOR.0));
        assert_eq!(virtual_key(dom_key::PAGE_DOWN), Some(VK_NEXT.0));
        assert_eq!(virtual_key(dom_key::END), Some(VK_END.0));
        assert_eq!(virtual_key(dom_key::HOME), Some(VK_HOME.0));
        assert_eq!(virtual_key(dom_key::ARROW_LEFT), Some(VK_LEFT.0));
        assert_eq!(virtual_key(dom_key::ARROW_UP), Some(VK_UP.0));
        assert_eq!(virtual_key(dom_key::ARROW_RIGHT), Some(VK_RIGHT.0));
        assert_eq!(virtual_key(dom_key::ARROW_DOWN), Some(VK_DOWN.0));
        assert_eq!(virtual_key(dom_key::INSERT), Some(VK_INSERT.0));
        assert_eq!(virtual_key(dom_key::DELETE), Some(VK_DELETE.0));
        assert_eq!(virtual_key(dom_key::META), Some(VK_LWIN.0));
        assert_eq!(virtual_key(dom_key::NUM_LOCK), Some(VK_NUMLOCK.0));
        assert_eq!(virtual_key(dom_key::SCROLL_LOCK), Some(VK_SCROLL.0));
        assert_eq!(virtual_key(dom_key::F1), Some(VK_F1.0));
        assert_eq!(virtual_key(dom_key::F12), Some(VK_F12.0));

        assert_eq!(mouse_down_flag(0), Some(MOUSEEVENTF_LEFTDOWN.0));
        assert_eq!(mouse_down_flag(1), Some(MOUSEEVENTF_MIDDLEDOWN.0));
        assert_eq!(mouse_down_flag(2), Some(MOUSEEVENTF_RIGHTDOWN.0));
        assert_eq!(mouse_up_flag(0), Some(MOUSEEVENTF_LEFTUP.0));
        assert_eq!(mouse_up_flag(1), Some(MOUSEEVENTF_MIDDLEUP.0));
        assert_eq!(mouse_up_flag(2), Some(MOUSEEVENTF_RIGHTUP.0));
    }
}
