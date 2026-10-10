//! Chromium drops collapsed TSF selections when committing IME text. Forward
//! one marked arrow to its focused editor; other hosts use native selection.
use crate::tip::{lock, TipState};
use std::sync::Arc;
use windows::Win32::{
    Foundation::{E_FAIL, HWND},
    System::Threading::GetCurrentThreadId,
    UI::{Input::KeyboardAndMouse::*, TextServices::*, WindowsAndMessaging::*},
};
use windows_core::{Result, BOOL};

const MARKER: usize = 0x5254_5053;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Left,
    Right,
}

fn chromium_class(name: &str) -> bool {
    name.starts_with("Chrome_WidgetWin_") || name == "Chrome_RenderWidgetHostHWND"
}

fn focused_editor(context: &ITfContext) -> Option<HWND> {
    // SAFETY: Read-only window/TSF queries on the current apartment. Require the
    // context's window and keyboard focus to share the foreground root/window thread.
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid()
            || GetWindowThreadProcessId(foreground, None) != GetCurrentThreadId()
        {
            return None;
        }
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(GetCurrentThreadId(), &mut info).ok()?;
        let focus = info.hwndFocus;
        let window = context.GetActiveView().ok()?.GetWnd().ok()?;
        if focus.is_invalid()
            || window.is_invalid()
            || GetAncestor(focus, GA_ROOT) != foreground
            || GetAncestor(window, GA_ROOT) != foreground
        {
            return None;
        }
        let mut class = [0u16; 128];
        let len = GetClassNameW(window, &mut class);
        chromium_class(&String::from_utf16_lossy(&class[..len.max(0) as usize])).then_some(focus)
    }
}

pub(crate) fn needs_forwarding(context: &ITfContext) -> bool {
    #[cfg(test)]
    if TEST_FORWARD.with(|test| test.borrow().is_some()) {
        return true;
    }
    focused_editor(context).is_some()
}

fn keyboard(vk: VIRTUAL_KEY, up: bool, extended: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: (if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                }) | if extended {
                    KEYEVENTF_EXTENDEDKEY
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                dwExtraInfo: MARKER,
                ..Default::default()
            },
        },
    }
}

fn inputs(direction: Direction, shifts: &[VIRTUAL_KEY]) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(2 + shifts.len() * 2);
    for &shift in shifts {
        inputs.push(keyboard(shift, true, false));
    }
    let arrow = match direction {
        Direction::Left => VK_LEFT,
        Direction::Right => VK_RIGHT,
    };
    inputs.push(keyboard(arrow, false, true));
    inputs.push(keyboard(arrow, true, true));
    for &shift in shifts {
        inputs.push(keyboard(shift, false, false));
    }
    inputs
}

pub(crate) fn forward(
    state: &Arc<TipState>,
    context: &ITfContext,
    direction: Direction,
) -> Result<()> {
    #[cfg(test)]
    if TEST_FORWARD.with(|test| {
        test.borrow_mut()
            .as_mut()
            .map(|events| events.push(direction))
            .is_some()
    }) {
        return Ok(());
    }
    let Some(focus) = focused_editor(context) else {
        return Err(E_FAIL.into());
    };
    // SAFETY: Forward only a single cursor step in the current focused host.
    // Input is queued until the TSF transaction returns, after Chromium commits.
    // Release/restore only physically held Shift keys so '(' never selects text.
    unsafe {
        let down = |key: VIRTUAL_KEY| GetAsyncKeyState(i32::from(key.0)) < 0;
        if [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
            .into_iter()
            .any(down)
        {
            return Err(E_FAIL.into());
        }
        let shifts: Vec<_> = [VK_LSHIFT, VK_RSHIFT]
            .into_iter()
            .filter(|&key| down(key))
            .collect();
        let batch = inputs(direction, &shifts);
        if GetFocus() != focus {
            return Err(E_FAIL.into());
        }
        *lock(&state.symbol_cursor_context) = Some(context.clone());
        let sent = SendInput(&batch, std::mem::size_of::<INPUT>() as i32);
        if sent != batch.len() as u32 {
            // Release the arrow as well as restoring Shift. If its keydown was
            // already accepted, keep the closer: deleting it would make that
            // queued cursor step move past the opener instead of inside the pair.
            let arrow_sent = sent as usize > shifts.len();
            let arrow = if direction == Direction::Left {
                VK_LEFT
            } else {
                VK_RIGHT
            };
            let mut restore = vec![keyboard(arrow, true, true)];
            restore.extend(shifts.into_iter().map(|key| keyboard(key, false, false)));
            SendInput(&restore, std::mem::size_of::<INPUT>() as i32);
            tracing::warn!(
                sent,
                expected = batch.len(),
                "Symbol caret forwarding failed"
            );
            return if arrow_sent {
                Ok(())
            } else {
                Err(E_FAIL.into())
            };
        }
    }
    Ok(())
}

/// Bypass ordinary navigation and Shift shortcuts for our own cursor step.
/// Eat arrows if focus changed before dispatch; never move another input field.
pub(crate) fn routed_event(
    state: &TipState,
    context: Option<&ITfContext>,
    vk: usize,
) -> Option<BOOL> {
    if !matches!(vk, 0x25 | 0x27 | 0x10 | 0xa0 | 0xa1) {
        return None;
    }
    // SAFETY: The marker is read from the current keyboard message only.
    let marked = unsafe { GetMessageExtraInfo().0 as usize == MARKER };
    if !marked {
        return None;
    }
    let same =
        context.is_some_and(|context| lock(&state.symbol_cursor_context).as_ref() == Some(context));
    route(marked, state.is_activated(), same, vk).map(BOOL::from)
}

fn route(marked: bool, active: bool, same_context: bool, vk: usize) -> Option<bool> {
    if !marked || !matches!(vk, 0x25 | 0x27 | 0x10 | 0xa0 | 0xa1) {
        return None;
    }
    Some(matches!(vk, 0x25 | 0x27) && (!active || !same_context))
}

#[cfg(test)]
thread_local! {
    static TEST_FORWARD: std::cell::RefCell<Option<Vec<Direction>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(crate) fn emulate_chromium(enabled: bool) {
    TEST_FORWARD.with(|test| *test.borrow_mut() = enabled.then(Vec::new));
}
#[cfg(test)]
pub(crate) fn take_forwarded() -> Vec<Direction> {
    TEST_FORWARD.with(|test| {
        test.borrow_mut()
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwarding_is_one_step_with_shift_restored_and_every_event_marked() {
        for direction in [Direction::Left, Direction::Right] {
            let batch = inputs(direction, &[VK_LSHIFT, VK_RSHIFT]);
            // SAFETY: Every input was constructed as INPUT_KEYBOARD.
            let keys: Vec<_> = batch
                .iter()
                .map(|input| unsafe { input.Anonymous.ki })
                .collect();
            assert_eq!(keys.len(), 6);
            assert!(keys.iter().all(|key| key.dwExtraInfo == MARKER));
            assert_eq!(keys[0].wVk, VK_LSHIFT);
            assert!(keys[0].dwFlags.contains(KEYEVENTF_KEYUP));
            assert_eq!(keys[1].wVk, VK_RSHIFT);
            assert!(keys[1].dwFlags.contains(KEYEVENTF_KEYUP));
            assert_eq!(
                keys[2].wVk,
                if direction == Direction::Left {
                    VK_LEFT
                } else {
                    VK_RIGHT
                }
            );
            assert_eq!(keys[2].dwFlags, KEYEVENTF_EXTENDEDKEY);
            assert_eq!(keys[3].dwFlags, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP);
            assert_eq!(keys[4].wVk, VK_LSHIFT);
            assert_eq!(keys[5].wVk, VK_RSHIFT);
            assert_eq!(keys[4].dwFlags, KEYBD_EVENT_FLAGS(0));
            assert_eq!(keys[5].dwFlags, KEYBD_EVENT_FLAGS(0));
        }
        assert_eq!(inputs(Direction::Left, &[]).len(), 2);
    }
    #[test]
    fn only_chromium_editor_classes_use_forwarding() {
        assert!(chromium_class("Chrome_WidgetWin_1"));
        assert!(chromium_class("Chrome_RenderWidgetHostHWND"));
        assert!(!chromium_class("Edit"));
        assert!(!chromium_class("RichEditD2DPT"));
        assert!(!chromium_class("Chrome"));
    }
    #[test]
    fn internal_keys_bypass_shortcuts_and_navigation_but_arrows_cannot_follow_focus() {
        for key in [0x25, 0x27, 0x10, 0xa0, 0xa1] {
            assert_eq!(route(true, true, true, key), Some(false));
            assert_eq!(route(false, true, true, key), None);
        }
        for arrow in [0x25, 0x27] {
            assert_eq!(route(true, true, false, arrow), Some(true));
            assert_eq!(route(true, false, true, arrow), Some(true));
        }
        // Modifier restoration must still reach Windows after a focus change.
        assert_eq!(route(true, false, false, 0xa0), Some(false));
        assert_eq!(route(true, true, true, 0x41), None);
    }
}
