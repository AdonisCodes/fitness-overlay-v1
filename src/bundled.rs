//! Resolve bundled ffmpeg/ffprobe binaries shipped beside the executable.

use std::env;
use std::path::PathBuf;
use std::process::Command;

/// Path to ffmpeg: bundled next to the binary, then compile-time OUT_DIR, then PATH.
pub fn ffmpeg_path() -> PathBuf {
    resolve_tool("ffmpeg").unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

/// Path to ffprobe.
pub fn ffprobe_path() -> PathBuf {
    resolve_tool("ffprobe").unwrap_or_else(|| PathBuf::from("ffprobe"))
}

pub fn ffmpeg_command() -> Command {
    Command::new(ffmpeg_path())
}

pub fn ffprobe_command() -> Command {
    Command::new(ffprobe_path())
}

fn resolve_tool(name: &str) -> Option<PathBuf> {
    if let Ok(dir) = env::var("FITNESSOVERLAY_BUNDLED_BIN_DIR") {
        let p = PathBuf::from(&dir).join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            for p in [
                parent.join(name),
                parent.join("libexec").join(name),
                parent.join("bin").join(name),
            ] {
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    let built = PathBuf::from(env!("OUT_DIR")).join(name);
    if built.is_file() {
        return Some(built);
    }
    None
}

/// True when we are using a bundled binary (not bare PATH lookup).
pub fn using_bundled(tool: &str) -> bool {
    resolve_tool(tool).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_ffmpeg_path() {
        let p = ffmpeg_path();
        assert!(!p.as_os_str().is_empty());
    }
}
