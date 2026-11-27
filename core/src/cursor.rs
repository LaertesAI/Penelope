//! Penelope's own cursor.
//!
//! The terminal can name a point on the screen, but only a program of the desktop can put a pointer there. This module
//! uses `cursor.command` from `config.json` when it is set, else `xdotool` (X11) or `ydotool` (Wayland). The command
//! gets `{x}`, `{y}` and `{label}` as shell-quoted words. Without such a program the module says so and changes nothing.

use crate::config::Ctx;
use serde_json::Value;
#[cfg(all(not(windows), not(target_os = "macos")))]
use std::process::Command;

/// What this PC can use for the cursor. The first line is for `penelope doctor`.
pub fn backend(ctx: &Ctx) -> (String, bool) {
    let custom = ctx.cfg["cursor"]["command"].as_str().unwrap_or("").trim();
    if !custom.is_empty() {
        return (format!("cursor.command: {}", custom.chars().take(80).collect::<String>()), true);
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(p) = crate::config::which("cliclick") {
            return (format!("cliclick: {}", p.display()), true);
        }
        if std::path::Path::new("/usr/bin/python3").is_file() {
            return ("macOS Quartz through /usr/bin/python3".to_string(), true);
        }
    }
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        if let Some(p) = crate::config::which("hyprctl") {
            return (format!("hyprctl native Wayland cursor: {}", p.display()), true);
        }
    }
    if let Some(p) = crate::config::which("xdotool") {
        return (format!("xdotool: {}", p.display()), true);
    }
    if let Some(p) = crate::config::which("ydotool") {
        return (format!("ydotool: {}", p.display()), true);
    }
    ("no cursor program: set cursor.command, or install xdotool, ydotool or cliclick".to_string(), false)
}

fn run(ctx: &Ctx, x: i64, y: i64, label: &str, click: bool) -> Result<String, String> {
    if !ctx.cfg["cursor"]["enabled"].as_bool().unwrap_or(true) {
        return Err("NOT moved. The cursor is off (cursor.enabled).".to_string());
    }
    let custom = ctx.cfg["cursor"]["command"].as_str().unwrap_or("").trim().to_string();
    if !custom.is_empty() {
        let command = custom.replace("{x}", &crate::platform::shell_quote(&x.to_string()))
            .replace("{y}", &crate::platform::shell_quote(&y.to_string()))
            .replace("{label}", &crate::platform::shell_quote(label));
        let out = crate::platform::shell(&command).output().map_err(|e| format!("NOT moved. {}", e))?;
        if !out.status.success() {
            return Err(format!("NOT moved. {} ended with {}", custom, out.status));
        }
        if click {
            #[cfg(any(windows, target_os = "macos"))]
            crate::platform::move_pointer(x, y, true)?;
            #[cfg(all(not(windows), not(target_os = "macos")))]
            click_at(x, y)?;
        }
        return Ok(format!("cursor at {}, {}{}", x, y, if label.is_empty() { String::new() } else { format!(" ({})", label) }));
    }
    // The platform moves the pointer; Windows also clicks in the same PowerShell run.
    let (moved_x, moved_y) = platform_move(ctx, x, y, click)?;
    if let Some((got_x, got_y)) = crate::platform::pointer_position() {
        if (got_x - moved_x).abs() > 2 || (got_y - moved_y).abs() > 2 {
            return Err(format!("NOT moved. The command ran but the pointer stayed at {}, {} (Wayland?). Install ydotool with a running ydotoold, or set cursor.command.", got_x, got_y));
        }
    }
    Ok(format!("cursor at {}, {}{}", x, y, if label.is_empty() { String::new() } else { format!(" ({})", label) }))
}

/// The move through the platform. On Unix the click is a second command, on Windows the script moves and clicks.
fn platform_move(ctx: &Ctx, x: i64, y: i64, click: bool) -> Result<(i64, i64), String> {
    let _ = ctx;
    #[cfg(any(windows, target_os = "macos"))]
    {
        crate::platform::move_pointer(x, y, click)?;
        Ok((x, y))
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        crate::platform::move_pointer(x, y, false)?;
        if click {
            click_at(x, y)?;
        }
        Ok((x, y))
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn click_at(x: i64, y: i64) -> Result<(), String> {
    // Wayland without ydotool can use the uinput helper; X11 uses xdotool.
    if std::env::var("WAYLAND_DISPLAY").is_ok() && crate::config::which("ydotool").is_none() {
        let helper = crate::platform::input_binary();
        if helper.is_file() {
            let out = Command::new(helper).args([x.to_string(), y.to_string(), "1".to_string()]).output().map_err(|e| e.to_string())?;
            return if out.status.success() { Ok(()) } else { Err("the uinput helper failed".to_string()) };
        }
        return Err("NOT clicked. Wayland needs ydotool or: penelope input install && penelope input enable".to_string());
    }
    let (program, args): (&str, &[&str]) = if crate::config::which("xdotool").is_some() { ("xdotool", &["click", "1"]) } else { ("ydotool", &["click", "0xC0"]) };
    let out = Command::new(program).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(format!("the click with {} failed", program)) }
}

/// Moves the cursor without a tool call. Used before a click of ScreenCodec, where a failure is not an error.
pub fn move_to(ctx: &Ctx, x: i64, y: i64, label: &str) -> Option<String> {
    run(ctx, x, y, label, false).ok()
}

/// The tool `cursor`.
pub fn tool(ctx: &Ctx, args: &Value) -> Result<String, String> {
    let x = args["x"].as_i64().unwrap_or(0);
    let y = args["y"].as_i64().unwrap_or(0);
    if !(0..=100_000).contains(&x) || !(0..=100_000).contains(&y) {
        return Err("NOT moved. Give x and y in screen pixels.".to_string());
    }
    let label = args["label"].as_str().unwrap_or("");
    run(ctx, x, y, label, args["click"].as_bool().unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn a_command_receives_quoted_coordinates_and_a_label() {
        let dir = std::env::temp_dir().join(format!("penelope-cursor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log");
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        #[cfg(not(windows))]
        let (command, label, written) = (format!("printf '%s|%s|%s' {{x}} {{y}} {{label}} > {}", log.display()), "Save; rm -rf /", "640|360|Save; rm -rf /");
        // cmd has no printf; echo writes the quoted words as they are. The & stays inside the label and starts nothing.
        #[cfg(windows)]
        let (command, label, written) = (format!("echo {{x}} {{y}} {{label}}> {}", log.display()), "Save & echo injected", "\"640\" \"360\" \"Save & echo injected\"");
        ctx.cfg["cursor"] = json!({"command": command});
        assert!(backend(&ctx).1);
        let said = tool(&ctx, &json!({"x": 640, "y": 360, "label": label})).unwrap();
        assert!(said.contains("640, 360"), "{}", said);
        assert_eq!(fs::read_to_string(&log).unwrap().trim_end(), written);
        ctx.cfg["cursor"] = json!({"enabled": false});
        assert!(tool(&ctx, &json!({"x": 1, "y": 2})).unwrap_err().contains("cursor is off"));
        let _ = fs::remove_dir_all(&dir);
    }
}
