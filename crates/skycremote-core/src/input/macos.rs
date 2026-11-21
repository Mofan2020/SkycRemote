//! macOS 的输入注入。
//!
//! 用的是 CoreGraphics 的事件接口 —— 系统自己的键盘和鼠标最终也是
//! 从这里进来的，所以注入进去的东西和真人按的没有区别。
//!
//! **这里需要辅助功能权限。** macOS 从 10.14 起，往系统里塞键盘
//! 鼠标事件必须先拿到这个权限，否则事件会被安静地丢掉：不报错，也
//! 不动，什么都看不出来。被控端第一次运行时要向用户申请。
//!
//! 字符和按键走的是两条路。字符用 Unicode 送过去，跟被控端摆的是
//! 什么键盘没有关系；按键按物理键位送，只有方向键、回车这些没有
//! 字符的键才走这条路。

use core::ffi::c_void;
use std::ffi::c_int;

use crate::capture::Screen;
use crate::protocol::InputEvent;

use super::{dom_key, global_position, Injector, InputError, Modifiers};

/// CoreGraphics 的事件对象。
type CGEventRef = *mut c_void;
/// CoreGraphics 的事件来源。
type CGEventSourceRef = *mut c_void;

/// 一个位置。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

extern "C" {
    fn CGEventSourceCreate(state_id: c_int) -> CGEventSourceRef;
    fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        keycode: u16,
        keydown: bool,
    ) -> CGEventRef;
    fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        mouse_type: u32,
        location: CGPoint,
        button: u32,
    ) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent(
        source: CGEventSourceRef,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;
    fn CGEventSetLocation(event: CGEventRef, location: CGPoint);
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventKeyboardSetUnicodeString(event: CGEventRef, length: usize, string: *const u16);
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CFRelease(cf: *const c_void);
}

/// 从硬件层发出去的事件。
///
/// 这一层在系统里最靠里，注入进去的东西和真的按键盘一样，别的程序
/// 分不出来。
const EVENT_TAP_HID: u32 = 0;

/// 鼠标移动。
const EVENT_MOUSE_MOVED: u32 = 5;
/// 左键按下。
const EVENT_LEFT_MOUSE_DOWN: u32 = 1;
/// 左键松开。
const EVENT_LEFT_MOUSE_UP: u32 = 2;
/// 右键按下。
const EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
/// 右键松开。
const EVENT_RIGHT_MOUSE_UP: u32 = 4;
/// 中键按下。
const EVENT_OTHER_MOUSE_DOWN: u32 = 25;
/// 中键松开。
const EVENT_OTHER_MOUSE_UP: u32 = 26;

/// Shift。
const FLAG_SHIFT: u64 = 0x0002_0000;
/// Control。
const FLAG_CONTROL: u64 = 0x0004_0000;
/// Option。
const FLAG_ALTERNATE: u64 = 0x0008_0000;
/// Command。
const FLAG_COMMAND: u64 = 0x0010_0000;

/// 滚轮按行算，不按像素。
/// 滚轮按像素走，不按行。
///
/// 按行的话一格滚轮会变成一大段；控制端送来的本来就是像素量，
/// 直接按像素投进去，手感和本机摸到的一致。
const SCROLL_UNITS_PIXEL: u32 = 0;

/// DOM 的键码 → macOS 的虚拟键码。
///
/// 只有那些没有字符的键在这里。字母和数字走字符那条路，因为按物理
/// 键位会被对面键盘的布局影响 —— 被控端摆的是中文键盘的话，按 'a'
/// 就打不出 a。
fn virtual_key(code: u32) -> Option<u16> {
    Some(match code {
        dom_key::BACKSPACE => 51,
        dom_key::TAB => 48,
        dom_key::ENTER => 36,
        dom_key::SHIFT => 56,
        dom_key::CONTROL => 59,
        dom_key::ALT => 58,
        dom_key::CAPS_LOCK => 57,
        dom_key::ESCAPE => 53,
        dom_key::SPACE => 49,
        dom_key::PAGE_UP => 116,
        dom_key::PAGE_DOWN => 121,
        dom_key::END => 119,
        dom_key::HOME => 115,
        dom_key::ARROW_LEFT => 123,
        dom_key::ARROW_UP => 126,
        dom_key::ARROW_RIGHT => 124,
        dom_key::ARROW_DOWN => 125,
        dom_key::DELETE => 117,
        dom_key::META => 55,
        dom_key::F1 => 122,
        dom_key::F2 => 120,
        dom_key::F3 => 99,
        dom_key::F4 => 118,
        dom_key::F5 => 96,
        dom_key::F6 => 97,
        dom_key::F7 => 98,
        dom_key::F8 => 100,
        dom_key::F9 => 101,
        dom_key::F10 => 109,
        dom_key::F11 => 103,
        dom_key::F12 => 111,
        _ => return None,
    })
}

/// 在这台 Mac 上注入输入。
pub struct MacosInjector {
    /// 事件的来源。
    ///
    /// 留着是为了让系统知道这些事件是从哪来的；没有它，注入的事件
    /// 会被当成不知道来源的输入，有些程序会不收。
    source: CGEventSourceRef,
    /// 按住不放的修饰键。
    modifiers: Modifiers,
}

impl MacosInjector {
    /// 做好一个注入器。
    pub fn new() -> Result<Self, InputError> {
        // The state of the whole system, so that the events carry the
        // same modifiers the real keyboard is in.
        let source = unsafe { CGEventSourceCreate(1) };
        if source.is_null() {
            return Err(InputError::Refused(
                "the system would not give out an event source".to_string(),
            ));
        }
        Ok(Self {
            source,
            modifiers: Modifiers::default(),
        })
    }

    /// 按住不放的那些键，现在是什么状态。
    fn flags(&self) -> u64 {
        let mut flags = 0;
        if self.modifiers.shift {
            flags |= FLAG_SHIFT;
        }
        if self.modifiers.control {
            flags |= FLAG_CONTROL;
        }
        if self.modifiers.alt {
            flags |= FLAG_ALTERNATE;
        }
        if self.modifiers.command {
            flags |= FLAG_COMMAND;
        }
        flags
    }

    /// 发一个键盘事件。
    fn post_key(&self, keycode: u16, down: bool) -> Result<(), InputError> {
        unsafe {
            let event = CGEventCreateKeyboardEvent(self.source, keycode, down);
            if event.is_null() {
                return Err(InputError::Refused(
                    "the keyboard event could not be made".to_string(),
                ));
            }
            CGEventSetFlags(event, self.flags());
            CGEventPost(EVENT_TAP_HID, event);
            CFRelease(event as *const c_void);
        }
        Ok(())
    }

    /// 发一段字符。
    ///
    /// 字符送过去之后，对面摆的是什么键盘就不重要了。
    fn post_text(&self, text: &str) -> Result<(), InputError> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        if utf16.is_empty() {
            return Ok(());
        }
        unsafe {
            // The key code is left at zero: there is no key that means
            // this character.  The characters ride along in the event
            // itself.
            let down = CGEventCreateKeyboardEvent(self.source, 0, true);
            if down.is_null() {
                return Err(InputError::Refused(
                    "the keyboard event could not be made".to_string(),
                ));
            }
            CGEventSetFlags(down, self.flags());
            CGEventKeyboardSetUnicodeString(down, utf16.len(), utf16.as_ptr());
            CGEventPost(EVENT_TAP_HID, down);
            CFRelease(down as *const c_void);

            let up = CGEventCreateKeyboardEvent(self.source, 0, false);
            if up.is_null() {
                return Err(InputError::Refused(
                    "the keyboard event could not be made".to_string(),
                ));
            }
            CGEventPost(EVENT_TAP_HID, up);
            CFRelease(up as *const c_void);
        }
        Ok(())
    }

    /// 发一个鼠标事件。
    fn post_mouse(
        &self,
        mouse_type: u32,
        position: CGPoint,
        button: u32,
    ) -> Result<(), InputError> {
        unsafe {
            let event = CGEventCreateMouseEvent(self.source, mouse_type, position, button);
            if event.is_null() {
                return Err(InputError::Refused(
                    "the mouse event could not be made".to_string(),
                ));
            }
            CGEventSetLocation(event, position);
            CGEventPost(EVENT_TAP_HID, event);
            CFRelease(event as *const c_void);
        }
        Ok(())
    }
}

// The event source is a CoreFoundation object, so the compiler cannot
// know it is safe to move between threads.  It is: Apple says an event
// source may be used from any thread, and the session runs its input
// on whichever thread the network happens to be on.
unsafe impl Send for MacosInjector {}

impl Injector for MacosInjector {
    fn inject(&mut self, event: &InputEvent, screen: &Screen) -> Result<(), InputError> {
        match event {
            InputEvent::KeyDown { key } => {
                // The modifier is remembered before the key is sent, so
                // that the key itself already carries it.
                self.modifiers.press(*key);
                let keycode = virtual_key(*key).ok_or(InputError::UnknownKey(*key))?;
                self.post_key(keycode, true)
            }
            InputEvent::KeyUp { key } => {
                let keycode = virtual_key(*key).ok_or(InputError::UnknownKey(*key))?;
                self.post_key(keycode, false)?;
                self.modifiers.release(*key);
                Ok(())
            }
            InputEvent::Text { text } => self.post_text(text),
            InputEvent::MouseMove { x, y } => {
                let (gx, gy) = global_position(screen, *x, *y);
                self.post_mouse(EVENT_MOUSE_MOVED, CGPoint { x: gx, y: gy }, 0)
            }
            InputEvent::MouseDown { button, x, y } => {
                let (gx, gy) = global_position(screen, *x, *y);
                let position = CGPoint { x: gx, y: gy };
                let mouse_type = match button {
                    1 => EVENT_OTHER_MOUSE_DOWN,
                    2 => EVENT_RIGHT_MOUSE_DOWN,
                    _ => EVENT_LEFT_MOUSE_DOWN,
                };
                self.post_mouse(mouse_type, position, u32::from(*button))
            }
            InputEvent::MouseUp { button, x, y } => {
                let (gx, gy) = global_position(screen, *x, *y);
                let position = CGPoint { x: gx, y: gy };
                let mouse_type = match button {
                    1 => EVENT_OTHER_MOUSE_UP,
                    2 => EVENT_RIGHT_MOUSE_UP,
                    _ => EVENT_LEFT_MOUSE_UP,
                };
                self.post_mouse(mouse_type, position, u32::from(*button))
            }
            InputEvent::Scroll { dx, dy, x, y } => {
                let (gx, gy) = global_position(screen, *x, *y);
                unsafe {
                    let event = CGEventCreateScrollWheelEvent(
                        self.source,
                        SCROLL_UNITS_PIXEL,
                        2,
                        *dy as i32,
                        *dx as i32,
                        0,
                    );
                    if event.is_null() {
                        return Err(InputError::Refused(
                            "the scroll event could not be made".to_string(),
                        ));
                    }
                    CGEventSetLocation(event, CGPoint { x: gx, y: gy });
                    CGEventPost(EVENT_TAP_HID, event);
                    CFRelease(event as *const c_void);
                }
                Ok(())
            }
            InputEvent::Clipboard { .. } => Err(InputError::Unsupported),
        }
    }
}

impl Drop for MacosInjector {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.source as *const c_void);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_special_keys_all_have_a_place_on_this_keyboard() {
        // Every key that has no character must be in the table, or it
        // would be dropped on the way in.
        for code in [
            dom_key::BACKSPACE,
            dom_key::TAB,
            dom_key::ENTER,
            dom_key::SHIFT,
            dom_key::CONTROL,
            dom_key::ALT,
            dom_key::ESCAPE,
            dom_key::PAGE_UP,
            dom_key::PAGE_DOWN,
            dom_key::END,
            dom_key::HOME,
            dom_key::ARROW_LEFT,
            dom_key::ARROW_UP,
            dom_key::ARROW_RIGHT,
            dom_key::ARROW_DOWN,
            dom_key::DELETE,
            dom_key::META,
            dom_key::F1,
            dom_key::F12,
        ] {
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
        for code in [
            dom_key::BACKSPACE,
            dom_key::TAB,
            dom_key::ENTER,
            dom_key::SHIFT,
            dom_key::CONTROL,
            dom_key::ALT,
            dom_key::ESCAPE,
            dom_key::PAGE_UP,
            dom_key::PAGE_DOWN,
            dom_key::END,
            dom_key::HOME,
            dom_key::ARROW_LEFT,
            dom_key::ARROW_UP,
            dom_key::ARROW_RIGHT,
            dom_key::ARROW_DOWN,
            dom_key::DELETE,
            dom_key::META,
        ] {
            let keycode = virtual_key(code).expect("the key is in the table");
            if let Some(other) = seen.insert(keycode, code) {
                panic!("the key {code} and the key {other} share the place {keycode}");
            }
        }
    }

    #[test]
    fn a_key_that_is_not_in_the_table_is_said_so() {
        assert!(virtual_key(65).is_none(), "a letter has no place here");
    }
}
