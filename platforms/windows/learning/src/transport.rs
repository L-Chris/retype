//! Bounded overlapped named-pipe IO. No functions here run on a TSF callback.
#![allow(unsafe_code)]
use crate::protocol::MAX_FRAME;
use std::io;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Pipes::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::System::IO::*;

pub fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

pub struct Handle(pub HANDLE);
impl Handle {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns a valid Windows handle.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn token_sid(token: HANDLE) -> io::Result<String> {
    let mut bytes = 0;
    // SAFETY: first call only asks for the required length.
    unsafe {
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut bytes);
    }
    if bytes == 0 {
        return Err(io::Error::last_os_error());
    }
    // usize storage guarantees TOKEN_USER alignment.
    let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    let mut sid_string = null_mut();
    // SAFETY: aligned output storage is sized from GetTokenInformation. The SID remains alive
    // until it has been converted; the returned string is released with LocalFree.
    unsafe {
        if GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid_string) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut len = 0;
        while *sid_string.add(len) != 0 {
            len += 1;
        }
        let result = String::from_utf16_lossy(std::slice::from_raw_parts(sid_string, len));
        LocalFree(sid_string.cast());
        Ok(result)
    }
}
pub fn user_sid() -> io::Result<String> {
    let mut token = null_mut();
    // SAFETY: process pseudo-handle and a valid token out parameter.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    token_sid(Handle::new(token)?.0)
}
pub fn pipe_name(sid: &str) -> String {
    format!(r"\\.\pipe\retype-learning-{sid}-v1")
}

pub fn is_app_container() -> io::Result<bool> {
    let mut token = null_mut();
    // SAFETY: process pseudo-handle and a valid token out parameter.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle::new(token)?;
    let mut value = 0u32;
    let mut bytes = 0;
    // SAFETY: this token information class returns one DWORD in aligned storage.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenIsAppContainer,
            (&mut value as *mut u32).cast(),
            size_of::<u32>() as u32,
            &mut bytes,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(value != 0)
}

pub fn private_directory(path: &std::path::Path, sid: &str) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    let text = wide(&format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{sid})"));
    let mut descriptor = null_mut();
    // SAFETY: valid SDDL and owned LocalAlloc descriptor, used only during this call.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    let descriptor = Security(descriptor);
    let name = wide(&path.to_string_lossy());
    // SAFETY: nul-terminated path and validated descriptor. Preserve owner and SACL.
    if unsafe {
        SetFileSecurityW(
            name.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor.0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn read_machine_registry(name: &str) -> Option<String> {
    let key = wide(r"Software\retype");
    let name = wide(name);
    let mut buffer = vec![0u16; 32768];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: UTF-16 output storage has the advertised size; x86 reads the shared x64 view.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buffer.iter().position(|c| *c == 0)?;
    (len > 0).then(|| String::from_utf16_lossy(&buffer[..len]))
}

struct Security(*mut core::ffi::c_void);
impl Security {
    fn new(sid: &str) -> io::Result<Self> {
        // The pipe (not the database) admits AppContainers and low-integrity clients.
        // authenticate_client also checks the actual user's SID. A single writer serves
        // the same account's desktop sessions, rather than racing over the same database.
        let sddl = wide(&format!(
            "O:{sid}D:P(A;;GA;;;SY)(A;;GA;;;{sid})(A;;GRGW;;;AC)S:(ML;;NW;;;LW)"
        ));
        let mut descriptor = null_mut();
        // SAFETY: null-terminated SDDL; LocalAlloc output released by Drop.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }
}
impl Drop for Security {
    fn drop(&mut self) {
        // SAFETY: descriptor returned by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn overlapped(
    handle: &Handle,
    timeout: u32,
    start: impl FnOnce(*mut OVERLAPPED) -> i32,
) -> io::Result<u32> {
    // SAFETY: independent unnamed event owned for the entire operation.
    let event = Handle::new(unsafe { CreateEventW(null(), 1, 0, null()) })?;
    let mut operation = OVERLAPPED {
        hEvent: event.0,
        ..Default::default()
    };
    let started = start(&mut operation);
    // SAFETY: called immediately after the IO initiation on the same thread.
    let error = unsafe { GetLastError() };
    if started == 0 && error != ERROR_IO_PENDING {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    // SAFETY: the operation's event and handle remain alive through completion/cancellation.
    unsafe {
        if started == 0 && WaitForSingleObject(event.0, timeout) != WAIT_OBJECT_0 {
            CancelIoEx(handle.0, &operation);
            let mut ignored = 0;
            // Drain cancellation before dropping the OVERLAPPED or its caller's buffer.
            GetOverlappedResult(handle.0, &operation, &mut ignored, 1);
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "learning pipe timed out",
            ));
        }
        let mut bytes = 0;
        if GetOverlappedResult(handle.0, &operation, &mut bytes, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(bytes)
    }
}

fn write_all(pipe: &Handle, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let count = overlapped(pipe, 2000, |op| {
            // SAFETY: slice and OVERLAPPED live until overlapped() drains completion.
            unsafe { WriteFile(pipe.0, bytes.as_ptr(), bytes.len() as u32, null_mut(), op) }
        })? as usize;
        if count == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        bytes = &bytes[count..];
    }
    Ok(())
}
fn read_exact(pipe: &Handle, mut bytes: &mut [u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let count = overlapped(pipe, 2000, |op| {
            // SAFETY: output slice and OVERLAPPED live until completion.
            unsafe {
                ReadFile(
                    pipe.0,
                    bytes.as_mut_ptr(),
                    bytes.len() as u32,
                    null_mut(),
                    op,
                )
            }
        })? as usize;
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        bytes = &mut bytes[count..];
    }
    Ok(())
}
pub fn write_frame(pipe: &Handle, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX_FRAME {
        return Err(io::ErrorKind::InvalidData.into());
    }
    write_all(pipe, &(bytes.len() as u32).to_le_bytes())?;
    write_all(pipe, bytes)
}
pub fn read_frame(pipe: &Handle) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    read_exact(pipe, &mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut bytes = vec![0; length];
    read_exact(pipe, &mut bytes)?;
    Ok(bytes)
}
pub fn exchange(name: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
    exchange_inner(name, bytes, false)
}

/// Explicit user action only: let the authenticated desktop broker bring the
/// requested settings window forward. All pipe IO still runs on a worker.
pub fn exchange_with_foreground(name: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
    exchange_inner(name, bytes, true)
}

fn exchange_inner(name: &str, bytes: &[u8], foreground: bool) -> io::Result<Vec<u8>> {
    let name = wide(name);
    // Identification only: a server cannot impersonate this client to access its files.
    // SAFETY: valid nul-terminated name; returned handle exclusively owned below.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let pipe = loop {
        // SAFETY: name is valid and returned handles are immediately owned or rejected.
        let opened = Handle::new(unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                null_mut(),
            )
        });
        match opened {
            Ok(pipe) => break pipe,
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && std::time::Instant::now() < deadline =>
            {
                // SAFETY: bounded wait for this pipe only, on the client's background worker.
                unsafe {
                    WaitNamedPipeW(name.as_ptr(), 50);
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(error) => return Err(error),
        }
    };
    authenticate_server(&pipe)?;
    if foreground {
        let mut pid = 0;
        // SAFETY: the connected pipe's owner has been authenticated above. Grant
        // foreground permission to this one server, never to arbitrary processes.
        unsafe {
            if GetNamedPipeServerProcessId(pipe.0, &mut pid) != 0 {
                windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid);
            }
        }
    }
    write_frame(&pipe, bytes)?;
    let response = read_frame(&pipe)?;
    write_all(&pipe, &[1])?;
    Ok(response)
}

fn authenticate_server(pipe: &Handle) -> io::Result<()> {
    let mut owner = null_mut();
    let mut descriptor = null_mut();
    // SAFETY: valid connected handle; GetSecurityInfo allocates an owned descriptor.
    let error = unsafe {
        GetSecurityInfo(
            pipe.0,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if error != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let _descriptor = Security(descriptor);
    let expected = user_sid()?;
    let expected = wide(&expected);
    let mut expected_sid = null_mut();
    // SAFETY: validated user SID spelling and LocalAlloc storage returned by the conversion.
    if unsafe { ConvertStringSidToSidW(expected.as_ptr(), &mut expected_sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _expected_sid = Security(expected_sid);
    // The broker explicitly assigns its owner to TokenUser, also when started elevated.
    // SAFETY: both SIDs are valid for this scope and their storage is kept alive.
    if owner.is_null() || unsafe { EqualSid(owner, expected_sid) } == 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

pub struct Listener {
    pipe: Handle,
}
impl Listener {
    pub fn new(name: &str, sid: &str) -> io::Result<Self> {
        let descriptor = Security::new(sid)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        let name = wide(name);
        // Hold this one instance throughout the broker lifetime, preventing a second writer.
        // SAFETY: name and SECURITY_ATTRIBUTES live through the call; Windows copies the ACL.
        let pipe = Handle::new(unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                65536,
                65536,
                100,
                &attributes,
            )
        })?;
        Ok(Self { pipe })
    }
    pub fn receive(&self, sid: &str) -> io::Result<Vec<u8>> {
        let connected = overlapped(&self.pipe, 500, |op| {
            // SAFETY: owned pipe; operation lives until connected or cancellation drains.
            unsafe { ConnectNamedPipe(self.pipe.0, op) }
        });
        if connected.is_err()
            && connected.as_ref().err().and_then(io::Error::raw_os_error)
                != Some(ERROR_PIPE_CONNECTED as i32)
        {
            return connected.map(|_| Vec::new());
        }
        // A byte must be read before ImpersonateNamedPipeClient can authenticate a new client.
        let bytes = read_frame(&self.pipe)?;
        self.authenticate_client(sid)?;
        Ok(bytes)
    }
    fn authenticate_client(&self, sid: &str) -> io::Result<()> {
        // SAFETY: the pipe is connected and a request has been read from this client.
        unsafe {
            if ImpersonateNamedPipeClient(self.pipe.0) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                // SAFETY: this thread impersonated solely to inspect the client's identity.
                unsafe {
                    RevertToSelf();
                }
            }
        }
        let _revert = Revert;
        let mut token = null_mut();
        // SAFETY: query the impersonation token; OpenAsSelf avoids low-integrity token access restrictions.
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if token_sid(Handle::new(token)?.0)? != sid {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }
    pub fn respond(&self, bytes: &[u8]) -> io::Result<()> {
        write_frame(&self.pipe, bytes)?;
        let mut acknowledgement = [0];
        // Do not disconnect until the peer consumed the response. FlushFileBuffers would
        // block without a timeout if a client hung; this handshake is bounded instead.
        read_exact(&self.pipe, &mut acknowledgement)
    }
    pub fn disconnect(&self) {
        // SAFETY: reusable server pipe owned by this listener; failed disconnect is harmless.
        unsafe {
            DisconnectNamedPipe(self.pipe.0);
        }
    }
}
