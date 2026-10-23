//! Read-only file images. Windows handles disallow writes for the mapping lifetime.
#[cfg(any(windows, target_os = "android", target_os = "macos"))]
use std::fs::File;
use std::{fs::OpenOptions, io, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileIdentity {
    #[cfg(windows)]
    Windows(u64, u64),
    #[cfg(any(target_os = "android", target_os = "macos"))]
    ImmutableUnix(u64, u64, u64, i64, i64),
    #[cfg(not(any(windows, target_os = "android")))]
    Snapshot([u8; 32]),
}

#[derive(Debug)]
enum Image {
    #[cfg(any(windows, target_os = "android", target_os = "macos"))]
    Mapped(memmap2::Mmap),
    #[cfg(not(any(windows, target_os = "android")))]
    Snapshot(Box<[u8]>),
}

#[derive(Debug)]
pub struct ReadOnlyFile {
    data: Image,
    // Keep the no-write-sharing handle alive until after the view is unmapped.
    #[cfg(any(windows, target_os = "android", target_os = "macos"))]
    _file: Option<File>,
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
                data: Image::Mapped(data),
                _file: Some(file),
                identity,
            })
        }
        #[cfg(any(target_os = "android", target_os = "macos"))]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = file.metadata()?;
            if meta.mode() & 0o222 == 0 {
                let identity = FileIdentity::ImmutableUnix(
                    meta.dev(),
                    meta.ino(),
                    size,
                    meta.mtime(),
                    meta.mtime_nsec(),
                );
                // SAFETY: Android/macOS images are app-private, chmod read-only before opening,
                // and our installer only publishes new inodes via atomic rename. No writer
                // truncates or overwrites a mapped image. Other Unix callers use snapshots.
                let data = unsafe { memmap2::MmapOptions::new().map(&file)? };
                return Ok(Self {
                    data: Image::Mapped(data),
                    _file: Some(file),
                    identity,
                });
            }
            #[cfg(target_os = "android")]
            {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Dictionary images must be read-only and replaced atomically",
                ))
            }
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
                data: Image::Snapshot(data.into_boxed_slice()),
                #[cfg(target_os = "macos")]
                _file: None,
                identity,
            })
        }
    }
    pub fn identity(&self) -> FileIdentity {
        self.identity
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn immutable_mapping_survives_atomic_replacement_and_mutable_files_use_snapshots(
    ) -> io::Result<()> {
        let root = std::env::temp_dir().join(format!("retype-map-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("image.bin");
        std::fs::write(&path, b"first")?;
        let mutable = ReadOnlyFile::open(&path, 1024)?;
        assert!(matches!(mutable.identity(), FileIdentity::Snapshot(_)));
        std::fs::write(&path, b"other")?;
        assert_eq!(mutable.as_ref(), b"first");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400))?;
        let mapped = ReadOnlyFile::open(&path, 1024)?;
        assert!(matches!(mapped.identity(), FileIdentity::ImmutableUnix(..)));
        let replacement = root.join("next.bin");
        std::fs::write(&replacement, b"third")?;
        std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o400))?;
        std::fs::rename(&replacement, &path)?;
        let current = ReadOnlyFile::open(&path, 1024)?;
        assert_ne!(mapped.identity(), current.identity());
        assert_eq!(mapped.as_ref(), b"other");
        assert_eq!(current.as_ref(), b"third");
        drop((mapped, current));
        std::fs::remove_file(&path)?;
        std::fs::remove_dir(root)?;
        Ok(())
    }
}
impl AsRef<[u8]> for ReadOnlyFile {
    fn as_ref(&self) -> &[u8] {
        match &self.data {
            #[cfg(any(windows, target_os = "android", target_os = "macos"))]
            Image::Mapped(data) => data,
            #[cfg(not(any(windows, target_os = "android")))]
            Image::Snapshot(data) => data,
        }
    }
}
