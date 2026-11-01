use nc_core::{NexusError, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_FILE_NAME_BYTES: usize = 255;

const INVALID_FILE_NAME_CHARS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
const RESERVED_BASE_NAMES: &[&str] = &["CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$"];

pub fn sanitize_file_name(name: impl AsRef<str>) -> Result<String> {
    let name = name.as_ref();
    if name.is_empty() {
        return Err(NexusError::invalid("文件名不能为空"));
    }
    if name == "." || name == ".." {
        return Err(NexusError::invalid("文件名不能是相对路径标记"));
    }
    if name
        .chars()
        .any(|character| character.is_control() || INVALID_FILE_NAME_CHARS.contains(&character))
    {
        return Err(NexusError::invalid(format!("文件名包含非法字符：{name}")));
    }

    let trimmed = name.trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        return Err(NexusError::invalid("文件名不能只由点或空格组成"));
    }

    let limited = truncate_name(trimmed);
    if limited.is_empty() || limited == "." || limited == ".." {
        return Err(NexusError::invalid("净化后的文件名是相对路径标记"));
    }
    ensure_not_reserved(&limited)?;
    Ok(limited)
}

pub fn unique_destination(dir: &Path, name: impl AsRef<str>) -> Result<PathBuf> {
    let safe_name = sanitize_file_name(name)?;
    for index in 0..10_000_u32 {
        let candidate_name = if index == 0 {
            safe_name.clone()
        } else {
            numbered_name(&safe_name, index)
        };
        let candidate = dir.join(candidate_name);
        if !path_is_taken(&candidate)? && !path_is_taken(&part_path(&candidate))? {
            return Ok(candidate);
        }
    }
    Err(NexusError::invalid("同名文件过多，无法生成唯一目标路径"))
}

pub fn default_download_dir() -> PathBuf {
    if let Some(user_dirs) = directories::UserDirs::new() {
        if let Some(download_dir) = user_dirs.download_dir() {
            return download_dir.to_path_buf();
        }
    }
    for variable in ["USERPROFILE", "HOME"] {
        if let Some(home) = std::env::var_os(variable) {
            if !home.is_empty() {
                return PathBuf::from(home).join("Downloads");
            }
        }
    }
    std::env::temp_dir()
}

pub(crate) fn part_path(destination: &Path) -> PathBuf {
    let mut part_name = destination.as_os_str().to_owned();
    part_name.push(".part");
    PathBuf::from(part_name)
}

fn path_is_taken(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn ensure_not_reserved(name: &str) -> Result<()> {
    let base = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let reserved = RESERVED_BASE_NAMES.contains(&base.as_str())
        || numbered_reserved(&base, "COM")
        || numbered_reserved(&base, "LPT");
    if reserved {
        return Err(NexusError::invalid(format!("文件名是系统保留名：{name}")));
    }
    Ok(())
}

fn numbered_reserved(base: &str, prefix: &str) -> bool {
    base.strip_prefix(prefix)
        .is_some_and(|suffix| matches!(suffix.as_bytes(), [b'1'..=b'9']))
}

fn truncate_name(name: &str) -> String {
    if name.len() <= MAX_FILE_NAME_BYTES {
        return name.to_owned();
    }

    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty());
    if let Some(extension) = extension {
        let suffix = format!(".{extension}");
        if suffix.len() < MAX_FILE_NAME_BYTES {
            let budget = MAX_FILE_NAME_BYTES - suffix.len();
            let stem = name.strip_suffix(&suffix).unwrap_or(name);
            let stem = trim_to_utf8_boundary(stem, budget)
                .trim_end_matches([' ', '.'])
                .trim();
            if !stem.is_empty() {
                return format!("{stem}{suffix}");
            }
        }
    }

    trim_to_utf8_boundary(name, MAX_FILE_NAME_BYTES)
        .trim_end_matches([' ', '.'])
        .to_owned()
}

fn trim_to_utf8_boundary(text: &str, maximum_bytes: usize) -> &str {
    if text.len() <= maximum_bytes {
        return text;
    }
    let mut boundary = maximum_bytes;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &text[..boundary]
}

fn numbered_name(name: &str, index: u32) -> String {
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(name);
    let extension = path.extension().and_then(|value| value.to_str());
    let suffix = match extension {
        Some(extension) => format!(" ({index}).{extension}"),
        None => format!(" ({index})"),
    };
    let generated = format!("{stem}{suffix}");
    if generated.len() <= MAX_FILE_NAME_BYTES {
        generated
    } else {
        truncate_name(&generated)
    }
}
