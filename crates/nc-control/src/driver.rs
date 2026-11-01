use crate::config::ControlConfig;
use crate::event::ControlEvent;
use crate::wire;
use nc_core::DeviceInfo;
use nc_crypto::{ReplayWindow, SessionKeys};
use nc_proto::{ControlFrame, ControlMessage, FrameKind, HandshakeMessage, LinkPurpose};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncWrite, WriteHalf};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::{interval_at, sleep_until, timeout, Instant, MissedTickBehavior};

pub(crate) enum Command {
    ConfirmSas(bool),
    Send(ControlMessage),
    Close,
}

pub(crate) struct StartedSession {
    pub(crate) commands: UnboundedSender<Command>,
    pub(crate) closed: Arc<AtomicBool>,
}

pub(crate) struct SessionStart<S> {
    pub(crate) stream: S,
    pub(crate) role: Role,
    pub(crate) config: ControlConfig,
    pub(crate) events: mpsc::Sender<ControlEvent>,
    pub(crate) peer: DeviceInfo,
    pub(crate) peer_static: [u8; 32],
    pub(crate) peer_certificate_fingerprint: [u8; 32],
    pub(crate) session: SessionKeys,
    pub(crate) pairing: bool,
    pub(crate) pending_certificate_update: bool,
    pub(crate) purpose: LinkPurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Client,
    Server,
}

enum Inbound {
    Frame(ControlFrame),
    Error(String),
}

enum Failure {
    Closed,
    Pairing(String),
    Connection(String),
}

pub(crate) fn spawn<S>(start: SessionStart<S>) -> StartedSession
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let SessionStart {
        stream,
        role,
        config,
        events,
        peer,
        peer_static,
        peer_certificate_fingerprint,
        session,
        pairing,
        pending_certificate_update,
        purpose,
    } = start;
    let (read_half, write_half) = tokio::io::split(stream);
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let closed = Arc::new(AtomicBool::new(false));
    let reader = tokio::spawn(read_loop(read_half, inbound_tx));
    let runner = SessionRunner {
        role,
        config,
        events,
        write: write_half,
        read_rx: inbound_rx,
        commands: command_rx,
        session,
        peer,
        peer_static,
        peer_certificate_fingerprint,
        pairing,
        pending_certificate_update,
        purpose,
        pending_sas_confirmation: None,
        outbound_high: VecDeque::new(),
        outbound_normal: VecDeque::new(),
        send_seq: 0,
        replay: ReplayWindow::new(1024),
        last_inbound: Instant::now(),
        heartbeat_seq: 0,
    };
    let closed_for_task = Arc::clone(&closed);
    tokio::spawn(async move {
        runner.run(reader).await;
        closed_for_task.store(true, Ordering::Release);
    });
    StartedSession {
        commands: command_tx,
        closed,
    }
}

async fn read_loop<R>(mut reader: R, inbound: UnboundedSender<Inbound>)
where
    R: AsyncRead + Unpin,
{
    loop {
        match wire::read_frame(&mut reader).await {
            Ok(frame) => {
                if inbound.send(Inbound::Frame(frame)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = inbound.send(Inbound::Error(error.to_string()));
                return;
            }
        }
    }
}

struct SessionRunner<S> {
    role: Role,
    config: ControlConfig,
    events: mpsc::Sender<ControlEvent>,
    write: WriteHalf<S>,
    read_rx: UnboundedReceiver<Inbound>,
    commands: UnboundedReceiver<Command>,
    session: SessionKeys,
    peer: DeviceInfo,
    peer_static: [u8; 32],
    peer_certificate_fingerprint: [u8; 32],
    pairing: bool,
    pending_certificate_update: bool,
    purpose: LinkPurpose,
    pending_sas_confirmation: Option<bool>,
    outbound_high: VecDeque<ControlMessage>,
    outbound_normal: VecDeque<ControlMessage>,
    send_seq: u64,
    replay: ReplayWindow,
    last_inbound: Instant,
    heartbeat_seq: u64,
}

impl<S> SessionRunner<S>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    async fn run(mut self, reader: JoinHandle<()>) {
        let outcome = self.perform_handshake().await;
        match outcome {
            Ok(()) => {
                let reason = self.established_loop().await;
                let _ = send_event(&self.events, ControlEvent::Disconnected { reason }).await;
            }
            Err(Failure::Closed) => {
                let reason = "本地关闭".to_string();
                let _ = send_event(&self.events, ControlEvent::Disconnected { reason }).await;
            }
            Err(Failure::Pairing(reason)) => {
                tracing::warn!(%reason, "控制配对失败");
                let _ = send_event(&self.events, ControlEvent::PairingFailed { reason }).await;
            }
            Err(Failure::Connection(reason)) => {
                tracing::warn!(%reason, "控制连接失败");
                let _ = send_event(&self.events, ControlEvent::Disconnected { reason }).await;
            }
        }
        reader.abort();
    }

    async fn perform_handshake(&mut self) -> std::result::Result<(), Failure> {
        if self.pairing {
            match self.run_pairing().await {
                Ok(()) => {}
                Err(Failure::Closed) => return Err(Failure::Closed),
                Err(Failure::Pairing(reason)) => return Err(Failure::Pairing(reason)),
                Err(Failure::Connection(reason)) => return Err(Failure::Pairing(reason)),
            }
        }
        self.exchange_ready()
            .await
            .map_err(|failure| match failure {
                Failure::Closed => Failure::Closed,
                Failure::Pairing(reason) | Failure::Connection(reason) => {
                    Failure::Connection(reason)
                }
            })?;
        if self.pending_certificate_update {
            self.config
                .update_certificate_fingerprint(&self.peer.id, self.peer_certificate_fingerprint)
                .map_err(|error| Failure::Connection(error.to_string()))?;
        }
        send_event(
            &self.events,
            ControlEvent::Connected {
                peer: self.peer.clone(),
                trusted: true,
                certificate_fingerprint: self.peer_certificate_fingerprint,
                media_key: *self.session.media_key.as_bytes(),
                purpose: self.purpose,
            },
        )
        .await?;
        Ok(())
    }

    async fn run_pairing(&mut self) -> std::result::Result<(), Failure> {
        match self.role {
            Role::Client => self.run_client_pairing().await,
            Role::Server => self.run_server_pairing().await,
        }
    }

    async fn run_client_pairing(&mut self) -> std::result::Result<(), Failure> {
        let deadline = Instant::now() + self.config.handshake_timeout;
        let sas = match self.next_peer_handshake(deadline).await? {
            HandshakeMessage::PairSas { sas } => sas,
            HandshakeMessage::PairResult {
                accepted: false,
                reason,
            } => {
                return Err(Failure::Pairing(
                    reason.unwrap_or_else(|| "对端拒绝配对".into()),
                ));
            }
            _ => {
                return Err(Failure::Connection("等待 SAS 时收到意外握手消息".into()));
            }
        };
        if sas != self.session.sas {
            let _ = self
                .send_pair_result(false, Some("SAS 与本地派生结果不一致".into()))
                .await;
            return Err(Failure::Pairing("SAS 与本地派生结果不一致".into()));
        }
        send_event(
            &self.events,
            ControlEvent::PairingSas {
                peer: self.peer.clone(),
                sas,
            },
        )
        .await?;

        if !self.wait_for_sas_confirmation(deadline).await? {
            let _ = self
                .send_pair_result(false, Some("用户拒绝 SAS".into()))
                .await;
            return Err(Failure::Pairing("用户拒绝 SAS".into()));
        }
        self.send_handshake(&HandshakeMessage::PairConfirm {
            sas: self.session.sas.clone(),
        })
        .await?;

        match self.next_peer_handshake(deadline).await? {
            HandshakeMessage::PairResult { accepted: true, .. } => {}
            HandshakeMessage::PairResult {
                accepted: false,
                reason,
            } => {
                return Err(Failure::Pairing(
                    reason.unwrap_or_else(|| "对端拒绝配对".into()),
                ));
            }
            _ => {
                return Err(Failure::Connection("等待配对结果时收到意外握手消息".into()));
            }
        }

        self.config
            .store_peer(
                &self.peer,
                self.peer_static,
                self.peer_certificate_fingerprint,
            )
            .map_err(|error| Failure::Pairing(error.to_string()))?;
        send_event(
            &self.events,
            ControlEvent::PairingSucceeded {
                peer: self.peer.clone(),
            },
        )
        .await?;
        Ok(())
    }

    async fn run_server_pairing(&mut self) -> std::result::Result<(), Failure> {
        let deadline = Instant::now() + self.config.handshake_timeout;
        self.send_handshake(&HandshakeMessage::PairSas {
            sas: self.session.sas.clone(),
        })
        .await?;
        send_event(
            &self.events,
            ControlEvent::PairingSas {
                peer: self.peer.clone(),
                sas: self.session.sas.clone(),
            },
        )
        .await?;

        let mut local_confirmation = self.pending_sas_confirmation.take();
        let mut peer_confirmed = false;
        loop {
            if local_confirmation == Some(false) {
                let _ = self
                    .send_pair_result(false, Some("服务端用户拒绝 SAS".into()))
                    .await;
                return Err(Failure::Pairing("用户拒绝 SAS".into()));
            }
            if local_confirmation == Some(true) && peer_confirmed {
                break;
            }

            tokio::select! {
                command = self.commands.recv() => {
                    let Some(command) = command else {
                        return Err(Failure::Closed);
                    };
                    match command {
                        Command::ConfirmSas(accepted) => local_confirmation = Some(accepted),
                        Command::Send(message) => self.queue_message(message),
                        Command::Close => return Err(Failure::Closed),
                    }
                }
                inbound = self.read_rx.recv() => {
                    match inbound {
                        Some(Inbound::Frame(frame)) => {
                            match wire::decode_handshake_frame(&frame) {
                                Ok(HandshakeMessage::PairConfirm { sas }) => {
                                    if sas != self.session.sas {
                                        let _ = self
                                            .send_pair_result(false, Some("SAS 确认值不一致".into()))
                                            .await;
                                        return Err(Failure::Pairing("SAS 确认值不一致".into()));
                                    }
                                    peer_confirmed = true;
                                }
                                Ok(HandshakeMessage::PairResult { accepted: false, reason }) => {
                                    return Err(Failure::Pairing(
                                        reason.unwrap_or_else(|| "对端拒绝配对".into()),
                                    ));
                                }
                                Ok(_) => {
                                    let _ = self
                                        .send_pair_result(false, Some("配对阶段收到意外消息".into()))
                                        .await;
                                    return Err(Failure::Pairing("配对阶段收到意外握手消息".into()));
                                }
                                Err(error) => return Err(Failure::Connection(error.to_string())),
                            }
                        }
                        Some(Inbound::Error(reason)) => return Err(Failure::Connection(reason)),
                        None => return Err(Failure::Connection("连接读取任务已结束".into())),
                    }
                }
                _ = sleep_until(deadline) => {
                    let _ = self
                        .send_pair_result(false, Some("等待 SAS 确认超时".into()))
                        .await;
                    return Err(Failure::Pairing("等待 SAS 确认超时".into()));
                }
            }
        }

        if let Err(error) = self
            .config
            .store_peer(&self.peer, self.peer_static, [0u8; 32])
        {
            let _ = self
                .send_pair_result(false, Some("保存信任记录失败".into()))
                .await;
            return Err(Failure::Pairing(error.to_string()));
        }
        self.send_handshake(&HandshakeMessage::PairResult {
            accepted: true,
            reason: None,
        })
        .await?;
        send_event(
            &self.events,
            ControlEvent::PairingSucceeded {
                peer: self.peer.clone(),
            },
        )
        .await?;
        Ok(())
    }

    async fn exchange_ready(&mut self) -> std::result::Result<(), Failure> {
        let deadline = Instant::now() + self.config.handshake_timeout;
        match self.role {
            Role::Client => {
                self.send_handshake(&HandshakeMessage::Ready).await?;
                match self.next_peer_handshake(deadline).await? {
                    HandshakeMessage::Ready => Ok(()),
                    _ => Err(Failure::Connection("等待 Ready 时收到意外消息".into())),
                }
            }
            Role::Server => {
                match self.next_peer_handshake(deadline).await? {
                    HandshakeMessage::Ready => {}
                    _ => return Err(Failure::Connection("等待 Ready 时收到意外消息".into())),
                }
                self.send_handshake(&HandshakeMessage::Ready).await
            }
        }
    }

    async fn wait_for_sas_confirmation(
        &mut self,
        deadline: Instant,
    ) -> std::result::Result<bool, Failure> {
        if let Some(accepted) = self.pending_sas_confirmation.take() {
            return Ok(accepted);
        }
        loop {
            tokio::select! {
                command = self.commands.recv() => {
                    let Some(command) = command else {
                        return Err(Failure::Closed);
                    };
                    match command {
                        Command::ConfirmSas(accepted) => return Ok(accepted),
                        Command::Send(message) => self.queue_message(message),
                        Command::Close => return Err(Failure::Closed),
                    }
                }
                inbound = self.read_rx.recv() => {
                    match inbound {
                        Some(Inbound::Frame(frame)) => {
                            match wire::decode_handshake_frame(&frame) {
                                Ok(HandshakeMessage::PairResult { accepted: false, reason }) => {
                                    return Err(Failure::Pairing(
                                        reason.unwrap_or_else(|| "对端取消配对".into()),
                                    ));
                                }
                                Ok(_) => {
                                    return Err(Failure::Connection(
                                        "等待 SAS 确认时收到意外握手消息".into(),
                                    ));
                                }
                                Err(error) => return Err(Failure::Connection(error.to_string())),
                            }
                        }
                        Some(Inbound::Error(reason)) => return Err(Failure::Connection(reason)),
                        None => return Err(Failure::Connection("连接读取任务已结束".into())),
                    }
                }
                _ = sleep_until(deadline) => {
                    return Err(Failure::Connection("等待 SAS 确认超时".into()));
                }
            }
        }
    }

    async fn next_peer_handshake(
        &mut self,
        deadline: Instant,
    ) -> std::result::Result<HandshakeMessage, Failure> {
        loop {
            tokio::select! {
                command = self.commands.recv() => {
                    let Some(command) = command else {
                        return Err(Failure::Closed);
                    };
                    self.handle_handshake_command(command)?;
                }
                inbound = self.read_rx.recv() => {
                    match inbound {
                        Some(Inbound::Frame(frame)) => {
                            if frame.kind != FrameKind::Handshake {
                                return Err(Failure::Connection(
                                    "握手阶段收到加密帧".into(),
                                ));
                            }
                            return wire::decode_handshake_frame(&frame)
                                .map_err(|error| Failure::Connection(error.to_string()));
                        }
                        Some(Inbound::Error(reason)) => return Err(Failure::Connection(reason)),
                        None => return Err(Failure::Connection("连接读取任务已结束".into())),
                    }
                }
                _ = sleep_until(deadline) => {
                    return Err(Failure::Connection("握手超时".into()));
                }
            }
        }
    }

    fn handle_handshake_command(&mut self, command: Command) -> std::result::Result<(), Failure> {
        match command {
            Command::ConfirmSas(accepted) => self.pending_sas_confirmation = Some(accepted),
            Command::Send(message) => self.queue_message(message),
            Command::Close => return Err(Failure::Closed),
        }
        Ok(())
    }

    async fn send_handshake(
        &mut self,
        message: &HandshakeMessage,
    ) -> std::result::Result<(), Failure> {
        let frame = wire::handshake_frame(message)
            .map_err(|error| Failure::Connection(error.to_string()))?;
        timeout(
            self.config.handshake_timeout,
            wire::write_frame(&mut self.write, &frame),
        )
        .await
        .map_err(|_| Failure::Connection("发送握手消息超时".into()))?
        .map_err(|error| Failure::Connection(error.to_string()))
    }

    async fn send_pair_result(
        &mut self,
        accepted: bool,
        reason: Option<String>,
    ) -> std::result::Result<(), Failure> {
        self.send_handshake(&HandshakeMessage::PairResult { accepted, reason })
            .await
    }

    fn queue_message(&mut self, message: ControlMessage) {
        if message.is_high_priority() {
            self.outbound_high.push_back(message);
        } else {
            self.outbound_normal.push_back(message);
        }
    }

    async fn flush_outbound(&mut self) -> std::result::Result<(), String> {
        loop {
            let Some(message) = self
                .outbound_high
                .pop_front()
                .or_else(|| self.outbound_normal.pop_front())
            else {
                return Ok(());
            };
            self.send_control(message).await?;
        }
    }

    async fn send_control(&mut self, message: ControlMessage) -> std::result::Result<(), String> {
        self.send_seq = self
            .send_seq
            .checked_add(1)
            .ok_or_else(|| "发送序号耗尽".to_string())?;
        let frame = wire::encrypt_control_frame(&self.session.send_key, self.send_seq, &message)
            .map_err(|error| error.to_string())?;
        timeout(
            self.config.handshake_timeout,
            wire::write_frame(&mut self.write, &frame),
        )
        .await
        .map_err(|_| "发送控制消息超时".to_string())?
        .map_err(|error| error.to_string())
    }

    async fn established_loop(&mut self) -> String {
        self.last_inbound = Instant::now();
        let mut heartbeat = interval_at(
            Instant::now() + self.config.heartbeat_interval,
            self.config.heartbeat_interval,
        );
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            if let Err(reason) = self.flush_outbound().await {
                return reason;
            }
            let idle_deadline = self
                .last_inbound
                .checked_add(self.config.heartbeat_timeout)
                .unwrap_or_else(Instant::now);

            tokio::select! {
                command = self.commands.recv() => {
                    match command {
                        Some(Command::Send(message)) => self.queue_message(message),
                        Some(Command::ConfirmSas(_)) => {
                            tracing::debug!("已连接后忽略配对命令");
                        }
                        Some(Command::Close) | None => return "本地关闭".into(),
                    }
                }
                inbound = self.read_rx.recv() => {
                    match inbound {
                        Some(Inbound::Frame(frame)) => {
                            if let Err(reason) = self.handle_control_frame(frame).await {
                                return reason;
                            }
                        }
                        Some(Inbound::Error(reason)) => return reason,
                        None => return "连接读取任务已结束".into(),
                    }
                }
                _ = heartbeat.tick() => {
                    self.heartbeat_seq = self.heartbeat_seq.wrapping_add(1);
                    let message = ControlMessage::Heartbeat {
                        seq: self.heartbeat_seq,
                        timestamp_us: now_us(),
                    };
                    if let Err(reason) = self.send_control(message).await {
                        return reason;
                    }
                }
                _ = sleep_until(idle_deadline) => {
                    return format!(
                        "{:.1}s 内未收到任何入站数据",
                        self.config.heartbeat_timeout.as_secs_f64()
                    );
                }
            }
        }
    }

    async fn handle_control_frame(
        &mut self,
        frame: ControlFrame,
    ) -> std::result::Result<(), String> {
        if frame.kind != FrameKind::Encrypted {
            return Err("已连接状态收到握手帧".into());
        }
        if frame.seq == 0 || !self.replay.check_and_update(frame.seq) {
            return Err("检测到重放或非法控制帧序号".into());
        }
        let message = wire::decrypt_control_frame(&self.session.recv_key, &frame)
            .map_err(|error| error.to_string())?;
        self.last_inbound = Instant::now();

        match message {
            ControlMessage::Heartbeat { seq, timestamp_us } => {
                let offset_us = clock_offset_us(timestamp_us, now_us());
                self.send_control(ControlMessage::HeartbeatAck { seq, timestamp_us })
                    .await?;
                send_event(&self.events, ControlEvent::RemoteClock { offset_us })
                    .await
                    .map_err(failure_reason)?;
            }
            ControlMessage::HeartbeatAck { timestamp_us, .. } => {
                let rtt_us = now_us().saturating_sub(timestamp_us);
                send_event(&self.events, ControlEvent::Heartbeat { rtt_us })
                    .await
                    .map_err(failure_reason)?;
            }
            message => {
                send_event(&self.events, ControlEvent::Message(message))
                    .await
                    .map_err(failure_reason)?;
            }
        }
        Ok(())
    }
}

async fn send_event(
    events: &mpsc::Sender<ControlEvent>,
    event: ControlEvent,
) -> std::result::Result<(), Failure> {
    events
        .send(event)
        .await
        .map_err(|_| Failure::Connection("事件接收端已关闭".into()))
}

fn failure_reason(failure: Failure) -> String {
    match failure {
        Failure::Closed => "本地关闭".into(),
        Failure::Pairing(reason) | Failure::Connection(reason) => reason,
    }
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros() as u64
}

fn clock_offset_us(remote_send_us: u64, local_receive_us: u64) -> i64 {
    local_receive_us as i64 - remote_send_us as i64
}

