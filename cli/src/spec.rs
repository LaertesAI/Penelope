//! One table for every built-in tool: name, description and the JSON schema of its arguments.
//!
//! This table is the only place where the interface of a tool is written. `mcp.rs` builds the list a model sees from
//! it, `route.rs` checks every call against it, and the tests in this file prove that the table and the dispatch agree.
//! A tool whose contract is broken fails at start, not in the middle of a turn.

use serde_json::{json, Value};

/// Name, description and input schema of one tool. The schema is JSON text, parsed once per request.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: &'static str,
}

pub const TOOLS: &[Tool] = &[
    Tool { name: "run",
        description: "Run a shell command. Short output comes back as it is. Long output is condensed by a weaker model, so that it costs few tokens. Use it for builds, tests, logs and searches.",
        schema: r#"{"type":"object","properties":{"command":{"type":"string"},"question":{"type":"string","description":"What you want to know from the output."},"cwd":{"type":"string","description":"Working directory."}},"required":["command"]}"# },
    Tool { name: "run_detail",
        description: "Lines of the stored raw output of an earlier run that hold a word.",
        schema: r#"{"type":"object","properties":{"id":{"type":"integer"},"pattern":{"type":"string"}},"required":["id"]}"# },
    Tool { name: "job_start",
        description: "Start a command in the background and return at once with a job number. For servers, watchers and long builds. A command outside the safe list asks the user first, as with run.",
        schema: r#"{"type":"object","properties":{"command":{"type":"string"},"cwd":{"type":"string","description":"Working directory."}},"required":["command"]}"# },
    Tool { name: "job_read",
        description: "The output a background job wrote since the last job_read, at most 60 lines, and whether the job still runs.",
        schema: r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"# },
    Tool { name: "job_stop",
        description: "End a background job and every process it started.",
        schema: r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"# },
    Tool { name: "fetch",
        description: "Load a web page, turn the main article into Markdown (scripts, navigation and cookie banners removed) and let a weaker model answer a question from it. The page itself does not enter your context.",
        schema: r#"{"type":"object","properties":{"url":{"type":"string"},"question":{"type":"string"}},"required":["url","question"]}"# },
    Tool { name: "web_search",
        description: "Search the web across several engines. Returns up to 8 ranked results with title, address, snippet, engine and date. Use fresh true to skip the local cache, engine to name one source, limit 1 to 20. Read a result with fetch.",
        schema: r#"{"type":"object","properties":{"query":{"type":"string"},"engine":{"type":"string","description":"startpage, duckduckgo, bing, searxng, brave, tavily, exa, firecrawl or wikipedia."},"fresh":{"type":"boolean","description":"Skip the cache."},"limit":{"type":"integer","description":"1 to 20."}},"required":["query"]}"# },
    Tool { name: "research",
        description: "Research a question: several web searches, the main text of the best pages, and a short cited report. The pages stay out of your context. depth quick uses one query, standard two, deep four.",
        schema: r#"{"type":"object","properties":{"question":{"type":"string"},"depth":{"type":"string","enum":["quick","standard","deep"]},"max_pages":{"type":"integer","description":"1 to 8, default 4."},"recency":{"type":"string","enum":["","day","week","month","year"]}},"required":["question"]}"# },
    Tool { name: "deep_research",
        description: "Deep research on a question: it asks the user when the question is ambiguous, splits it into 3 to 6 sub-questions, reads full pages in parallel, runs a gap loop of up to max_rounds rounds, writes a cited report, re-checks each citation against its source and lists assumptions. Slow and costly; use research for a quick lookup. Files go to .penelope/research/SLUG/.",
        schema: r#"{"type":"object","properties":{"question":{"type":"string"},"max_rounds":{"type":"integer","description":"1 to 8, default from the setting research.max_rounds (3)."}},"required":["question"]}"# },
    Tool { name: "read",
        description: "Read a text file with line numbers. Without offset and limit the first 300 lines come back.",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","description":"First line, counted from 1."},"limit":{"type":"integer","description":"Number of lines, at most 1500."},"cwd":{"type":"string"}},"required":["path"]}"# },
    Tool { name: "write",
        description: "Create a file or replace its whole content. The user is asked first.",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"},"cwd":{"type":"string"}},"required":["path","text"]}"# },
    Tool { name: "edit",
        description: "Replace pieces of text in one file. Give old and new for one place, or the list edits for several places in the same file. Every old text has to occur exactly once, so copy enough lines from read. All replacements of a call happen or none. The user is asked once and sees one diff.",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"},"all":{"type":"boolean","description":"Replace every occurrence."},"edits":{"type":"array","description":"Several replacements for this file, applied in order.","items":{"type":"object","properties":{"old":{"type":"string"},"new":{"type":"string"},"all":{"type":"boolean"}},"required":["old","new"]}},"cwd":{"type":"string"}},"required":["path"]}"# },
    Tool { name: "unzip",
        description: "Unpack a ZIP archive into a folder below the working directory. Zip-slip paths, symbolic links, more than 10,000 entries, files over 50 MB and a total over 200 MB are refused. The user is asked once and sees the file list.",
        schema: r#"{"type":"object","properties":{"archive":{"type":"string","description":"Path of the .zip file."},"into":{"type":"string","description":"Target folder; default is the archive name without .zip."},"cwd":{"type":"string"}},"required":["archive"]}"# },
    Tool { name: "undo",
        description: "Take back the last change that write or edit made: the file gets its old content, a file that was new is removed. Call it again to go one change further back. It asks no question.",
        schema: r#"{"type":"object","properties":{}}"# },
    Tool { name: "find",
        description: "List files by name pattern such as *.rs, or by path pattern such as src/**/*.rs. Folders like .git, target and node_modules are skipped.",
        schema: r#"{"type":"object","properties":{"pattern":{"type":"string"},"dir":{"type":"string"},"cwd":{"type":"string"}},"required":["pattern"]}"# },
    Tool { name: "grep",
        description: "Find lines that hold a piece of text in the files below a folder. Plain text by default, lower case text ignores case. With regex true the text is a regular expression that is matched against each line, case sensitive unless it starts with (?i).",
        schema: r#"{"type":"object","properties":{"text":{"type":"string"},"dir":{"type":"string"},"files":{"type":"string","description":"Name or path pattern of the files to look into, for example *.rs."},"regex":{"type":"boolean","description":"Read text as a regular expression."},"cwd":{"type":"string"}},"required":["text"]}"# },
    Tool { name: "tree",
        description: "Folders and files below a folder as an indented list, 2 levels deep unless depth says otherwise. Use it first in a project you do not know.",
        schema: r#"{"type":"object","properties":{"dir":{"type":"string"},"depth":{"type":"integer"},"cwd":{"type":"string"}}}"# },
    Tool { name: "outline",
        description: "The outline of a file: every line that opens a function, a type, a class or a section, with its line number. Cheaper than read for a long file.",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"cwd":{"type":"string"}},"required":["path"]}"# },
    Tool { name: "lsp",
        description: "Ask rust-analyzer about one Rust file: every error and warning with file:line:column and message, the type at a position, or where a symbol is defined. Read-only. A missing rust-analyzer names the install command.",
        schema: r#"{"type":"object","properties":{"action":{"type":"string","enum":["diagnostics","type","definition"],"description":"diagnostics: all errors and warnings of the file; type: the type at line and column; definition: where the symbol at line and column is defined."},"path":{"type":"string","description":"Rust file, relative to cwd."},"line":{"type":"integer","description":"Line counted from 1, for type and definition."},"column":{"type":"integer","description":"Column counted from 1, for type and definition."},"cwd":{"type":"string","description":"Working directory."}},"required":["action","path"]}"# },
    Tool { name: "ask_user",
        description: "Ask the user before you guess. Use it when two readings of the request lead to different work, or before you create a cluster, a workflow or a scheduled task with a missing goal, limit, schedule or right. Send questions: 1 to 6 questions in one dialog, each with a short header, 2 to 6 options (a label and an optional description), multi_select for several answers and allow_text (default true) so the person can type their own answer. Ask everything you need in one call. The old form with question and options (texts) still works.",
        schema: r#"{"type":"object","properties":{"questions":{"type":"array","minItems":1,"maxItems":6,"items":{"type":"object","properties":{"question":{"type":"string"},"header":{"type":"string","description":"Short label, up to 16 characters."},"options":{"type":"array","minItems":2,"maxItems":6,"items":{"type":"object","properties":{"label":{"type":"string"},"description":{"type":"string"}},"required":["label"]}},"multi_select":{"type":"boolean"},"allow_text":{"type":"boolean","description":"The person may type a free answer. Default true."}},"required":["question","options"]}},"question":{"type":"string","description":"Old form: one question."},"options":{"type":"array","items":{"type":"string"},"description":"Old form: 2 to 6 answers."}}}"# },
    Tool { name: "scout",
        description: "Hand a lookup or research task to a weaker model. It searches the web, reads pages and files, runs read-only commands and returns a short report. Its tool calls cost you nothing. Give the full question and say what you need back.",
        schema: r#"{"type":"object","properties":{"task":{"type":"string"},"cwd":{"type":"string"}},"required":["task"]}"# },
    Tool { name: "scouts",
        description: "Run 2 to 4 lookups side by side, each on the weaker model. The reports come back in the order of the tasks. Use it when one task does not depend on another, for example one lookup per module.",
        schema: r#"{"type":"object","properties":{"tasks":{"type":"array","items":{"type":"string"},"description":"2 to 4 tasks."},"cwd":{"type":"string"}},"required":["tasks"]}"# },
    Tool { name: "plan",
        description: "Store the steps of the running task, each with a state: pending, doing or done. Send the whole list every time. The user sees it next to the conversation.",
        schema: r#"{"type":"object","properties":{"items":{"type":"array","items":{"type":"object","properties":{"text":{"type":"string"},"state":{"type":"string","enum":["pending","doing","done"]}},"required":["text","state"]}}},"required":["items"]}"# },
    Tool { name: "memory_go",
        description: "Follow the memory map to the waypoint for a topic and return the notes on the way: rules, preferences and facts the user wants kept. A note lives in this project or, with scope global, in every project.",
        schema: r#"{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}"# },
    Tool { name: "memory_note",
        description: "Store one rule, preference or fact at a waypoint of the memory map, for example path \"rust/errors\". Without a path Penelope files it by language or topic. scope global keeps a rule for every project of this PC.",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"},"scope":{"type":"string","enum":["project","global","auto"]}},"required":["text"]}"# },
    Tool { name: "memory_forget",
        description: "Take back one stored note by its text. Use it when a note is wrong or out of date. The command memory unravel takes back the newest notes instead.",
        schema: r#"{"type":"object","properties":{"text":{"type":"string"},"scope":{"type":"string","enum":["project","global","auto"]}},"required":["text"]}"# },
    Tool { name: "memory_find",
        description: "Find where the memory holds an answer: returns pointers only (node id, title, one line, path, origin level, score), never whole notes. Read a pointer with memory_go. If nothing matches it answers unknown.",
        schema: r#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20,"description":"default 8"}},"required":["query"]}"# },
    Tool { name: "memory_map",
        description: "The whole memory map as an indented list of waypoints. The Obsidian vault of the same map is written by penelope brain.",
        schema: r#"{"type":"object","properties":{}}"# },
    Tool { name: "skill_new",
        description: "Write a reusable skill for this project: a name, a one-line description and a Markdown body of instructions. Use it after a procedure worked and will be needed again. The skill then appears in the system prompt and in /skills.",
        schema: r#"{"type":"object","properties":{"name":{"type":"string","description":"Lower case letters, digits, - and _."},"description":{"type":"string"},"body":{"type":"string","description":"Markdown with the steps, the checks and the pitfalls."}},"required":["name","description","body"]}"# },
    Tool { name: "skill",
        description: "Read the instructions of one installed skill by its name. The system prompt lists the skills.",
        schema: r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}"# },
    Tool { name: "git_commit",
        description: "Stage the named files and commit them with a message. It never pushes. The user sees the file list and the message before anything happens.",
        schema: r#"{"type":"object","properties":{"files":{"type":"array","items":{"type":"string"},"description":"Paths to stage, at least one."},"message":{"type":"string"},"cwd":{"type":"string"}},"required":["files","message"]}"# },
    Tool { name: "pr_create",
        description: "Open a pull request with gh. The branch must already be pushed; this never pushes. The user is asked once with the title and the base branch.",
        schema: r#"{"type":"object","properties":{"title":{"type":"string"},"body":{"type":"string"},"base":{"type":"string","description":"Target branch, for example main."},"cwd":{"type":"string"}},"required":["title","body"]}"# },
    Tool { name: "check",
        description: "Hand a claim and its evidence to the judge role. The judge answers PASS or FAIL and one sentence. Use it before you report a task as done when a test or command is the evidence.",
        schema: r#"{"type":"object","properties":{"claim":{"type":"string"},"evidence":{"type":"string"}},"required":["claim","evidence"]}"# },
    Tool { name: "tool_new",
        description: "Write a new tool for this project. The tool is a shell command with placeholders such as {text}; every placeholder must be a declared argument. A call asks the user like run and passes the privacy filter. Use it when a task repeats and a fixed command would save steps.",
        schema: r#"{"type":"object","properties":{"name":{"type":"string","description":"Lower case letters, digits, _ and -."},"description":{"type":"string"},"command":{"type":"string","description":"Shell command with {argument} placeholders, quoted arguments are inserted safely."},"schema":{"type":"object","description":"JSON schema of the arguments, type object with properties."},"cwd":{"type":"string"}},"required":["name","description","command","schema"]}"# },
    Tool { name: "tool_list",
        description: "The tools Penelope wrote for this project, with their arguments and how often they were called.",
        schema: r#"{"type":"object","properties":{}}"# },
    Tool { name: "mcp_servers",
        description: "The MCP servers of the installed plugins, and with probe true the tools each one offers. With server and tool it shows that tool's full argument schema. Use it before mcp_call, because a server names its tools itself.",
        schema: r#"{"type":"object","properties":{"probe":{"type":"boolean","description":"Start each server and list its tools. At most 8 servers."},"server":{"type":"string","description":"With tool: show the full argument schema of that one tool."},"tool":{"type":"string"}}}"# },
    Tool { name: "mcp_call",
        description: "Call one tool of a plugin MCP server. The plugin must be allowed; a denied plugin changes nothing. Use mcp_servers first to learn the tool name and its arguments.",
        schema: r#"{"type":"object","properties":{"server":{"type":"string","description":"The prefixed server name from mcp_servers."},"tool":{"type":"string"},"arguments":{"type":"object"}},"required":["server","tool"]}"# },
    Tool { name: "attachment_read",
        description: "Read a range of an attachment. A long tool result comes as one line with an attachment uri; this tool returns its lines or bytes, at most 16 KB per call. Without a range it returns the first 16 KB.",
        schema: r#"{"type":"object","properties":{"uri":{"type":"string","description":"The uri from the attachment line."},"lines":{"type":"array","items":{"type":"integer"},"description":"[first, last], counted from 1, both included."},"bytes":{"type":"array","items":{"type":"integer"},"description":"[start, end), counted from 0."},"server":{"type":"string","description":"The MCP server named in the attachment line, if any."}},"required":["uri"]}"# },
    Tool { name: "tool_search",
        description: "Find a tool, plugin or skill for a capability you lack. Searches the connected servers and installed plugins first, then the official MCP Registry. It only suggests; the user installs with /tools install.",
        schema: r#"{"type":"object","properties":{"query":{"type":"string","description":"The capability, for example read a PDF. Paths, emails and tokens are removed before the registry sees it."},"limit":{"type":"integer","description":"At most 20, default 8."}},"required":["query"]}"# },
    Tool { name: "session_search",
        description: "Search the stored conversations of this folder for a word or phrase. Returns the session, its date and short matching lines. Use it when the user refers to earlier work that is not in this turn.",
        schema: r#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","description":"At most 20, default 8."}},"required":["query"]}"# },
    Tool { name: "http_request",
        description: "Send one HTTP request and return the status and the first part of the answer. For an API that is not a web page. Use fetch for a page that a weaker model should read.",
        schema: r#"{"type":"object","properties":{"url":{"type":"string"},"method":{"type":"string","enum":["GET","POST","PUT","PATCH","DELETE"]},"headers":{"type":"object"},"body":{"type":"string"}},"required":["url"]}"# },
    Tool { name: "json_query",
        description: "Read one value out of a JSON file by a dot path such as packages.0.name, or list the keys of an object. Cheaper than read for a large JSON file.",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"pointer":{"type":"string","description":"Dot path into the JSON; empty shows the top."}},"required":["path"]}"# },
    Tool { name: "git_diff",
        description: "The diff of the working tree or of the staged changes, optionally of one path. Read-only; use git_commit to commit.",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"cwd":{"type":"string"},"staged":{"type":"boolean"}}}"# },
    Tool { name: "verify_fix",
        description: "Run your reproduction test on the original code (it must fail) and on your change (it must pass), then the module tests (they must still pass). Runs in a throwaway git worktree; your files stay as they are. Call it before the final answer on a code task. With best_of N it also asks the weaker model for N fixes: your change wins when it passes, else the smallest model fix that passes. Commands need the same permission as run.",
        schema: r#"{"type":"object","properties":{"test":{"type":"string","description":"Path of the reproduction test, relative to cwd. Write it from the issue text first."},"run":{"type":"string","description":"Command that runs the test; {test} stands for its path. Empty: python3, node, sh or ruby by the file ending."},"module_tests":{"type":"string","description":"Command for the existing tests of the module. Empty: check.module_tests or test_command."},"cwd":{"type":"string"},"best_of":{"type":"integer","description":"0 checks only your change. 1 to 8 adds that many candidates from the weaker model."},"issue":{"type":"string","description":"The issue text, needed with best_of."},"files":{"type":"array","items":{"type":"string"},"description":"Files the candidates may change; empty: the files you changed. Test configs such as conftest.py or sitecustomize.py only when listed."}},"required":["test"]}"# },
    Tool { name: "clipboard",
        description: "Read the clipboard as text or put text on it. It uses wl-clipboard, xclip or pbcopy when one is installed, else clipboard.read and clipboard.write from config.json.",
        schema: r#"{"type":"object","properties":{"action":{"type":"string","enum":["read","write"]},"text":{"type":"string"}},"required":["action"]}"# },
    Tool { name: "agent",
        description: "Hand a whole goal to a named subagent. It plans, works and checks on its own roles and returns a short report. At most the limits you give it. A subagent cannot start another subagent.",
        schema: r#"{"type":"object","properties":{"name":{"type":"string","description":"The name shown in the crew panel."},"goal":{"type":"string"},"role":{"type":"string","enum":["orchestrator","lead","worker","judge"]},"thinking":{"type":"string","enum":["max","high","medium","low"]},"max_sub_agents":{"type":"integer"},"max_parallel":{"type":"integer"},"max_steps":{"type":"integer"},"budget_eur":{"type":"number"},"verify":{"type":"boolean"}},"required":["name","goal"]}"# },
    Tool { name: "cluster_start",
        description: "Start an orchestration of clusters, not subagents. The orchestrator splits the goal into 1 to 4 clusters from profiles such as coding, backend, frontend, live-test, research or solo, and runs them in parallel where the plan allows. Each cluster has parts, one memory file and a validator. Penelope asks the user for every limit that goal and arguments leave open (done criterion, abort rule, budget, rounds, rights, models, results) and for a final yes, so pass what you know. Returns the cluster ids and their status.",
        schema: r#"{"type":"object","properties":{"goal":{"type":"string"},"profile":{"type":"string","description":"Force one profile instead of letting the orchestrator plan. Empty lets it plan."},"parallel":{"type":"boolean","description":"Run independent clusters at the same time, default true."},"done":{"type":"string","description":"When the cluster is done."},"abort":{"type":"string","description":"When it gives up."},"budget":{"type":"string","description":"For example 5 EUR or 200k tokens."},"max_iterations":{"type":"string","description":"For example 10 iterations."},"rights":{"type":"string","description":"read-only, write files, network, install packages."},"models":{"type":"string"},"results_to":{"type":"string"}},"required":["goal"]}"# },
    Tool { name: "cluster_spawn",
        description: "Start a subcluster from a profile while this cluster part is running. The root profile's max_depth and max_models limit the whole tree; a part at the depth limit gets a refusal with the reason. The child reports to its parent, which reports to the orchestrator.",
        schema: r#"{"type":"object","properties":{"profile":{"type":"string","description":"A profile such as research, solo or a profile under .penelope/clusters/profiles."},"task":{"type":"string","description":"The concrete job of the subcluster."},"name":{"type":"string","description":"Optional short name; the profile name is the default."}},"required":["profile","task"]}"# },
    Tool { name: "cluster_ask",
        description: "Ask another cluster a question and keep the answer in the asking cluster's memory. The question goes into the target's memory, the answer comes from the target's current output or memory, and both appear in the report stream. No model call is started.",
        schema: r#"{"type":"object","properties":{"target":{"type":"string","description":"The id or the name of the target cluster."},"question":{"type":"string"}},"required":["target","question"]}"# },
    Tool { name: "cluster_status",
        description: "The cluster tree, or one cluster with its parts, report, error and memory. A cluster is a group of parts with one job, not a subagent.",
        schema: r#"{"type":"object","properties":{"id":{"type":"string","description":"Empty shows the whole tree."}}}"# },
    Tool { name: "cluster_kill",
        description: "Stop a running cluster at once, including a model call that is still waiting. Its report and memory stay on disk.",
        schema: r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"# },
    Tool { name: "cluster_dump",
        description: "The compressed state of a stuck or killed cluster: parts, memory tail and the cause that a respawn must name.",
        schema: r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"# },
    Tool { name: "cluster_respawn",
        description: "Kill a cluster and start it again with its old task plus the cause from its dump. The dump must exist; at most two respawns per cluster.",
        schema: r#"{"type":"object","properties":{"id":{"type":"string"},"extra":{"type":"string","description":"An extra instruction added after the cause."}},"required":["id"]}"# },
    Tool { name: "cluster_profile",
        description: "Create or change a cluster profile under .penelope/clusters/profiles. Fields use the profile keys: workers, validator, max_models, max_parallel, report_every, stuck_after, budget_eur, worktree.",
        schema: r#"{"type":"object","properties":{"name":{"type":"string"},"fields":{"type":"object","description":"Profile keys and values; they are merged into an existing profile."}},"required":["name","fields"]}"# },
    Tool { name: "computer",
        description: "Hand a whole computer task to the tool role: a smaller model that drives ScreenCodec and the plugin MCP servers. Use it for a page, a form, a desktop window or a sequence of clicks, so those steps do not fill your own context.",
        schema: r#"{"type":"object","properties":{"task":{"type":"string","description":"What to do on the computer, with the address or the window and what counts as done."},"max_steps":{"type":"integer","description":"At most 20, default 8."}},"required":["task"]}"# },
    Tool { name: "screen",
        description: "Use ScreenCodec as the eyes and hands of the computer: open a page, read the current page as text, click or type by the element id from that text, press a key, scroll, zoom, read a picture or the real screen. Penelope moves her own cursor when a click carries x and y. No screenshot enters your context.",
        schema: r#"{"type":"object","properties":{"action":{"type":"string","enum":["open","snapshot","click","type","select","key","scroll","zoom","image","desktop","desktop_click","confirm","call","tools"]},"url":{"type":"string"},"task":{"type":"string","description":"The user's instruction word for word, for the click check of ScreenCodec."},"level":{"type":"string","enum":["full","compact","minimal","lean"],"description":"How much detail a snapshot carries."},"id":{"type":"string","description":"The element id from the last snapshot, for click, type, select or desktop_click."},"text":{"type":"string"},"key":{"type":"string","description":"A key name such as Tab, Escape or PageDown."},"amount":{"type":"integer","description":"Pixels to scroll, negative goes up."},"option":{"type":"string"},"x0":{"type":"integer"},"y0":{"type":"integer"},"x1":{"type":"integer"},"y1":{"type":"integer"},"x":{"type":"integer","description":"Optional screen pixel for Penelope's cursor before a click."},"y":{"type":"integer"},"path":{"type":"string"},"question":{"type":"string","description":"What you want to know; it is shown next to the snapshot, the page itself stays text."},"code":{"type":"string","description":"The code of a ScreenCodec desktop capture or a waiting click."},"summary":{"type":"string"},"find":{"type":"string","description":"Only elements that match this text, for desktop."},"scope":{"type":"string","description":"window, monitor or all, for desktop."},"tool":{"type":"string","description":"With action call: the ScreenCodec tool name."},"arguments":{"type":"object","description":"With action call: the arguments of that tool."}},"required":["action"]}"# },
    Tool { name: "cursor",
        description: "Move Penelope's own cursor on the screen, or click with it. It uses the command set in cursor.command, else xdotool or ydotool when one is installed.",
        schema: r#"{"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"integer"},"click":{"type":"boolean"},"label":{"type":"string","description":"A short label shown next to the cursor when the command supports one."}},"required":["x","y"]}"# },
    Tool { name: "hand_off",
        description: "Give a task to another mode of Penelope (chat, work, agents or code). It opens a sub-session in that mode with your short context, runs it, stores the full result as an attachment and returns one summary line plus the attachment card. The mode agents runs the task through the cluster runner and asks the user first. At most two hand-offs deep. After outside text entered this session the user must say yes.",
        schema: r#"{"type":"object","properties":{"to_mode":{"type":"string","enum":["chat","work","agents","code"]},"task":{"type":"string","description":"What the other mode must do, with the done criterion."},"context":{"type":"string","description":"A short summary of what the other mode needs to know, at most 1500 characters. It does not see this conversation."}},"required":["to_mode","task"]}"# },
    Tool { name: "brief_check",
        description: "Open a brief: one task with its attachments. The client packs files, pictures, context and links; this tool checks size, SHA-256 and kind, reads only the project and plugin folders and links only from allowed hosts, then returns steps, findings and the next call. A later call may name the brief_id instead of sending the content again.",
        schema: r#"{"type":"object","properties":{"brief":{"type":"object","description":"The task and its attachments. Write the short form: task plus attach, one entry per attachment as kind:name with kind image, file, context or link.","properties":{"v":{"type":"integer"},"task":{"type":"string","maxLength":2000},"attach":{"type":"array","items":{"type":"string"},"maxItems":16},"items":{"type":"array","items":{"type":"object"},"maxItems":16}},"required":["task"],"additionalProperties":false},"brief_id":{"type":"string","description":"The id of a brief this server already opened, for example b1."}}}"# },
    Tool { name: "brief_put",
        description: "Send one chunk of a large attachment before the call that uses it. The client of a Laertes MCP server calls this on its own; a model normally does not.",
        schema: r#"{"type":"object","properties":{"id":{"type":"string"},"seq":{"type":"integer"},"data":{"type":"string"},"last":{"type":"boolean"}},"required":["id","seq","data","last"]}"# },
];

pub fn get(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

pub fn names() -> Vec<&'static str> {
    TOOLS.iter().map(|t| t.name).collect()
}

/// The schema of one tool as JSON.
pub fn schema(name: &str) -> Option<Value> {
    get(name).and_then(|t| serde_json::from_str(t.schema).ok())
}

/// The list a model sees. A forged tool is appended by `forge::specs`.
pub fn list() -> Vec<Value> {
    TOOLS.iter().map(|t| json!({"name": t.name, "description": t.description, "inputSchema": schema(t.name).unwrap_or_else(|| json!({"type": "object"}))})).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The contract of every tool: a unique name, a parseable schema, a required list that names declared fields, and
    /// no required field without a type. A broken contract stops the build, not a turn.
    #[test]
    fn every_tool_has_one_clean_contract() {
        let mut seen = HashSet::new();
        for t in TOOLS {
            assert!(t.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "{}", t.name);
            assert!(seen.insert(t.name), "{} occurs twice", t.name);
            assert!(t.description.len() > 30, "{}", t.name);
            let s: Value = serde_json::from_str(t.schema).unwrap_or_else(|e| panic!("{}: {}", t.name, e));
            assert_eq!(s["type"], "object", "{}", t.name);
            let props = s["properties"].as_object().unwrap_or_else(|| panic!("{} has no properties", t.name));
            for req in s["required"].as_array().cloned().unwrap_or_default() {
                let name = req.as_str().unwrap_or("");
                assert!(props.contains_key(name), "{} requires unknown field {}", t.name, name);
                assert!(props[name]["type"].is_string(), "{} requires {} without a type", t.name, name);
            }
            for (k, v) in props {
                assert!(v["type"].is_string() || v["enum"].is_array(), "{}.{} has no type", t.name, k);
                if let Some(e) = v["enum"].as_array() {
                    assert!(!e.is_empty(), "{}.{} has an empty enum", t.name, k);
                }
            }
        }
        assert_eq!(list().as_slice().len(), TOOLS.len());
    }

    /// The dispatch in `mcp::call` must know every name in this table. An empty call may fail on a missing argument,
    /// but it must never say that the tool is unknown.
    #[test]
    fn every_name_in_the_table_reaches_a_handler() {
        let root = std::env::temp_dir().join(format!("penelope-spec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ctx = crate::config::load(&root);
        for t in TOOLS {
            let said = crate::mcp::call_raw(&ctx, t.name, &json!({})).unwrap_or_default();
            assert!(!said.contains("unknown tool"), "{} has no handler: {}", t.name, said);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
