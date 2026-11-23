//! katecli: Kate on the command line.

mod backend;
mod cat;
mod config;
mod mcp;
mod memory;
mod plugins;
mod tools;
mod weak;

use std::io::{BufRead, Write};

const HELP: &str = "katecli                      chat with Kate
katecli ask \"TEXT\"           one question, one answer
katecli init                 create a .kate folder in this directory
katecli memory map           show the memory map
katecli memory go QUERY      follow the map to a waypoint and print the notes on the way
katecli memory note [--path a/b] TEXT
                             store a rule or fact at a waypoint (without --path: filed by language or topic)
katecli run [--ask QUESTION] COMMAND
                             run a command, long output condensed by the weaker model
katecli plugin add DIR       register a plugin folder in the format of Claude Code (skills and MCP servers)
katecli plugin list          show the registered plugins
katecli stats                how much tool output the weaker model kept away from the main model
katecli mcp [--root DIR]     serve Kate's tools as an MCP server on stdin and stdout
katecli doctor               check the programs and models Kate needs";

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
        let root = std::env::current_dir().unwrap().join(".kate");
        let ctx = config::load(&root);
        memory::Memory::open(&ctx.root);
        cat::say(&format!("ready: {}", root.display()));
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
        "doctor" => doctor(&ctx),
        "stats" => print!("{}", tools::stats(&ctx)),
        "plugin" => match args.get(1).map(String::as_str) {
            Some("add") if args.len() >= 3 => println!("{}", plugins::add(&ctx, &args[2])),
            _ => print!("{}", plugins::list(&ctx)),
        },
        "chat" => repl(&ctx),
        other => {
            cat::warn(&format!("unknown command: {}", other));
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
        Some(other) => cat::warn(&format!("unknown memory command: {}", other)),
    }
}

fn answer(ctx: &config::Ctx, chat: &mut backend::Chat, text: &str) {
    let (notes, trail, cached) = memory::Memory::open(&ctx.root).recall(text, 1600);
    if !trail.is_empty() {
        cat::say(&format!("memory: start / {}{}", trail.last().unwrap().replace('/', " / "), if cached { " (stored walk)" } else { "" }));
    }
    match chat.turn(ctx, text, &notes) {
        Ok(a) => {
            println!("{}", a);
            cat::say(&format!("tokens so far: {} in, {} out", chat.input_tokens, chat.output_tokens));
            if ctx.cfg["auto_learn"].as_bool().unwrap_or(true) {
                for id in tools::learn(ctx, text, &a) {
                    cat::say(&format!("learned: {}", id));
                }
            }
        }
        Err(e) => cat::warn(&e),
    }
}

fn repl(ctx: &config::Ctx) {
    cat::banner(&format!("main model: {} ({})   weaker model: {} ({})",
        ctx.cfg["main"]["model"].as_str().unwrap_or("?"), ctx.cfg["main"]["kind"].as_str().unwrap_or("?"),
        ctx.cfg["weak"]["model"].as_str().unwrap_or("?"), ctx.cfg["weak"]["kind"].as_str().unwrap_or("?")));
    println!("Type a request. /map shows the memory map, /quit ends.");
    let mut chat = backend::Chat::new();
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
    cat::say("bye");
}

fn doctor(ctx: &config::Ctx) {
    cat::banner("doctor");
    let line = |name: &str, ok: bool, detail: String| println!("{} {}{}", if ok { "ok   " } else { "FAIL " }, name, if detail.is_empty() { String::new() } else { format!(": {}", detail) });
    line("folder", true, ctx.root.display().to_string());
    let claude = config::which("claude");
    line("claude command (login of the user, main and weaker model)", claude.is_some(), claude.map(|p| p.display().to_string()).unwrap_or_else(|| "not on the PATH".into()));
    let sc = ctx.cfg["screencodec"]["command"].as_str().filter(|c| !c.is_empty()).map(std::path::PathBuf::from).or_else(|| config::which("screencodec-mcp"));
    line("screencodec-mcp (eyes: pages and pictures as text)", sc.is_some(), sc.map(|p| p.display().to_string()).unwrap_or_else(|| "not on the PATH, Kate runs without it".into()));
    line("run policy", true, ctx.cfg["run_policy"].as_str().unwrap_or("safe").to_string());
    line("memory map", true, format!("{} lines", tools::memory_map(ctx).lines().count()));
}
