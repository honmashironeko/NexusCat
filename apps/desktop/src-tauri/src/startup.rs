use crate::config::AppPaths;
use std::io::Write;
use std::path::PathBuf;

#[cfg(windows)]
const WEBVIEW2_CLIENT_ID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
pub const WEBVIEW2_DOWNLOAD_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";

pub fn log_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("NexusCat");
    let _ = std::fs::create_dir_all(&base);
    base.join("startup.log")
}

pub fn log(message: &str) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let line = format!("[{stamp}] {message}\n");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = file.write_all(line.as_bytes());
    }
    #[cfg(debug_assertions)]
    eprint!("{line}");
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let detail = format!("{info}");
        log(&format!("panic: {detail}"));
        show_error(
            "NexusCat 无法继续运行",
            &format!(
                "启动或运行时发生未处理错误：\n\n{detail}\n\n启动日志：\n{}",
                log_path().display()
            ),
        );
    }));
    log(&format!(
        "启动 NexusCat {} / {} / pid={} / exe={}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::process::id(),
        std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    ));
}

pub fn fatal(title: &str, detail: &str) -> ! {
    log(&format!("{title}: {detail}"));
    show_error(title, detail);
    std::process::exit(1);
}

pub fn fatal_with_link(title: &str, detail: &str, url: &str) -> ! {
    log(&format!("{title}: {detail}"));
    let body = format!("{detail}\n\n是否打开下载页面？\n{url}");
    if show_yes_no(title, &body) {
        open_url(url);
    }
    std::process::exit(1);
}

pub fn resolve_paths(test_mode: bool) -> (AppPaths, Option<String>) {
    match AppPaths::resolve(test_mode) {
        Ok(paths) => match ensure_writable(&paths) {
            Ok(()) => {
                log(&format!("数据目录：{}", paths.data_dir.display()));
                (paths, None)
            }
            Err(error) => {
                log(&format!(
                    "数据目录不可写（{}）：{error}",
                    paths.data_dir.display()
                ));
                fallback_paths(test_mode, &format!("数据目录不可写：{error}"))
            }
        },
        Err(error) => {
            log(&format!("解析数据目录失败：{error}"));
            fallback_paths(test_mode, &format!("解析数据目录失败：{error}"))
        }
    }
}

fn fallback_paths(test_mode: bool, reason: &str) -> (AppPaths, Option<String>) {
    let local_base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("NexusCat");
    let local = AppPaths::from_base(local_base.clone(), local_base, test_mode);
    if ensure_writable(&local).is_ok() {
        log(&format!(
            "已回退到本地数据目录：{}（{reason}）",
            local.data_dir.display()
        ));
        return (local, Some(reason.to_string()));
    }
    let temp_base = std::env::temp_dir().join(format!("NexusCat-{}", std::process::id()));
    let temp = AppPaths::from_base(temp_base.clone(), temp_base, test_mode);
    if ensure_writable(&temp).is_ok() {
        log(&format!(
            "已回退到临时数据目录：{}（{reason}）",
            temp.data_dir.display()
        ));
        return (temp, Some(reason.to_string()));
    }
    fatal(
        "NexusCat 无法创建数据目录",
        &format!(
            "已尝试用户数据目录、本地数据目录与临时目录，均无法写入。\n\n原因：{reason}\n\n\
             请检查当前用户对以下目录的写权限，或联系管理员：\n\
             - {}\n- {}",
            local.data_dir.display(),
            temp.data_dir.display()
        ),
    );
}

fn ensure_writable(paths: &AppPaths) -> std::io::Result<()> {
    paths.ensure()?;
    for directory in [&paths.data_dir, &paths.local_dir] {
        let probe = directory.join(".write-test");
        std::fs::write(&probe, b"ok")?;
        let _ = std::fs::remove_file(&probe);
    }
    Ok(())
}

pub fn check_webview2() -> Result<(), String> {
    #[cfg(windows)]
    {
        if let Some(version) = webview2_version() {
            log(&format!("WebView2 运行时已安装：{version}"));
            return Ok(());
        }
        log("未检测到 WebView2 运行时");
        Err(format!(
            "NexusCat 需要 Microsoft Edge WebView2 Runtime 才能显示界面。\n\n\
             这台设备没有检测到 WebView2。请安装后重新启动：\n{url}\n\n\
             也可以运行 NexusCat 安装包，它会尝试安装运行时。",
            url = WEBVIEW2_DOWNLOAD_URL
        ))
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

pub async fn bind_control_listener(
    port: u16,
) -> std::io::Result<(tokio::net::TcpListener, Option<String>)> {
    match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
        Ok(listener) => Ok((listener, None)),
        Err(error) if port != 0 => {
            let listener = tokio::net::TcpListener::bind(("0.0.0.0", 0)).await?;
            let actual = listener
                .local_addr()
                .map(|address| address.port())
                .unwrap_or(0);
            let warning = format!("控制端口 {port} 不可用（{error}），已改用 {actual}");
            log(&warning);
            Ok((listener, Some(warning)))
        }
        Err(error) => Err(error),
    }
}

pub fn show_error(title: &str, body: &str) {
    message_box(title, body, false);
}

fn show_yes_no(title: &str, body: &str) -> bool {
    message_box(title, body, true)
}

#[cfg(windows)]
fn message_box(title: &str, body: &str, yes_no: bool) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
    };

    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let body: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();
    let style = if yes_no {
        MB_YESNO | MB_ICONERROR | MB_TOPMOST
    } else {
        MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST
    };
    let result = unsafe { MessageBoxW(None, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), style) };
    yes_no && result == IDYES
}

#[cfg(not(windows))]
fn message_box(title: &str, body: &str, _yes_no: bool) -> bool {
    eprintln!("{title}: {body}");
    false
}

#[cfg(windows)]
fn open_url(url: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let url: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(url.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(not(windows))]
fn open_url(url: &str) {
    eprintln!("请手动打开：{url}");
}

#[cfg(windows)]
fn webview2_version() -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };

    let subkeys = [
        (
            HKEY_LOCAL_MACHINE,
            "SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients",
        ),
        (
            HKEY_LOCAL_MACHINE,
            "SOFTWARE\\Microsoft\\EdgeUpdate\\Clients",
        ),
        (
            HKEY_CURRENT_USER,
            "SOFTWARE\\Microsoft\\EdgeUpdate\\Clients",
        ),
    ];
    for (root, base) in subkeys {
        let subkey = format!("{base}\\{WEBVIEW2_CLIENT_ID}");
        let subkey: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
        let value: Vec<u16> = "pv".encode_utf16().chain(std::iter::once(0)).collect();
        let mut buffer = [0u16; 128];
        let mut size = (buffer.len() * std::mem::size_of::<u16>()) as u32;
        let status = unsafe {
            RegGetValueW(
                root,
                PCWSTR(subkey.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status.is_ok() {
            let length = buffer.iter().position(|item| *item == 0).unwrap_or(0);
            let version = String::from_utf16_lossy(&buffer[..length]);
            if !version.trim().is_empty() {
                return Some(version);
            }
        }
    }
    None
}
