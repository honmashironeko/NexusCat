use nc_core::{NexusError, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub(crate) struct PortableScript<'a> {
    pub wait_pid: u32,
    pub source: &'a Path,
    pub destination: &'a Path,
    pub applied_path: &'a Path,
    pub version: &'a str,
    pub kind: &'a str,
    pub status: &'a str,
}

type ScriptEnvironment = Vec<(&'static str, OsString)>;

fn env_path(path: &Path) -> OsString {
    #[cfg(windows)]
    {
        OsString::from(path.to_string_lossy().replace('/', "\\"))
    }
    #[cfg(not(windows))]
    {
        path.as_os_str().to_owned()
    }
}

pub(crate) fn script_path(versions_dir: &Path, version: &str, action: &str) -> PathBuf {
    let extension = if cfg!(windows) { "cmd" } else { "sh" };
    versions_dir.join(format!("{version}-{action}.{extension}"))
}

pub(crate) fn write_portable_script(path: &Path, spec: &PortableScript<'_>) -> Result<()> {
    #[cfg(windows)]
    let contents = {
        let _ = spec;
        windows_script()
    };
    #[cfg(not(windows))]
    let contents = unix_script(spec, &portable_status_json(spec));

    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

pub(crate) fn spawn_script_with_env(
    path: &Path,
    env: &[(&'static str, OsString)],
) -> Result<()> {
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(path);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new("/bin/sh");
        command.arg(path);
        command
    };
    for (key, value) in env {
        command.env(key, value);
    }
    spawn_detached(command).map(|_| ())
}

pub(crate) fn portable_script_env(spec: &PortableScript<'_>) -> ScriptEnvironment {
    let wait_status = spec
        .applied_path
        .with_file_name(format!(".update-wait-{}.tmp", spec.wait_pid));
    let working_directory = spec
        .destination
        .parent()
        .filter(|directory| !directory.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    vec![
        ("NEXUSCAT_UPDATE_PID", OsString::from(spec.wait_pid.to_string())),
        ("NEXUSCAT_UPDATE_WAIT", env_path(&wait_status)),
        ("NEXUSCAT_UPDATE_SOURCE", env_path(spec.source)),
        ("NEXUSCAT_UPDATE_DEST", env_path(spec.destination)),
        ("NEXUSCAT_UPDATE_STATUS", env_path(spec.applied_path)),
        (
            "NEXUSCAT_UPDATE_STATUS_JSON",
            portable_status_json(spec).into(),
        ),
        ("NEXUSCAT_UPDATE_FAILED", portable_failed_json(spec).into()),
        ("NEXUSCAT_UPDATE_DIR", env_path(&working_directory)),
    ]
}

fn portable_status_json(spec: &PortableScript<'_>) -> String {
    serde_json::json!({
        "version": spec.version,
        "kind": spec.kind,
        "status": spec.status,
    })
    .to_string()
}

fn portable_failed_json(spec: &PortableScript<'_>) -> String {
    serde_json::json!({
        "version": spec.version,
        "kind": spec.kind,
        "status": "failed",
    })
    .to_string()
}

pub(crate) fn spawn_installer(package: &Path) -> Result<u32> {
    #[cfg(windows)]
    let command = if package
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("msi"))
    {
        let mut command = Command::new("msiexec");
        command.arg("/i").arg(package).args(["/qn", "/norestart"]);
        command
    } else {
        let mut command = Command::new(package);
        command.arg("/S");
        command
    };
    #[cfg(target_os = "macos")]
    let command = {
        if package
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dmg"))
        {
            return Err(NexusError::Update(
                "macOS 自动更新只支持 .pkg 安装包；请将 .app 打包为 .pkg 后重试".to_string(),
            ));
        }
        let mut command = Command::new("/usr/bin/open");
        command.args(["-W", "-pkg"]).arg(package);
        command
    };
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let command = {
        let mut command = Command::new(package);
        command.arg("/S");
        command
    };
    spawn_detached(command).map(|child| child.id())
}

pub(crate) fn spawn_detached(mut command: Command) -> Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW);
    }

    command
        .spawn()
        .map_err(|error| NexusError::Update(format!("启动更新进程失败：{error}")))
}

pub(crate) fn write_restart_script(
    path: &Path,
    wait_pid: u32,
    installer_pid: u32,
    launch: &Path,
) -> Result<()> {
    #[cfg(windows)]
    let contents = {
        let _ = (wait_pid, installer_pid, launch);
        windows_restart_script()
    };
    #[cfg(not(windows))]
    let contents = unix_restart_script(wait_pid, installer_pid, launch);

    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

pub(crate) fn restart_script_env(
    wait_pid: u32,
    installer_pid: u32,
    launch: &Path,
) -> ScriptEnvironment {
    let wait_status = std::env::temp_dir().join(format!(".nexuscat-restart-{wait_pid}.tmp"));
    let installer_status =
        std::env::temp_dir().join(format!(".nexuscat-installer-{installer_pid}.tmp"));
    let working_directory = launch
        .parent()
        .filter(|directory| !directory.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    vec![
        ("NEXUSCAT_UPDATE_PID", OsString::from(wait_pid.to_string())),
        (
            "NEXUSCAT_UPDATE_INSTALLER_PID",
            OsString::from(installer_pid.to_string()),
        ),
        ("NEXUSCAT_UPDATE_WAIT", env_path(&wait_status)),
        (
            "NEXUSCAT_UPDATE_INSTALLER_WAIT",
            env_path(&installer_status),
        ),
        ("NEXUSCAT_UPDATE_DEST", env_path(launch)),
        ("NEXUSCAT_UPDATE_DIR", env_path(&working_directory)),
    ]
}

#[cfg(windows)]
fn windows_restart_script() -> String {
    [
        "@echo off",
        "setlocal",
        ":wait_for_app",
        "tasklist /FI \"PID eq %NEXUSCAT_UPDATE_PID%\" /FO CSV /NH >\"%NEXUSCAT_UPDATE_WAIT%\" 2>NUL",
        "findstr /C:\"%NEXUSCAT_UPDATE_PID%\" \"%NEXUSCAT_UPDATE_WAIT%\" >NUL",
        "if not errorlevel 1 (",
        "  ping -n 2 127.0.0.1 >NUL",
        "  goto wait_for_app",
        ")",
        ":wait_for_installer",
        "tasklist /FI \"PID eq %NEXUSCAT_UPDATE_INSTALLER_PID%\" /FO CSV /NH >\"%NEXUSCAT_UPDATE_INSTALLER_WAIT%\" 2>NUL",
        "findstr /C:\"%NEXUSCAT_UPDATE_INSTALLER_PID%\" \"%NEXUSCAT_UPDATE_INSTALLER_WAIT%\" >NUL",
        "if not errorlevel 1 (",
        "  ping -n 2 127.0.0.1 >NUL",
        "  goto wait_for_installer",
        ")",
        "del /Q \"%NEXUSCAT_UPDATE_WAIT%\" >NUL 2>NUL",
        "del /Q \"%NEXUSCAT_UPDATE_INSTALLER_WAIT%\" >NUL 2>NUL",
        "ping -n 4 127.0.0.1 >NUL",
        "start \"\" /D \"%NEXUSCAT_UPDATE_DIR%\" \"%NEXUSCAT_UPDATE_DEST%\"",
        "exit /b 0",
    ]
    .join("\r\n")
        + "\r\n"
}

#[cfg(not(windows))]
fn unix_restart_script(wait_pid: u32, installer_pid: u32, launch: &Path) -> String {
    [
        "#!/bin/sh".to_owned(),
        "set -eu".to_owned(),
        format!("while kill -0 {wait_pid} 2>/dev/null; do sleep 1; done"),
        format!("while kill -0 {installer_pid} 2>/dev/null; do sleep 1; done"),
        "sleep 3".to_owned(),
        unix_launch_command(launch),
        "exit 0".to_owned(),
    ]
    .join("\n")
        + "\n"
}

#[cfg(not(windows))]
fn unix_launch_command(launch: &Path) -> String {
    if let Some(app) = mac_app_bundle(launch) {
        return format!("open -n {}", shell_quote_path(app));
    }
    let working_directory = launch.parent().unwrap_or_else(|| Path::new("."));
    format!(
        "cd {} && nohup {} >/dev/null 2>&1 &",
        shell_quote_path(working_directory),
        shell_quote_path(launch)
    )
}

#[cfg(not(windows))]
fn mac_app_bundle(path: &Path) -> Option<&Path> {
    path.ancestors().find(|ancestor| {
        ancestor
            .extension()
            .is_some_and(|extension| extension.to_str() == Some("app"))
    })
}

#[cfg(windows)]
fn windows_script() -> String {
    [
        "@echo off",
        "setlocal",
        ":wait_for_exit",
        "tasklist /FI \"PID eq %NEXUSCAT_UPDATE_PID%\" /FO CSV /NH >\"%NEXUSCAT_UPDATE_WAIT%\" 2>NUL",
        "findstr /C:\"%NEXUSCAT_UPDATE_PID%\" \"%NEXUSCAT_UPDATE_WAIT%\" >NUL",
        "if not errorlevel 1 (",
        "  ping -n 2 127.0.0.1 >NUL",
        "  goto wait_for_exit",
        ")",
        "del /Q \"%NEXUSCAT_UPDATE_WAIT%\" >NUL 2>NUL",
        "copy /Y \"%NEXUSCAT_UPDATE_SOURCE%\" \"%NEXUSCAT_UPDATE_DEST%\" >NUL",
        "if errorlevel 1 (",
        "  >\"%NEXUSCAT_UPDATE_STATUS%\" echo %NEXUSCAT_UPDATE_FAILED%",
        "  exit /b 1",
        ")",
        ">\"%NEXUSCAT_UPDATE_STATUS%\" echo %NEXUSCAT_UPDATE_STATUS_JSON%",
        "start \"\" /D \"%NEXUSCAT_UPDATE_DIR%\" \"%NEXUSCAT_UPDATE_DEST%\"",
        "exit /b 0",
    ]
    .join("\r\n")
        + "\r\n"
}

#[cfg(not(windows))]
fn unix_script(spec: &PortableScript<'_>, status: &str) -> String {
    let working_directory = spec.destination.parent().unwrap_or_else(|| Path::new("."));
    [
        "#!/bin/sh".to_owned(),
        "set -eu".to_owned(),
        format!(
            "while kill -0 {} 2>/dev/null; do sleep 1; done",
            spec.wait_pid
        ),
        format!(
            "cp -p -- {} {}",
            shell_quote_path(spec.source),
            shell_quote_path(spec.destination)
        ),
        format!(
            "chmod +x {} 2>/dev/null || true",
            shell_quote_path(spec.destination)
        ),
        format!(
            "printf '%s\\n' {} > {}",
            shell_quote_value(status),
            shell_quote_path(spec.applied_path)
        ),
        format!(
            "cd {} && nohup {} >/dev/null 2>&1 &",
            shell_quote_path(working_directory),
            shell_quote_path(spec.destination)
        ),
        "exit 0".to_owned(),
    ]
    .join("\n")
        + "\n"
}

#[cfg(not(windows))]
fn shell_quote_path(path: &Path) -> String {
    shell_quote_value(&path.to_string_lossy())
}

#[cfg(not(windows))]
fn shell_quote_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

