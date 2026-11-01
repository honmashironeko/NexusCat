use crate::elevation;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const AUTOSTART_ARG: &str = "--autostart";

fn task_name() -> String {
    elevation::current_user_sid_string()
        .map(|sid| format!("NexusCatAutostart-{sid}"))
        .unwrap_or_else(|| "NexusCatAutostart".to_string())
}

pub fn sync_startup(enabled: bool) -> Result<(), String> {
    if enabled {
        let name = task_name();
        if task_exists(&name)? {
            Ok(())
        } else {
            create_task(&name)
        }
    } else {
        delete_task()
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if enabled {
        create_task(&task_name())
    } else {
        delete_task()
    }
}

fn task_exists(name: &str) -> Result<bool, String> {
    let output = schtasks()
        .arg("/Query")
        .arg("/TN")
        .arg(name)
        .output()
        .map_err(|error| format!("查询登录自启计划任务失败：{error}"))?;
    Ok(output.status.success())
}

fn create_task(name: &str) -> Result<(), String> {
    let exe =
        std::env::current_exe().map_err(|error| format!("读取当前可执行文件失败：{error}"))?;
    let mut user_ids = Vec::new();
    if let Some(sid) = elevation::current_user_sid_string() {
        user_ids.push(sid);
    }
    if let Some(name) = user_name() {
        if !user_ids.contains(&name) {
            user_ids.push(name);
        }
    }
    if user_ids.is_empty() {
        return Err("无法读取当前用户标识".into());
    }

    let mut last_error = String::new();
    for (index, user_id) in user_ids.iter().enumerate() {
        let xml = task_xml(&exe, user_id);
        let path = std::env::temp_dir().join(format!(
            "nexuscat-autostart-{}-{index}.xml",
            std::process::id()
        ));
        write_utf16(&path, &xml)?;
        let output = schtasks()
            .arg("/Create")
            .arg("/TN")
            .arg(name)
            .arg("/XML")
            .arg(&path)
            .arg("/F")
            .output();
        let _ = std::fs::remove_file(&path);
        let output = output.map_err(|error| format!("运行 schtasks 失败：{error}"))?;
        if output.status.success() {
            tracing::info!(%user_id, "已更新登录自启计划任务");
            return Ok(());
        }
        last_error = output_message(&output);
    }
    Err(format!("创建登录自启计划任务失败：{last_error}"))
}

fn delete_task() -> Result<(), String> {
    let name = task_name();
    if !task_exists(&name)? {
        return Ok(());
    }
    let output = schtasks()
        .arg("/Delete")
        .arg("/TN")
        .arg(&name)
        .arg("/F")
        .output()
        .map_err(|error| format!("删除登录自启计划任务失败：{error}"))?;
    if output.status.success() {
        tracing::info!("已删除登录自启计划任务");
        Ok(())
    } else {
        Err(format!(
            "删除登录自启计划任务失败：{}",
            output_message(&output)
        ))
    }
}

fn schtasks() -> Command {
    let program = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
        .join("schtasks.exe");
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn output_message(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!(
        "退出码 {}；stdout: {}；stderr: {}",
        output.status,
        stdout.trim(),
        stderr.trim()
    )
}

fn user_name() -> Option<String> {
    let user = std::env::var("USERNAME")
        .ok()
        .filter(|value| !value.is_empty())?;
    let domain = std::env::var("USERDOMAIN")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("COMPUTERNAME")
                .ok()
                .filter(|value| !value.is_empty())
        })?;
    Some(format!("{domain}\\{user}"))
}

fn task_xml(exe: &Path, user_id: &str) -> String {
    let working_directory = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let exe = xml_escape(&exe.to_string_lossy());
    let working_directory = xml_escape(&working_directory.to_string_lossy());
    let user_id = xml_escape(user_id);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>NexusCat</Author>
    <Description>Start NexusCat elevated at user logon</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user_id}</UserId>
      <Delay>PT10S</Delay>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user_id}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{AUTOSTART_ARG}</Arguments>
      <WorkingDirectory>{working_directory}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        AUTOSTART_ARG = AUTOSTART_ARG
    )
}

fn write_utf16(path: &Path, text: &str) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(text.len() * 2 + 2);
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    std::fs::write(path, bytes).map_err(|error| format!("写入计划任务 XML 失败：{error}"))
}

fn xml_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}
