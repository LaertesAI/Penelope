# Auto-update blueprint

The updater is built; the server is not. `penelope update --check` reads a manifest and `penelope update` installs a
newer build after a checksum and, when present, a minisign signature. Until a server exists, `update.url` is empty and
both commands say so. This file is the contract for the server and the release pipeline.

## The manifest

One JSON file per channel:

```json
{
  "version": "0.12.0",
  "channel": "stable",
  "platform": "linux-x86_64",
  "url": "https://updates.laertes.ai/penelope/0.12.0/penelope-linux-x86_64",
  "sha256": "0f1e2d...64 hex characters",
  "size": 6233016,
  "min_version": "0.9.0",
  "notes": "https://laertes.ai/penelope/changelog#0.12.0",
  "signature": "untrusted comment: ...\nRWQ...\n"
}
```

- `version` is SemVer with three numbers. A suffix such as `-beta.1` sorts below the release with the same numbers.
- `platform` is `<os>-<arch>` from Rust: `linux-x86_64`, `linux-aarch64`, `windows-x86_64`, `macos-aarch64`.
- `url` points at one file, not an archive, for the desktop. The same version may have one artifact per platform.
- `sha256` is mandatory. The client refuses an update without it.
- `signature` is optional in the file format but should be mandatory on the server. The client requires
  `update.public_key` and the `minisign` command when a signature is present.
- `notes` is a page a person can open; the client does not parse it.

Address: `update.url` is either the full manifest (`.../stable.json`) or a base (`https://updates.laertes.ai/penelope`);
the client adds `/<channel>.json`.

## The server

A static host is enough; no application server is needed for the happy path:

```
https://updates.laertes.ai/penelope/
  stable.json
  beta.json
  0.12.0/penelope-linux-x86_64
  0.12.0/penelope-windows-x86_64.exe
  0.12.0/penelope-macos-aarch64
```

- Build on a clean machine: `cargo build --release --locked`.
- Rename the artifact by platform.
- Compute `sha256sum` and sign the manifest with minisign. Keep the private key off the web host.
- Upload the artifact first, then the manifest. A manifest must never point at a missing file.
- A CDN with `Cache-Control: max-age=60` on the manifest and `immutable` on versioned artifacts is enough.
- A simple `curl` check on every channel after upload catches a broken URL or a wrong checksum.

## The client

`penelope update --check` prints installed, newest, channel and platform; it writes nothing.

`penelope update`:

1. downloads the manifest and refuses a platform that does not match;
2. refuses a version that is not newer;
3. downloads the artifact to the folder of the running binary;
4. checks SHA-256; a mismatch deletes nothing and installs nothing;
5. checks minisign when a signature is present;
6. on Unix, moves the old binary to `penelope.old` and renames the new one into place; a failed rename puts the old one
   back;
7. on Windows, writes `penelope.new.exe` and asks for a restart, because a running `.exe` cannot be replaced.

The old binary stays as `penelope.old` for one rollback. No automatic restart: the person starts Penelope again.

## Channels

| Channel | Who gets it | Rule |
|---|---|---|
| `stable` | everyone | only builds that passed the test suite and the live smoke test |
| `beta` | volunteers | one week before stable, with the devlog entry |
| `nightly` | the developer | every main build; not signed for the public |

`update.channel` in `config.json` picks one; the default is `stable`.

## Signing

```bash
minisign -G -p penelope.pub -s penelope.key          # once, offline
minisign -Sm stable.json -s penelope.key              # every release
# the client stores the contents of penelope.pub in update.public_key
```

The public key is the trust root. It lives in the settings and in the release notes. A key rotation ships both keys for
one release; the client tries the new one first and the old one second. Until that is built, rotate by hand.

## Rollback

- Manual: `penelope.old` is the previous binary; move it back and restart.
- Automatic after a failed rename: the client restores the old file itself.
- A broken release is pulled by replacing the manifest with the previous version and a note. The client never
  downgrades by itself; the release page says `penelope update --to VERSION` once that flag exists.

## Windows and macOS

- Windows: `cmd /C`, `taskkill`, `clip`, PowerShell pointer code are in `platform.rs`; no Windows machine was available,
  so this is untested. The updater writes `penelope.new.exe` and never touches a running program.
- macOS: the same Unix path as Linux. The binary is not signed or notarised yet; Gatekeeper would block it until a
  Developer ID certificate exists.

## What is not built

- No server, no CDN, no signing key.
- No delta update; each release downloads the whole binary.
- No `--to VERSION` downgrade.
- No progress bar; the download is one request.
- No update check at start; only the command and `/update`.

[`DEVLOG.md`](DEVLOG.md) holds the measured build; `cli/src/update.rs` holds the client.
