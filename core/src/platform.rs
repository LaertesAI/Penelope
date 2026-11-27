//! The few places where Linux and Windows differ: the shell, a process tree, the clipboard, the cursor and the PATH.
//!
//! Linux is the tested platform. Windows code compiles and uses `cmd /C`, `taskkill`, `clip` and a PowerShell script for
//! the pointer; it is marked untested in the README, because no Windows machine was available.

use std::path::PathBuf;
use std::process::Command;

/// The home folder: `HOME` on Unix, `USERPROFILE` on Windows.
pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into()))
}

pub fn home_str() -> String {
    home().display().to_string()
}

/// The native pointer helper of a Wayland desktop without ydotool. The command line installs it, ScreenCodec calls it.
pub fn input_binary() -> PathBuf {
    home().join(".local").join("bin").join("penelope-uinput")
}

/// A shell command that runs one line. Unix uses `sh -c`, Windows `cmd /S /C "line"`.
///
/// On Windows the line goes to cmd unchanged. `Command::arg` would escape inner quotes as `\"`, which cmd does not
/// understand; with `/S` cmd removes only the outer pair of quotes and keeps the rest of the line as written.
pub fn shell(command: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        c.raw_arg(format!("/S /C \"{}\"", command));
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
}

/// One word for the line of `shell`: single quotes for sh, double quotes for cmd.
///
/// cmd keeps everything between double quotes as one word, so `&`, `|` and `>` inside lose their meaning. A double
/// quote inside becomes `""`, which programs built with the C runtime of Windows read as one literal quote.
pub fn shell_quote(value: &str) -> String {
    #[cfg(windows)]
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    }
    #[cfg(not(windows))]
    {
        crate::forge::quote(value)
    }
}

/// Ends a process and every process it started. True when a signal or taskkill was sent.
pub fn kill_tree(pid: u64) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        let out = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
        out.map(|o| o.status.success()).unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        let mut sent = false;
        for signal in ["-TERM", "-KILL"] {
            let out = Command::new("kill").args([signal, "--", &format!("-{}", pid)]).output();
            sent |= out.map(|o| o.status.success()).unwrap_or(false);
            if signal == "-TERM" {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
        sent
    }
}

/// True while a process with this id lives. On Unix the command line must hold `marker`, so a reused id does not count.
/// tasklist gives no command line, so Windows checks the id and start time only.
pub fn process_alive(pid: u64, marker: &str) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        let _ = marker;
        Command::new("tasklist").args(["/FI", &format!("PID eq {}", pid), "/NH"]).output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string())).unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        let line = std::fs::read(format!("/proc/{}/cmdline", pid)).unwrap_or_default();
        String::from_utf8_lossy(&line).contains(marker)
    }
    // macOS and the BSDs have no /proc; ps prints the command line of one process.
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        Command::new("ps").args(["-p", &pid.to_string(), "-o", "command="]).output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(marker)).unwrap_or(false)
    }
}

/// The program and arguments for the clipboard, if this system has one.
pub fn clipboard_program(write: bool) -> Option<(String, Vec<String>)> {
    let probe = |names: &[(&str, &[&str])]| -> Option<(String, Vec<String>)> {
        names.iter().find(|(p, _)| crate::config::which(p).is_some())
            .map(|(p, args)| (p.to_string(), args.iter().map(|a| a.to_string()).collect()))
    };
    #[cfg(windows)]
    {
        // clip writes, PowerShell reads. Neither needs a window.
        if write {
            probe(&[("clip", &[])])
        } else {
            Some(("powershell".to_string(), vec!["-NoProfile".to_string(), "-Command".to_string(), "Get-Clipboard -Raw".to_string()]))
        }
    }
    #[cfg(not(windows))]
    {
        if write {
            probe(&[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"]), ("pbcopy", &[])])
        } else {
            probe(&[("wl-paste", &["--no-newline"]), ("xclip", &["-selection", "clipboard", "-o"]), ("pbpaste", &[])])
        }
    }
}

/// Where the pointer is, for a check after a move, when the platform can say it.
pub fn pointer_position() -> Option<(i64, i64)> {
    #[cfg(windows)]
    {
        let script = "Add-Type -AssemblyName System.Windows.Forms; $p=[System.Windows.Forms.Cursor]::Position; Write-Output \"$($p.X) $($p.Y)\"";
        let out = Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", script]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut parts = text.split_whitespace();
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    }
    #[cfg(target_os = "macos")]
    {
        if crate::config::which("cliclick").is_some() {
            let out = Command::new("cliclick").arg("p").output().ok()?;
            let text = String::from_utf8_lossy(&out.stdout);
            let (x, y) = text.trim().split_once(',')?;
            return Some((x.trim().parse().ok()?, y.trim().parse().ok()?));
        }
        let script = "import Quartz\ne=Quartz.CGEventCreate(None)\np=Quartz.CGEventGetLocation(e)\nprint(int(p.x), int(p.y))";
        let out = Command::new("/usr/bin/python3").args(["-c", script]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut parts = text.split_whitespace();
        return Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if std::env::var("WAYLAND_DISPLAY").is_ok() && crate::config::which("hyprctl").is_some() {
            let out = Command::new("hyprctl").arg("cursorpos").output().ok()?;
            let text = String::from_utf8_lossy(&out.stdout);
            let (x, y) = text.trim().split_once(',')?;
            return Some((x.trim().parse().ok()?, y.trim().parse().ok()?));
        }
        if crate::config::which("xdotool").is_none() {
            return None;
        }
        let out = Command::new("xdotool").args(["getmouselocation", "--shell"]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let get = |key: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{}=", key))).and_then(|v| v.trim().parse::<i64>().ok());
        Some((get("X")?, get("Y")?))
    }
}

/// Moves the pointer with the system's own means. Returns the command text for the log.
pub fn move_pointer(x: i64, y: i64, click: bool) -> Result<String, String> {
    #[cfg(windows)]
    {
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public class P {{ [DllImport(\"user32.dll\")] public static extern bool SetCursorPos(int X, int Y); [DllImport(\"user32.dll\")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, int e); }}'; [P]::SetCursorPos({x},{y}) | Out-Null; {}",
            if click { "[P]::mouse_event(0x0002,0,0,0,0); [P]::mouse_event(0x0004,0,0,0,0)" } else { "" });
        let out = Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("PowerShell ended with {}", out.status));
        }
        Ok(format!("cursor at {}, {}", x, y))
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(cliclick) = crate::config::which("cliclick") {
            let action = if click { format!("c:{},{}", x, y) } else { format!("m:{},{}", x, y) };
            let out = Command::new(cliclick).arg(action).output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(format!("cliclick ended with {}", out.status));
            }
            return Ok(format!("cursor at {}, {}", x, y));
        }
        let script = format!(
            "import Quartz\ne=Quartz.CGEventCreateMouseEvent(None, Quartz.kCGEventMouseMoved, ({x},{y}), 0)\nQuartz.CGEventPost(Quartz.kCGHIDEventTap,e)\n{}",
            if click { format!("d=Quartz.CGEventCreateMouseEvent(None, Quartz.kCGEventLeftMouseDown, ({x},{y}), 0)\nQuartz.CGEventPost(Quartz.kCGHIDEventTap,d)\nu=Quartz.CGEventCreateMouseEvent(None, Quartz.kCGEventLeftMouseUp, ({x},{y}), 0)\nQuartz.CGEventPost(Quartz.kCGHIDEventTap,u)") } else { String::new() });
        let out = Command::new("/usr/bin/python3").args(["-c", &script]).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("Quartz ended with {}. Install cliclick or use the python3 of macOS with PyObjC.", out.status));
        }
        Ok(format!("cursor at {}, {}", x, y))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = click;          // the Unix path clicks after the move in cursor.rs
        if std::env::var("WAYLAND_DISPLAY").is_ok() && crate::config::which("hyprctl").is_some() {
            let code = format!("hl.dispatch(hl.dsp.cursor.move({{x={},y={}}}))", x, y);
            let out = Command::new("hyprctl").args(["eval", &code]).output().map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            if out.status.success() && text.trim() == "ok" {
                return Ok(format!("cursor at {}, {}", x, y));
            }
        }
        if crate::config::which("xdotool").is_some() {
            let out = Command::new("xdotool").args(["mousemove", &x.to_string(), &y.to_string()]).output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(format!("xdotool ended with {}", out.status));
            }
            return Ok(format!("cursor at {}, {}", x, y));
        }
        if crate::config::which("ydotool").is_some() {
            let out = Command::new("ydotool").args(["mousemove", "--absolute", &x.to_string(), &y.to_string()]).output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(format!("ydotool ended with {}", out.status));
            }
            return Ok(format!("cursor at {}, {}", x, y));
        }
        Err("no cursor program".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_runs_one_line_on_this_platform() {
        let out = shell("echo penelope-platform").output().unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "penelope-platform");
        assert!(!process_alive(0, "x"));
    }
}
