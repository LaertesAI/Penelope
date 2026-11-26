# Penelope operations

## Daily search probe

The keyless engines rate-limit automated clients. One probe a day records which source is alive and writes
`.penelope/search/health.json`.

```bash
penelope search probe --daily
systemctl --user link "$PWD/deploy/penelope-search-probe.service"
systemctl --user link "$PWD/deploy/penelope-search-probe.timer"
systemctl --user daemon-reload
systemctl --user enable --now penelope-search-probe.timer
```

The unit uses `/home/rene/.local/bin/penelope` and `/home/rene/.penelope`; change both if the binary or the settings
folder lives somewhere else. A project-local `.penelope` needs its own `WorkingDirectory` and `--root`.

## SearXNG pool

```bash
penelope search instances add http://127.0.0.1:8080
penelope search instances
```

`search.url` is tried first, then every entry of `search.urls`. The first instance with a result wins.

## Dependency audit

```bash
cargo install cargo-audit --locked
cd cli && cargo audit
```

CI runs the same command. The website runs `pip-audit` in its own workflow.
