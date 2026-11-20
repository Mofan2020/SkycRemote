//! 把对面送来的输入变成这台机器上的动作。
//!
//! 三件事要在这里说清楚：
//!
//! **键码。** 线上走的是 DOM 的 `KeyboardEvent.keyCode`（退格是 8，
//! F1 是 112）。这一套是网页标准，跨浏览器一致，每个平台再把它换成
//! 自己认识的键码。
//!
//! **坐标。** 线上走的是捕获那张图的像素。系统要的是全局位置，所以
//! 要加上那块屏幕在桌面上摆的地方，还要按缩放折算。
//!
//! **字符与按键的区别。** 能打出来的字符（字母、数字、中文、符号）
//! 走字符事件，用 Unicode 注入；只有方向键、回车那些没有字符的键才
//! 走按键事件。
//!
//! 这么分是有道理的：注入按键是按**物理键位**来的，被控端换成中文
//! 布局，按 'a' 就打不出 a 了。注入字符是直接送字符过去，跟对面
//! 摆的是什么键盘没有关系 —— 这才是远程控制该有的样子。

use crate::capture::Screen;
use crate::protocol::InputEvent;

/// DOM 的 `KeyboardEvent.keyCode`。
///
/// 线上就是用这一套，所以这里的每个数都是线上的约定，改动会让新旧
/// 两端对不上。
pub mod dom_key {
    /// 退格。
    pub const BACKSPACE: u32 = 8;
    /// 制表。
    pub const TAB: u32 = 9;
    /// 回车。
    pub const ENTER: u32 = 13;
    /// Shift，左边右边同一个数。
    pub const SHIFT: u32 = 16;
    /// Control。
    pub const CONTROL: u32 = 17;
    /// Alt / Option。
    pub const ALT: u32 = 18;
    /// 暂停。
    pub const PAUSE: u32 = 19;
    /// 大写锁定。
    pub const CAPS_LOCK: u32 = 20;
    /// 退出。
    pub const ESCAPE: u32 = 27;
    /// 空格。
    pub const SPACE: u32 = 32;
    /// 向上翻页。
    pub const PAGE_UP: u32 = 33;
    /// 向下翻页。
    pub const PAGE_DOWN: u32 = 34;
    /// 行尾。
    pub const END: u32 = 35;
    /// 行首。
    pub const HOME: u32 = 36;
    /// 左箭头。
    pub const ARROW_LEFT: u32 = 37;
    /// 上箭头。
    pub const ARROW_UP: u32 = 38;
    /// 右箭头。
    pub const ARROW_RIGHT: u32 = 39;
    /// 下箭头。
    pub const ARROW_DOWN: u32 = 40;
    /// 插入。
    pub const INSERT: u32 = 45;
    /// 删除。
    pub const DELETE: u32 = 46;
    /// Command / Windows 键。
    pub const META: u32 = 91;
    /// 菜单键。
    pub const CONTEXT_MENU: u32 = 93;
    /// 数字锁定。
    pub const NUM_LOCK: u32 = 144;
    /// 滚动锁定。
    pub const SCROLL_LOCK: u32 = 145;

    /// F1。
    ///
    /// DOM 的规定是 F1 是 112，往后顺延。
    pub const F1: u32 = 112;
    /// F2。
    pub const F2: u32 = 113;
    /// F3。
    pub const F3: u32 = 114;
    /// F4。
    pub const F4: u32 = 115;
    /// F5。
    pub const F5: u32 = 116;
    /// F6。
    pub const F6: u32 = 117;
    /// F7。
    pub const F7: u32 = 118;
    /// F8。
    pub const F8: u32 = 119;
    /// F9。
    pub const F9: u32 = 120;
    /// F10。
    pub const F10: u32 = 121;
    /// F11。
    pub const F11: u32 = 122;
    /// F12。
    pub const F12: u32 = 123;
}

/// 输入没做成的理由。
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// 这台机器上还没有能注入输入的办法。
    #[error("this platform cannot inject input yet")]
    Unsupported,
    /// 系统不认这个键码。
    #[error("the key code {0} means nothing on this platform")]
    UnknownKey(u32),
    /// 系统拒绝了这次注入。
    ///
    /// macOS 上最常见的原因是程序没有辅助功能权限。
    #[error("the system refused the event: {0}")]
    Refused(String),
}

/// 把输入变成这台机器上的动作。
pub trait Injector: Send {
    /// 处理一个输入。
    ///
    /// 坐标是捕获那张图里的像素；注入器知道那块屏幕在桌面上的位置，
    /// 自己换算成系统要的单位。
    fn inject(&mut self, event: &InputEvent, screen: &Screen)
        -> Result<(), InputError>;
}

/// 按住不放的修饰键。
///
/// 要一直记着，因为打一个大写字母靠的是「Shift 按着 + 字母键」，
/// 而字母键那一拍本身不带这个信息。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Modifiers {
    /// Shift。
    pub shift: bool,
    /// Control。
    pub control: bool,
    /// Alt / Option。
    pub alt: bool,
    /// Command / Windows。
    pub command: bool,
}

impl Modifiers {
    /// 记住一个键被按下了。
    pub fn press(&mut self, code: u32) {
        match code {
            dom_key::SHIFT => self.shift = true,
            dom_key::CONTROL => self.control = true,
            dom_key::ALT => self.alt = true,
            dom_key::META => self.command = true,
            _ => {}
        }
    }

    /// 记住一个键被松开了。
    pub fn release(&mut self, code: u32) {
        match code {
            dom_key::SHIFT => self.shift = false,
            dom_key::CONTROL => self.control = false,
            dom_key::ALT => self.alt = false,
            dom_key::META => self.command = false,
            _ => {}
        }
    }

    /// 有没有按住修饰键。
    pub fn any(&self) -> bool {
        self.shift || self.control || self.alt || self.command
    }
}

/// 把捕获那张图里的像素换成桌面上的位置。
///
/// 屏幕上摆的位置用的是平台自己的单位（macOS 是点，别的平台是
/// 像素），而图里的像素是物理像素，所以要按缩放折算。
pub fn global_position(screen: &Screen, x: u32, y: u32) -> (f64, f64) {
    let scale = if screen.scale_factor > 0.0 && screen.scale_factor.is_finite() {
        screen.scale_factor
    } else {
        1.0
    };
    (
        screen.origin_x + f64::from(x) / scale,
        screen.origin_y + f64::from(y) / scale,
    )
}

/// 这个进程能不能往系统里注入输入。
///
/// macOS 上要往系统里塞键盘鼠标事件，得先拿到「辅助功能」权限。
/// 没有的话事件会被安静地丢掉 —— 不报错，也不动。所以这个要在开始
/// 之前问清楚，而不是等用户发现「点了没反应」。
#[cfg(target_os = "macos")]
pub fn can_inject() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

/// 这个进程能不能往系统里注入输入。
///
/// 除了 macOS 以外还没有能注入的平台，所以这里是说不行。
#[cfg(not(target_os = "macos"))]
pub fn can_inject() -> bool {
    false
}

/// 造一个这台机器上的注入器。
pub fn injector() -> Result<Box<dyn Injector>, InputError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::MacosInjector::new()?))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(InputError::Unsupported)
    }
}

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(origin_x: f64, origin_y: f64, scale_factor: f64) -> Screen {
        Screen {
            index: 0,
            name: "test".into(),
            width: 2880,
            height: 1800,
            scale_factor,
            primary: true,
            origin_x,
            origin_y,
        }
    }

    #[test]
    fn a_point_on_a_retina_screen_moves_into_points() {
        // The picture is 2880 wide but the desktop counts 1440 points,
        // so half way across the picture is 720 points.
        let (x, y) = global_position(&screen(0.0, 0.0, 2.0), 1440, 900);
        assert!((x - 720.0).abs() < 0.001, "{x}");
        assert!((y - 450.0).abs() < 0.001, "{y}");
    }

    #[test]
    fn a_point_keeps_its_place_on_the_desktop() {
        // The second screen sits to the right, so its points start
        // where the first one ends.
        let (x, y) = global_position(&screen(1440.0, 0.0, 1.0), 10, 20);
        assert!((x - 1450.0).abs() < 0.001, "{x}");
        assert!((y - 20.0).abs() < 0.001, "{y}");
    }

    #[test]
    fn a_screen_with_no_scale_factor_is_taken_at_face_value() {
        let (x, _) = global_position(&screen(0.0, 0.0, 0.0), 100, 0);
        assert!((x - 100.0).abs() < 0.001, "{x}");
    }

    #[test]
    fn modifiers_remember_what_is_held_down() {
        let mut modifiers = Modifiers::default();
        assert!(!modifiers.any());

        modifiers.press(dom_key::SHIFT);
        assert!(modifiers.shift);
        assert!(modifiers.any());

        modifiers.press(dom_key::CONTROL);
        assert!(modifiers.control);

        modifiers.release(dom_key::SHIFT);
        assert!(!modifiers.shift);
        assert!(modifiers.control);
    }

    #[test]
    fn an_ordinary_key_does_not_become_a_modifier() {
        let mut modifiers = Modifiers::default();
        modifiers.press(65);
        assert!(!modifiers.any());
        modifiers.release(65);
        assert!(!modifiers.any());
    }

    #[test]
    fn both_shifts_are_the_same_key() {
        let mut modifiers = Modifiers::default();
        modifiers.press(dom_key::SHIFT);
        modifiers.release(dom_key::SHIFT);
        assert!(!modifiers.shift);
    }
}
