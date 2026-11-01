#[cfg(windows)]
use std::sync::OnceLock;

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use std::mem::size_of;
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
#[cfg(windows)]
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TokenUser, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER,
};
#[cfg(windows)]
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

#[cfg(windows)]
const ELEVATED_ARG: &str = "--elevated";

#[cfg(windows)]
pub fn is_elevated() -> bool {
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(|| unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0_u32;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        let _ = CloseHandle(token);
        result.is_ok() && elevation.TokenIsElevated != 0
    })
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

pub fn ensure_elevated() -> bool {
    #[cfg(windows)]
    {
        if test_mode() || is_elevated() {
            return true;
        }
        if std::env::args().any(|arg| arg == ELEVATED_ARG) {
            crate::startup::fatal(
                "需要管理员权限",
                "NexusCat 需要管理员权限才能运行，但当前进程仍未获得提升。",
            );
        }
        if let Err(error) = relaunch_elevated() {
            crate::startup::fatal(
                "需要管理员权限",
                &format!("NexusCat 需要管理员权限才能运行。\n\n{error}"),
            );
        }
        false
    }
    #[cfg(not(windows))]
    {
        true
    }
}

#[cfg(windows)]
pub(crate) fn user_sid_string(token: HANDLE) -> Option<String> {
    let mut size = 0_u32;
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut size) };
    if size == 0 {
        return None;
    }
    let words = (size as usize).div_ceil(size_of::<usize>());
    let mut buffer = vec![0_usize; words];
    let mut returned = 0_u32;
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr() as *mut c_void),
            size,
            &mut returned,
        )
    }
    .ok()?;
    let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
    sid_to_string(user.User.Sid)
}

#[cfg(windows)]
pub fn current_user_sid_string() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let sid = user_sid_string(token);
        let _ = CloseHandle(token);
        sid
    }
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn current_user_sid_string() -> Option<String> {
    None
}

#[cfg(windows)]
fn sid_to_string(sid: windows::Win32::Security::PSID) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    let mut value = PWSTR::null();
    unsafe {
        if ConvertSidToStringSidW(sid, &mut value).is_err() {
            return None;
        }
    }
    let text = unsafe { value.to_string().ok() };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(value.0.cast())));
    }
    text
}

#[cfg(windows)]
fn relaunch_elevated() -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let exe =
        std::env::current_exe().map_err(|error| format!("读取当前可执行文件失败：{error}"))?;
    let exe_wide = wide(&exe.to_string_lossy());
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    arguments.push(ELEVATED_ARG.to_string());
    let parameters = arguments
        .iter()
        .map(|argument| quote_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let parameters_wide = wide(&parameters);
    let verb = wide("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(exe_wide.as_ptr()),
        lpParameters: PCWSTR(parameters_wide.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }
        .map_err(|error| format!("请求管理员权限失败：{error}"))?;
    if !info.hProcess.is_invalid() {
        unsafe {
            let _ = CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn test_mode() -> bool {
    std::env::var("NEXUSCAT_TEST")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(windows)]
fn quote_argument(argument: &str) -> String {
    if !argument.is_empty()
        && !argument
            .chars()
            .any(|character| character.is_whitespace() || character == '"')
    {
        return argument.to_string();
    }
    let mut quoted = String::from("\"");
    let mut backslashes = 0_usize;
    for character in argument.chars() {
        match character {
            '\\' => {
                backslashes += 1;
                quoted.push('\\');
            }
            '"' => {
                for _ in 0..backslashes {
                    quoted.push('\\');
                }
                quoted.push('\\');
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                backslashes = 0;
                quoted.push(character);
            }
        }
    }
    for _ in 0..backslashes {
        quoted.push('\\');
    }
    quoted.push('"');
    quoted
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
