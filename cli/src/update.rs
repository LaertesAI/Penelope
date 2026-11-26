//! The updater. The server does not exist yet, so this module is a working client for the contract in
//! `docs/AUTOUPDATE.md` and a clear refusal when no address is set.
//!
//! The manifest is one JSON object per channel. The artifact is downloaded to the folder of the running binary, its
//! SHA-256 is checked, an optional minisign signature is checked, and the file is renamed over the old one. On Unix a
//! running binary can be replaced by a rename; Windows gets `penelope.new.exe` and needs a restart.

use crate::config::Ctx;
use serde_json::Value;
use std::fs;
use std::io::Read;

/// A three-part version as numbers. A suffix such as `-beta.1` is lower than the release with the same numbers.
pub fn version(text: &str) -> (u64, u64, u64, bool) {
    let clean = text.trim().trim_start_matches('v');
    let (core, pre) = clean.split_once('-').map(|(a, b)| (a, Some(b))).unwrap_or((clean, None));
    let mut parts = core.split('.').map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0), pre.is_none())
}

pub fn is_newer(remote: &str, local: &str) -> bool {
    version(remote) > version(local)
}

fn sha256_hex(bytes: &[u8]) -> String {
    // A small SHA-256, so the updater has no dependency. The code follows FIPS 180-4.
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut data = bytes.to_vec();
    let bit_len = (data.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in data.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) = (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(t1); d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b); h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f); h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|n| format!("{:08x}", n)).collect()
}

fn manifest_url(ctx: &Ctx) -> String {
    let base = ctx.cfg["update"]["url"].as_str().unwrap_or("").trim().to_string();
    if base.is_empty() {
        return String::new();
    }
    let channel = ctx.cfg["update"]["channel"].as_str().unwrap_or("stable");
    if base.ends_with(".json") {
        base
    } else {
        format!("{}/{}.json", base.trim_end_matches('/'), channel)
    }
}

/// Checks the manifest and says what would happen. No file is written.
pub fn check(ctx: &Ctx) -> Result<String, String> {
    let url = manifest_url(ctx);
    if url.is_empty() {
        return Ok("No update server is set. The blueprint is in docs/AUTOUPDATE.md.\n\
Set update.url in config.json, for example https://updates.laertes.ai/penelope, then run penelope update --check.".to_string());
    }
    let body = ureq::get(&url).timeout(std::time::Duration::from_secs(20)).call()
        .map_err(|e| format!("the update manifest could not be read from {}: {}", url, e))?
        .into_string().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("the manifest is no JSON: {}", e))?;
    let remote = v["version"].as_str().ok_or("the manifest has no version")?;
    let local = env!("CARGO_PKG_VERSION");
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let fits = v["platform"].as_str().map(|p| p == platform).unwrap_or(true);
    Ok(format!("installed {} · newest {} · channel {} · platform {} {}\n{}\n{}", local, remote,
               v["channel"].as_str().unwrap_or("stable"), platform, if fits { "(fits)" } else { "(no artifact for this platform)" },
               if is_newer(remote, local) { "an update is available: penelope update" } else { "this is the newest version" },
               if url.is_empty() { String::new() } else { format!("manifest: {}", url) }))
}

/// Downloads and installs the manifest at `update.url`. A checksum is mandatory; a minisign signature is checked when
/// the manifest carries one. No server exists yet, so this path has only run against a local test server.
pub fn install(ctx: &Ctx) -> Result<String, String> {
    let url = manifest_url(ctx);
    if url.is_empty() {
        return Err("No update server is set. The blueprint is in docs/AUTOUPDATE.md.".to_string());
    }
    let body = ureq::get(&url).timeout(std::time::Duration::from_secs(20)).call()
        .map_err(|e| format!("the manifest could not be read: {}", e))?.into_string().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("the manifest is no JSON: {}", e))?;
    let remote = v["version"].as_str().ok_or("the manifest has no version")?;
    let local = env!("CARGO_PKG_VERSION");
    if !is_newer(remote, local) {
        return Ok(format!("no update: {} is not newer than {}", remote, local));
    }
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    if let Some(p) = v["platform"].as_str() {
        if p != platform {
            return Err(format!("the manifest is for {}, this PC is {}", p, platform));
        }
    }
    let artifact = v["url"].as_str().ok_or("the manifest has no url")?;
    let want = v["sha256"].as_str().ok_or("the manifest has no sha256. An update without a checksum is refused.")?.to_lowercase();
    let mut bytes = Vec::new();
    ureq::get(artifact).timeout(std::time::Duration::from_secs(120)).call()
        .map_err(|e| format!("the artifact could not be read: {}", e))?.into_reader().read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let got = sha256_hex(&bytes);
    if got != want {
        return Err(format!("the checksum does not match. Expected {}, got {}. Nothing was installed.", want, got));
    }
    if let Some(sig) = v["signature"].as_str().filter(|s| !s.is_empty()) {
        if crate::config::which("minisign").is_none() {
            return Err("the manifest is signed, but minisign is not installed. Nothing was installed.".to_string());
        }
        let dir = std::env::temp_dir().join(format!("penelope-update-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let (file, sigfile) = (dir.join("penelope.new"), dir.join("penelope.new.minisig"));
        fs::write(&file, &bytes).map_err(|e| e.to_string())?;
        fs::write(&sigfile, sig).map_err(|e| e.to_string())?;
        let key = ctx.cfg["update"]["public_key"].as_str().unwrap_or("");
        if key.is_empty() {
            return Err("the manifest is signed, but update.public_key is empty. Nothing was installed.".to_string());
        }
        let out = std::process::Command::new("minisign").args(["-Vm", &file.to_string_lossy(), "-x", &sigfile.to_string_lossy(), "-P", key]).output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("the signature is not valid: {}. Nothing was installed.", String::from_utf8_lossy(&out.stderr).trim()));
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = if cfg!(windows) { exe.with_file_name("penelope.new.exe") } else { exe.with_file_name("penelope.new") };
    fs::write(&new, &bytes).map_err(|e| format!("cannot write {}: {}", new.display(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&new, fs::Permissions::from_mode(0o755));
        let old = exe.with_file_name("penelope.old");
        let _ = fs::remove_file(&old);
        let _ = fs::rename(&exe, &old);
        if let Err(e) = fs::rename(&new, &exe) {
            let _ = fs::rename(&old, &exe);
            return Err(format!("the new binary could not replace the old one: {}. The old one is back.", e));
        }
        Ok(format!("installed {} over {}. The last binary is {}. Start penelope again.", remote, local, old.display()))
    }
    #[cfg(windows)]
    {
        Ok(format!("{} is downloaded. Windows cannot replace a running program: close Penelope, rename {} to penelope.exe, and start it again.", new.display(), new.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_the_checksum_are_exact() {
        assert!(is_newer("0.12.0", "0.11.9") && is_newer("1.0.0", "0.99.99") && !is_newer("0.11.0", "0.11.0"));
        assert!(!is_newer("0.12.0-beta.1", "0.12.0"));
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(version("v1.2.3"), (1, 2, 3, true));
        assert_eq!(version("0.12.0-beta.1"), (0, 12, 0, false));
    }
}
