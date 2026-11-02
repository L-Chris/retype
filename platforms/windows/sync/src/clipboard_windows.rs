//! Short native clipboard transactions. Encoding and synchronization run on owned bytes.
#![allow(unsafe_code)]
use windows_sys::Win32::{
    Foundation::*,
    System::{DataExchange::*, Memory::*},
};

pub(super) struct Clipboard {
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Clipboard {
    pub(super) fn open(window: HWND) -> Option<Self> {
        if window.is_null() {
            return None;
        }
        // SAFETY: The calling thread owns the matching CloseClipboard guard.
        if unsafe { OpenClipboard(window) } != 0 {
            Some(Self {
                _thread: std::marker::PhantomData,
            })
        } else {
            None
        }
    }
    pub(super) fn bytes(&self, format: u32, limit: usize) -> Option<Vec<u8>> {
        // SAFETY: Clipboard remains open until the bounded global-memory copy ends.
        unsafe {
            if IsClipboardFormatAvailable(format) == 0 {
                return None;
            }
            let handle = GetClipboardData(format);
            if handle.is_null() {
                return None;
            }
            let size = GlobalSize(handle);
            if size == 0 || size > limit {
                return None;
            }
            let pointer = GlobalLock(handle).cast::<u8>();
            if pointer.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(pointer, size).to_vec();
            GlobalUnlock(handle);
            Some(bytes)
        }
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

pub(super) enum Read {
    Busy,
    Stale,
    Data(Option<Vec<u8>>, u32),
}
pub(super) fn read(window: HWND, sequence: u32, excluded: u32, format: u32, limit: usize) -> Read {
    let Some(clipboard) = Clipboard::open(window) else {
        return Read::Busy;
    };
    // SAFETY: These queries run inside the short native transaction. Returning
    // owned bytes drops the guard before the caller can decode or synchronize.
    if unsafe {
        GetClipboardSequenceNumber() != sequence || IsClipboardFormatAvailable(excluded) != 0
    } {
        return Read::Stale;
    }
    let bytes = clipboard.bytes(format, limit);
    // Delayed rendering can advance the sequence while this thread still owns
    // the clipboard; that is the same copy, not a newer local operation.
    Read::Data(bytes, unsafe { GetClipboardSequenceNumber() })
}

pub(super) struct Prepared {
    allocations: Vec<(u32, HGLOBAL)>,
}
impl Prepared {
    pub(super) fn new(formats: &[(u32, Vec<u8>)]) -> Option<Self> {
        if formats.is_empty() {
            return None;
        }
        let mut prepared = Self {
            allocations: Vec::with_capacity(formats.len()),
        };
        // SAFETY: These allocations belong to this value until SetClipboardData
        // succeeds. No clipboard is opened while allocating/copying image bytes.
        unsafe {
            for (format, bytes) in formats {
                let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
                if handle.is_null() {
                    return None;
                }
                prepared.allocations.push((*format, handle));
                let pointer = GlobalLock(handle).cast::<u8>();
                if pointer.is_null() {
                    return None;
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
                GlobalUnlock(handle);
            }
        }
        Some(prepared)
    }
    pub(super) fn apply(&mut self, window: HWND, expected: u32) -> Option<u32> {
        let _clipboard = Clipboard::open(window)?;
        // SAFETY: Only native publication occurs under the clipboard guard.
        // A local copy during preparation wins, and successful handles transfer
        // to Windows. Capture our sequence before releasing the native lock.
        unsafe {
            if GetClipboardSequenceNumber() != expected || EmptyClipboard() == 0 {
                return None;
            }
            let mut applied = false;
            for (format, handle) in &mut self.allocations {
                if !handle.is_null() && !SetClipboardData(*format, *handle).is_null() {
                    *handle = std::ptr::null_mut();
                    applied = true;
                }
            }
            applied.then(|| GetClipboardSequenceNumber())
        }
    }
}
impl Drop for Prepared {
    fn drop(&mut self) {
        // SAFETY: Transferred handles are null; every remaining handle is ours.
        unsafe {
            for (_, handle) in &self.allocations {
                if !handle.is_null() {
                    GlobalFree(*handle);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct ProbeWindow(HWND);
    impl ProbeWindow {
        fn new() -> Self {
            use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, HWND_MESSAGE};
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: Built-in class, private message-only window, no desktop UI.
            let window = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    class.as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            assert!(!window.is_null(), "Create clipboard probe window");
            Self(window)
        }
    }
    impl Drop for ProbeWindow {
        fn drop(&mut self) {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
            }
        }
    }
    fn other_thread_can_open() -> bool {
        std::thread::spawn(|| {
            let window = ProbeWindow::new();
            let held = Clipboard::open(window.0);
            let opened = held.is_some();
            drop(held);
            opened
        })
        .join()
        .unwrap_or(false)
    }
    #[test]
    fn only_the_native_transaction_holds_the_clipboard() {
        let window = ProbeWindow::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let held = loop {
            if let Some(held) = Clipboard::open(window.0) {
                break held;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Clipboard remained unavailable"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(!other_thread_can_open());
        drop(held);
        assert!(other_thread_can_open());
        let prepared = Prepared::new(&[(13, vec![0; 1024 * 1024])]);
        assert!(prepared.is_some());
        assert!(
            other_thread_can_open(),
            "Preparing outgoing bytes must not lock the clipboard"
        );
        drop(prepared);
    }
}
