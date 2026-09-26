//! Win32 按键 → 内核 `Key` 的翻译层。
//!
//! 平台适配层唯一该做的事就是翻译：把 `wParam`/`GetKeyState` 变成
//! [`retype_types::InputEvent`]，剩下的全交给内核。这样 Android 端只需要写一个
//! 同样薄的 `KeyEvent → InputEvent` 翻译层，内核一行都不用改。

use retype_types::{Key, Modifiers};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, GetKeyboardState, MapVirtualKeyW, ToUnicodeEx,
    MAPVK_VK_TO_CHAR, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

/// Translate printable keys with the actual Shift/CapsLock/layout state. Bit 2
/// prevents OnTestKeyDown from modifying the OS dead-key buffer (Windows 10 1607+).
pub fn translate_event(vk: u16, scan: u32) -> Option<Key> {
    let base = translate_vk(vk);
    if base.is_some_and(|key| !matches!(key, Key::Char(_))) {
        return base;
    }
    let mut keys = [0u8; 256];
    let mut buffer = [0u16; 8];
    // SAFETY: Fixed-size buffers meet Win32 requirements; this is a read-only translation.
    let count = unsafe {
        GetKeyboardState(&mut keys).ok()?;
        ToUnicodeEx(
            vk as u32,
            scan,
            &keys,
            &mut buffer,
            4,
            Some(GetKeyboardLayout(0)),
        )
    };
    if count != 1 {
        return None;
    }
    let c = char::from_u32(buffer[0] as u32)?;
    (!c.is_control()).then_some(Key::Char(c))
}

/// 高位为 1 表示按下（`GetKeyState` 的约定）。
fn is_down(vk: u32) -> bool {
    // SAFETY: GetKeyState 只读当前线程的键盘状态，无副作用
    unsafe { (GetKeyState(vk as i32) as u16) & 0x8000 != 0 }
}

/// 读取当前修饰键状态。
///
/// 用 `GetKeyState`（同步键状态，反映本次消息时刻）而不是 `GetAsyncKeyState`
/// （实时物理状态）：后者在快速打字时会读到「已经松开」的 Shift，导致
/// 偶发地把普通字母当成 Shift+字母处理。
pub fn read_modifiers() -> Modifiers {
    let mut m = Modifiers::NONE;
    if is_down(VK_SHIFT.0 as u32) {
        m = m.union(Modifiers::SHIFT);
    }
    if is_down(VK_CONTROL.0 as u32) {
        m = m.union(Modifiers::CTRL);
    }
    if is_down(VK_MENU.0 as u32) {
        m = m.union(Modifiers::ALT);
    }
    if is_down(VK_LWIN.0 as u32) || is_down(VK_RWIN.0 as u32) {
        m = m.union(Modifiers::WIN);
    }
    m
}

/// 虚拟键码 → 抽象按键。返回 `None` 表示「这个键与输入法无关」，应直接放行。
pub fn translate_vk(vk: u16) -> Option<Key> {
    let key = match vk {
        0x08 => Key::Backspace,
        0x09 => Key::Tab,
        0x0D => Key::Enter,
        0x1B => Key::Escape,
        0x20 => Key::Space,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x23 => Key::End,
        0x24 => Key::Home,
        0x25 => Key::Left,
        0x26 => Key::Up,
        0x27 => Key::Right,
        0x28 => Key::Down,
        0x2E => Key::Delete,
        0x30..=0x39 => Key::Char(vk as u8 as char),
        0x41..=0x5A => Key::Char((vk as u8).to_ascii_lowercase() as char),
        0x70..=0x79 => Key::F((vk - 0x70 + 1) as u8),
        // 标点/符号：交给键盘布局去翻译，避免自己维护一张 US 布局表
        // （德语键盘上 `'` 的位置就不一样）
        other => {
            // SAFETY: MapVirtualKeyW 是纯查表函数
            let ch = unsafe { MapVirtualKeyW(other as u32, MAPVK_VK_TO_CHAR) } as u16;
            let c = char::from_u32(ch as u32)?;
            if c.is_ascii_graphic() || c == '\'' {
                Key::Char(c)
            } else {
                return None;
            }
        }
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn letters_map_to_lowercase_chars() {
        assert_eq!(translate_vk(0x41), Some(Key::Char('a')));
        assert_eq!(translate_vk(0x5A), Some(Key::Char('z')));
        // 大小写由 Modifiers::SHIFT 表达，Key 本身始终是小写，
        // 否则内核要为两种情况各写一遍分支
    }

    #[test]
    fn digits_and_named_keys_map() {
        assert_eq!(translate_vk(0x30), Some(Key::Char('0')));
        assert_eq!(translate_vk(0x39), Some(Key::Char('9')));
        assert_eq!(translate_vk(0x08), Some(Key::Backspace));
        assert_eq!(translate_vk(0x0D), Some(Key::Enter));
        assert_eq!(translate_vk(0x1B), Some(Key::Escape));
        assert_eq!(translate_vk(0x20), Some(Key::Space));
        assert_eq!(translate_vk(0x25), Some(Key::Left));
        assert_eq!(translate_vk(0x2E), Some(Key::Delete));
        assert_eq!(translate_vk(0x70), Some(Key::F(1)));
        assert_eq!(translate_vk(0x79), Some(Key::F(10)));
    }

    #[test]
    fn unrelated_keys_are_rejected() {
        // F17 及以上、以及各类媒体键：输入法不该插手
        assert_eq!(translate_vk(0x80), None, "VK_F17 不属于我们的映射表");
    }

    #[test]
    fn modifier_helpers_are_const_and_composable() {
        let m = Modifiers::NONE
            .union(Modifiers::SHIFT)
            .union(Modifiers::CTRL);
        assert!(m.contains(Modifiers::SHIFT));
        assert!(m.contains(Modifiers::CTRL));
        assert!(!m.contains(Modifiers::ALT));
        assert!(!m.is_plain(), "带 Ctrl 就不是普通输入");
        assert!(Modifiers::SHIFT.is_plain(), "只带 Shift 仍算普通输入");
        assert!(!m.difference(Modifiers::CTRL).contains(Modifiers::CTRL));
        assert_eq!(Modifiers::from_bits(3).bits(), 3);
    }
}
