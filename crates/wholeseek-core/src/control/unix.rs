use super::{Caller, Request, Response, transport};
use std::fs::{self, DirBuilder};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Endpoint(PathBuf);

impl Endpoint {
    pub fn new(private_dir: &Path, namespace: &str) -> io::Result<Self> {
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
        Ok(Self(private_dir.join(format!("{namespace}.sock"))))
    }

    fn private_parent(&self, create: bool) -> io::Result<()> {
        let dir = self.0.parent().ok_or(io::ErrorKind::InvalidInput)?;
        if create {
            if let Some(parent) = dir.parent() {
                fs::create_dir_all(parent)?;
            }
            match DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        let meta = fs::symlink_metadata(dir)?;
        if !meta.is_dir() || meta.uid() != effective_uid() || meta.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "control directory is not private",
            ));
        }
        Ok(())
    }

    pub fn connect(&self) -> io::Result<Stream> {
        self.private_parent(false)?;
        let meta = fs::symlink_metadata(&self.0)?;
        if !meta.file_type().is_socket()
            || meta.uid() != effective_uid()
            || meta.mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "control socket is not private",
            ));
        }
        let socket = UnixStream::connect(&self.0)?;
        check_peer(&socket)?;
        Stream::new(socket, Duration::from_secs(60 * 60))
    }

    /// Remove only an owner socket that has no live listener. The caller
    /// must hold the cross-process session lease.
    pub fn remove_stale(&self) -> io::Result<()> {
        match self.private_parent(false) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            result => result?,
        }
        let meta = match fs::symlink_metadata(&self.0) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if !meta.file_type().is_socket() || meta.uid() != effective_uid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "will not replace non-control file",
            ));
        }
        match UnixStream::connect(&self.0) {
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "control socket is live",
            )),
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(&self.0),
            Err(e) => Err(e),
        }
    }

    pub fn bind(&self) -> io::Result<Listener> {
        self.private_parent(true)?;
        let server = UnixListener::bind(&self.0)?;
        let meta = fs::symlink_metadata(&self.0)?;
        let listener = Listener {
            server,
            path: self.0.clone(),
            identity: (meta.dev(), meta.ino()),
        };
        fs::set_permissions(&self.0, fs::Permissions::from_mode(0o600))?;
        listener.server.set_nonblocking(true)?;
        Ok(listener)
    }
}

impl Caller for Endpoint {
    fn call(&mut self, req: &Request) -> io::Result<Response> {
        transport::exchange(&mut self.connect()?, req)
    }
}

pub struct Listener {
    server: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
}

impl Listener {
    pub fn accept(&mut self) -> io::Result<Option<Stream>> {
        match self.server.accept() {
            Ok((socket, _)) => {
                check_peer(&socket)?;
                socket.set_nonblocking(false)?;
                Stream::new(socket, Duration::from_secs(5)).map(Some)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Ok(meta) = fs::symlink_metadata(&self.path)
            && (meta.dev(), meta.ino()) == self.identity
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub struct Stream(UnixStream);

impl Stream {
    fn new(socket: UnixStream, timeout: Duration) -> io::Result<Self> {
        socket.set_read_timeout(Some(timeout))?;
        socket.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(Self(socket))
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

pub(super) fn effective_uid() -> u32 {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: takes no arguments and returns the process's effective uid.
    unsafe { geteuid() }
}

fn check_peer(socket: &UnixStream) -> io::Result<()> {
    if peer_uid(socket)? != effective_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "control peer has a different uid",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn peer_uid(socket: &UnixStream) -> io::Result<u32> {
    use std::ffi::{c_int, c_void};
    #[repr(C)]
    struct Ucred {
        pid: c_int,
        uid: u32,
        gid: u32,
    }
    unsafe extern "C" {
        fn getsockopt(
            fd: c_int,
            level: c_int,
            option: c_int,
            value: *mut c_void,
            len: *mut u32,
        ) -> c_int;
    }
    const SOL_SOCKET: c_int = 1;
    const SO_PEERCRED: c_int = 17;
    let mut cred = Ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = size_of::<Ucred>() as u32;
    // SAFETY: owned live fd and writable buffer of exactly the reported size.
    if unsafe {
        getsockopt(
            socket.as_raw_fd(),
            SOL_SOCKET,
            SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut len,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if len != size_of::<Ucred>() as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected peer credential size",
        ));
    }
    Ok(cred.uid)
}

#[cfg(target_os = "macos")]
fn peer_uid(socket: &UnixStream) -> io::Result<u32> {
    use std::ffi::c_int;
    unsafe extern "C" {
        fn getpeereid(fd: c_int, euid: *mut u32, egid: *mut u32) -> c_int;
    }
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: owned live fd and two writable uid_t/gid_t outputs.
    if unsafe { getpeereid(socket.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{ErrorCode, Operation};
    use serde_json::json;
    use std::thread;
    use std::time::Instant;

    fn accept(listener: &mut Listener) -> Stream {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(stream) = listener.accept().unwrap() {
                return stream;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for connection"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn unix_round_trip_and_whitelist_rejection() {
        let tmp = tempfile::tempdir().unwrap();
        let mut endpoint = Endpoint::new(&tmp.path().join("agent"), "control").unwrap();
        let mut listener = endpoint.bind().unwrap();
        assert!(listener.accept().unwrap().is_none());
        assert_eq!(
            fs::symlink_metadata(&endpoint.0).unwrap().mode() & 0o777,
            0o600
        );
        let server = thread::spawn(move || {
            transport::serve(&mut accept(&mut listener), |request| {
                assert_eq!(request.command, Operation::Status);
                Response::success(json!({"mode": "app", "connected": true}))
            })
            .unwrap();
            transport::serve(&mut accept(&mut listener), |_| {
                panic!("forbidden request dispatched")
            })
            .unwrap();
        });
        let reply = endpoint.call(&Request::new(Operation::Status)).unwrap();
        assert_eq!(reply.data.unwrap()["mode"], "app");

        let mut stream = endpoint.connect().unwrap();
        stream.write_all(b"{\"v\":1,\"cmd\":\"play\"}\n").unwrap();
        let reply: Response = serde_json::from_reader(stream).unwrap();
        assert_eq!(reply.error.unwrap().code, ErrorCode::Forbidden);
        server.join().unwrap();
        assert!(!endpoint.0.exists());
    }

    #[test]
    fn refuses_non_private_directories_and_non_socket_files() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent");
        let endpoint = Endpoint::new(&path, "control").unwrap();
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(endpoint.bind(), Err(e) if e.kind() == io::ErrorKind::PermissionDenied));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&endpoint.0, b"not a socket").unwrap();
        assert!(
            matches!(endpoint.connect(), Err(e) if e.kind() == io::ErrorKind::PermissionDenied)
        );
        assert!(endpoint.bind().is_err());
        assert_eq!(fs::read(&endpoint.0).unwrap(), b"not a socket");
    }
}
