use nc_input::{create_input_injector, InputInjector};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, SetThreadDesktop,
    DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, UOI_NAME,
};

use super::config::{self, load_config, AgentConfig};
use super::frame::{read_frame, write_frame, AgentMessage};
use super::identity::{process_image_path, process_user_sid, same_path};
use super::pipe::{connect_client, PipeStream};

const RETRY_DELAY: Duration = Duration::from_secs(2);

const DESKTOP_CHECK_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Default)]
struct DesktopState {
    name: String,
    checked_at: Option<Instant>,
}

pub fn run_agent() {
    loop {
        let config = match load_config() {
            Ok(config) => config,
            Err(error) => {
                config::log(format!("读取输入服务配置失败：{error}"));
                thread::sleep(RETRY_DELAY);
                continue;
            }
        };
        if !config.is_current() {
            config::log("输入服务配置版本不匹配，等待重新安装");
            thread::sleep(Duration::from_secs(5));
            continue;
        }
        let mut pipe = match connect_client() {
            Ok(pipe) => pipe,
            Err(error) => {
                config::log(format!("连接输入服务管道失败：{error}"));
                thread::sleep(RETRY_DELAY);
                continue;
            }
        };
        if !verify_server(&pipe, &config) {
            config::log("拒绝未通过身份校验的输入服务管道服务端");
            thread::sleep(RETRY_DELAY);
            continue;
        }
        if write_frame(
            &mut pipe,
            &AgentMessage::Hello {
                token: config.token.clone(),
                version: config.version.clone(),
                protocol_version: config.protocol_version,
            },
        )
        .is_err()
        {
            config::log("发送输入服务握手失败");
            thread::sleep(RETRY_DELAY);
            continue;
        }
        match read_frame::<_, AgentMessage>(&mut pipe) {
            Ok(AgentMessage::ServerHello {
                token,
                version,
                protocol_version,
            }) if token == config.token
                && version == config.version
                && protocol_version == config.protocol_version => {}
            Ok(_) => {
                config::log("输入服务服务端握手内容无效");
                thread::sleep(RETRY_DELAY);
                continue;
            }
            Err(error) => {
                config::log(format!("读取输入服务服务端握手失败：{error}"));
                thread::sleep(RETRY_DELAY);
                continue;
            }
        }

        config::log("输入服务代理已连接桌面应用");
        let mut injector = match create_input_injector() {
            Ok(injector) => injector,
            Err(error) => {
                config::log(format!("创建输入注入器失败：{error}"));
                return;
            }
        };
        let mut desktop = DesktopState::default();
        loop {
            match read_frame::<_, AgentMessage>(&mut pipe) {
                Ok(AgentMessage::Input { event }) => {
                    if let Err(error) = ensure_input_desktop(&mut desktop, injector.as_mut()) {
                        config::log(format!("切换输入桌面失败：{error}"));
                        continue;
                    }
                    if let Err(error) = injector.inject(&event) {
                        config::log(format!("注入远端输入失败：{error}"));
                        desktop.checked_at = None;
                    }
                }
                Ok(_) => {
                    config::log("输入服务代理收到未知消息");
                    break;
                }
                Err(error) => {
                    config::log(format!("输入服务管道读取结束：{error}"));
                    break;
                }
            }
        }
        thread::sleep(RETRY_DELAY);
    }
}

fn verify_server(pipe: &PipeStream, config: &AgentConfig) -> bool {
    let Ok(process_id) = pipe.server_process_id() else {
        return false;
    };
    let Some(path) = process_image_path(process_id) else {
        return false;
    };
    let Some(user_sid) = process_user_sid(process_id) else {
        return false;
    };
    same_path(&path, &config.app_path) && user_sid.eq_ignore_ascii_case(&config.user_sid)
}

fn ensure_input_desktop(
    state: &mut DesktopState,
    injector: &mut dyn InputInjector,
) -> Result<(), String> {
    if let Some(checked_at) = state.checked_at {
        if checked_at.elapsed() < DESKTOP_CHECK_INTERVAL {
            return Ok(());
        }
    }
    let handle = unsafe {
        OpenInputDesktop(
            DESKTOP_CONTROL_FLAGS(0),
            false,
            DESKTOP_ACCESS_FLAGS(0x000F_01FF),
        )
    }
    .map_err(|error| format!("打开活动输入桌面失败：{error}"))?;
    let name = match desktop_name(handle) {
        Ok(name) => name,
        Err(error) => {
            let _ = unsafe { CloseDesktop(handle) };
            return Err(error);
        }
    };
    if name == state.name {
        let _ = unsafe { CloseDesktop(handle) };
        state.checked_at = Some(Instant::now());
        return Ok(());
    }
    if let Err(error) = unsafe { SetThreadDesktop(handle) } {
        let _ = unsafe { CloseDesktop(handle) };
        state.checked_at = None;
        return Err(format!("切换线程桌面失败：{error}"));
    }
    let _ = unsafe { CloseDesktop(handle) };
    state.checked_at = Some(Instant::now());
    let previous = std::mem::replace(&mut state.name, name.clone());
    let _ = injector.release_all();
    injector.reset_desktop();
    config::log(format!("输入桌面切换：{previous} -> {name}"));
    Ok(())
}

fn desktop_name(
    handle: windows::Win32::System::StationsAndDesktops::HDESK,
) -> Result<String, String> {
    let mut buffer = [0_u16; 256];
    let mut needed = 0_u32;
    unsafe {
        GetUserObjectInformationW(
            windows::Win32::Foundation::HANDLE(handle.0),
            UOI_NAME,
            Some(buffer.as_mut_ptr().cast()),
            (buffer.len() * std::mem::size_of::<u16>()) as u32,
            Some(&mut needed),
        )
    }
    .map_err(|error| format!("读取桌面名称失败：{error}"))?;
    let length = (needed as usize / std::mem::size_of::<u16>()).saturating_sub(1);
    Ok(String::from_utf16_lossy(
        &buffer[..length.min(buffer.len())],
    ))
}
