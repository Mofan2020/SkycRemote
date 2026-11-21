//! The icon in the corner of the screen.
//!
//! Closing the window does not stop the program: the share keeps
//! running, and the program keeps the tracker going.  This icon is the
//! way back to the window, and the way to stop the program for good.
//!
//! There is no such icon on Linux: there the program stops when the
//! window is closed, the way it always did.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What a person did with the icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Bring the window back.
    Show,
    /// Stop the program.
    Quit,
    /// Nothing that matters.
    Nothing,
}

/// The icon in the corner, with the two things a person can do.
pub struct Tray {
    /// The icon itself.  It has to be kept: when it is let go, the
    /// icon goes away with it.
    _icon: TrayIcon,
    /// The item that brings the window back.
    show: MenuId,
    /// The item that stops the program.
    quit: MenuId,
}

impl Tray {
    /// Put the icon in the corner.
    ///
    /// It has to be made while the event loop is running, so this is
    /// called from inside the loop.
    pub fn make() -> Option<Self> {
        let show = MenuItem::new("显示 SkycRemote", true, None);
        let quit = MenuItem::new("退出 SkycRemote", true, None);
        let menu = Menu::new();
        let _ = menu.append(&show);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&quit);

        let icon = TrayIconBuilder::new()
            .with_icon_templated(bolt())
            .with_menu(Box::new(menu))
            .with_tooltip("SkycRemote")
            .with_menu_on_left_click(false)
            .build()
            .ok()?;

        Some(Tray {
            _icon: icon,
            show: show.id().clone(),
            quit: quit.id().clone(),
        })
    }

    /// What a person did with the icon itself.
    pub fn action(&self, event: &TrayIconEvent) -> Action {
        match event {
            TrayIconEvent::Click { .. } => Action::Show,
            _ => Action::Nothing,
        }
    }

    /// What a person did with the menu of the icon.
    pub fn menu_action(&self, event: &MenuEvent) -> Action {
        let id = event.id();
        if id == &self.show {
            Action::Show
        } else if id == &self.quit {
            Action::Quit
        } else {
            Action::Nothing
        }
    }

    /// Say that the program is running in the background.
    pub fn set_background_note(&self, running: bool) {
        let note = if running {
            "SkycRemote（正在后台运行）"
        } else {
            "SkycRemote"
        };
        let _ = self._icon.set_tooltip(Some(note));
    }
}

/// The little picture: a lightning bolt, drawn by hand.
///
/// A picture in a file would have to be carried along and read at
/// run time; a shape with a few points is a few lines of arithmetic
/// and nothing to keep.
fn bolt() -> Icon {
    const SIZE: u32 = 32;

    // The points of the bolt, as parts of the picture.
    let points = [
        (0.58, 0.04),
        (0.24, 0.55),
        (0.45, 0.55),
        (0.40, 0.96),
        (0.74, 0.43),
        (0.53, 0.43),
    ];

    let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let px = (x as f32 + 0.5) / SIZE as f32;
            let py = (y as f32 + 0.5) / SIZE as f32;
            if inside(&points, px, py) {
                let at = ((y * SIZE + x) * 4) as usize;
                pixels[at] = 0x1d;
                pixels[at + 1] = 0x1d;
                pixels[at + 2] = 0x1f;
                pixels[at + 3] = 0xff;
            }
        }
    }

    Icon::from_rgba(pixels, SIZE, SIZE).unwrap_or_else(|_| {
        // A picture that cannot be made is a picture of nothing: the
        // program still works, there is just no icon.
        Icon::from_rgba(vec![0u8; 4], 1, 1).expect("an empty picture is always possible")
    })
}

/// Whether the point is inside the shape.
fn inside(points: &[(f32, f32); 6], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}
