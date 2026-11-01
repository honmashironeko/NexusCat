use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LookupPrivilegeValueW, TokenElevation, TokenUser,
    LUID_AND_ATTRIBUTES, PSID, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION,
    TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn sid_to_string(sid: PSID) -> Option<String> {
    let mut value = PWSTR::null();
    unsafe {
        if ConvertSidToStringSidW(sid, &mut value).is_err() {
            return None;
        }
    }
    let text = unsafe { value.to_string().ok() };
    unsafe {
        let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            value.0.cast(),
        )));
    }
    text
}

pub fn process_user_sid(process_id: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;
        let token = open_process_token(process);
        let _ = CloseHandle(process);
        let token = token?;
        let sid = token_user_sid(token);
        let _ = CloseHandle(token);
        sid
    }
}

pub fn current_user_sid() -> Option<String> {
    unsafe {
        let token = open_process_token(GetCurrentProcess())?;
        let sid = token_user_sid(token);
        let _ = CloseHandle(token);
        sid
    }
}

fn open_process_token(process: HANDLE) -> Option<HANDLE> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()? };
    Some(token)
}

fn token_user_sid(token: HANDLE) -> Option<String> {
    let mut size = 0_u32;
    unsafe {
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
    }
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0_u8; size as usize];
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size,
            &mut size,
        )
        .ok()?;
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        sid_to_string(user.User.Sid)
    }
}

pub fn process_image_path(process_id: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;
        let mut buffer = vec![0_u16; 32_768];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        String::from_utf16(&buffer[..size as usize]).ok()
    }
}

pub fn is_elevated() -> bool {
    unsafe {
        let Some(token) = open_process_token(GetCurrentProcess()) else {
            return false;
        };
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0_u32;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        let _ = CloseHandle(token);
        result.is_ok() && elevation.TokenIsElevated != 0
    }
}

pub const SYSTEM_SID: &str = "S-1-5-18";

pub fn enable_privilege(name: &str) -> Result<(), String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
            &mut token,
        )
        .map_err(|error| format!("打开进程令牌失败：{error}"))?;
        let name = wide(name);
        let mut luid = Default::default();
        let lookup = LookupPrivilegeValueW(
            windows::core::PCWSTR::null(),
            windows::core::PCWSTR(name.as_ptr()),
            &mut luid,
        );
        if lookup.is_err() {
            let _ = CloseHandle(token);
            return Err("查询特权 LUID 失败".into());
        }
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let result = AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None);
        let _ = CloseHandle(token);
        result.map_err(|error| format!("启用特权失败：{error}"))
    }
}

pub fn same_path(left: &str, right: &str) -> bool {
    if let (Ok(left), Ok(right)) = (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        return left == right;
    }
    fn normalize(path: &str) -> String {
        path.replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    }
    normalize(left) == normalize(right)
}
