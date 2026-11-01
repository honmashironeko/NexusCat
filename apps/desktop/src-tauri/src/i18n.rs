use nc_core::{Language, Mode, PermissionKind};

fn pick(language: Language, zh: &str, en: &str) -> String {
    match language {
        Language::Zh => zh.to_string(),
        Language::En => en.to_string(),
    }
}

pub fn device_offline(language: Language) -> (String, String) {
    (
        pick(language, "设备不在线", "Device offline"),
        pick(
            language,
            "请等待设备重新出现在发现列表中",
            "Wait for the device to appear in discovery again",
        ),
    )
}

pub fn device_port_not_ready(language: Language) -> (String, String) {
    (
        pick(language, "设备端口未就绪", "Device port not ready"),
        pick(
            language,
            "对端尚未完成启动，请稍后重试",
            "The peer has not finished starting; try again shortly",
        ),
    )
}

pub fn pair_while_controlling(language: Language) -> (String, String) {
    (
        pick(language, "正在控制其他设备", "Controlling another device"),
        pick(
            language,
            "请先释放当前控制，再配对新的设备",
            "Release the current control before pairing a new device",
        ),
    )
}

pub fn controlled_by_other(language: Language) -> (String, String) {
    (
        pick(
            language,
            "本机正在被控制",
            "This device is being controlled",
        ),
        pick(
            language,
            "被控期间本机键鼠只在本机生效，不能发起新的控制",
            "While controlled, local input stays local and cannot start a new control session",
        ),
    )
}

pub fn session_rejected(language: Language, reason: &str) -> (String, String) {
    (
        pick(language, "对端拒绝了控制请求", "Control request rejected"),
        reason.to_string(),
    )
}

pub fn discovery_failed(language: Language, reason: &str) -> (String, String) {
    (
        pick(language, "设备发现启动失败", "Device discovery failed"),
        reason.to_string(),
    )
}

pub fn update_package_unsupported(language: Language) -> (String, String) {
    (
        pick(language, "不支持的更新包格式", "Unsupported update package"),
        pick(
            language,
            "请选择 exe 安装包、msi 安装包或便携版 exe",
            "Choose an exe installer, msi installer, or portable exe",
        ),
    )
}

pub fn file_reason_connection_lost(language: Language) -> String {
    pick(
        language,
        "连接已断开，文件传输中断",
        "Connection lost; file transfer stopped",
    )
}

pub fn update_receive_interrupted(language: Language) -> String {
    pick(
        language,
        "连接已断开，更新接收中断",
        "Connection lost; update receive stopped",
    )
}

pub fn install_action(language: Language) -> String {
    pick(language, "安装", "Install")
}

pub fn generic_device(language: Language) -> String {
    pick(language, "设备", "Device")
}

pub fn generic_remote_device(language: Language) -> String {
    pick(language, "远端设备", "Remote device")
}

pub fn generic_pairing_failed(language: Language) -> String {
    pick(language, "配对失败", "Pairing failed")
}

pub fn generic_disconnected(language: Language) -> String {
    pick(language, "连接已断开", "Connection lost")
}

pub fn generic_reconnect_failed(language: Language) -> String {
    pick(language, "重连失败", "Reconnect failed")
}

pub fn file_reason_no_peer(language: Language) -> String {
    pick(
        language,
        "没有可用的对端连接",
        "No peer connection is available",
    )
}

pub fn file_reason_resume_unsupported(language: Language) -> String {
    pick(
        language,
        "不支持从非零偏移续传",
        "Resuming from a non-zero offset is not supported",
    )
}

pub fn file_reason_disabled(language: Language) -> String {
    pick(
        language,
        "接收方已关闭文件传输",
        "File transfer is disabled on the receiver",
    )
}

pub fn screen_label(language: Language, index: usize) -> String {
    match language {
        Language::Zh => format!("屏幕 {}", index + 1),
        Language::En => format!("Screen {}", index + 1),
    }
}

pub fn paired(language: Language, device: &str) -> (String, String) {
    (
        pick(language, "配对成功", "Paired"),
        match language {
            Language::Zh => format!("已与 {device} 建立信任"),
            Language::En => format!("Trusted {device}"),
        },
    )
}

pub fn pairing_failed(language: Language, reason: &str) -> (String, String) {
    (
        pick(language, "配对失败", "Pairing failed"),
        reason.to_string(),
    )
}

pub fn connected(language: Language, device: &str) -> (String, String) {
    (
        pick(language, "已连接", "Connected"),
        match language {
            Language::Zh => format!("已连接到 {device}"),
            Language::En => format!("Connected to {device}"),
        },
    )
}

pub fn disconnected(language: Language, reason: &str) -> (String, String) {
    (
        pick(language, "连接断开", "Disconnected"),
        reason.to_string(),
    )
}

pub fn reconnecting(language: Language, attempt: u32) -> (String, String) {
    (
        pick(language, "正在重连", "Reconnecting"),
        match language {
            Language::Zh => format!("第 {attempt} 次重连…"),
            Language::En => format!("Reconnect attempt {attempt}…"),
        },
    )
}

pub fn reconnect_failed(language: Language, reason: &str) -> (String, String) {
    (
        pick(language, "重连失败", "Reconnect failed"),
        match language {
            Language::Zh => format!("{reason}。可从设备页手动恢复。"),
            Language::En => format!("{reason}. Retry from the devices page."),
        },
    )
}

pub fn switch_timed_out(language: Language) -> (String, String) {
    (
        pick(language, "切换设备超时", "Device switch timed out"),
        pick(
            language,
            "已回到本机。请从工作台重新选择目标设备。",
            "Returned to this device. Choose the target again from the workbench.",
        ),
    )
}

pub fn connection_failed(language: Language, detail: &str) -> (String, String) {
    (
        pick(language, "无法开始控制", "Could not start control"),
        detail.to_string(),
    )
}

pub fn host_takeover_released(language: Language, requester: &str) -> (String, String) {
    (
        pick(language, "主机已移交", "Host released"),
        match language {
            Language::Zh => format!("「{requester}」已接管主机，本机已释放控制"),
            Language::En => format!("{requester} took over as host; this device released control"),
        },
    )
}

pub fn host_offline(language: Language, host: &str) -> (String, String) {
    (
        pick(language, "主机已离线", "Host offline"),
        match language {
            Language::Zh => {
                format!("「{host}」已离线，主机锁已清除。可重新选择目标设备。")
            }
            Language::En => {
                format!("{host} is offline. The host lock was cleared; choose a target again.")
            }
        },
    )
}

pub fn host_takeover_timed_out(language: Language, host: &str) -> (String, String) {
    (
        pick(language, "接管主机超时", "Host takeover timed out"),
        match language {
            Language::Zh => format!("「{host}」未响应释放请求。请重试，或直接在该设备上释放控制。"),
            Language::En => {
                format!("{host} did not release control. Retry, or release control on that device.")
            }
        },
    )
}

pub fn retry_action(language: Language) -> String {
    pick(language, "重试", "Retry")
}

pub fn open_settings_action(language: Language) -> String {
    pick(language, "打开设置", "Open settings")
}

pub fn permission_lost(language: Language, kind: PermissionKind) -> (String, String) {
    let name_zh = match kind {
        PermissionKind::ScreenRecording => "屏幕录制",
        PermissionKind::Accessibility => "辅助功能",
        PermissionKind::InputMonitoring => "输入监控",
    };
    let name_en = match kind {
        PermissionKind::ScreenRecording => "Screen Recording",
        PermissionKind::Accessibility => "Accessibility",
        PermissionKind::InputMonitoring => "Input Monitoring",
    };
    (
        pick(language, "权限缺失", "Permission required"),
        match language {
            Language::Zh => format!("{name_zh}权限未授予，相关功能受限"),
            Language::En => format!("{name_en} permission is missing; features are limited"),
        },
    )
}

pub fn update_available(language: Language, version: &str) -> (String, String) {
    (
        pick(language, "发现更新", "Update available"),
        match language {
            Language::Zh => format!("版本 {version} 已就绪"),
            Language::En => format!("Version {version} is ready"),
        },
    )
}

pub fn update_staged(language: Language, version: &str) -> (String, String) {
    (
        pick(language, "更新已暂存", "Update staged"),
        match language {
            Language::Zh => format!("版本 {version} 已校验，可安装"),
            Language::En => format!("Version {version} verified and ready to install"),
        },
    )
}

pub fn update_failed(language: Language, reason: &str) -> (String, String) {
    (
        pick(language, "更新失败", "Update failed"),
        reason.to_string(),
    )
}

pub fn update_push_unavailable(language: Language) -> (String, String) {
    (
        pick(language, "无法推送更新", "Cannot push update"),
        pick(
            language,
            "没有已配对的设备",
            "No paired devices are available",
        ),
    )
}

pub fn update_push_busy(language: Language) -> (String, String) {
    (
        pick(language, "更新正在推送", "Update already in progress"),
        pick(
            language,
            "请等待当前广播推送完成后再试",
            "Wait for the current broadcast to finish",
        ),
    )
}

pub fn update_peer_offline(language: Language) -> &'static str {
    match language {
        Language::Zh => "设备离线",
        Language::En => "Device offline",
    }
}

pub fn update_peer_platform_mismatch(language: Language) -> &'static str {
    match language {
        Language::Zh => "平台不匹配",
        Language::En => "Platform mismatch",
    }
}

pub fn update_push_title(language: Language) -> String {
    pick(language, "更新推送", "Update push").to_string()
}

pub fn update_push_finished(language: Language, done: usize, failed: usize) -> String {
    match language {
        Language::Zh => {
            if failed == 0 {
                format!("{done} 台设备已接收更新")
            } else {
                format!("{done} 台已接收，{failed} 台失败")
            }
        }
        Language::En => {
            if failed == 0 {
                format!("{done} device(s) received the update")
            } else {
                format!("{done} succeeded, {failed} failed")
            }
        }
    }
}

pub fn file_received(language: Language, name: &str) -> (String, String) {
    (
        pick(language, "文件已接收", "File received"),
        name.to_string(),
    )
}

pub fn file_sent(language: Language, name: &str) -> (String, String) {
    (pick(language, "文件已发送", "File sent"), name.to_string())
}

pub fn file_disabled(language: Language) -> (String, String) {
    (
        pick(language, "文件传输已关闭", "File transfer is off"),
        pick(
            language,
            "请先在设置中开启文件传输",
            "Enable file transfer in settings first",
        ),
    )
}

pub fn file_no_peer(language: Language) -> (String, String) {
    (
        pick(language, "无法发送文件", "Cannot send files"),
        pick(
            language,
            "当前没有已连接的对端设备",
            "No peer device is currently connected",
        ),
    )
}

pub fn file_drop_hint(language: Language) -> (String, String) {
    (
        pick(language, "选择目标屏幕", "Choose a target screen"),
        pick(
            language,
            "把文件拖到右下角浮层里的目标屏幕后松开",
            "Drop the files onto a target screen in the bottom-right overlay",
        ),
    )
}

pub fn file_failed(language: Language, reason: &str) -> (String, String) {
    (
        pick(language, "文件传输失败", "File transfer failed"),
        reason.to_string(),
    )
}

pub fn mode_changed(language: Language, mode: Mode) -> (String, String) {
    let label = match mode {
        Mode::Kvm => pick(language, "多屏流转", "Multi-screen flow"),
        Mode::Aggregation => pick(language, "单屏聚合", "Single-screen aggregation"),
    };
    (pick(language, "模式已切换", "Mode changed"), label)
}

pub fn webcodecs_unsupported(language: Language) -> (String, String) {
    (
        pick(language, "视频解码不可用", "Video decoding unavailable"),
        pick(
            language,
            "当前 WebView 不支持 WebCodecs H.264 解码，聚合画面将无法显示",
            "This WebView does not support WebCodecs H.264 decoding; aggregate video is unavailable",
        ),
    )
}

pub fn shortcut_registration_failed(language: Language, detail: &str) -> (String, String) {
    (
        pick(language, "快捷键未生效", "Shortcut unavailable"),
        match language {
            Language::Zh => format!(
                "以下快捷键未生效：{detail}。请在设置中改用其他组合，或关闭占用该组合的程序。"
            ),
            Language::En => format!(
                "These shortcuts are unavailable: {detail}. Choose another combination in settings or close the app using it."
            ),
        },
    )
}

pub fn control_handover(language: Language, device: &str) -> (String, String) {
    (
        pick(language, "控制权移交铁驭", "Control handed over"),
        match language {
            Language::Zh => format!("{device} 正在控制本机的键盘与鼠标"),
            Language::En => format!("{device} is now controlling this device's keyboard and mouse"),
        },
    )
}

pub fn control_osd(language: Language, device: &str) -> (String, String, String, String) {
    let (title, subtitle) = control_handover(language, device);
    (
        pick(language, "NEXUSCAT · 单屏聚合", "NEXUSCAT · AGGREGATION"),
        title,
        subtitle,
        pick(
            language,
            "本机键鼠操作后隐藏",
            "Local input hides this overlay",
        ),
    )
}

pub fn control_returned(language: Language) -> (String, String) {
    (
        pick(language, "控制权已收回", "Control returned"),
        pick(
            language,
            "远端已释放输入，本机键盘与鼠标恢复控制",
            "The remote device released input; this device has control again",
        ),
    )
}

pub fn input_service_needs_update(language: Language) -> (String, String) {
    (
        pick(
            language,
            "系统输入服务需要更新",
            "System input service needs an update",
        ),
        pick(
            language,
            "服务副本与当前程序不一致，UAC/锁屏期间可能无法注入输入；请在设置页重新安装系统输入服务",
            "The service copy differs from this build and may not inject input on UAC/lock screens; reinstall it from Settings",
        ),
    )
}

pub fn file_drop_broker_unavailable(language: Language) -> (String, String) {
    (
        pick(
            language,
            "文件拖放代理不可用",
            "File drop broker unavailable",
        ),
        pick(
            language,
            "管理员模式下无法启动或连接文件拖放代理，拖放文件暂时不可用；请改用“发送文件”按钮，或重启应用后重试",
            "The file drop broker could not start while running as administrator; use the Send file button or restart the app",
        ),
    )
}

pub fn file_drop_broker_disconnected(language: Language) -> (String, String) {
    (
        pick(
            language,
            "文件拖放代理已断开",
            "File drop broker disconnected",
        ),
        pick(
            language,
            "管理员模式下的文件拖放已停止；重启应用会自动重连代理，也可以改用“发送文件”按钮",
            "File drop from the desktop stopped while running as administrator; restart the app to reconnect, or use the Send file button",
        ),
    )
}

pub fn autostart_task_failed(language: Language) -> (String, String) {
    (
        pick(language, "开机自启未生效", "Autostart not applied"),
        pick(
            language,
            "无法创建或更新登录自启计划任务；请在设置中关闭后重新开启，或以管理员权限重试",
            "Could not create or update the logon autostart task; toggle it in Settings or retry as administrator",
        ),
    )
}

pub fn secure_desktop_paused(language: Language) -> (String, String) {
    (
        pick(language, "安全桌面已激活", "Secure desktop active"),
        pick(
            language,
            "UAC/锁屏期间已暂停远端输入；返回桌面后可重新接管",
            "Remote input is paused while UAC/lock screen is active; resume after returning",
        ),
    )
}

pub fn secure_desktop_service_active(language: Language) -> (String, String) {
    (
        pick(
            language,
            "安全桌面已激活（系统注入）",
            "Secure desktop active (service injection)",
        ),
        pick(
            language,
            "远端输入继续由系统输入服务注入 UAC/锁屏桌面",
            "Remote input keeps flowing through the system input service on the UAC/lock screen desktop",
        ),
    )
}

pub fn secure_desktop_service_resumed(language: Language) -> (String, String) {
    (
        pick(language, "已离开安全桌面", "Left the secure desktop"),
        pick(
            language,
            "远端输入已恢复正常注入",
            "Remote input injection has returned to normal",
        ),
    )
}

pub fn secure_desktop_resumed(language: Language) -> (String, String) {
    (
        pick(language, "已返回桌面", "Back to desktop"),
        pick(
            language,
            "安全桌面已关闭，可重新跨越屏幕边缘接管远端",
            "The secure desktop closed; cross the screen edge again to take control",
        ),
    )
}

pub fn input_injection_blocked(language: Language) -> (String, String) {
    (
        pick(language, "远端输入被系统阻止", "Remote input blocked"),
        pick(
            language,
            "目标窗口权限更高或处于安全桌面，输入已交回本机",
            "The target window has higher privileges or is on the secure desktop; input returned to this device",
        ),
    )
}

pub fn target_accessibility_missing(language: Language) -> (String, String) {
    (
        pick(language, "需要辅助功能权限", "Accessibility permission needed"),
        pick(
            language,
            "远程键鼠注入不可用：请在系统提示中允许，或打开设置手动开启辅助功能",
            "Remote mouse and keyboard injection is unavailable: allow it in the system prompt, or open Settings to enable Accessibility",
        ),
    )
}

pub fn emergency_release(language: Language) -> (String, String) {
    (
        pick(language, "已释放输入", "Input released"),
        pick(
            language,
            "热键中止：键盘与鼠标已交还本机",
            "Hotkey abort: keyboard and mouse returned to this device",
        ),
    )
}

pub fn edge_osd_handoff(language: Language, mode: Mode) -> (String, String) {
    let kicker = pick(language, "边缘停留 100ms", "Edge dwell 100ms");
    let hint = match (language, mode) {
        (Language::Zh, Mode::Aggregation) => "键鼠已移交 · 全屏视图",
        (Language::En, Mode::Aggregation) => "Input handed off · fullscreen",
        (Language::Zh, Mode::Kvm) => "键鼠已移交 · 移回本机方向释放",
        (Language::En, Mode::Kvm) => "Input handed off · move back to the local edge to release",
    };
    (kicker, hint.to_string())
}

pub fn edge_halo_target(language: Language) -> String {
    pick(language, "停留 100ms 移交", "Dwell 100ms to hand off")
}

pub fn edge_halo_no_device(language: Language) -> (String, String) {
    (
        pick(language, "此方向无设备", "No device in this direction"),
        pick(language, "边缘停住", "Stops at this edge"),
    )
}

pub fn edge_osd_release(language: Language, emergency: bool) -> (String, String) {
    let kicker = if emergency {
        pick(language, "紧急释放", "Emergency release")
    } else {
        pick(language, "已释放", "Released")
    };
    let hint = pick(language, "键鼠回到本机", "Input returned to this device");
    (kicker, hint)
}

pub fn media_start_failed(language: Language, reason: &str) -> (String, String) {
    (
        pick(language, "画面启动失败", "Screen capture failed"),
        match language {
            Language::Zh => format!("无法启动远端画面：{reason}"),
            Language::En => format!("Could not start the remote video stream: {reason}"),
        },
    )
}

pub fn pairing_requested(language: Language, device: &str) -> (String, String) {
    (
        pick(language, "配对请求", "Pairing request"),
        match language {
            Language::Zh => format!("{device} 请求配对，请核对 6 位配对码。"),
            Language::En => format!("{device} wants to pair. Verify the 6-digit code."),
        },
    )
}

pub fn pairing_rejected_busy(language: Language, device: &str) -> (String, String) {
    (
        pick(language, "已拒绝配对", "Pairing declined"),
        match language {
            Language::Zh => format!("正在控制其他设备，已拒绝 {device} 的配对请求。"),
            Language::En => format!(
                "This device is controlling another device; the pairing request from {device} was declined."
            ),
        },
    )
}

