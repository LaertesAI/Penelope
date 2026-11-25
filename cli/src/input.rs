//! A native pointer for Wayland desktops without ydotool.
//!
//! ydotool needs root and a running daemon. The Linux kernel accepts a virtual mouse from `/dev/uinput`, and many
//! distributions give the desktop user an ACL on that device. `penelope input install` compiles a 100-line C helper
//! that moves the real cursor to a point and clicks; ScreenCodec can call it through `SDL_MCP_INPUT_CMD`. This is the
//! same path the live test used: counter 0 became 1 on a Chromium page.
//!
//! On Windows the PowerShell path in `platform.rs` does the injection; this helper is not used there.

use crate::config::Ctx;
use std::fs;
use std::path::PathBuf;

const SOURCE: &str = include_str!("../tools/uinput-click.c");

pub fn binary() -> PathBuf {
    crate::platform::home().join(".local").join("bin").join("penelope-uinput")
}

/// Compiles the helper into `~/.local/bin/penelope-uinput`.
pub fn install() -> Result<String, String> {
    let Some(gcc) = crate::config::which("gcc").or_else(|| crate::config::which("cc")) else {
        return Err("NOT installed. No C compiler (gcc or cc) on the PATH.".to_string());
    };
    let dir = crate::platform::home().join(".local").join("bin");
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    let src = crate::platform::home().join(".penelope").join("penelope-uinput.c");
    if let Some(parent) = src.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&src, SOURCE).map_err(|e| format!("cannot write {}: {}", src.display(), e))?;
    let out = binary();
    let result = std::process::Command::new(gcc).args(["-O2", "-o", &out.to_string_lossy(), &src.to_string_lossy()]).output()
        .map_err(|e| format!("the compiler could not start: {}", e))?;
    if !result.status.success() {
        return Err(format!("the compiler failed: {}", String::from_utf8_lossy(&result.stderr).trim()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&out, fs::Permissions::from_mode(0o755));
    }
    Ok(format!("helper installed: {}. Turn it on with: penelope input enable", out.display()))
}

/// Writes `screencodec.input_cmd` and turns desktop input on. Only a person should run this.
pub fn enable(ctx: &mut Ctx) -> Result<String, String> {
    if !binary().is_file() {
        return Err("NOT enabled. Run: penelope input install".to_string());
    }
    ctx.cfg["screencodec"]["input_cmd"] = serde_json::json!(binary().to_string_lossy());
    ctx.cfg["screencodec"]["desktop_input"] = serde_json::json!("on");
    // The person ran `input enable`: this is the explicit consent for screen capture and clicks.
    ctx.cfg["screencodec"]["desktop"] = serde_json::json!("on");
    fs::write(ctx.root.join("config.json"), serde_json::to_string_pretty(&ctx.cfg).unwrap())
        .map_err(|e| format!("cannot write config.json: {}", e))?;
    Ok(format!("desktop input is on, through {}. ScreenCodec will move and click the real pointer.", binary().display()))
}

pub fn status(ctx: &Ctx) -> String {
    let helper = if binary().is_file() { format!("installed at {}", binary().display()) } else { "not installed (penelope input install)".to_string() };
    let ydo = if crate::config::which("ydotool").is_some() { "ydotool on the PATH" } else { "no ydotool" };
    let mut out = format!("helper: {}\n{}\nWayland: {}\n", helper, ydo,
                          std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "no".into()));
    out.push_str(&format!("desktop input: {}\ninput command: {}\n", ctx.cfg["screencodec"]["desktop_input"].as_str().unwrap_or("off"),
                          ctx.cfg["screencodec"]["input_cmd"].as_str().unwrap_or("")));
    out.push_str("Install: penelope input install · enable: penelope input enable\n");
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_helper_source_is_embedded() {
        assert!(super::SOURCE.contains("UI_DEV_SETUP") && super::SOURCE.contains("REL_X"));
    }
}
