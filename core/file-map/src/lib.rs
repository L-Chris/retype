//! Read-only file images. Windows handles disallow writes for the mapping lifetime.
#[cfg(any(windows, target_os = "android"))]
use std::fs::File;
use std::{fs::OpenOptions, io, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileIdentity {
    #[cfg(windows)]
    Windows(u64, u64),
    #[cfg(target_os = "android")]
    Android(u64, u64, u64, i64, i64),
    #[cfg(not(any(windows, target_os = "android")))]
    Snapshot([u8; 32]),
}

#[derive(Debug)]
pub struct ReadOnlyFile {
    // Keep the no-write-sharing handle alive until after the view is unmapped.
    #[cfg(any(windows, target_os = "android"))]
    data: memmap2::Mmap,
    #[cfg(not(any(windows, target_os = "android")))]
    data: Box<[u8]>,
    #[cfg(any(windows, target_os = "android"))]
    _file: File,
    identity: FileIdentity,
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
            let identity = FileIdentity::Windows(
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
        #[cfg(target_os = "android")]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = file.metadata()?;
            if meta.mode() & 0o222 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Android dictionary images must be read-only and replaced atomically",
                ));
            }
            let identity = FileIdentity::Android(
                meta.dev(),
                meta.ino(),
                size,
                meta.mtime(),
                meta.mtime_nsec(),
            );
            // SAFETY: Android images are app-private, chmod read-only before opening,
            // and our installer only publishes new inodes via atomic rename. No writer
            // truncates or overwrites a mapped image. Other Unix callers use snapshots.
            let data = unsafe { memmap2::MmapOptions::new().map(&file)? };
            Ok(Self {
                data,
                _file: file,
                identity,
            })
        }
        #[cfg(not(any(windows, target_os = "android")))]
        {
            use sha2::{Digest, Sha256};
            use std::io::Read;
            let mut data = Vec::new();
            file.take(maximum.saturating_add(1))
                .read_to_end(&mut data)?;
            if data.len() as u64 > maximum {
                return Err(io::Error::from(io::ErrorKind::InvalidData));
            }
            // Snapshot bytes, rather than size/timestamps, identify the loaded
            // generation: replacement or in-place writes can preserve both.
            let identity = FileIdentity::Snapshot(Sha256::digest(&data).into());
            Ok(Self {
                data: data.into_boxed_slice(),
                identity,
            })
        }
    }
    pub fn identity(&self) -> FileIdentity {
        self.identity
    }
}
impl AsRef<[u8]> for ReadOnlyFile {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}
