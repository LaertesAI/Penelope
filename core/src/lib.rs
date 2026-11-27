//! The shared core of Penelope.
//!
//! The core holds every rule that the command line and the desktop must agree on: settings, sessions, provider
//! tables, model calls, the tool gate, clusters, memory, workflows, cost and the device paths. It draws nothing and
//! imports no user interface crate. The command line keeps `main`, `tui`, `input` and `update`; a desktop frontend
//! talks to the same public functions over its own layer.
//!
//! The public surface is the modules below. Everything a frontend needs is a plain Rust function, a Serde type or a
//! JSON value.

pub mod acp;
pub mod advise;
pub mod agents;
pub mod api;
pub mod apps;
pub mod attach;
pub mod audit;
pub mod auth;
pub mod autoroute;
pub mod backend;
pub mod brain;
pub mod brief;
pub mod clarify;
pub mod clusters;
pub mod checker;
pub mod config;
pub mod context;
pub mod cost;
pub mod crew;
pub mod cursor;
pub mod doctor;
pub mod eurykleia;
pub mod deepresearch;
pub mod eval;
pub mod factories;
pub mod files;
pub mod forge;
pub mod gates;
pub mod graph;
pub mod jobs;
pub mod llm;
pub mod loom;
pub mod lsp;
pub mod mcp;
pub mod mcpclient;
pub mod mcpmem;
pub mod mcpserv;
pub mod mcpvet;
pub mod memory;
pub mod mitos;
pub mod mode;
pub mod modes;
pub mod mods;
pub mod net;
pub mod notes;
pub mod platform;
pub mod pii;
pub mod plugins;
pub mod privacy_bench;
pub mod privacy;
pub mod proof;
pub mod providers;
pub mod ask;
pub mod research;
pub mod route;
pub mod scout;
pub mod scrape;
pub mod screen;
pub mod search;
pub mod server;
pub mod sessions;
pub mod setup;
pub mod skills;
pub mod spec;
pub mod startup;
pub mod theme;
pub mod tools;
pub mod toolscan;
pub mod toolsearch;
pub mod vet;
pub mod weak;
pub mod work;
