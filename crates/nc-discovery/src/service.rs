use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nc_core::{DeviceId, DeviceInfo, NexusError, Result};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, watch};
use tokio::time::{interval, sleep, MissedTickBehavior};
use tracing::{debug, info};

use crate::announcement::{decode_announcement, encode_announcement, Announcement};
use crate::model::{discoverable_local_info, DiscoveredDevice, DiscoveryConfig, DiscoveryEvent};
use crate::probe::{decode_probe_request, encode_probe_request, has_probe_magic};

const GLOBAL_BROADCAST_ADDRESS: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 255);
const RECEIVE_BUFFER_BYTES: usize = 2048;
const RECEIVE_ERROR_BACKOFF: Duration = Duration::from_millis(20);
const REAP_INTERVAL_MIN: Duration = Duration::from_millis(25);
const REAP_INTERVAL_MAX: Duration = Duration::from_millis(500);
const PROBE_REPLY_MIN_INTERVAL: Duration = Duration::from_millis(250);

struct DeviceEntry {
    device: DiscoveredDevice,
    last_seen: Instant,
}

struct Inner {
    config: DiscoveryConfig,
    local_info: DeviceInfo,
    socket: UdpSocket,
    event_tx: broadcast::Sender<DiscoveryEvent>,
    devices: Mutex<HashMap<DeviceId, DeviceEntry>>,
    last_probe_reply: Mutex<Option<Instant>>,
    shutdown_tx: watch::Sender<bool>,
}

#[derive(Clone)]
pub struct DiscoveryService {
    inner: Arc<Inner>,
    handles: Arc<()>,
}

impl DiscoveryService {
    pub fn start(
        config: DiscoveryConfig,
        event_tx: broadcast::Sender<DiscoveryEvent>,
    ) -> Result<Self> {
        Self::start_with_device_info(config, discoverable_local_info(), event_tx)
    }

    pub fn start_with_device_info(
        mut config: DiscoveryConfig,
        local_info: DeviceInfo,
        event_tx: broadcast::Sender<DiscoveryEvent>,
    ) -> Result<Self> {
        config.validate()?;
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(NexusError::Network(
                "发现服务必须在 Tokio 运行时内启动".to_owned(),
            ));
        }

        let probe = Announcement::new(local_info.clone(), epoch_millis());
        encode_announcement(&probe)
            .map_err(|error| NexusError::invalid(format!("本机发现通告无效：{error}")))?;

        let socket = bind_socket(config.discovery_port)?;
        let local_address = socket
            .local_addr()
            .map_err(|error| NexusError::Network(format!("读取发现套接字地址失败：{error}")))?;
        if config.discovery_port == 0 {
            config.discovery_port = local_address.port();
        }

        let (shutdown_tx, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            config,
            local_info,
            socket,
            event_tx,
            devices: Mutex::new(HashMap::new()),
            last_probe_reply: Mutex::new(None),
            shutdown_tx,
        });

        let receive_shutdown = inner.shutdown_tx.subscribe();
        let broadcast_shutdown = inner.shutdown_tx.subscribe();
        let reap_shutdown = inner.shutdown_tx.subscribe();

        tokio::spawn(receive_loop(Arc::clone(&inner), receive_shutdown));
        tokio::spawn(broadcast_loop(Arc::clone(&inner), broadcast_shutdown));
        tokio::spawn(reap_loop(Arc::clone(&inner), reap_shutdown));

        info!(
            address = %local_address,
            device_id = %inner.local_info.id,
            interface_broadcast = inner.config.interface_broadcast,
            "LAN 发现服务已启动"
        );

        Ok(Self {
            inner,
            handles: Arc::new(()),
        })
    }

    pub fn local_info(&self) -> &DeviceInfo {
        &self.inner.local_info
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.inner
            .socket
            .local_addr()
            .map_err(|error| NexusError::Network(format!("读取发现套接字地址失败：{error}")))
    }

    pub fn snapshot(&self) -> Vec<DiscoveredDevice> {
        let now = Instant::now();
        let timeout = self.inner.config.device_timeout;
        let devices = lock_devices(&self.inner.devices);
        let mut snapshot: Vec<_> = devices
            .values()
            .filter(|entry| now.saturating_duration_since(entry.last_seen) < timeout)
            .map(|entry| entry.device.clone())
            .collect();
        snapshot.sort_by_key(|device| device.info.id);
        snapshot
    }

    pub async fn probe(&self) -> Result<usize> {
        let announcement = self
            .inner
            .announcement_payload()
            .ok_or_else(|| NexusError::Network("本机发现通告无效，无法发起探测".to_owned()))?;
        let request = encode_probe_request(self.inner.local_info.id);
        let targets = self.inner.broadcast_targets();
        if targets.is_empty() {
            return Err(NexusError::Network("没有可用的发现目标".to_owned()));
        }

        let mut delivered = 0;
        let mut last_error = None;
        for target in targets {
            if let Err(error) = self.inner.socket.send_to(&announcement, target).await {
                last_error = Some(error);
            }
            match self.inner.socket.send_to(&request, target).await {
                Ok(_) => delivered += 1,
                Err(error) => last_error = Some(error),
            }
        }

        if delivered == 0 {
            let detail = last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "未知错误".to_owned());
            Err(NexusError::Network(format!("发送发现探测失败：{detail}")))
        } else {
            Ok(delivered)
        }
    }

    pub fn stop(&self) {
        let _ = self.inner.shutdown_tx.send(true);
    }
}

impl Drop for DiscoveryService {
    fn drop(&mut self) {
        if Arc::strong_count(&self.handles) == 1 {
            let _ = self.inner.shutdown_tx.send(true);
        }
    }
}

fn bind_socket(port: u16) -> Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|error| NexusError::Network(format!("创建发现套接字失败：{error}")))?;
    socket
        .set_reuse_address(true)
        .map_err(|error| NexusError::Network(format!("设置 SO_REUSEADDR 失败：{error}")))?;
    #[cfg(unix)]
    socket
        .set_reuse_port(true)
        .map_err(|error| NexusError::Network(format!("设置 SO_REUSEPORT 失败：{error}")))?;
    socket
        .set_broadcast(true)
        .map_err(|error| NexusError::Network(format!("启用 UDP 广播失败：{error}")))?;
    socket
        .set_nonblocking(true)
        .map_err(|error| NexusError::Network(format!("设置发现套接字非阻塞失败：{error}")))?;

    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    socket.bind(&address.into()).map_err(|error| {
        NexusError::Network(format!(
            "绑定发现端口 {address} 失败（端口可能被占用或权限不足）：{error}"
        ))
    })?;

    let std_socket: StdUdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
        .map_err(|error| NexusError::Network(format!("注册发现套接字到 Tokio 失败：{error}")))
}

impl Inner {
    async fn handle_datagram(&self, payload: &[u8], address: SocketAddr) {
        if has_probe_magic(payload) {
            match decode_probe_request(payload) {
                Ok(device) => self.handle_probe(device, address).await,
                Err(error) => debug!(%address, %error, "忽略无效探测请求"),
            }
            return;
        }

        match decode_announcement(payload) {
            Ok(announcement) => self.handle_announcement(announcement, address),
            Err(error) => debug!(%address, %error, "忽略无效发现通告"),
        }
    }

    async fn handle_probe(&self, device: DeviceId, address: SocketAddr) {
        if device == self.local_info.id {
            return;
        }

        {
            let mut last_reply = self
                .last_probe_reply
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if last_reply.is_some_and(|instant| instant.elapsed() < PROBE_REPLY_MIN_INTERVAL) {
                return;
            }
            *last_reply = Some(Instant::now());
        }

        let Some(payload) = self.announcement_payload() else {
            return;
        };
        if let Err(error) = self.socket.send_to(&payload, address).await {
            debug!(%address, %error, "回应发现探测失败");
        }
    }

    fn handle_announcement(&self, announcement: Announcement, address: SocketAddr) {
        if announcement.device.id == self.local_info.id {
            return;
        }

        let last_seen = Instant::now();
        let last_seen_ms = epoch_millis();
        let event = {
            let mut devices = lock_devices(&self.devices);
            match devices.get_mut(&announcement.device.id) {
                Some(entry) => {
                    let changed =
                        entry.device.info != announcement.device || entry.device.address != address;
                    entry.last_seen = last_seen;
                    entry.device.last_seen_ms = last_seen_ms;
                    if changed {
                        entry.device.info = announcement.device;
                        entry.device.address = address;
                        Some(DiscoveryEvent::Updated(entry.device.clone()))
                    } else {
                        None
                    }
                }
                None => {
                    let device = DiscoveredDevice {
                        info: announcement.device,
                        address,
                        last_seen_ms,
                    };
                    devices.insert(
                        device.info.id,
                        DeviceEntry {
                            device: device.clone(),
                            last_seen,
                        },
                    );
                    Some(DiscoveryEvent::Found(device))
                }
            }
        };

        if let Some(event) = event {
            let _ = self.event_tx.send(event);
        }
    }

    fn announcement_payload(&self) -> Option<Vec<u8>> {
        let announcement = Announcement::new(self.local_info.clone(), epoch_millis());
        match encode_announcement(&announcement) {
            Ok(payload) => Some(payload),
            Err(error) => {
                debug!(%error, "跳过无法编码的本机发现通告");
                None
            }
        }
    }

    async fn announce(&self) {
        let Some(payload) = self.announcement_payload() else {
            return;
        };
        for target in self.broadcast_targets() {
            if let Err(error) = self.socket.send_to(&payload, target).await {
                debug!(%target, %error, "发送发现通告失败");
            }
        }
    }

    fn broadcast_targets(&self) -> Vec<SocketAddr> {
        let mut targets = self.config.extra_targets.clone();
        if self.config.interface_broadcast {
            targets.push(SocketAddr::new(
                IpAddr::V4(GLOBAL_BROADCAST_ADDRESS),
                self.config.discovery_port,
            ));
            match if_addrs::get_if_addrs() {
                Ok(interfaces) => {
                    for interface in interfaces {
                        let if_addrs::IfAddr::V4(address) = interface.addr else {
                            continue;
                        };
                        if address.ip.is_loopback() || address.ip.is_unspecified() {
                            continue;
                        }
                        let broadcast = address.broadcast.unwrap_or_else(|| {
                            let ip = u32::from(address.ip);
                            let mask = u32::from(address.netmask);
                            Ipv4Addr::from(ip | !mask)
                        });
                        targets.push(SocketAddr::new(
                            IpAddr::V4(broadcast),
                            self.config.discovery_port,
                        ));
                    }
                }
                Err(error) => debug!(%error, "枚举网络接口失败，仅发送全局广播"),
            }
        }

        targets.sort_unstable();
        targets.dedup();
        targets
    }

    fn reap_expired(&self) {
        let now = Instant::now();
        let timeout = self.config.device_timeout;
        let mut lost = Vec::new();
        {
            let mut devices = lock_devices(&self.devices);
            devices.retain(|device_id, entry| {
                let expired = now.saturating_duration_since(entry.last_seen) >= timeout;
                if expired {
                    lost.push(*device_id);
                }
                !expired
            });
        }

        for device_id in lost {
            info!(%device_id, "发现设备离线");
            let _ = self.event_tx.send(DiscoveryEvent::Lost(device_id));
        }
    }
}

async fn receive_loop(inner: Arc<Inner>, mut shutdown: watch::Receiver<bool>) {
    let mut buffer = vec![0u8; RECEIVE_BUFFER_BYTES];
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            received = inner.socket.recv_from(&mut buffer) => {
                match received {
                    Ok((length, address)) => inner.handle_datagram(&buffer[..length], address).await,
                    Err(error) => {
                        debug!(%error, "接收发现通告失败");
                        tokio::select! {
                            _ = shutdown.changed() => break,
                            _ = sleep(RECEIVE_ERROR_BACKOFF) => {}
                        }
                    }
                }
            }
        }
    }
}

async fn broadcast_loop(inner: Arc<Inner>, mut shutdown: watch::Receiver<bool>) {
    let mut ticker = interval(inner.config.announce_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = ticker.tick() => inner.announce().await,
        }
    }
}

async fn reap_loop(inner: Arc<Inner>, mut shutdown: watch::Receiver<bool>) {
    let reap_interval =
        (inner.config.device_timeout / 4).clamp(REAP_INTERVAL_MIN, REAP_INTERVAL_MAX);
    let mut ticker = interval(reap_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = ticker.tick() => inner.reap_expired(),
        }
    }
}

fn lock_devices(
    devices: &Mutex<HashMap<DeviceId, DeviceEntry>>,
) -> MutexGuard<'_, HashMap<DeviceId, DeviceEntry>> {
    devices
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}
