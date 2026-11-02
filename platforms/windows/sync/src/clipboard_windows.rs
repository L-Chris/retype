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
    fn other_thread_can_open() -> bool {
        std::thread::spawn(|| Clipboard::open(std::ptr::null_mut()).is_some())
            .join()
            .unwrap_or(false)
    }
    #[test]
    fn only_the_native_transaction_holds_the_clipboard() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let held = loop {
            if let Some(held) = Clipboard::open(std::ptr::null_mut()) {
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
