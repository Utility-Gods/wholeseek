//! Windows local control endpoint. The actual token user SID names and owns
//! the pipe, and is the only SID granted access by its protected DACL.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use std::thread;
use std::time::{Duration, Instant};
use wholeseek_core::control::{Caller, Request, Response, transport};
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_DATA, ERROR_PIPE_CONNECTED,
    ERROR_PIPE_LISTENING, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE,
    INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, READ_CONTROL, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_NOWAIT,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, SetNamedPipeHandleState,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(10);

#[derive(Clone)]
pub struct Endpoint {
    path: String,
    sid: String,
}

impl Endpoint {
    pub fn new(namespace: &str) -> io::Result<Self> {
        if namespace.is_empty()
            || !namespace
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid control namespace",
            ));
        }
        let sid = user_sid()?;
        Ok(Self {
            path: format!(r"\\.\pipe\wholeseek-{sid}-{namespace}"),
            sid,
        })
    }

    pub fn connect(&self) -> io::Result<Stream> {
        let file = OpenOptions::new()
            .access_mode(GENERIC_READ | GENERIC_WRITE | READ_CONTROL)
            .security_qos_flags(SECURITY_IDENTIFICATION | SECURITY_SQOS_PRESENT)
            .open(&self.path)?;
        let mut owner = null_mut();
        let mut descriptor = null_mut();
        // SAFETY: file is open and both out-pointers are valid for the call.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            )
        };
        let _descriptor = LocalAllocation(descriptor);
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        if sid_string(owner)? != self.sid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "control pipe has a different owner",
            ));
        }
        let mode = PIPE_NOWAIT;
        // SAFETY: live pipe and pointer to a valid pipe read/wait mode.
        if unsafe { SetNamedPipeHandleState(file.as_raw_handle(), &mode, null(), null()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Stream {
            file,
            server: false,
            read_timeout: Duration::from_secs(3600),
        })
    }

    pub fn bind(&self) -> io::Result<Listener> {
        Ok(Listener {
            pending: self.instance(true)?,
            endpoint: self.clone(),
        })
    }

    fn instance(&self, first: bool) -> io::Result<File> {
        let path = wide(&self.path);
        let sddl = wide(&format!("O:{sid}D:P(A;;GA;;;{sid})", sid = self.sid));
        let mut desc = null_mut();
        // SAFETY: SDDL is NUL-terminated and desc is a writable output.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut desc,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let desc = LocalAllocation(desc);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: desc.0,
            bInheritHandle: 0,
        };
        let flags = PIPE_ACCESS_DUPLEX
            | if first {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                0
            };
        // SAFETY: valid NUL-terminated name and security attributes alive for this call.
        let raw = unsafe {
            CreateNamedPipeW(
                path.as_ptr(),
                flags,
                PIPE_TYPE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                65536,
                65536,
                0,
                &attributes,
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: success returned an owned handle.
        Ok(unsafe { File::from_raw_handle(raw) })
    }
}

impl Caller for Endpoint {
    fn call(&mut self, req: &Request) -> io::Result<Response> {
        transport::exchange(&mut self.connect()?, req)
    }
}

pub struct Listener {
    pending: File,
    endpoint: Endpoint,
}

impl Listener {
    pub fn accept(&mut self) -> io::Result<Option<Stream>> {
        // SAFETY: live non-overlapped, nonblocking instance; no OVERLAPPED needed.
        let connected = unsafe { ConnectNamedPipe(self.pending.as_raw_handle(), null_mut()) };
        if connected == 0 {
            let error = io::Error::last_os_error();
            match win_code(&error) {
                Some(ERROR_PIPE_LISTENING) => return Ok(None),
                Some(ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED) => {
                    // SAFETY: disconnect a vanished client to recycle this instance.
                    unsafe { DisconnectNamedPipe(self.pending.as_raw_handle()) };
                    return Ok(None);
                }
                Some(ERROR_PIPE_CONNECTED) => {}
                _ => return Err(error),
            }
        } else {
            // For PIPE_NOWAIT, a successful ConnectNamedPipe may just mean
            // the instance is ready to listen. Check again for a real client.
            // SAFETY: same live, non-overlapped instance as above.
            if unsafe { ConnectNamedPipe(self.pending.as_raw_handle(), null_mut()) } == 0 {
                let error = io::Error::last_os_error();
                if win_code(&error) == Some(ERROR_PIPE_LISTENING) {
                    return Ok(None);
                }
                if win_code(&error) != Some(ERROR_PIPE_CONNECTED) {
                    return Err(error);
                }
            }
        }
        let replacement = self.endpoint.instance(false)?;
        let file = std::mem::replace(&mut self.pending, replacement);
        Ok(Some(Stream {
            file,
            server: true,
            read_timeout: IO_TIMEOUT,
        }))
    }
}

pub struct Stream {
    file: File,
    server: bool,
    read_timeout: Duration,
}

fn win_code(error: &io::Error) -> Option<u32> {
    error.raw_os_error().map(|e| e as u32)
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "control pipe timed out")
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let deadline = Instant::now() + self.read_timeout;
        loop {
            match self.file.read(buf) {
                Err(ref e) if win_code(e) == Some(ERROR_NO_DATA) => {}
                Err(ref e)
                    if matches!(
                        win_code(e),
                        Some(ERROR_PIPE_NOT_CONNECTED | ERROR_BROKEN_PIPE)
                    ) =>
                {
                    return Ok(0);
                }
                result => return result,
            }
            if Instant::now() >= deadline {
                return Err(timed_out());
            }
            thread::sleep(POLL);
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            match self.file.write(buf) {
                Ok(0) => {}
                Err(ref e) if win_code(e) == Some(ERROR_NO_DATA) => {}
                result => return result,
            }
            if Instant::now() >= deadline {
                return Err(timed_out());
            }
            thread::sleep(POLL);
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        // Closing a pipe instance can discard unread bytes. The client
        // closes after reading the reply; wait briefly for that EOF rather
        // than using unbounded FlushFileBuffers on a stalled client.
        if self.server {
            let deadline = Instant::now() + IO_TIMEOUT;
            let mut discard = [0; 64];
            while Instant::now() < deadline {
                match self.file.read(&mut discard) {
                    Ok(0) => return Ok(()),
                    Ok(_) => {}
                    Err(ref e)
                        if matches!(
                            win_code(e),
                            Some(ERROR_PIPE_NOT_CONNECTED | ERROR_BROKEN_PIPE)
                        ) =>
                    {
                        return Ok(());
                    }
                    Err(ref e) if win_code(e) == Some(ERROR_NO_DATA) => thread::sleep(POLL),
                    Err(e) => return Err(e),
                }
            }
            return Err(timed_out());
        }
        Ok(())
    }
}

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: pointer was returned by an API allocating via LocalAlloc.
            unsafe { LocalFree(self.0) };
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text = null_mut();
    // SAFETY: caller holds the token/security descriptor buffer backing sid;
    // ConvertSidToStringSidW validates it and allocates a NUL-terminated string.
    if sid.is_null() || unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _text = LocalAllocation(text.cast());
    let mut len = 0;
    // SAFETY: the successful API call produced a readable NUL-terminated UTF-16 string.
    unsafe {
        while *text.add(len) != 0 {
            len += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(text, len)).map_err(io::Error::other)
    }
}

fn user_sid() -> io::Result<String> {
    let mut token = null_mut();
    // SAFETY: process pseudo-handle is valid and token is a writable output.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcessToken succeeded and returned an owned closable handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut needed = 0;
    // SAFETY: the documented zero-buffer sizing call only writes needed.
    let sized = unsafe {
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut needed)
    };
    let error = io::Error::last_os_error();
    if sized != 0
        || win_code(&error) != Some(ERROR_INSUFFICIENT_BUFFER)
        || (needed as usize) < size_of::<TOKEN_USER>()
    {
        return Err(io::Error::other(
            "could not size the control pipe owner SID",
        ));
    }
    let mut buf = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    // SAFETY: allocated size and alignment suit TOKEN_USER. The buffer lives
    // through the SID conversion, including the variable-length SID tail.
    unsafe {
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        sid_string((*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid)
    }
}
