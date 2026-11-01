use std::io::{self, Read, Write};
use std::mem::size_of;
use std::ptr::null_mut;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSidToSidW, SetEntriesInAclW, EXPLICIT_ACCESS_W, GRANT_ACCESS, TRUSTEE_IS_SID,
    TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows::Win32::Security::{
    InitializeSecurityDescriptor, SetSecurityDescriptorDacl, ACE_FLAGS, ACL, PSECURITY_DESCRIPTOR,
    PSID, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE,
    FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};

use super::config::PIPE_NAME;
use super::identity::{wide, SYSTEM_SID};

const PIPE_BUFFER_SIZE: u32 = 64 * 1024;
const ERROR_PIPE_CONNECTED_CODE: u32 = 535;

pub struct PipeStream {
    handle: HANDLE,
}

unsafe impl Send for PipeStream {}

impl PipeStream {
    pub fn server_process_id(&self) -> io::Result<u32> {
        let mut process_id = 0_u32;
        unsafe { GetNamedPipeServerProcessId(self.handle, &mut process_id) }.map_err(io_error)?;
        Ok(process_id)
    }
}

impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut read = 0_u32;
        unsafe { ReadFile(self.handle, Some(buffer), Some(&mut read), None) }.map_err(io_error)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "命名管道已关闭",
            ));
        }
        Ok(read as usize)
    }
}

impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut written = 0_u32;
        unsafe { WriteFile(self.handle, Some(buffer), Some(&mut written), None) }
            .map_err(io_error)?;
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "命名管道写入 0 字节",
            ));
        }
        Ok(written as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for PipeStream {
    fn drop(&mut self) {
        unsafe {
            let _ = DisconnectNamedPipe(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}

pub fn create_server(user_sid: &str) -> io::Result<PipeStream> {
    let mut security = PipeSecurity::new(user_sid)?;
    let attributes = security.attributes();
    let name = wide(PIPE_NAME);
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            PIPE_BUFFER_SIZE,
            PIPE_BUFFER_SIZE,
            0,
            Some(&attributes),
        )
    };
    if handle.is_invalid() {
        return Err(io::Error::last_os_error());
    }
    Ok(PipeStream { handle })
}

pub fn wait_for_client(pipe: &PipeStream) -> io::Result<()> {
    match unsafe { ConnectNamedPipe(pipe.handle, None) } {
        Ok(()) => Ok(()),
        Err(error) if (error.code().0 as u32 & 0xFFFF) == ERROR_PIPE_CONNECTED_CODE => Ok(()),
        Err(error) => Err(io::Error::other(error)),
    }
}

pub fn connect_client() -> io::Result<PipeStream> {
    let name = wide(PIPE_NAME);
    let handle = unsafe {
        CreateFileW(
            PCWSTR(name.as_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(io_error)?;
    Ok(PipeStream { handle })
}

fn io_error(error: windows::core::Error) -> io::Error {
    io::Error::other(error)
}

struct PipeSecurity {
    descriptor: SECURITY_DESCRIPTOR,
    acl: *mut ACL,
}

impl PipeSecurity {
    fn new(user_sid: &str) -> io::Result<Self> {
        let system = string_sid(SYSTEM_SID)?;
        let user = string_sid(user_sid)?;
        let entries = [
            explicit_access(system, GENERIC_READ.0 | GENERIC_WRITE.0),
            explicit_access(user, GENERIC_READ.0 | GENERIC_WRITE.0),
        ];
        let mut acl: *mut ACL = null_mut();
        let result = unsafe { SetEntriesInAclW(Some(&entries), None, &mut acl) };
        unsafe {
            let _ = LocalFree(Some(HLOCAL(system.0)));
            let _ = LocalFree(Some(HLOCAL(user.0)));
        }
        if result.0 != 0 {
            return Err(io::Error::from_raw_os_error(result.0 as i32));
        }
        let mut descriptor = SECURITY_DESCRIPTOR::default();
        let descriptor_ptr =
            PSECURITY_DESCRIPTOR((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast());
        unsafe {
            InitializeSecurityDescriptor(descriptor_ptr, 1).map_err(io_error)?;
            SetSecurityDescriptorDacl(descriptor_ptr, true, Some(acl), false).map_err(io_error)?;
        }
        Ok(Self { descriptor, acl })
    }

    fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: (&mut self.descriptor as *mut SECURITY_DESCRIPTOR).cast(),
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if !self.acl.is_null() {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.acl.cast())));
            }
        }
    }
}

fn string_sid(text: &str) -> io::Result<PSID> {
    let text = wide(text);
    let mut sid = PSID(null_mut());
    unsafe {
        ConvertStringSidToSidW(PCWSTR(text.as_ptr()), &mut sid).map_err(io_error)?;
    }
    Ok(sid)
}

fn explicit_access(sid: PSID, access: u32) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: access,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: ACE_FLAGS(0),
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: Default::default(),
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: PWSTR(sid.0.cast()),
        },
    }
}
