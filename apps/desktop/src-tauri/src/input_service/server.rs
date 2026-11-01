use crossbeam_channel::{bounded, Receiver, Sender};
use nc_core::InputEvent;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use super::config::{self, load_config};
use super::frame::{read_frame, write_frame, AgentMessage};
use super::pipe::{create_server, wait_for_client};

const EVENT_QUEUE: usize = 4096;
const RETRY_DELAY: Duration = Duration::from_secs(2);

static SERVER: OnceLock<ServerHandle> = OnceLock::new();

struct ServerHandle {
    connected: Arc<AtomicBool>,
    sender: Sender<AgentMessage>,
}

pub fn start() {
    let _ = SERVER.get_or_init(|| {
        let (sender, receiver) = bounded(EVENT_QUEUE);
        let connected = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&connected);
        if let Err(error) = thread::Builder::new()
            .name("nc-input-service-server".into())
            .spawn(move || run(receiver, flag))
        {
            config::log(format!("启动输入服务管道线程失败：{error}"));
        }
        ServerHandle { connected, sender }
    });
}

pub fn is_connected() -> bool {
    SERVER
        .get()
        .map(|server| server.connected.load(Ordering::SeqCst))
        .unwrap_or(false)
}

pub fn send(event: &InputEvent) -> bool {
    let Some(server) = SERVER.get() else {
        return false;
    };
    if !server.connected.load(Ordering::SeqCst) {
        return false;
    }
    server
        .sender
        .try_send(AgentMessage::Input {
            event: event.clone(),
        })
        .is_ok()
}

fn run(receiver: Receiver<AgentMessage>, connected: Arc<AtomicBool>) {
    loop {
        connected.store(false, Ordering::SeqCst);
        let config = match load_config() {
            Ok(config) => config,
            Err(_) => {
                thread::sleep(RETRY_DELAY);
                continue;
            }
        };
        if !config.is_current() {
            config::log("输入服务配置版本不匹配，请重新安装服务");
            thread::sleep(RETRY_DELAY);
            continue;
        }
        let mut pipe = match create_server(&config.user_sid) {
            Ok(pipe) => pipe,
            Err(error) => {
                config::log(format!("创建输入服务管道失败：{error}"));
                thread::sleep(RETRY_DELAY);
                continue;
            }
        };
        if let Err(error) = wait_for_client(&pipe) {
            config::log(format!("等待输入服务代理连接失败：{error}"));
            thread::sleep(RETRY_DELAY);
            continue;
        }
        match read_frame::<_, AgentMessage>(&mut pipe) {
            Ok(AgentMessage::Hello {
                token,
                version,
                protocol_version,
            }) if token == config.token
                && version == config.version
                && protocol_version == config.protocol_version =>
            {
                if write_frame(
                    &mut pipe,
                    &AgentMessage::ServerHello {
                        token: config.token.clone(),
                        version: config.version.clone(),
                        protocol_version: config.protocol_version,
                    },
                )
                .is_err()
                {
                    config::log("向输入服务代理发送握手失败");
                    continue;
                }
            }
            Ok(_) => {
                config::log("输入服务代理握手内容无效");
                continue;
            }
            Err(error) => {
                config::log(format!("读取输入服务代理握手失败：{error}"));
                continue;
            }
        }

        while receiver.try_recv().is_ok() {}
        connected.store(true, Ordering::SeqCst);
        config::log("输入服务代理已连接");
        loop {
            match receiver.recv() {
                Ok(message) => {
                    if let Err(error) = write_frame(&mut pipe, &message) {
                        config::log(format!("写入输入服务代理失败：{error}"));
                        break;
                    }
                }
                Err(_) => {
                    connected.store(false, Ordering::SeqCst);
                    return;
                }
            }
        }
        connected.store(false, Ordering::SeqCst);
        config::log("输入服务代理连接已断开");
    }
}
