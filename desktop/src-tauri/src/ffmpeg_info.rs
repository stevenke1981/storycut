//! Read-only FFmpeg environment probe for the desktop status chip. It only runs
//! `ffmpeg -version` / `-encoders` from PATH; it never downloads or installs anything.

use serde_json::{Value, json};
use std::process::{Command, Stdio};

const WANTED_ENCODERS: [&str; 3] = ["libx264", "aac", "h264_nvenc"];

pub fn read() -> Value {
    let version = run(&["-hide_banner", "-version"]).map(|text| parse_version(&text));
    let Some(version) = version else {
        return json!({"available": false, "version": null, "encoders": {}});
    };
    let encoders = run(&["-hide_banner", "-encoders"]).unwrap_or_default();
    let mut found = serde_json::Map::new();
    for name in WANTED_ENCODERS {
        found.insert(name.to_owned(), Value::Bool(has_encoder(&encoders, name)));
    }
    json!({"available": true, "version": version, "encoders": found})
}

fn run(args: &[&str]) -> Option<String> {
    let mut command = Command::new("ffmpeg");
    command.args(args).stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_version(text: &str) -> Option<String> {
    let first = text.lines().next()?;
    first.strip_prefix("ffmpeg version ")?.split_whitespace().next().map(str::to_owned)
}

fn has_encoder(listing: &str, name: &str) -> bool {
    listing.lines().any(|line| {
        let mut parts = line.split_whitespace();
        matches!((parts.next(), parts.next()), (Some(flags), Some(found)) if (flags.starts_with('V') || flags.starts_with('A')) && found == name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_and_encoder_rows() {
        assert_eq!(parse_version("ffmpeg version 7.1-essentials_build Copyright"), Some("7.1-essentials_build".into()));
        assert_eq!(parse_version("not ffmpeg"), None);
        let listing = " V....D libx264              libx264 H.264\n A....D aac                  AAC\n V....D h264_nvenc2 fake\n";
        assert!(has_encoder(listing, "libx264"));
        assert!(has_encoder(listing, "aac"));
        assert!(!has_encoder(listing, "h264_nvenc"));
    }

    #[test]
    fn real_probe_reports_x264_when_ffmpeg_is_installed() {
        let info = read();
        if info["available"] == true {
            assert_eq!(info["encoders"]["libx264"], true);
        }
    }
}
