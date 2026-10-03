//! Read-only file images. Windows handles disallow writes for the mapping lifetime.
#[cfg(windows)]
use std::fs::File;
use std::{fs::OpenOptions, io, path::Path};

#[derive(Debug)]
pub struct ReadOnlyFile {
    // Keep the no-write-sharing handle alive until after the view is unmapped.
    #[cfg(windows)]
    data: memmap2::Mmap,
    #[cfg(not(windows))]
    data: Box<[u8]>,
    #[cfg(windows)]
    _file: File,
    identity: (u64, u64),
}
impl ReadOnlyFile {
    #[allow(unsafe_code)]
    pub fn open(path: &Path, maximum: u64) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Atomic replacement is allowed; in-place writes/truncation are not.
            options.share_mode(1 | 4);
        }
        let file = options.open(path)?;
        let size = file.metadata()?.len();
        if size == 0 || size > maximum {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "empty or oversized file image",
            ));
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::*;
            let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: Owned file handle and correctly sized writable output.
            if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let identity = (
                info.dwVolumeSerialNumber as u64,
                ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
            );
            // SAFETY: The retained handle denies all in-place writers, including
            // existing conflicting handles. Delete/rename preserves this file's
            // identity and contents until the mapping and handle are released.
            let data = unsafe { memmap2::MmapOptions::new().map(&file)? };
            Ok(Self {
                data,
                _file: file,
                identity,
            })
        }
        #[cfg(not(windows))]
        {
            use std::io::Read;
            let modified = file
                .metadata()?
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64);
            let mut data = Vec::new();
            file.take(maximum.saturating_add(1))
                .read_to_end(&mut data)?;
            if data.len() as u64 > maximum {
                return Err(io::Error::from(io::ErrorKind::InvalidData));
            }
            Ok(Self {
                data: data.into_boxed_slice(),
                identity: (size, modified),
            })
        }
    }
    pub fn identity(&self) -> (u64, u64) {
        self.identity
    }
}
impl AsRef<[u8]> for ReadOnlyFile {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}
