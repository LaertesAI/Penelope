//! penelope: the Laertes harness on the command line.

mod acp;
#[allow(dead_code)]
mod advise;
mod agents;
mod apps;
mod audit;
mod auth;
mod backend;
mod brain;
mod brief;
mod loom;
mod config;
mod clusters;
mod context;
mod cost;
mod crew;
mod cursor;
mod forge;
mod eurykleia;
mod eval;
mod factories;
mod files;
mod input;
mod jobs;
mod llm;
mod lsp;
mod mcp;
mod mcpclient;
mod mitos;
mod memory;
#[allow(dead_code)]
mod notes;
mod net;
mod mode;
mod mods;
mod platform;
mod plugins;
#[allow(dead_code)]
mod proof;
mod privacy;
mod providers;
mod research;
mod route;
mod scout;
mod scrape;
mod screen;
mod search;
mod server;
mod setup;
mod sessions;
mod skills;
mod spec;
mod startup;
mod toolsearch;
mod tools;
mod tui;
mod update;
mod vet;
mod weak;
mod work;

use std::io::{BufRead, IsTerminal, Write};

const HELP: &str = "penelope                      chat with Penelope in the full screen interface
penelope chat --plain         chat line by line, without the full screen interface
penelope chat --resume N      go on with session N of the list
penelope ask \"TEXT\"           one question, one answer
penelope ask --json \"TEXT\"    the same turn as newline-delimited JSON events
penelope init                 create a .penelope folder in this directory
penelope setup [--here]       answer seven questions: models, who searches, privacy, policies, web search
penelope providers            list the provider table and say per entry whether its key is set
penelope login [NAME]         sign in to Claude, or store the API key of a provider
penelope logout NAME          remove the stored key of a provider
penelope auth                 the login status of Claude and the providers
penelope auth verify [ROLE]   one real request per model setting (main, weak or scout)
penelope update [--check]     check the update server, or install a newer signed build
penelope audit                look for keys and plain-http leaks in the settings folder
penelope eval TASKS.jsonl --setup large|large-subagents|cluster|all [--runs PATH]
                              run the eval tasks, measure checks, tokens, euro and wall time; writes docs/RUNS.md
penelope replay ID --model MODEL
                              re-run the stored prompts of an eval run read-only; no file changed, no command or check run
penelope input [install|enable|status]
                              a native Wayland pointer for ScreenCodec, without ydotool
penelope cluster run GOAL [--profile NAME] [--shadow PROFILE] [--dry] [--sequential]
                              ask the orchestrator for clusters and run them; a shadow profile runs beside the first
                              cluster on the same task, in its own worktree, and its files are never merged; --dry
                              prints parts, model and an estimate, starts nothing
penelope cluster list        the cluster tree
penelope cluster status ID   one cluster with its parts, report and memory
penelope cluster profiles    the parts of every profile (built-in, .json or .cluster)
penelope cluster kill ID     stop a running cluster at once
penelope cluster dump ID     the state dump with the cause of a stuck cluster
penelope cluster respawn ID [EXTRA]
                              kill ID and start it again with the cause from its dump (at most 2)
penelope cluster advise [NAME] [--apply]
                              one suggestion per part and task type the validator rejected often, worst first;
                              --apply writes the suggested model into the profile for the next run and refuses a running cluster
penelope cluster profile NAME [JSON]
                              create or change a profile under .penelope/clusters/profiles
penelope factory new NAME --profile P (--every 3h | --at HH:MM) [--goal TEXT] [--dry] [--yes]
                              copy a cluster profile into a factory and show its operating-system timer
penelope factory list        the factories, schedules, runs and armed state
penelope factory install NAME [--dry] [--yes]
                              show the unit files; --yes writes them and enables the timer
penelope factory run NAME    run one factory now, through `cluster run`, without the interface
penelope factory stop NAME   stop the timer and remove its unit files; the profile stays
penelope app list            installed apps with version, view and rights
penelope app install LINK|DIR|ZIP [--yes]
                              copy, clone or unpack an app, check the scripts, show the rights, install after a yes; --yes needs apps.allow_install
penelope app update NAME [--yes]
                              pull a git app in a staging copy, check it, replace the installed folder after a yes
penelope app on NAME         the app the launcher marks as chosen
penelope app off             no app is the chosen one; the folders stay
penelope app remove NAME     delete an installed app and its folder
penelope app show NAME       the TUI view of a built-in app, or the path of its HTML entry
penelope models [PROVIDER]   the chat models the provider reports, with its effort levels
penelope provider use NAME [--role main|weak|scout] [--model MODEL] [--workspace ID] [--base-url URL]
                              write one model setting from the table to config.json
penelope sessions             list the stored sessions, newest first
penelope crew                 the eight model families and what does each job today
penelope memory map           show the memory map
penelope memory go QUERY      follow the map to a waypoint and print the notes on the way
penelope memory note [--path a/b] TEXT
                              store a rule or fact at a waypoint (without --path: filed by language or topic)
penelope memory unravel [N]   take back the last note, or the last N notes
penelope search [--engine E] [--fresh] WORDS
                              search several engines, cache, rank, print up to 8 results
penelope search fanout on|off  ask every configured engine at once and merge the results
penelope search health [--probe]
                              the last call of each engine
penelope research QUESTION    search, read the best pages and print a cited report
penelope scout TASK           let the weaker model look something up with its own tool calls
penelope run [--ask QUESTION] COMMAND
                              run a command, long output condensed by the weaker model
penelope undo                 take back the last change that the tools write or edit made to a file
penelope allow [list|clear]   confirmed shell commands for this project (values are masked)
penelope jobs                 list the background jobs of this folder
penelope jobs stop N          end background job N
penelope plugin add DIR       register a plugin folder after a local check of its scripts (skills and MCP servers)
penelope plugin list          show the registered plugins and whether the tools of their MCP servers are allowed
penelope plugin allow NAME    put the tools of a plugin's MCP servers on the allow list of the main model
penelope plugin deny NAME     take them off the allow list (the state of a new plugin)
penelope plugin update NAME   pull a git plugin, or re-read a local or ZIP plugin
penelope plugin remove NAME   take a plugin out of the registry; the folder stays
penelope plugin commands [NAME]
                              the slash commands of the plugins
penelope plugin command NAME [ARGS]
                              print the expanded prompt of one plugin command
penelope stats                how much tool output the weaker model kept away from the main model
penelope cost                 tokens and euro per role and model
penelope price MODEL IN OUT   set a price in dollars per million tokens
penelope tools                every tool with its arguments and the roles that may call it
penelope screen tools         the tools ScreenCodec really offers
penelope screen demo URL      open a page and read it as text in one process
penelope screen ACTION JSON   one ScreenCodec action, for example snapshot
penelope screen desktop-click TEXT
                              capture the real screen, click the element whose text matches TEXT, capture again
penelope cursor X Y [--click] move Penelope's own cursor, or click there
penelope mcp list [--probe]   the MCP servers of the plugins, optionally with their tools
penelope tool list            the tools Penelope wrote for this project
penelope tool call NAME JSON  call one tool through the same gate a model uses
penelope route                the roles, their tools and their models
penelope mode [NAME]          auto, manual, safe, review, privacy or power
penelope agents               the agents written with $name(...)
penelope skills               the skills Penelope wrote for this project
penelope run-agent NAME --goal TEXT
                              run one stored agent
penelope orchestrate TEXT      run an agent from a $name(...) call or from a plain goal
penelope brain [--write]       the memory map as a graph; --write makes an Obsidian vault
penelope consolidate          take the lasting rules of the last session into the shared memory
penelope plugin install LINK  clone a plugin from git, check its scripts and register it
penelope plugin import        take over the plugins of other harnesses on this PC
penelope acp                  serve the Agent Client Protocol to Zed or Neovim on stdin and stdout
penelope mcp [--root DIR]     serve Penelope's tools as an MCP server on stdin and stdout
penelope mcp add NAME COMMAND [ARGS...] [--allow]
                              register a standalone MCP server
penelope mcp remove NAME      remove a standalone MCP server
penelope mcp allow|deny NAME  put its tools on or off the allow list
penelope mcp allow-tool SERVER TOOL
                              allow one tool of a server while the server keeps its default
penelope mcp deny-tool SERVER TOOL
                              deny one tool of an allowed server
penelope mcp call SERVER TOOL JSON
                              call one tool of an MCP server
penelope serve [--bind HOST:PORT] [--allow-remote] [--print-config]
                              the local HTTP API: search, read, embed, rerank, redact
penelope work new NAME        make a git worktree for one task under .penelope/worktrees
penelope work list            the worktrees and how many files changed
penelope work diff NAME       the patch the worker made, without touching the live tree
penelope work apply NAME      apply that patch to the live tree
penelope work remove NAME     remove the worktree (--force discards changes)
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
    // A leading slash is allowed at the top level too: `penelope /orchestrate ...`, `/cost`, `/login`.
    if let Some(first) = args.first_mut() {
        if let Some(rest) = first.strip_prefix('/') {
            if !rest.is_empty() {
                *first = rest.to_string();
            }
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
    let mut ctx = config::load(&config::find_root(root_arg.as_deref()));
    match cmd {
        "tools" => print!("{}", tools_table(&ctx)),
        "tool" => match args.get(1).map(String::as_str) {
            Some("list") | None => print!("{}", forge::list(&ctx)),
            Some("call") if args.len() >= 3 => {
                let payload: serde_json::Value = args.get(3).and_then(|j| serde_json::from_str(j).ok()).unwrap_or_else(|| serde_json::json!({}));
                match route::dispatch(&ctx, &args[2], &payload) {
                    Ok(text) => println!("{}", text),
                    Err(e) => println!("{}", e),
                }
            }
            _ => print!("{}", forge::list(&ctx)),
        },
        "route" => print!("{}", route_text(&ctx)),
        "mode" => {
            match args.get(1) {
                Some(name) => match mode::apply(&mut ctx, name) {
                    Ok(text) => println!("{}", text),
                    Err(e) => { loom::warn(&e); std::process::exit(2); }
                },
                None => print!("{}", mode::report(&ctx)),
            }
        }
        "screen" => {
            let action = args.get(1).cloned().unwrap_or_else(|| "tools".to_string());
            if action == "desktop-click" {
                let find = args[2..].join(" ");
                if find.trim().is_empty() {
                    loom::warn("usage: penelope screen desktop-click TEXT (screencodec.desktop must be on)");
                    std::process::exit(2);
                }
                match screen_desktop_click(&ctx, &find) {
                    Ok(text) => println!("{}", text),
                    Err(e) => { loom::warn(&e); std::process::exit(2); }
                }
            } else if action == "demo" {
                let url = args.get(2).cloned().unwrap_or_default();
                let task = args.iter().position(|a| a == "--task").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "read this page".to_string());
                let open = screen::tool(&ctx, &serde_json::json!({"action": "open", "url": url, "task": task, "level": "lean"}));
                match open {
                    Ok(text) => println!("{}", text),
                    Err(e) => { loom::warn(&e); std::process::exit(2); }
                }
                match screen::tool(&ctx, &serde_json::json!({"action": "snapshot", "level": "lean"})) {
                    Ok(text) => println!("\n--- the same process, snapshot ---\n{}", text),
                    Err(e) => loom::warn(&e),
                }
            } else {
                let mut payload: serde_json::Value = args.get(2).and_then(|j| serde_json::from_str(j).ok()).unwrap_or_else(|| serde_json::json!({}));
                payload["action"] = serde_json::json!(action);
                match screen::tool(&ctx, &payload) {
                    Ok(text) => println!("{}", text),
                    Err(e) => { loom::warn(&e); std::process::exit(2); }
                }
            }
        }
        "cursor" => {
            let x: i64 = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            let y: i64 = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(0);
            let click = args.iter().any(|a| a == "--click");
            match cursor::tool(&ctx, &serde_json::json!({"x": x, "y": y, "click": click})) {
                Ok(text) => println!("{}", text),
                Err(e) => { loom::warn(&e); std::process::exit(2); }
            }
        }
        "cluster" => {
            let verb = args.get(1).map(String::as_str).unwrap_or("");
            match verb {
                "list" | "" => print!("{}", clusters::status_text(&ctx, None)),
                "status" => print!("{}", clusters::status_text(&ctx, args.get(2).map(String::as_str))),
                "dump" => {
                    let id = args.get(2).cloned().unwrap_or_default();
                    if id.is_empty() { loom::warn("usage: penelope cluster dump ID"); std::process::exit(2); }
                    print!("{}", clusters::dump(&ctx, &id));
                }
                "profiles" => print!("{}", clusters::profiles_text(&ctx)),
                "kill" => {
                    let id = args.get(2).cloned().unwrap_or_default();
                    if id.is_empty() { loom::warn("usage: penelope cluster kill ID"); std::process::exit(2); }
                    println!("{}", clusters::kill(&ctx, &id));
                }
                "respawn" => {
                    let id = args.get(2).cloned().unwrap_or_default();
                    if id.is_empty() { loom::warn("usage: penelope cluster respawn ID [EXTRA]"); std::process::exit(2); }
                    println!("{}", clusters::respawn(&ctx, &id, &args.iter().skip(3).cloned().collect::<Vec<_>>().join(" ")));
                }
                "switch" => {
                    let sequential = args.iter().any(|a| a == "--sequential");
                    let kv = args.iter().position(|a| a == "--kv").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
                        .unwrap_or(clusters::SWITCH_KV_TOKENS_DESKTOP);
                    let tokens = args.iter().position(|a| a == "--tokens").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok());
                    let text = args.iter().skip(2).filter(|a| !a.starts_with("--") && a.parse::<usize>().is_err())
                        .cloned().collect::<Vec<_>>().join(" ");
                    print!("{}", clusters::switch_text(tokens, &text, sequential, kv));
                }
                "advise" => {
                    let apply = args.iter().skip(2).any(|a| a == "--apply");
                    let name = args.iter().skip(2).map(String::as_str)
                        .find(|a| !a.starts_with("--") && !a.trim().is_empty());
                    if apply {
                        match clusters::advise_apply(&ctx, name) {
                            Ok(text) => print!("{}", text),
                            Err(e) => { loom::warn(&e); std::process::exit(2); }
                        }
                    } else {
                        match clusters::advise_text(&ctx, name) {
                            Ok(text) => print!("{}", text),
                            Err(e) => { loom::warn(&e); std::process::exit(2); }
                        }
                    }
                }
                "profile" => {
                    let name = args.get(2).cloned().unwrap_or_default();
                    if name.is_empty() { loom::warn("usage: penelope cluster profile NAME [JSON]"); std::process::exit(2); }
                    let fields = match args.get(3) {
                        Some(text) => match serde_json::from_str::<serde_json::Value>(text) {
                            Ok(value) => value,
                            Err(e) => { loom::warn(&format!("the profile fields are not JSON: {}", e)); std::process::exit(2); }
                        },
                        None => serde_json::json!({}),
                    };
                    match clusters::profile_upsert(&ctx, &name, &fields) {
                        Ok(text) => println!("{}", text),
                        Err(e) => { loom::warn(&e); std::process::exit(2); }
                    }
                }
                "run" => {
                    let mut profile = None;
                    let mut shadow = None;
                    let mut parallel = true;
                    let mut dry = false;
                    let mut words = Vec::new();
                    let mut i = 2;
                    while i < args.len() {
                        match args[i].as_str() {
                            "--profile" => { profile = args.get(i + 1).cloned(); i += 2; }
                            "--shadow" => { shadow = args.get(i + 1).cloned(); i += 2; }
                            "--sequential" => { parallel = false; i += 1; }
                            "--dry" => { dry = true; i += 1; }
                            other => { words.push(other.to_string()); i += 1; }
                        }
                    }
                    let goal = words.join(" ");
                    if dry {
                        match clusters::dry_run(&ctx, &goal, profile.as_deref()) {
                            Ok(text) => {
                                print!("{}", text);
                                if let Some(name) = shadow.as_deref().filter(|s| !s.trim().is_empty()) {
                                    println!("  shadow: {} would run beside the first cluster on the same task; its files are never merged.", name);
                                }
                            }
                            Err(e) => { loom::warn(&e); std::process::exit(2); }
                        }
                    } else {
                        if goal.trim().is_empty() { loom::warn("usage: penelope cluster run GOAL [--profile NAME] [--shadow PROFILE] [--dry] [--sequential]"); std::process::exit(2); }
                        match clusters::orchestrate_with(&ctx, &goal, profile.as_deref(), parallel, shadow.as_deref()) {
                            Ok(text) => println!("{}", text),
                            Err(e) => { loom::warn(&e); std::process::exit(2); }
                        }
                    }
                }
                other => {
                    // A bare word after cluster is the goal, not a verb.
                    let goal = args[1..].join(" ");
                    if other.starts_with('-') || goal.trim().is_empty() {
                        loom::warn("usage: penelope cluster run GOAL [--profile NAME] [--shadow PROFILE] [--dry] | list | status [ID] | dump ID | respawn ID [EXTRA] | advise [NAME] [--apply] | profile NAME [JSON] | profiles | kill ID");
                        std::process::exit(2);
                    }
                    match clusters::orchestrate(&ctx, &goal, None, true) {
                        Ok(text) => println!("{}", text),
                        Err(e) => { loom::warn(&e); std::process::exit(2); }
                    }
                }
            }
        }
        "factory" => match factories::cli(&ctx, &args[1..]) {
            Ok(text) => print!("{}", text),
            Err(e) => { loom::warn(&e); std::process::exit(2); }
        },
        "app" => match apps::cli(&ctx, &args[1..]) {
            Ok(text) => print!("{}", text),
            Err(e) => { loom::warn(&e); std::process::exit(2); }
        },
        "agents" => print!("{}", agents::list(&ctx)),
        "skills" => print!("{}", skills_list(&ctx)),
        "run-agent" => {
            let name = args.get(1).cloned().unwrap_or_default();
            let goal = args.iter().position(|a| a == "--goal").and_then(|i| args.get(i + 1)).cloned().unwrap_or_default();
            if name.is_empty() || goal.is_empty() {
                loom::warn("usage: penelope run-agent NAME --goal TEXT");
                std::process::exit(2);
            }
            let spec = agents::Spec { name, role: "orchestrator".into(), thinking: "high".into(), goal, max_sub_agents: 3, max_parallel: 2,
                                      max_steps: 12, budget_eur: 0.0, model: String::new(), verify: ctx.cfg["agent_verify"].as_bool().unwrap_or(true), language: "en".into(), mcp: Vec::new() };
            match agents::run(&ctx, &spec) {
                Ok(text) => println!("{}", text),
                Err(e) => { loom::warn(&e); std::process::exit(2); }
            }
        }
        "orchestrate" => {
            let text = args[1..].join(" ");
            if text.trim().is_empty() {
                loom::warn("usage: penelope orchestrate \"$name(goal=\\\"...\\\")\" or a plain goal");
                std::process::exit(2);
            }
            let spec = match agents::parse(text.trim()) {
                Ok(spec) => spec,
                Err(_) => agents::Spec { name: "orchestrator".into(), role: "orchestrator".into(), thinking: "high".into(), goal: text.trim().to_string(),
                                         max_sub_agents: 3, max_parallel: 2, max_steps: 12, budget_eur: 0.0, model: String::new(), verify: ctx.cfg["agent_verify"].as_bool().unwrap_or(true), language: "en".into(), mcp: Vec::new() },
            };
            if let Err(e) = agents::save(&ctx, &spec) {
                loom::warn(&e);
            }
            match agents::run(&ctx, &spec) {
                Ok(text) => println!("{}", text),
                Err(e) => { loom::warn(&e); std::process::exit(2); }
            }
        }
        "brain" => {
            if args.iter().any(|a| a == "--write") {
                match brain::write_vault(&ctx) {
                    Ok((path, files)) => println!("{} files in {}. Open the folder as a vault in Obsidian.", files, brain::show(&path)),
                    Err(e) => { loom::warn(&e); std::process::exit(2); }
                }
            } else {
                print!("{}", brain::ascii(&ctx));
            }
        }
        "cost" => print!("{}", cost::report(&ctx)),
        "price" => {
            let parts: Vec<&str> = args[1..].iter().map(String::as_str).collect();
            if parts.len() != 3 || parts[1].parse::<f64>().is_err() || parts[2].parse::<f64>().is_err() {
                loom::warn("usage: penelope price MODEL INPUT OUTPUT (dollars per million tokens)");
                std::process::exit(2);
            }
            ctx.cfg["prices"][parts[0]] = serde_json::json!([parts[1].parse::<f64>().unwrap(), parts[2].parse::<f64>().unwrap()]);
            let _ = std::fs::write(ctx.root.join("config.json"), serde_json::to_string_pretty(&ctx.cfg).unwrap());
            println!("price for {}: {} / {} dollars per million tokens", parts[0], parts[1], parts[2]);
        }
        "consolidate" => {
            let users = last_session_users(&ctx);
            let ids = tools::consolidate(&ctx, &users);
            if ids.is_empty() { println!("no lasting rule found in the last session"); }
            for id in ids { println!("kept for every project: {}", id); }
        }
        "providers" => print!("{}", providers::list(&ctx)),
        "login" => match auth::login(args.get(1).map(String::as_str).unwrap_or("")) {
            Ok(text) => println!("{}", text),
            Err(e) => { loom::warn(&e); std::process::exit(2); }
        },
        "logout" => {
            let name = args.get(1).cloned().unwrap_or_default();
            if name.is_empty() {
                loom::warn("usage: penelope logout NAME");
                std::process::exit(2);
            }
            if name == "claude" || name == "claude-cli" {
                let status = std::process::Command::new("claude").arg("auth").arg("logout").status();
                println!("claude logout: {}", status.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string()));
            } else {
                println!("{}", auth::remove(&name));
            }
        }
        "auth" => match args.get(1).map(String::as_str) {
            Some("verify") => print!("{}", auth::verify(&ctx, args.get(2).map(String::as_str))),
            _ => print!("{}", auth::status(&ctx)),
        },
        "audit" => print!("{}", audit::run(&ctx)),
        "eval" => {
            let mut file = None;
            let mut setup = None;
            let mut runs = None;
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--setup" => { setup = args.get(i + 1).cloned(); i += 2; }
                    "--runs" => { runs = args.get(i + 1).cloned().or(runs); i += 2; }
                    other => { file = Some(other.to_string()); i += 1; }
                }
            }
            let (Some(file), Some(setup)) = (file, setup) else {
                loom::warn("usage: penelope eval TASKS.jsonl --setup large|large-subagents|cluster|all [--runs PATH]");
                std::process::exit(2);
            };
            // The project around .penelope keeps its runs in docs/RUNS.md even when the command is called from a subfolder.
            let runs = runs.unwrap_or_else(|| {
                let parent = ctx.root.parent().unwrap_or(std::path::Path::new("."));
                if parent.join("docs").is_dir() {
                    parent.join("docs").join("RUNS.md").to_string_lossy().to_string()
                } else {
                    "docs/RUNS.md".to_string()
                }
            });
            match eval::eval(&ctx, &file, &setup, Some(std::path::Path::new(&runs))) {
                Ok(text) => println!("{}", text),
                Err(e) => { loom::warn(&e); std::process::exit(2); }
            }
        }
        "replay" => {
            let id = args.get(1).cloned().unwrap_or_default();
            let model = args.iter().position(|a| a == "--model").and_then(|i| args.get(i + 1)).cloned().unwrap_or_default();
            if id.is_empty() || id.starts_with('-') || model.is_empty() {
                loom::warn("usage: penelope replay ID --model MODEL");
                std::process::exit(2);
            }
            match eval::replay(&ctx, &id, &model) {
                Ok(text) => println!("{}", text),
                Err(e) => { loom::warn(&e); std::process::exit(2); }
            }
        }
        "input" => match args.get(1).map(String::as_str) {
            Some("install") => match input::install() {
                Ok(text) => println!("{}", text),
                Err(e) => { loom::warn(&e); std::process::exit(2); }
            },
            Some("enable") => match input::enable(&mut ctx) {
                Ok(text) => println!("{}", text),
                Err(e) => { loom::warn(&e); std::process::exit(2); }
            },
            _ => print!("{}", input::status(&ctx)),
        },
        "update" => {
            let result = if args.iter().any(|a| a == "--check") { update::check(&ctx) } else { update::install(&ctx) };
            match result {
                Ok(text) => println!("{}", text),
                Err(e) => { loom::warn(&e); std::process::exit(2); }
            }
        }
        "models" => {
            let current = providers::of(&ctx.cfg["main"]).map(|p| p.name.to_string());
            let provider = args.get(1).cloned().or(current).unwrap_or_else(|| "claude-cli".to_string());
            let model = if providers::of(&ctx.cfg["main"]).map(|p| p.name) == Some(provider.as_str()) {
                ctx.cfg["main"].clone()
            } else if let Some(p) = providers::get(&provider) {
                serde_json::json!({"kind": p.kind, "base_url": p.base, "model": p.model, "api_key_env": p.key_env})
            } else {
                loom::warn(&format!("no provider named {} (penelope providers)", provider));
                std::process::exit(2);
            };
            match providers::fetch_models(&ctx, &provider, &model) {
                Ok(list) => {
                    println!("{}: {} chat models", provider, list.len());
                    for m in list {
                        println!("{:<44} {:<32} {}", m.id, m.label.chars().take(32).collect::<String>(),
                                 if m.efforts.is_empty() { "no effort".to_string() } else { m.efforts.join(" ") });
                    }
                }
                Err(e) => {
                    loom::warn(&e);
                    let static_list = providers::static_models(&provider);
                    if static_list.is_empty() { std::process::exit(2); }
                    println!("built-in names for {}:", provider);
                    for m in static_list { println!("{:<44} {}", m.id, if m.efforts.is_empty() { "no effort".to_string() } else { m.efforts.join(" ") }); }
                }
            }
        }
        "provider" => match args.get(1).map(String::as_str) {
            Some("use") => {
                let report = providers::use_provider(&mut ctx, &args[2..]);
                println!("{}", report);
                if report.starts_with("NOT") {
                    std::process::exit(2);
                }
            }
            _ => print!("{}", providers::list(&ctx)),
        },
        "undo" => println!("{}", files::undo(&ctx)),
        "allow" => match args.get(1).map(String::as_str) {
            Some("clear") => println!("{}", tools::clear_allowed(&ctx)),
            _ => print!("{}", tools::allowed_commands(&ctx)),
        },
        "jobs" => match (args.get(1).map(String::as_str), args.get(2).and_then(|n| n.parse::<u64>().ok())) {
            (Some("stop"), Some(id)) => println!("{}", jobs::stop(&ctx, id)),
            (Some("read"), Some(id)) => println!("{}", jobs::read(&ctx, id)),
            _ => print!("{}", jobs::list(&ctx)),
        },
        "mcp" if args.get(1).map(String::as_str) == Some("add") && args.len() >= 4 => {
            let allow = args[4..].iter().any(|a| a == "--allow");
            let extra: Vec<String> = args[4..].iter().filter(|a| *a != "--allow").cloned().collect();
            println!("{}", plugins::mcp_add(&ctx, &args[2], &args[3], &extra, allow));
        }
        "mcp" if args.get(1).map(String::as_str) == Some("remove") && args.len() >= 3 => {
            println!("{}", plugins::mcp_remove(&ctx, &args[2]));
        }
        "mcp" if args.get(1).map(String::as_str) == Some("allow") && args.len() >= 3 => {
            println!("{}", plugins::mcp_set_allow(&ctx, &args[2], true));
        }
        "mcp" if args.get(1).map(String::as_str) == Some("deny") && args.len() >= 3 => {
            println!("{}", plugins::mcp_set_allow(&ctx, &args[2], false));
        }
        "mcp" if args.get(1).map(String::as_str) == Some("allow-tool") && args.len() >= 4 => {
            println!("{}", plugins::mcp_set_tool_allow(&ctx, &args[2], &args[3], true));
        }
        "mcp" if args.get(1).map(String::as_str) == Some("deny-tool") && args.len() >= 4 => {
            println!("{}", plugins::mcp_set_tool_allow(&ctx, &args[2], &args[3], false));
        }
        "mcp" if args.get(1).map(String::as_str) == Some("call") && args.len() >= 4 => {
            let payload: serde_json::Value = args.get(4).and_then(|j| serde_json::from_str(j).ok()).unwrap_or_else(|| serde_json::json!({}));
            match mcpclient::call(&ctx, &args[2], &args[3], &payload) {
                Ok(text) => println!("{}", text),
                Err(e) => println!("{}", e),
            }
        }
        "mcp" if args.get(1).map(String::as_str) == Some("list") => {
            print!("{}", mcpclient::list(&ctx, args.iter().any(|a| a == "--probe")));
        }
        "mcp" => mcp::serve(&ctx),
        "acp" => {
            if let Err(e) = acp::run(&ctx) {
                loom::warn(&e);
                std::process::exit(1);
            }
        }
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
            let mut json = false;
            let mut words = Vec::new();
            for arg in &args[1..] {
                if arg == "--json" { json = true; } else { words.push(arg.clone()); }
            }
            let text = words.join(" ");
            let mut chat = backend::Chat::new();
            if json {
                if !answer_json(&ctx, &mut chat, &text) { std::process::exit(1); }
            } else {
                answer(&ctx, &mut chat, &text);
            }
        }
        "search" if args.get(1).map(String::as_str) == Some("fanout") => match args.get(2).map(String::as_str) {
            Some("on") => println!("{}", search::set_fanout(&ctx, true)),
            Some("off") => println!("{}", search::set_fanout(&ctx, false)),
            _ => println!("{}", search::health(&ctx, false)),
        },
        "search" if args.get(1).map(String::as_str) == Some("health") => {
            print!("{}", search::health(&ctx, args.iter().any(|a| a == "--probe")));
        }
        "search" if args.get(1).map(String::as_str) == Some("probe") => {
            print!("{}", search::probe(&ctx, args.iter().any(|a| a == "--daily")));
        }
        "search" if args.get(1).map(String::as_str) == Some("instances") => match args.get(2).map(String::as_str) {
            Some("add") if args.len() >= 4 => println!("{}", search::instances_add(&ctx, &args[3])),
            _ => print!("{}", search::instances(&ctx)),
        },
        "search" => {
            let mut engine = None;
            let mut fresh = false;
            let mut words = Vec::new();
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--engine" => { engine = args.get(i + 1).cloned(); i += 2; }
                    "--fresh" => { fresh = true; i += 1; }
                    other => { words.push(other.to_string()); i += 1; }
                }
            }
            let opts = search::Options { engine, fresh, ..Default::default() };
            let q = words.join(" ");
            match search::search_hits(&ctx, &q, &opts) {
                Ok((hits, engines)) => println!("{}", search::format_hits(&hits, &q, &engines)),
                Err(e) => { loom::warn(&e); std::process::exit(2); }
            }
        }
        "research" => {
            let mut depth = "standard".to_string();
            let mut pages = 4usize;
            let mut recency = String::new();
            let mut words = Vec::new();
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--depth" => { depth = args.get(i + 1).cloned().unwrap_or(depth); i += 2; }
                    "--pages" => { pages = args.get(i + 1).and_then(|n| n.parse().ok()).unwrap_or(pages); i += 2; }
                    "--recency" => { recency = args.get(i + 1).cloned().unwrap_or_default(); i += 2; }
                    other => { words.push(other.to_string()); i += 1; }
                }
            }
            println!("{}", research::run(&ctx, &words.join(" "), &depth, pages, &recency));
        }
        "scout" => println!("{}", scout::run(&ctx, &args[1..].join(" "), "")),
        "work" => {
            let verb = args.get(1).map(String::as_str).unwrap_or("");
            let name = args.get(2).map(String::as_str).unwrap_or("");
            let cwd = std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            match verb {
                "new" if !name.is_empty() => println!("{}", work::new(&ctx, &cwd, name)),
                "list" | "" => print!("{}", work::list(&ctx, &cwd)),
                "diff" if !name.is_empty() => print!("{}", work::diff(&ctx, &cwd, name, args.iter().any(|a| a == "--stat"))),
                "apply" if !name.is_empty() => println!("{}", work::apply(&ctx, &cwd, name)),
                "remove" if !name.is_empty() => println!("{}", work::remove(&ctx, &cwd, name, args.iter().any(|a| a == "--force"))),
                _ => println!("penelope work new NAME | list | diff NAME | apply NAME | remove NAME [--force]"),
            }
        }
        "doctor" => doctor(&ctx),
        "serve" => {
            if let Err(e) = server::run(&ctx, &args[1..]) {
                loom::warn(&e);
                std::process::exit(1);
            }
        }
        "crew" => print!("{}", tools::crew(&ctx)),
        "sessions" => print!("{}", sessions::list(&ctx)),
        "stats" => print!("{}", tools::stats(&ctx)),
        "plugin" => match args.get(1).map(String::as_str) {
            Some("add") if args.len() >= 3 => println!("{}", plugins::add(&ctx, &args[2])),
            Some("install") if args.len() >= 3 => println!("{}", plugins::install(&ctx, &args[2])),
            Some("import") => {
                let lines = plugins::import_known(&ctx);
                if lines.is_empty() { println!("no new plugin found in the folders of other harnesses"); }
                for line in lines { println!("{}", line); }
            }
            Some(verb @ ("allow" | "deny")) if args.len() >= 3 => println!("{}", plugins::set_allow(&ctx, &args[2], verb == "allow")),
            Some("update") if args.len() >= 3 => println!("{}", plugins::update(&ctx, &args[2])),
            Some("remove") if args.len() >= 3 => println!("{}", plugins::remove(&ctx, &args[2])),
            Some("commands") => print!("{}", plugins::command_index(&ctx, args.get(2).map(String::as_str))),
            Some("command") if args.len() >= 3 => match plugins::expand_command(&ctx, &args[2], &args[3..].join(" ")) {
                Some(text) => println!("{}", text),
                None => {
                    loom::warn(&format!("no plugin command {} (penelope plugin commands)", args[2]));
                    std::process::exit(2);
                }
            },
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
                repl(&ctx, resume.map(|entry| backend::Chat::resume(&ctx, &entry)));
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
    // A server that lives between calls gets a clean stop when Penelope leaves.
    mcpclient::shutdown();
}

/// Captures the real screen, clicks the first element whose text holds `find`, captures again. One process, so the
/// desktop capture and the click share the ScreenCodec session.
fn screen_desktop_click(ctx: &config::Ctx, find: &str) -> Result<String, String> {
    let before = screen::tool(ctx, &serde_json::json!({"action": "desktop", "find": find, "level": "lean"}))?;
    let mut id = None;
    for line in before.lines() {
        if !line.to_lowercase().contains(&find.to_lowercase()) {
            continue;
        }
        if let Some(word) = line.split_whitespace().find(|w| {
            let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
            w.len() > 3 && w.chars().any(|c| c.is_ascii_digit()) && w.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
        }) {
            id = Some(word.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_string());
            break;
        }
    }
    let Some(id) = id else { return Err(format!("no element with the text \"{}\" on the screen. The capture was:\n{}", find, before)); };
    let click = screen::tool(ctx, &serde_json::json!({"action": "desktop_click", "id": id}))?;
    let after = screen::tool(ctx, &serde_json::json!({"action": "desktop", "level": "lean"}))?;
    Ok(format!("element {} · {}\n{}\n--- after the click ---\n{}", id, find, click, after))
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

/// One turn as newline-delimited JSON events, for CI and editors. Only JSON goes to stdout.
fn answer_json(ctx: &config::Ctx, chat: &mut backend::Chat, text: &str) -> bool {
    let expanded = context::expand(ctx, text);
    let (notes, trail, _) = tools::recall_all(ctx, &expanded.text, 1600);
    let emit = |value: serde_json::Value| {
        println!("{}", serde_json::to_string(&value).unwrap());
    };
    emit(serde_json::json!({"event": "start", "model": ctx.cfg["main"]["model"], "kind": ctx.cfg["main"]["kind"], "memory": trail}));
    let mut masked = 0usize;
    let result = chat.turn_with(ctx, &expanded.text, &notes, &mut |e| match e {
        backend::Event::Delta(s) => emit(serde_json::json!({"event": "delta", "text": s})),
        backend::Event::Thinking(s) => emit(serde_json::json!({"event": "thinking", "text": s})),
        backend::Event::Text(s) => emit(serde_json::json!({"event": "text", "text": s})),
        backend::Event::ToolStart { id, name, detail } => emit(serde_json::json!({"event": "tool_start", "id": id, "name": name, "detail": detail})),
        backend::Event::ToolEnd { id, ok, summary } => emit(serde_json::json!({"event": "tool_end", "id": id, "ok": ok, "summary": summary})),
        backend::Event::Masked(n) => emit(serde_json::json!({"event": "masked_request", "items": n})),
        backend::Event::MaskedAnswer(n) => { masked += n; emit(serde_json::json!({"event": "masked_answer", "items": n})); }
    });
    match result {
        Ok(answer) => {
            emit(serde_json::json!({"event": "result", "text": answer, "tokens_in": chat.input_tokens, "tokens_out": chat.output_tokens,
                                    "tool_calls": chat.trace.len(), "masked": masked}));
            true
        }
        Err(e) => {
            emit(serde_json::json!({"event": "error", "error": e}));
            false
        }
    }
}

fn answer(ctx: &config::Ctx, chat: &mut backend::Chat, text: &str) {
    let expanded = context::expand(ctx, text);
    for note in &expanded.notes {
        loom::warn(note);
    }
    if !expanded.refs.is_empty() {
        loom::say(&format!("context: {}", expanded.refs.iter().map(|r| format!("{} ({} bytes)", r.token, r.bytes)).collect::<Vec<_>>().join(", ")));
    }
    let (notes, trail, cached) = tools::recall_all(ctx, &expanded.text, 1600);
    if !trail.is_empty() {
        loom::say(&format!("memory: start / {}{}", trail.last().unwrap().replace('/', " / "), if cached { " (stored walk)" } else { "" }));
    }
    let mut masked = 0;
    let result = chat.turn_with(ctx, &expanded.text, &notes, &mut |e| {
        if let backend::Event::MaskedAnswer(n) = e {
            masked += n;
        }
    });
    match result {
        Ok(a) => {
            if !trail.is_empty() {
                memory::Memory::open(&ctx.root).mark_used(&trail);
            }
            println!("{}", a);
            if masked > 0 {
                loom::say(&format!("utis: {} items masked in the answer", masked));
            }
            loom::say(&format!("tokens so far: {} in, {} out", chat.input_tokens, chat.output_tokens));
            loom::say(&format!("tool calls of this turn: {}, {} of them failed or refused", chat.trace.len(), chat.trace.iter().filter(|c| tools::in_trouble(c)).count()));
            if ctx.cfg["auto_learn"].as_bool().unwrap_or(true) {
                for id in tools::learn(ctx, text, &a) {
                    loom::say(&format!("learned: {}", id));
                }
                for id in tools::learn_tools(ctx, text, &chat.trace) {
                    loom::say(&format!("lesson from the tool calls: {}", id));
                }
                for id in tools::consolidate(ctx, &[text.to_string()]) {
                    loom::say(&format!("kept for every project: {}", id));
                }
                if let Some(name) = skills::from_trace(ctx, text, &chat.trace) {
                    loom::say(&format!("learned skill: {}", name));
                }
            }
        }
        Err(e) => loom::warn(&e),
    }
}

fn repl(ctx: &config::Ctx, resume: Option<backend::Chat>) {
    for line in plugins::import_known(ctx) {
        loom::say(&format!("plugin taken over: {}", line));
    }
    startup::play(&format!("main model: {} ({})   weaker model: {} ({})",
        if ctx.cfg["main"]["kind"].as_str() != Some("claude-cli") {
            ctx.cfg["main"]["model"].as_str().unwrap_or("?").to_string()
        } else {
            format!("{} via claude", ctx.cfg["main"]["model"].as_str().unwrap_or("?"))
        },
        "ready", ctx.cfg["weak"]["model"].as_str().unwrap_or("?"), "condenser"), !std::env::args().any(|a| a == "--no-animation"));
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
        if line.is_empty() {
            continue;
        }
        let mut leave = false;
        for action in context::parse(line) {
            match action {
                context::Action::Command(command) => {
                    if !repl_command(ctx, &command) {
                        leave = true;
                        break;
                    }
                }
                context::Action::Turn(text) => answer(ctx, &mut chat, &text),
                context::Action::Agent(call) => match agents::parse(&call) {
                    Ok(spec) => {
                        let _ = agents::save(ctx, &spec);
                        match agents::run(ctx, &spec) {
                            Ok(text) => println!("{}", text),
                            Err(e) => loom::warn(&e),
                        }
                    }
                    Err(e) => loom::warn(&e),
                },
            }
        }
        if leave {
            break;
        }
    }
    loom::say("bye");
}

/// One slash command of the plain chat. False ends the session.
fn repl_command(ctx: &config::Ctx, line: &str) -> bool {
    let (name, rest) = line.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((line, ""));
    match name {
        "quit" | "q" | "exit" => return false,
        "map" => print!("{}", tools::memory_map(ctx)),
        "brain" => {
            if rest == "write" {
                match brain::write_vault(ctx) {
                    Ok((p, n)) => println!("{} files in {}", n, brain::show(&p)),
                    Err(e) => loom::warn(&e),
                }
            } else {
                print!("{}", brain::ascii(ctx));
            }
        }
        "cost" => print!("{}", cost::report(ctx)),
        "tools" => print!("{}", tools_table(ctx)),
        "route" => print!("{}", route_text(ctx)),
        "agents" => print!("{}", agents::list(ctx)),
        "plugins" => print!("{}", plugins::list(ctx)),
        "note" | "btw" if !rest.is_empty() => println!("{}", tools::memory_note_scope(ctx, "", rest, "project")),
        "rules" if !rest.is_empty() => println!("{}", tools::memory_note_scope(ctx, "rules", rest, "global")),
        "rules" => println!("{}", tools::memory_go(ctx, "rules")),
        "orchestrate" if !rest.is_empty() => {
            let spec = agents::parse(rest).unwrap_or_else(|_| agents::Spec { name: "orchestrator".into(), role: "orchestrator".into(),
                thinking: "high".into(), goal: rest.to_string(), max_sub_agents: 3, max_parallel: 2, max_steps: 12,
                budget_eur: 0.0, model: String::new(), verify: true, language: "en".into(), mcp: Vec::new() });
            let _ = agents::save(ctx, &spec);
            match agents::run(ctx, &spec) {
                Ok(text) => println!("{}", text),
                Err(e) => loom::warn(&e),
            }
        }
        "help" => println!("{}", HELP),
        other => loom::warn(&format!("no command /{} in the plain chat (use the full screen interface)", other)),
    }
    true
}

/// Every tool with the roles that may call it.
fn tools_table(ctx: &config::Ctx) -> String {
    route::table(ctx)
}

fn route_text(ctx: &config::Ctx) -> String {
    route::report(ctx)
}

fn skills_list(ctx: &config::Ctx) -> String {
    let local = crate::skills::list(ctx);
    let mut out = String::new();
    for (name, description) in &local {
        out.push_str(&format!("{} — {} (learned here)\n", name, description));
    }
    for p in plugins::load(ctx)["plugins"].as_array().cloned().unwrap_or_default() {
        for s in p["skills"].as_array().cloned().unwrap_or_default() {
            out.push_str(&format!("{} — {} (plugin {})\n", s["name"].as_str().unwrap_or("?"), s["description"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or("?")));
        }
    }
    if out.is_empty() { "no skill yet. A turn with 5 or more tool calls writes one, or the model writes one with skill_new.".to_string() } else { out }
}

fn last_session_users(ctx: &config::Ctx) -> Vec<String> {
    let Some(entry) = sessions::all(ctx).pop() else { return Vec::new() };
    let Some(id) = entry["id"].as_str() else { return Vec::new() };
    sessions::load_transcript(ctx, id).iter().filter(|i| i["k"].as_str() == Some("user")).filter_map(|i| i["t"].as_str().map(str::to_string)).collect()
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
    let needs_claude = ["main", "weak"].iter().any(|r| !llm::is_api(llm::kind_of(&ctx.cfg[*r])));
    line("claude command (login of the user)", claude.is_some() || !needs_claude, claude.map(|p| p.display().to_string())
        .unwrap_or_else(|| if needs_claude { "not on the PATH".into() } else { "not on the PATH, no model setting needs it".into() }));
    for role in ["main", "weak", "scout"] {
        let m = &ctx.cfg[role];
        if !m["model"].is_string() {
            continue;
        }
        // The line names the variable and says whether it is set. The key itself is never read into this text.
        let (table, env) = (providers::of(m), m["api_key_env"].as_str().unwrap_or(""));
        let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"].iter().any(|h| m["base_url"].as_str().unwrap_or("").starts_with(h));
        let optional = table.map(|p| p.key_optional).unwrap_or(env.is_empty()) || local;
        let state = table.map(providers::state_of).unwrap_or_else(|| providers::key_state(env, optional));
        line(&format!("{} model", role), state != "no key", format!("{} on {}, {}{}", m["model"].as_str().unwrap_or("?"),
            table.map(|p| p.name).unwrap_or("an address outside the table"), state, if env.is_empty() { String::new() } else { format!(" in {}", env) }));
    }
    let sc = ctx.cfg["screencodec"]["command"].as_str().filter(|c| !c.is_empty()).map(std::path::PathBuf::from).or_else(|| config::which("screencodec-mcp"));
    line("screencodec-mcp (eyes: pages and pictures as text)", sc.is_some(), sc.map(|p| p.display().to_string()).unwrap_or_else(|| "not on the PATH, Penelope runs without it".into()));
    line("run policy", true, ctx.cfg["run_policy"].as_str().unwrap_or("safe").to_string());
    line("edit policy (file changes)", true, ctx.cfg["edit_policy"].as_str().unwrap_or("ask").to_string());
    line("delegate (who searches)", true, format!("{}: {}", ctx.cfg["delegate"].as_str().unwrap_or("auto"), match ctx.cfg["delegate"].as_str().unwrap_or("auto") {
        "off" => "the main model searches itself",
        "offer" => "the main model may search itself or send a scout",
        "web" => "web research runs on the scout, the main model searches files itself",
        _ => "every search runs on the weaker model as scout",
    }));
    line("web search", true, search::kind(ctx).to_string());
    line("search API", true, search::api_status(ctx));
    let reader = ctx.cfg["reader"]["url"].as_str().filter(|u| !u.is_empty());
    line("reader API", true, reader.map(|u| u.to_string()).unwrap_or_else(|| "not set; static HTML and local PDF only".to_string()));
    line("pdftotext (local PDF)", config::which("pdftotext").is_some(), "a scanned PDF still needs OCR or the reader API".to_string());
    line("memory map", true, format!("{} lines", tools::memory_map(ctx).lines().count()));
    let shared = memory::open_global().index(200);
    line("shared memory", true, format!("{} waypoints (rules for every project)", if shared.is_empty() { 0 } else { shared.lines().count() }));
    line("tool contracts", true, format!("{} built-in tools, every call checked by route::dispatch", spec::TOOLS.len()));
    let (cursor, ok) = cursor::backend(ctx);
    line("cursor (Penelope's own pointer)", ok, cursor);
    let desktop = ctx.cfg["screencodec"]["desktop"].as_str().unwrap_or("off");
    line("ScreenCodec desktop", true, format!("{} (SDL_MCP_DESKTOP; ask needs your confirmation, on captures at once)", desktop));
    for (role, _) in route::ROLES {
        if matches!(*role, "router" | "filter") {
            line(&format!("role {}", role), true, route::model_name(ctx, role));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_names_the_eval_and_replay_commands() {
        assert!(HELP.contains("penelope eval TASKS.jsonl --setup large|large-subagents|cluster|all"), "{}", HELP);
        assert!(HELP.contains("penelope replay ID --model MODEL"), "{}", HELP);
        assert!(HELP.contains("penelope cluster run GOAL [--profile NAME] [--shadow PROFILE] [--dry] [--sequential]"), "{}", HELP);
        assert!(HELP.contains("a shadow profile runs beside the first"), "{}", HELP);
        assert!(HELP.contains("prints parts, model and an estimate, starts nothing"), "{}", HELP);
        assert!(HELP.contains("penelope cluster advise [NAME]"), "{}", HELP);
        assert!(HELP.contains("--apply writes the suggested model into the profile"), "{}", HELP);
        assert!(HELP.contains("(built-in, .json or .cluster)"), "{}", HELP);
        assert!(HELP.contains("penelope factory new NAME --profile P (--every 3h | --at HH:MM)"), "{}", HELP);
        assert!(HELP.contains("penelope factory list") && HELP.contains("penelope factory run NAME") && HELP.contains("penelope factory stop NAME"), "{}", HELP);
        assert!(HELP.contains("penelope acp") && HELP.contains("Agent Client Protocol"), "{}", HELP);
    }
}
