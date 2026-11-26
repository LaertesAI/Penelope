//! Factories: a cluster profile with one schedule and one purpose.
//!
//! A factory is meant to run while Penelope is closed. The schedule therefore lives in the timer of the operating
//! system: a systemd user timer on Linux, a launchd agent on macOS and a Task Scheduler task on Windows. Penelope
//! writes the unit files and enables the timer only after the user said yes. Every timer starts the normal command
//! `penelope cluster run --profile PROFILE GOAL`; there is no second run loop inside this module.
//!
//! The registry is `.penelope/factories.json`. The profile of a factory is a copy of the base profile under
//! `.penelope/clusters/profiles/NAME.json`, with the schedule fields `every` or `at` and the one `purpose`.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// A schedule shorter than this would start a model more often than a person can read the result.
const MIN_EVERY: u64 = 10;
/// A week. A factory that runs less often than this is a calendar entry, not a factory.
const MAX_EVERY: u64 = 7 * 24 * 3600;

#[derive(Clone, Debug, PartialEq)]
pub struct Factory {
    pub name: String,
    /// The profile the factory was copied from, for the list only.
    pub base: String,
    /// The profile the timer runs. It is the factory's own copy and carries the schedule.
    pub profile: String,
    pub goal: String,
    pub every: Option<String>,
    pub at: Option<String>,
    /// True when the unit is installed and enabled in the scheduler of this machine.
    pub armed: bool,
    /// `systemd`, `launchd`, `taskscheduler` or empty.
    pub installed: String,
    pub created: u64,
    pub runs: u64,
    pub last_run: u64,
    pub last_result: String,
}

/// One file of an operating-system timer. `name` is relative to the scheduler folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    pub name: String,
    pub body: String,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect()
}

/// A factory name is a profile name: lower case letters, digits, `_` or `-`, at most 32.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && name.len() <= 32
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Normalizes `3h`, `30m`, `45s` or `1d` (10 s to 7 d). The returned text is what the timer carries.
pub fn normalize_every(text: &str) -> Result<String, String> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return Err("every needs a duration such as 3h or 30m".to_string());
    }
    let end = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    if end == 0 {
        return Err(format!("every needs a number and s, m, h or d, got {}", text));
    }
    let number: u64 = text[..end].parse().map_err(|_| format!("every needs a whole number, got {}", text))?;
    let unit = text[end..].trim();
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return Err(format!("every needs s, m, h or d after the number, got {}", text)),
    };
    let seconds = number.checked_mul(multiplier).ok_or_else(|| format!("every is too large: {}", text))?;
    if seconds < MIN_EVERY {
        return Err(format!("every is at least {}s, got {}", MIN_EVERY, text));
    }
    if seconds > MAX_EVERY {
        return Err(format!("every is at most 7d, got {}", text));
    }
    Ok(format!("{}{}", number, unit))
}

/// Seconds of a normalized `every` text.
#[allow(dead_code)]          // used by the launchd and Task Scheduler renderers; Linux tests call it too
pub fn every_seconds(text: &str) -> Result<u64, String> {
    let text = normalize_every(text)?;
    let end = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    let number: u64 = text[..end].parse().map_err(|_| format!("every needs a whole number, got {}", text))?;
    Ok(number * match &text[end..] { "m" => 60, "h" => 3600, "d" => 86_400, _ => 1 })
}

/// Normalizes a time of day to `HH:MM`.
pub fn normalize_at(text: &str) -> Result<String, String> {
    let text = text.trim();
    let (hour, minute) = text.split_once(':').ok_or_else(|| format!("at needs a time of day HH:MM, got {}", text))?;
    let digits = |s: &str| !s.is_empty() && s.len() <= 2 && s.chars().all(|c| c.is_ascii_digit());
    if !digits(hour) || !digits(minute) {
        return Err(format!("at needs a time of day HH:MM, got {}", text));
    }
    let hour: u8 = hour.parse().map_err(|_| format!("at has no hour, got {}", text))?;
    let minute: u8 = minute.parse().map_err(|_| format!("at has no minute, got {}", text))?;
    if hour > 23 || minute > 59 {
        return Err(format!("at is between 00:00 and 23:59, got {}", text));
    }
    Ok(format!("{:02}:{:02}", hour, minute))
}

fn registry_path(ctx: &Ctx) -> PathBuf {
    ctx.root.join("factories.json")
}

fn from_value(v: &Value) -> Option<Factory> {
    let name = v["name"].as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(Factory {
        name: name.to_string(),
        base: v["base"].as_str().unwrap_or("").to_string(),
        profile: v["profile"].as_str().filter(|s| !s.is_empty()).unwrap_or(name).to_string(),
        goal: v["goal"].as_str().unwrap_or("").to_string(),
        every: v["every"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
        at: v["at"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
        armed: v["armed"].as_bool().unwrap_or(false),
        installed: v["installed"].as_str().unwrap_or("").to_string(),
        created: v["created"].as_u64().unwrap_or(0),
        runs: v["runs"].as_u64().unwrap_or(0),
        last_run: v["last_run"].as_u64().unwrap_or(0),
        last_result: v["last_result"].as_str().unwrap_or("").to_string(),
    })
}

fn to_value(f: &Factory) -> Value {
    json!({
        "name": f.name, "base": f.base, "profile": f.profile, "goal": f.goal,
        "every": f.every, "at": f.at, "armed": f.armed, "installed": f.installed,
        "created": f.created, "runs": f.runs, "last_run": f.last_run, "last_result": f.last_result
    })
}

/// Every factory, sorted by name. A broken registry is reported by the caller, never silently replaced.
fn load(ctx: &Ctx) -> Result<Vec<Factory>, String> {
    let path = registry_path(ctx);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Ok(Vec::new()),
    };
    let value: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not JSON: {}", path.display(), e))?;
    let list = match value {
        Value::Array(list) => list,
        Value::Object(mut object) => object.remove("factories").and_then(|v| v.as_array().cloned()).unwrap_or_default(),
        _ => return Err(format!("{} must hold a list of factories", path.display())),
    };
    let mut out: Vec<Factory> = list.iter().filter_map(from_value).collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn save(ctx: &Ctx, list: &[Factory]) -> Result<(), String> {
    let path = registry_path(ctx);
    let body = json!({"factories": list.iter().map(to_value).collect::<Vec<_>>()});
    let tmp = path.with_extension("json.tmp");
    fs::create_dir_all(&ctx.root).map_err(|e| format!("cannot make {}: {}", ctx.root.display(), e))?;
    fs::write(&tmp, serde_json::to_string_pretty(&body).unwrap()).map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
    fs::rename(&tmp, &path).map_err(|e| format!("cannot move {}: {}", tmp.display(), e))
}

fn find(ctx: &Ctx, name: &str) -> Result<Factory, String> {
    load(ctx)?.into_iter().find(|f| f.name == name)
        .ok_or_else(|| format!("no factory {}. penelope factory list shows the names.", name))
}

fn project_dir(ctx: &Ctx) -> PathBuf {
    ctx.root.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| ctx.root.clone())
}

fn log_dir(ctx: &Ctx, name: &str) -> PathBuf {
    ctx.root.join("factories").join(safe(name))
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(1000).collect()
}

fn schedule_text(f: &Factory) -> String {
    match (&f.every, &f.at) {
        (Some(every), _) => format!("every {}", every),
        (_, Some(at)) => format!("daily at {}", at),
        _ => "no schedule".to_string(),
    }
}

/// The command every timer starts. It is the normal cluster command; a factory has no runner of its own.
fn runner_args(ctx: &Ctx, f: &Factory) -> Vec<String> {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "penelope".to_string());
    vec![exe, "--root".to_string(), ctx.root.to_string_lossy().to_string(),
         "cluster".to_string(), "run".to_string(), "--profile".to_string(), f.profile.clone(), f.goal.clone()]
}

fn runner_text(ctx: &Ctx, f: &Factory) -> String {
    runner_args(ctx, f).iter().map(|a| sd_quote(a)).collect::<Vec<_>>().join(" ")
}

fn platform_name() -> &'static str {
    #[cfg(target_os = "macos")]
    { "launchd" }
    #[cfg(windows)]
    { "taskscheduler" }
    #[cfg(not(any(target_os = "macos", windows)))]
    { "systemd" }
}

/// The folder of this platform's user scheduler. Linux honours `XDG_CONFIG_HOME`.
fn unit_dir(ctx: &Ctx, f: &Factory) -> PathBuf {
    let _ = ctx;
    #[cfg(target_os = "macos")]
    {
        let _ = f;
        crate::platform::home().join("Library/LaunchAgents")
    }
    #[cfg(windows)]
    {
        log_dir(ctx, &f.name).join("scheduler")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = f;
        let base = std::env::var("XDG_CONFIG_HOME").ok().filter(|s| !s.trim().is_empty())
            .map(PathBuf::from).unwrap_or_else(|| crate::platform::home().join(".config"));
        base.join("systemd/user")
    }
}

/// One argument for a systemd `ExecStart`. A plain argument stays plain, so the unit is readable. Quotes group a path
/// with spaces; `%` and `$` are escaped so the unit is literal and no specifier or environment expansion changes the
/// goal.
fn sd_quote(text: &str) -> String {
    if text.is_empty() {
        return "\"\"".to_string();
    }
    if !text.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '\\' | '%' | '$')) {
        return text.to_string();
    }
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            '\n' | '\r' => out.push(' '),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[allow(dead_code)]          // the macOS renderer; its text is still tested on every host
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

#[allow(dead_code)]          // the Linux unit; its text is checked by a test on every host
fn systemd_units(ctx: &Ctx, f: &Factory) -> Vec<Unit> {
    let schedule = match (&f.every, &f.at) {
        (Some(every), _) => format!("OnActiveSec={}\nOnUnitActiveSec={}\nAccuracySec=1min", every, every),
        (_, Some(at)) => format!("OnCalendar=*-*-* {}:00\nPersistent=true\nAccuracySec=1min", at),
        _ => String::new(),
    };
    let path = std::env::var("PATH").ok().filter(|p| !p.trim().is_empty())
        .map(|p| format!("Environment={}\n", sd_quote(&format!("PATH={}", p)))).unwrap_or_default();
    // `%` is a specifier in a unit file, even in Description. The one goal line is escaped, not interpreted.
    let description = one_line(&f.goal).replace('%', "%%");
    let mut service = format!(
        "[Unit]\nDescription=Penelope factory {}: {}\nAfter=network-online.target\nWants=network-online.target\n\n\
         [Service]\nType=oneshot\nWorkingDirectory={}\n{}ExecStart={}\nTimeoutStartSec=infinity\nNice=10\n",
        f.name, description, sd_quote(&project_dir(ctx).to_string_lossy()), path, runner_text(ctx, f));
    service.push_str("\n[Install]\nWantedBy=default.target\n");
    let timer = format!(
        "[Unit]\nDescription=Penelope factory {} · {}\n\n[Timer]\n{}\nUnit=penelope-factory-{}.service\n\n\
         [Install]\nWantedBy=timers.target\n",
        f.name, schedule_text(f), schedule, f.name);
    vec![
        Unit { name: format!("penelope-factory-{}.service", f.name), body: service },
        Unit { name: format!("penelope-factory-{}.timer", f.name), body: timer },
    ]
}

#[allow(dead_code)]          // macOS unit; the text is checked by a test on every host
fn launchd_units(ctx: &Ctx, f: &Factory) -> Vec<Unit> {
    let label = format!("ai.laertes.penelope.factory.{}", f.name);
    let args = runner_args(ctx, f).iter().map(|a| format!("    <string>{}</string>\n", xml_escape(a))).collect::<String>();
    let trigger = match (&f.every, &f.at) {
        (Some(every), _) => format!("  <key>StartInterval</key>\n  <integer>{}</integer>\n", every_seconds(every).unwrap_or(3600)),
        (_, Some(at)) => {
            let (hour, minute) = at.split_once(':').unwrap_or(("0", "0"));
            format!("  <key>StartCalendarInterval</key>\n  <dict>\n    <key>Hour</key>\n    <integer>{}</integer>\n    <key>Minute</key>\n    <integer>{}</integer>\n  </dict>\n", hour, minute)
        }
        _ => String::new(),
    };
    let log = log_dir(ctx, &f.name);
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\
         \x20 <key>Label</key>\n  <string>{}</string>\n  <key>ProgramArguments</key>\n  <array>\n{}  </array>\n\
         \x20 <key>WorkingDirectory</key>\n  <string>{}</string>\n{}  <key>RunAtLoad</key>\n  <false/>\n\
         \x20 <key>StandardOutPath</key>\n  <string>{}</string>\n  <key>StandardErrorPath</key>\n  <string>{}</string>\n\
         </dict>\n</plist>\n",
        label, args, xml_escape(&project_dir(ctx).to_string_lossy()), trigger,
        xml_escape(&log.join("launchd.out.log").to_string_lossy()), xml_escape(&log.join("launchd.err.log").to_string_lossy()));
    vec![Unit { name: format!("{}.plist", label), body: plist }]
}

/// Quotes one argument for `CommandLineToArgvW`, which Task Scheduler uses.
#[allow(dead_code)]
fn win_quote(text: &str) -> String {
    if !text.is_empty() && !text.chars().any(|c| c.is_whitespace() || c == '"') {
        return text.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in text.chars() {
        match c {
            '\\' => { backslashes += 1; out.push('\\'); }
            '"' => {
                for _ in 0..=backslashes { out.push('\\'); }
                backslashes = 0;
                out.push('"');
            }
            _ => { backslashes = 0; out.push(c); }
        }
    }
    for _ in 0..backslashes { out.push('\\'); }
    out.push('"');
    out
}

#[allow(dead_code)]
fn task_name(f: &Factory) -> String {
    format!("PenelopeFactory-{}", f.name)
}

#[allow(dead_code)]          // Windows unit; the text is checked by a test on every host
fn windows_units(ctx: &Ctx, f: &Factory) -> Vec<Unit> {
    let stamp = crate::sessions::stamp(now());
    let (date, clock) = stamp.split_once(' ').unwrap_or((&stamp, "00:00"));
    let trigger = match (&f.every, &f.at) {
        (Some(every), _) => {
            let interval = every_seconds(every).unwrap_or(3600);
            format!("    <TimeTrigger>\n      <StartBoundary>{}T{}:00</StartBoundary>\n      \
                     <Enabled>true</Enabled>\n      <Repetition>\n        <Interval>PT{}S</Interval>\n        \
                     <StopAtDurationEnd>false</StopAtDurationEnd>\n      </Repetition>\n    </TimeTrigger>\n",
                    date, clock, interval)
        }
        (_, Some(at)) => format!(
            "    <CalendarTrigger>\n      <StartBoundary>{}T{}:00</StartBoundary>\n      <Enabled>true</Enabled>\n      \
             <ScheduleByDay>\n        <DaysInterval>1</DaysInterval>\n      </ScheduleByDay>\n    </CalendarTrigger>\n",
            date, at),
        _ => String::new(),
    };
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Task version=\"1.4\" \
         xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n  <RegistrationInfo>\n    \
         <Description>Penelope factory {}: {}</Description>\n  </RegistrationInfo>\n  <Triggers>\n{}  </Triggers>\n  \
         <Principals>\n    <Principal id=\"Author\">\n      <LogonType>InteractiveToken</LogonType>\n      \
         <RunLevel>LeastPrivilege</RunLevel>\n    </Principal>\n  </Principals>\n  <Settings>\n    \
         <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n    <StartWhenAvailable>true</StartWhenAvailable>\n    \
         <AllowHardTerminate>true</AllowHardTerminate>\n    <ExecutionTimeLimit>PT6H</ExecutionTimeLimit>\n    \
         <Enabled>true</Enabled>\n  </Settings>\n  <Actions Context=\"Author\">\n    <Exec>\n      \
         <Command>{}</Command>\n      <Arguments>{}</Arguments>\n      <WorkingDirectory>{}</WorkingDirectory>\n    \
         </Exec>\n  </Actions>\n</Task>\n",
        f.name, xml_escape(&one_line(&f.goal)), trigger,
        xml_escape(&runner_args(ctx, f)[0]),
        xml_escape(&runner_args(ctx, f)[1..].iter().map(|a| win_quote(a)).collect::<Vec<_>>().join(" ")),
        xml_escape(&project_dir(ctx).to_string_lossy()));
    vec![Unit { name: format!("{}.xml", task_name(f)), body: xml }]
}

/// The files this operating system needs for one factory.
pub fn units(ctx: &Ctx, f: &Factory) -> Vec<Unit> {
    #[cfg(target_os = "macos")]
    { launchd_units(ctx, f) }
    #[cfg(windows)]
    { windows_units(ctx, f) }
    #[cfg(not(any(target_os = "macos", windows)))]
    { systemd_units(ctx, f) }
}

/// Writes rendered units into a folder. The tests use a temporary folder; the real scheduler folder is only used
/// after the user answered yes.
fn write_units_to(dir: &Path, units: &[Unit]) -> Result<Vec<PathBuf>, String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {}", dir.display(), e))?;
    let mut written = Vec::new();
    for unit in units {
        let path = dir.join(&unit.name);
        fs::write(&path, &unit.body).map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
        written.push(path);
    }
    Ok(written)
}

fn report(ctx: &Ctx, f: &Factory) -> String {
    let dir = unit_dir(ctx, f);
    let mut out = format!(
        "factory {}\n  profile: {} (copied from {})\n  purpose: {}\n  schedule: {}\n  state: {}\n  runs: {}{}\n  runner: {}\n  scheduler: {} · {}\n",
        f.name, f.profile, if f.base.is_empty() { "?" } else { &f.base }, one_line(&f.goal), schedule_text(f),
        if f.armed { "armed" } else { "not armed" }, f.runs,
        if f.last_run > 0 { format!(" · last {} · {}", crate::sessions::stamp(f.last_run), f.last_result) } else { " · never run".to_string() },
        runner_text(ctx, f), platform_name(), dir.display());
    for unit in units(ctx, f) {
        out.push_str(&format!("\n--- {} ---\n{}\n", dir.join(&unit.name).display(), unit.body));
    }
    out
}

fn run_cmd(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("cannot start {}: {}", program, e))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text.trim().to_string())
    } else {
        Err(format!("{} {} ended with {}: {}", program, args.join(" "), out.status, text.trim()))
    }
}

fn scheduler_enable(ctx: &Ctx, f: &Factory) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let plist = unit_dir(ctx, f).join(format!("ai.laertes.penelope.factory.{}.plist", f.name));
        let uid = run_cmd("id", &["-u"])?;
        run_cmd("launchctl", &["bootstrap", &format!("gui/{}", uid.trim()), &plist.to_string_lossy()])
    }
    #[cfg(windows)]
    {
        let xml = unit_dir(ctx, f).join(format!("{}.xml", task_name(f)));
        run_cmd("schtasks", &["/Create", "/TN", &task_name(f), "/XML", &xml.to_string_lossy(), "/F"])
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = ctx;
        let _ = run_cmd("systemctl", &["--user", "daemon-reload"]).map_err(|e| {
            format!("the unit files are written, but systemd did not reload: {}. Run `systemctl --user daemon-reload` by hand.", e)
        })?;
        run_cmd("systemctl", &["--user", "enable", "--now", &format!("penelope-factory-{}.timer", f.name)])
    }
}

fn scheduler_disable(ctx: &Ctx, f: &Factory) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = ctx;
        let uid = run_cmd("id", &["-u"])?;
        run_cmd("launchctl", &["bootout", &format!("gui/{}", uid.trim()), &format!("ai.laertes.penelope.factory.{}", f.name)])
    }
    #[cfg(windows)]
    {
        let _ = ctx;
        run_cmd("schtasks", &["/Delete", "/TN", &task_name(f), "/F"])
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = ctx;
        let said = run_cmd("systemctl", &["--user", "disable", "--now", &format!("penelope-factory-{}.timer", f.name)]);
        let _ = run_cmd("systemctl", &["--user", "daemon-reload"]);
        said
    }
}

/// Creates the factory profile and registry entry. Nothing is installed and no scheduler is touched unless `yes`.
/// `dry` renders the units and writes neither the profile nor the registry.
pub fn new_factory(ctx: &Ctx, name: &str, base_name: &str, every: Option<&str>, at: Option<&str>, goal: &str,
                   dry: bool, yes: bool) -> Result<String, String> {
    let name = name.trim();
    let base_name = base_name.trim();
    if name.is_empty() || !valid_name(name) {
        return Err("NOT created. A factory name is 1 to 32 lower case letters, digits, _ or -, starting with a letter.".to_string());
    }
    if base_name.is_empty() {
        return Err("NOT created. --profile names the cluster profile the factory is copied from.".to_string());
    }
    if dry && yes {
        return Err("choose --dry (show the units, change nothing) or --yes (install), not both.".to_string());
    }
    if let Some(existing) = load(ctx)?.into_iter().find(|f| f.name == name) {
        return Err(format!("NOT created. factory {} exists ({}). Stop it first or choose another name.", name, schedule_text(&existing)));
    }
    if crate::clusters::BUILTIN_PROFILES.contains(&name) || crate::clusters::profile_names(ctx).iter().any(|n| n == name) {
        return Err(format!("NOT created. a cluster profile named {} exists. Choose a name for the factory itself; --profile is its base.", name));
    }
    let base = crate::clusters::profile(ctx, base_name)?;
    if let Some(part) = base.workers.iter().find(|p| p.role == "human") {
        return Err(format!("NOT created. part {} waits for a person, and nobody can answer while Penelope is closed. Choose a profile without a human part.", part.name));
    }
    let (every, at) = if every.is_some() || at.is_some() {
        let every = match every.map(str::trim).filter(|s| !s.is_empty()) {
            Some(text) => Some(normalize_every(text)?),
            None => None,
        };
        let at = match at.map(str::trim).filter(|s| !s.is_empty()) {
            Some(text) => Some(normalize_at(text)?),
            None => None,
        };
        (every, at)
    } else {
        // A base profile may already carry the schedule of a factory. Without a flag the copy keeps it.
        (base.every.clone(), base.at.clone())
    };
    if every.is_some() && at.is_some() {
        return Err("NOT created. give --every 3h or --at HH:MM, not both.".to_string());
    }
    if every.is_none() && at.is_none() {
        return Err("NOT created. give --every 3h or --at HH:MM (the base profile has no schedule).".to_string());
    }
    let goal = one_line(goal);
    let goal = if goal.is_empty() {
        base.purpose.clone().unwrap_or_else(|| format!("Do the one purpose of the {} profile and report what changed.", base.name))
    } else { goal };
    let f = Factory {
        name: name.to_string(), base: base.name.clone(), profile: name.to_string(), goal: goal.clone(),
        every: every.clone(), at: at.clone(), armed: false, installed: String::new(), created: now(),
        runs: 0, last_run: 0, last_result: String::new(),
    };
    if dry {
        return Ok(format!("Dry run · no file written, no timer installed.\n\n{}", report(ctx, &f)));
    }
    // The factory's profile is a copy, so a later change of the base does not silently change a running factory.
    let mut value = crate::clusters::profile_json(&base, name);
    value["every"] = json!(every);
    value["at"] = json!(at);
    value["purpose"] = json!(goal);
    crate::clusters::profile_upsert(ctx, name, &value)?;
    let mut all = load(ctx)?;
    all.push(f.clone());
    save(ctx, &all)?;
    let mut out = format!("factory {} created. Its profile is .penelope/clusters/profiles/{}.json.\n\n{}", name, name, report(ctx, &f));
    if yes {
        out.push_str(&format!("\n{}", install(ctx, name, false, true)?));
    } else {
        out.push_str(&format!("\nNot installed yet. The unit files are not written and no timer is enabled.\n\
                               Show them with `penelope factory install {}` and install after a yes with\n\
                               `penelope factory install {} --yes`.\n", name, name));
    }
    Ok(out)
}

/// Shows the units and, only with `yes`, writes them into the user scheduler and enables the timer.
pub fn install(ctx: &Ctx, name: &str, dry: bool, yes: bool) -> Result<String, String> {
    let mut f = find(ctx, name)?;
    // A factory profile that disappeared is reported before a timer would start a broken command.
    crate::clusters::profile(ctx, &f.profile)?;
    if dry && yes {
        return Err("choose --dry (show the units, change nothing) or --yes (install), not both.".to_string());
    }
    let dir = unit_dir(ctx, &f);
    let units = units(ctx, &f);
    let mut out = report(ctx, &f);
    if dry || !yes {
        out.push_str(&format!("\nNothing written and no timer enabled. After a yes, the files go to {}:\n", dir.display()));
        for unit in &units { out.push_str(&format!("  {}\n", dir.join(&unit.name).display())); }
        out.push_str(&format!("Install with `penelope factory install {} --yes`.\n", f.name));
        return Ok(out);
    }
    let written = write_units_to(&dir, &units)?;
    fs::create_dir_all(log_dir(ctx, &f.name)).map_err(|e| format!("cannot make {}: {}", log_dir(ctx, &f.name).display(), e))?;
    let said = match scheduler_enable(ctx, &f) {
        Ok(text) => text,
        Err(e) => return Err(format!("the unit files are written ({}), but the timer was not enabled: {}", written.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "), e)),
    };
    f.armed = true;
    f.installed = platform_name().to_string();
    let mut all = load(ctx)?;
    if let Some(slot) = all.iter_mut().find(|x| x.name == f.name) { *slot = f.clone(); }
    save(ctx, &all)?;
    out.push_str(&format!("\ninstalled {} with {}.\n{}\n", written.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "), platform_name(), said));
    Ok(out)
}

/// Stops the timer and removes its files. The registry entry and the profile stay, so `install` can arm it again.
pub fn stop(ctx: &Ctx, name: &str) -> Result<String, String> {
    let mut f = find(ctx, name)?;
    let mut notes = Vec::new();
    if f.armed || !f.installed.is_empty() {
        match scheduler_disable(ctx, &f) {
            Ok(text) if !text.is_empty() => notes.push(text),
            Ok(_) => {}
            Err(e) => notes.push(format!("the scheduler said: {}", e)),
        }
        let dir = unit_dir(ctx, &f);
        for unit in units(ctx, &f) {
            let path = dir.join(&unit.name);
            if fs::remove_file(&path).is_ok() { notes.push(format!("removed {}", path.display())); }
        }
    }
    f.armed = false;
    f.installed.clear();
    let mut all = load(ctx)?;
    if let Some(slot) = all.iter_mut().find(|x| x.name == f.name) { *slot = f.clone(); }
    save(ctx, &all)?;
    Ok(format!("factory {} stopped{}. Its profile {} stays. Arm it again with `penelope factory install {} --yes`.",
               name, if notes.is_empty() { String::new() } else { format!(": {}", notes.join(" · ")) }, f.profile, name))
}

/// Runs one factory now, without its timer. The run is the same command the timer starts: the cluster command, with
/// no interface. The result and the exit code go to `.penelope/factories/NAME/runs.log`.
pub fn run(ctx: &Ctx, name: &str) -> Result<String, String> {
    let mut f = find(ctx, name)?;
    crate::clusters::profile(ctx, &f.profile)?;
    let args = runner_args(ctx, &f);
    let dir = log_dir(ctx, &f.name);
    fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {}", dir.display(), e))?;
    let out = Command::new(&args[0]).args(&args[1..]).current_dir(project_dir(ctx))
        .stdin(Stdio::null()).output().map_err(|e| format!("cannot start the runner {}: {}", args[0], e))?;
    let code = out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".to_string());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let tail = |text: &str| text.chars().rev().take(4000).collect::<String>().chars().rev().collect::<String>();
    let body = format!("{} exit={} profile={} goal={}\nstdout:\n{}\nstderr:\n{}\n\n",
                       crate::sessions::stamp(now()), code, f.profile, f.goal, stdout.trim_end(), stderr.trim_end());
    let _ = fs::OpenOptions::new().create(true).append(true).open(dir.join("runs.log"))
        .and_then(|mut file| std::io::Write::write_all(&mut file, body.as_bytes()));
    f.runs = f.runs.saturating_add(1);
    f.last_run = now();
    f.last_result = format!("exit {}", code);
    let mut all = load(ctx)?;
    if let Some(slot) = all.iter_mut().find(|x| x.name == f.name) { *slot = f.clone(); }
    save(ctx, &all)?;
    let mut said = format!("factory {} ran · run {} · exit {} · profile {}\n", f.name, f.runs, code, f.profile);
    if !stdout.trim().is_empty() { said.push_str(&format!("\n{}", tail(stdout.trim_end()))); }
    if !stderr.trim().is_empty() { said.push_str(&format!("\nstderr:\n{}", tail(stderr.trim_end()))); }
    if !out.status.success() {
        return Err(format!("NOT successful. {}", said.trim_end()));
    }
    Ok(said)
}

pub fn list_text(ctx: &Ctx) -> String {
    let all = match load(ctx) {
        Ok(all) => all,
        Err(e) => return format!("{}\n", e),
    };
    if all.is_empty() {
        return "no factory yet. Start one with: /factory new NAME --profile P --every 3h\n\
                A factory is a cluster profile with a timer: it runs while Penelope is closed.\n".to_string();
    }
    let mut out = format!("Factories\n\n{:<14} {:<14} {:<20} {:<9} {:>4}  {}\n", "name", "profile", "schedule", "state", "runs", "last");
    let mut armed = 0;
    for f in &all {
        if f.armed { armed += 1; }
        out.push_str(&format!("{:<14} {:<14} {:<20} {:<9} {:>4}  {}\n", f.name, f.profile, schedule_text(f),
                              if f.armed { "armed" } else { "stopped" }, f.runs,
                              if f.last_run > 0 { format!("{} · {}", crate::sessions::stamp(f.last_run), f.last_result) } else { "never".to_string() }));
        if !f.base.is_empty() && f.base != f.profile {
            out.push_str(&format!("{:<14} copied from profile {}\n", "", f.base));
        }
    }
    out.push_str(&format!("\n{} factories, {} armed. /factory run NAME runs one now; /factory stop NAME stops its timer.\n", all.len(), armed));
    out.push_str(&format!("scheduler of this machine: {}. Units: {}\n", platform_name(), unit_dir(ctx, &all[0]).display()));
    out
}

fn help_text() -> String {
    "Factories\n\n\
     /factory new NAME --profile P --every 3h        create a factory, repeat after a duration\n\
     /factory new NAME --profile P --at 09:30        create a factory, once a day\n\
     /factory new NAME --profile P --every 3h --goal TEXT --yes\n\
                                                     create and install its timer after the report\n\
     /factory list                                   the factories, schedules, runs and state\n\
     /factory install NAME [--dry] [--yes]           show the timer; --yes writes and enables it\n\
     /factory run NAME                               run one factory now, no timer and no interface\n\
     /factory stop NAME                              stop the timer and remove its unit files\n\n\
     The timer starts `penelope cluster run --profile NAME GOAL` in the scheduler of this machine, so the factory\n\
     runs while Penelope is closed. Nothing is written into the user scheduler and no timer is enabled without --yes.\n"
        .to_string()
}

fn next_value(args: &[String], i: &mut usize, inline: Option<String>, key: &str) -> Result<String, String> {
    if let Some(text) = inline { return Ok(text); }
    *i += 1;
    args.get(*i).cloned().ok_or_else(|| format!("{} needs a value", key))
}

fn parse_new_args(args: &[String]) -> Result<(String, String, Option<String>, Option<String>, String, bool, bool), String> {
    let (mut name, mut profile) = (String::new(), String::new());
    let (mut every, mut at, mut goal) = (None, None, String::new());
    let (mut dry, mut yes) = (false, false);
    let mut goal_words = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let (key, inline) = match arg.split_once('=') {
            Some((key, value)) if key.starts_with("--") => (key, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        match key {
            "--profile" => profile = next_value(args, &mut i, inline, "--profile")?,
            "--every" => every = Some(next_value(args, &mut i, inline, "--every")?),
            "--at" => at = Some(next_value(args, &mut i, inline, "--at")?),
            "--goal" | "--purpose" => goal = next_value(args, &mut i, inline, "--goal")?,
            "--dry" => dry = true,
            "--yes" => yes = true,
            other if other.starts_with("--") => return Err(format!("unknown setting {} for factory new", other)),
            other if name.is_empty() => name = other.to_string(),
            other => goal_words.push(other.to_string()),
        }
        i += 1;
    }
    if goal.trim().is_empty() && !goal_words.is_empty() { goal = goal_words.join(" "); }
    if name.is_empty() { return Err("NOT created. usage: factory new NAME --profile P --every 3h|--at HH:MM".to_string()); }
    if profile.is_empty() { return Err("NOT created. --profile names the cluster profile the factory is copied from.".to_string()); }
    Ok((name, profile, every, at, goal, dry, yes))
}

pub fn cli(ctx: &Ctx, args: &[String]) -> Result<String, String> {
    let verb = args.first().map(String::as_str).unwrap_or("list");
    match verb {
        "" | "list" => Ok(list_text(ctx)),
        "help" | "--help" | "-h" => Ok(help_text()),
        "new" => {
            let (name, profile, every, at, goal, dry, yes) = parse_new_args(&args[1..])?;
            new_factory(ctx, &name, &profile, every.as_deref(), at.as_deref(), &goal, dry, yes)
        }
        "install" => {
            let name = args.get(1).cloned().unwrap_or_default();
            if name.is_empty() { return Err("usage: penelope factory install NAME [--dry] [--yes]".to_string()); }
            let dry = args.iter().any(|a| a == "--dry");
            let yes = args.iter().any(|a| a == "--yes");
            install(ctx, &name, dry, yes)
        }
        "run" => {
            let name = args.get(1).cloned().unwrap_or_default();
            if name.is_empty() { return Err("usage: penelope factory run NAME".to_string()); }
            run(ctx, &name)
        }
        "stop" => {
            let name = args.get(1).cloned().unwrap_or_default();
            if name.is_empty() { return Err("usage: penelope factory stop NAME".to_string()); }
            stop(ctx, &name)
        }
        other => Err(format!("unknown factory command {}. The commands are new, list, install, run and stop.", other)),
    }
}

/// The slash form of the same parser: `/factory new NAME --profile P --every 3h`.
pub fn slash(ctx: &Ctx, rest: &str) -> Result<String, String> {
    let args: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
    cli(ctx, &args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (Ctx, PathBuf) {
        let dir = std::env::temp_dir().join(format!("penelope-factories-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".penelope/clusters/profiles")).unwrap();
        fs::write(dir.join(".penelope/clusters/profiles/base.json"), r#"{
            "name": "base",
            "workers": [{"name": "one", "role": "worker", "model": "main", "task": "Do one thing."}],
            "validator": {"name": "check", "role": "judge", "model": "judge", "task": "Check it."},
            "max_models": 2, "max_steps": 3, "retries": 0, "worktree": false
        }"#).unwrap();
        (crate::config::load(&dir.join(".penelope")), dir)
    }

    #[test]
    fn a_schedule_accepts_a_duration_or_a_time_of_day_only() {
        assert_eq!(normalize_every("3h").unwrap(), "3h");
        assert_eq!(normalize_every(" 1m ").unwrap(), "1m");
        assert_eq!(every_seconds("3h").unwrap(), 10_800);
        assert!(normalize_every("5").unwrap_err().contains("s, m, h or d"));
        assert!(normalize_every("2x").unwrap_err().contains("s, m, h or d"));
        assert!(normalize_every("1s").unwrap_err().contains("at least"));
        assert_eq!(normalize_at("9:05").unwrap(), "09:05");
        assert_eq!(normalize_at("23:59").unwrap(), "23:59");
        assert!(normalize_at("24:00").unwrap_err().contains("00:00 and 23:59"));
        assert!(normalize_at("09.30").unwrap_err().contains("HH:MM"));
    }

    #[test]
    fn a_profile_carries_its_factory_schedule() {
        let (ctx, dir) = ctx();
        fs::write(dir.join(".penelope/clusters/profiles/mail.json"), r#"{
            "name": "mail", "workers": [{"name": "one", "role": "worker", "model": "main"}],
            "every": "3h", "purpose": "check my mail", "worktree": false
        }"#).unwrap();
        let p = crate::clusters::profile(&ctx, "mail").unwrap();
        assert_eq!((p.every.as_deref(), p.at.as_deref(), p.purpose.as_deref()), (Some("3h"), None, Some("check my mail")));
        fs::write(dir.join(".penelope/clusters/profiles/both.json"), r#"{
            "name": "both", "workers": [{"name": "one", "role": "worker", "model": "main"}], "every": "3h", "at": "09:30"
        }"#).unwrap();
        assert!(crate::clusters::profile(&ctx, "both").unwrap_err().contains("use every or at, not both"));
        let copy = crate::clusters::profile_json(&p, "mail2");
        assert_eq!(copy["every"], json!("3h"));
        assert_eq!(copy["purpose"], json!("check my mail"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dry_new_writes_nothing_and_prints_the_units_with_the_schedule() {
        let (ctx, dir) = ctx();
        // The dry run prints the units of this system's scheduler: systemd, launchd or Task Scheduler.
        #[cfg(not(any(target_os = "macos", windows)))]
        let (unit_name, every_line, at_lines) = ("penelope-factory-tick.service", "OnUnitActiveSec=1m", vec!["OnCalendar=*-*-* 09:30:00"]);
        #[cfg(target_os = "macos")]
        let (unit_name, every_line, at_lines) = ("ai.laertes.penelope.factory.tick.plist", "<integer>60</integer>", vec!["<key>Hour</key>\n    <integer>09</integer>", "<integer>30</integer>"]);
        #[cfg(windows)]
        let (unit_name, every_line, at_lines) = ("PenelopeFactory-tick.xml", "<Interval>PT60S</Interval>", vec!["T09:30:00"]);
        let said = new_factory(&ctx, "tick", "base", Some("1m"), None, "write one line", true, false).unwrap();
        assert!(said.starts_with("Dry run"), "{}", said);
        assert!(said.contains(unit_name), "{}", said);
        assert!(said.contains(every_line), "{}", said);
        assert!(said.contains("cluster run --profile tick"), "{}", said);
        assert!(!dir.join(".penelope/clusters/profiles/tick.json").exists(), "a dry run wrote a profile");
        assert!(!dir.join(".penelope/factories.json").exists(), "a dry run wrote the registry");
        let at = new_factory(&ctx, "morning", "base", None, Some("09:30"), "", true, false).unwrap();
        assert!(at_lines.iter().all(|l| at.contains(l)), "{}", at);
        // Both forms at once and a missing schedule are refused before anything is written.
        assert!(new_factory(&ctx, "both", "base", Some("1m"), Some("09:30"), "", true, false).unwrap_err().contains("not both"));
        fs::write(dir.join(".penelope/clusters/profiles/human.json"), r#"{
            "name": "human", "workers": [{"name": "yes", "role": "human", "task": "Say yes."}], "worktree": false
        }"#).unwrap();
        assert!(new_factory(&ctx, "ask", "human", Some("1m"), None, "", true, false).unwrap_err().contains("waits for a person"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_unit_files_are_written_into_a_temporary_directory() {
        let (ctx, dir) = ctx();
        let f = Factory { name: "tick".into(), base: "base".into(), profile: "tick".into(), goal: "write one line".into(),
                          every: Some("1m".into()), at: None, armed: false, installed: String::new(), created: 0,
                          runs: 0, last_run: 0, last_result: String::new() };
        // The systemd renderer is called by name, so the check of its text runs on every host.
        let units = systemd_units(&ctx, &f);
        let target = dir.join("units");
        let written = write_units_to(&target, &units).unwrap();
        assert_eq!(written.len(), 2);
        let timer = fs::read_to_string(target.join("penelope-factory-tick.timer")).unwrap();
        assert!(timer.contains("OnUnitActiveSec=1m") && timer.contains("Unit=penelope-factory-tick.service"), "{}", timer);
        let service = fs::read_to_string(target.join("penelope-factory-tick.service")).unwrap();
        assert!(service.contains("Type=oneshot") && service.contains("--profile") && service.contains("write one line"), "{}", service);
        assert!(service.contains("TimeoutStartSec=infinity"), "a long cluster run must not be killed after 90 s: {}", service);
        // The macOS and Windows renderers are code paths for those systems; their text is checked here on any host.
        let plist = launchd_units(&ctx, &f);
        assert_eq!(plist.len(), 1);
        assert!(plist[0].name.ends_with(".plist") && plist[0].body.contains("ai.laertes.penelope.factory.tick"), "{}", plist[0].body);
        assert!(plist[0].body.contains("<key>StartInterval</key>") && plist[0].body.contains("<integer>60</integer>"), "{}", plist[0].body);
        assert!(plist[0].body.contains("<string>--profile</string>") && plist[0].body.contains("<string>tick</string>"), "{}", plist[0].body);
        let task = windows_units(&ctx, &f);
        assert_eq!(task.len(), 1);
        assert!(task[0].body.contains("<Task version=\"1.4\"") && task[0].body.contains("<Interval>PT60S</Interval>"), "{}", task[0].body);
        assert!(task[0].body.contains("<Arguments>") && task[0].body.contains("--profile") && task[0].body.contains("tick"), "{}", task[0].body);
        let win_at = Factory { at: Some("09:30".into()), every: None, ..f.clone() };
        assert!(windows_units(&ctx, &win_at)[0].body.contains("<CalendarTrigger>") &&
                windows_units(&ctx, &win_at)[0].body.contains("T09:30:00"), "{}", windows_units(&ctx, &win_at)[0].body);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_factory_is_created_listed_and_stopped_without_touching_a_scheduler() {
        let (ctx, dir) = ctx();
        let said = new_factory(&ctx, "tick", "base", Some("1m"), None, "write one line", false, false).unwrap();
        assert!(said.contains("factory tick created") && said.contains("Not installed yet"), "{}", said);
        assert!(dir.join(".penelope/clusters/profiles/tick.json").is_file());
        let registry = fs::read_to_string(dir.join(".penelope/factories.json")).unwrap();
        assert!(registry.contains("\"tick\"") && registry.contains("\"every\": \"1m\"") && registry.contains("\"armed\": false"), "{}", registry);
        let list = list_text(&ctx);
        assert!(list.contains("tick") && list.contains("every 1m") && list.contains("stopped") && list.contains("0 armed"), "{}", list);
        let install_report = install(&ctx, "tick", false, false).unwrap();
        assert!(install_report.contains("Nothing written and no timer enabled") && install_report.contains("After a yes"), "{}", install_report);
        let dry = install(&ctx, "tick", true, false).unwrap();
        assert!(dry.contains("Dry run") == false && dry.contains("Nothing written"), "{}", dry);
        let stopped = stop(&ctx, "tick").unwrap();
        assert!(stopped.contains("factory tick stopped") && stopped.contains("profile tick stays"), "{}", stopped);
        assert!(list_text(&ctx).contains("stopped"));
        // A second factory with the same name is refused; the profile stays.
        assert!(new_factory(&ctx, "tick", "base", Some("1m"), None, "", false, false).unwrap_err().contains("exists"));
        let _ = fs::remove_dir_all(&dir);
    }
}
