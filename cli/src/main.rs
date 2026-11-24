//! penelope: the Polytrope harness on the command line.

mod backend;
mod loom;
mod config;
mod files;
mod mcp;
mod memory;
mod plugins;
mod privacy;
mod scout;
mod search;
mod setup;
mod sessions;
mod tools;
mod tui;
mod weak;

use std::io::{BufRead, IsTerminal, Write};

const HELP: &str = "penelope                      chat with Penelope in the full screen interface
penelope chat --plain         chat line by line, without the full screen interface
penelope chat --resume N      go on with session N of the list
penelope ask \"TEXT\"           one question, one answer
penelope init                 create a .penelope folder in this directory
penelope setup [--here]       answer seven questions: models, who searches, privacy, policies, web search
penelope sessions             list the stored sessions, newest first
penelope crew                 the eight model families and what does each job today
penelope memory map           show the memory map
penelope memory go QUERY      follow the map to a waypoint and print the notes on the way
penelope memory note [--path a/b] TEXT
                              store a rule or fact at a waypoint (without --path: filed by language or topic)
penelope memory unravel [N]   take back the last note, or the last N notes
penelope search WORDS         search the web and print up to 8 results
penelope scout TASK           let the weaker model look something up with its own tool calls
penelope run [--ask QUESTION] COMMAND
                              run a command, long output condensed by the weaker model
penelope plugin add DIR       register a plugin folder in the format of Claude Code (skills and MCP servers)
penelope plugin list          show the registered plugins
penelope stats                how much tool output the weaker model kept away from the main model
penelope mcp [--root DIR]     serve Penelope's tools as an MCP server on stdin and stdout
penelope doctor               check the programs and models Penelope needs";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut root_arg = None;
    if let Some(i) = args.iter().position(|a| a == "--root") {
        if i + 1 < args.len() {
            root_arg = Some(args[i + 1].clone());
            args.drain(i..i + 2);
        }
    }
    let cmd = args.first().map(String::as_str).unwrap_or("chat");
    if cmd == "-h" || cmd == "--help" || cmd == "help" {
        println!("{}", HELP);
        return;
    }
    if cmd == "init" {
        let root = std::env::current_dir().unwrap().join(".penelope");
        let ctx = config::load(&root);
        memory::Memory::open(&ctx.root);
        loom::say(&format!("ready: {}", root.display()));
        return;
    }
    if cmd == "setup" {
        // The settings go to the folder penelope would use here. `--here` makes a .penelope folder in this directory first.
        let root = if args.iter().any(|a| a == "--here") { std::env::current_dir().unwrap().join(".penelope") } else { config::find_root(root_arg.as_deref()) };
        let mut ctx = config::load(&root);
        ctx.cfg = setup::wizard(&mut std::io::stdin().lock(), &mut std::io::stdout(), ctx.cfg.clone());
        let _ = std::fs::write(root.join("config.json"), serde_json::to_string_pretty(&ctx.cfg).unwrap());
        loom::say(&format!("saved: {}", root.join("config.json").display()));
        return;
    }
    let ctx = config::load(&config::find_root(root_arg.as_deref()));
    match cmd {
        "mcp" => mcp::serve(&ctx),
        "memory" => memory_cmd(&ctx, &args[1..]),
        "run" => {
            let mut rest: Vec<String> = args[1..].to_vec();
            let mut question = String::new();
            if rest.first().map(String::as_str) == Some("--ask") && rest.len() >= 2 {
                question = rest[1].clone();
                rest.drain(0..2);
            }
            println!("{}", tools::run(&ctx, &rest.join(" "), &question, ""));
        }
        "ask" => {
            let text = args[1..].join(" ");
            let mut chat = backend::Chat::new();
            answer(&ctx, &mut chat, &text);
        }
        "search" => print!("{}", search::search(&ctx, &args[1..].join(" "))),
        "scout" => println!("{}", scout::run(&ctx, &args[1..].join(" "), "")),
        "doctor" => doctor(&ctx),
        "crew" => print!("{}", tools::crew(&ctx)),
        "sessions" => print!("{}", sessions::list(&ctx)),
        "stats" => print!("{}", tools::stats(&ctx)),
        "plugin" => match args.get(1).map(String::as_str) {
            Some("add") if args.len() >= 3 => println!("{}", plugins::add(&ctx, &args[2])),
            _ => print!("{}", plugins::list(&ctx)),
        },
        "chat" => {
            let mut resume = None;
            if let Some(i) = args.iter().position(|a| a == "--resume") {
                let key = args.get(i + 1).cloned().unwrap_or_default();
                match sessions::pick(&ctx, &key) {
                    Some(entry) => resume = Some(entry),
                    None => {
                        loom::warn(&format!("no session {} in the list (penelope sessions)", key));
                        std::process::exit(2);
                    }
                }
            }
            let plain = args.iter().any(|a| a == "--plain") || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal();
            if plain {
                repl(&ctx, resume.map(|entry| backend::Chat::resume(&entry)));
            } else if let Err(e) = tui::run(ctx, resume) {
                loom::warn(&e);
                std::process::exit(1);
            }
        }
        other => {
            loom::warn(&format!("unknown command: {}", other));
            println!("{}", HELP);
            std::process::exit(2);
        }
    }
}

fn memory_cmd(ctx: &config::Ctx, args: &[String]) {
    match args.first().map(String::as_str) {
        Some("map") | None => print!("{}", tools::memory_map(ctx)),
        Some("go") => println!("{}", tools::memory_go(ctx, &args[1..].join(" "))),
        Some("note") => {
            let mut rest: Vec<String> = args[1..].to_vec();
            let mut path = String::new();
            if rest.first().map(String::as_str) == Some("--path") && rest.len() >= 2 {
                path = rest[1].clone();
                rest.drain(0..2);
            }
            println!("{}", tools::memory_note(ctx, &path, &rest.join(" ")));
        }
        Some("unravel") => println!("{}", tools::unravel(ctx, args.get(1).and_then(|n| n.parse().ok()).unwrap_or(1))),
        Some(other) => loom::warn(&format!("unknown memory command: {}", other)),
    }
}

fn answer(ctx: &config::Ctx, chat: &mut backend::Chat, text: &str) {
    let (notes, trail, cached) = memory::Memory::open(&ctx.root).recall(text, 1600);
    if !trail.is_empty() {
        loom::say(&format!("memory: start / {}{}", trail.last().unwrap().replace('/', " / "), if cached { " (stored walk)" } else { "" }));
    }
    match chat.turn(ctx, text, &notes) {
        Ok(a) => {
            println!("{}", a);
            loom::say(&format!("tokens so far: {} in, {} out", chat.input_tokens, chat.output_tokens));
            if ctx.cfg["auto_learn"].as_bool().unwrap_or(true) {
                for id in tools::learn(ctx, text, &a) {
                    loom::say(&format!("learned: {}", id));
                }
            }
        }
        Err(e) => loom::warn(&e),
    }
}

fn repl(ctx: &config::Ctx, resume: Option<backend::Chat>) {
    loom::banner(&format!("main model: {} ({})   weaker model: {} ({})",
        ctx.cfg["main"]["model"].as_str().unwrap_or("?"), ctx.cfg["main"]["kind"].as_str().unwrap_or("?"),
        ctx.cfg["weak"]["model"].as_str().unwrap_or("?"), ctx.cfg["weak"]["kind"].as_str().unwrap_or("?")));
    println!("Type a request. /map shows the memory map, /quit ends.");
    let mut chat = resume.unwrap_or_else(backend::Chat::new);
    let stdin = std::io::stdin();
    loop {
        print!("you> ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim();
        match line {
            "" => continue,
            "/quit" | "/q" => break,
            "/map" => print!("{}", tools::memory_map(ctx)),
            _ => answer(ctx, &mut chat, line),
        }
    }
    loom::say("bye");
}

fn doctor(ctx: &config::Ctx) {
    loom::banner("doctor");
    print!("{}", doctor_text(ctx));
}

/// One line per program or setting Penelope depends on.
pub fn doctor_text(ctx: &config::Ctx) -> String {
    let mut out = String::new();
    let mut line = |name: &str, ok: bool, detail: String| {
        out.push_str(&format!("{} {}{}\n", if ok { "ok   " } else { "FAIL " }, name, if detail.is_empty() { String::new() } else { format!(": {}", detail) }));
    };
    line("folder", true, ctx.root.display().to_string());
    let claude = config::which("claude");
    line("claude command (login of the user, main and weaker model)", claude.is_some(), claude.map(|p| p.display().to_string()).unwrap_or_else(|| "not on the PATH".into()));
    let sc = ctx.cfg["screencodec"]["command"].as_str().filter(|c| !c.is_empty()).map(std::path::PathBuf::from).or_else(|| config::which("screencodec-mcp"));
    line("screencodec-mcp (eyes: pages and pictures as text)", sc.is_some(), sc.map(|p| p.display().to_string()).unwrap_or_else(|| "not on the PATH, Penelope runs without it".into()));
    line("run policy", true, ctx.cfg["run_policy"].as_str().unwrap_or("safe").to_string());
    line("edit policy (file changes)", true, ctx.cfg["edit_policy"].as_str().unwrap_or("ask").to_string());
    line("delegate (who searches)", true, format!("{}: {}", ctx.cfg["delegate"].as_str().unwrap_or("auto"), match ctx.cfg["delegate"].as_str().unwrap_or("auto") {
        "off" => "the main model searches itself",
        "offer" => "the main model may search itself or send a scout",
        _ => "every search runs on the weaker model as scout",
    }));
    line("web search", true, search::kind(ctx).to_string());
    line("memory map", true, format!("{} lines", tools::memory_map(ctx).lines().count()));
    out
}
