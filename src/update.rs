//! Release update channel (not analysis).
//!
//! Daily, non-blocking release check plus explicit `cxcap update`, built on
//! established primitives (`self_update` download/extract/replace, `sha2`
//! digest verification). Analysis itself stays offline-capable: any check
//! failure is silent, notices go to stderr (never `--json` stdout), and
//! nothing is ever written to an analyzed repository.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Env override for the release manifest URL (tests, mirrors, air-gapped
/// staging). Production default is set at release time (see below).
pub const MANIFEST_ENV: &str = "CXCAP_UPDATE_MANIFEST";
/// Set to `1`/`yes`/`true` to disable the daily release check.
pub const NO_CHECK_ENV: &str = "CXCAP_NO_UPDATE_CHECK";

/// Release-time constant: the owner points this at the public
/// `manifest.json` before 1.0.0 (e.g. a `manifest.json` release asset).
/// `None` until then: the daily check silently skips and `cxcap update`
/// reports that no update source is configured.
pub const DEFAULT_MANIFEST_URL: Option<&str> = None;

/// Notice join budget: the analysis never waits longer than this for a due
/// daily check (once per day at most); cached notices cost no network.
const CHECK_JOIN_BUDGET: Duration = Duration::from_millis(1500);
const CHECK_TIMEOUT: Duration = Duration::from_secs(8);
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(20);
const ASSET_TIMEOUT: Duration = Duration::from_secs(300);
const MANIFEST_MAX_BYTES: u64 = 1_000_000;
const ASSET_MAX_BYTES: u64 = 100_000_000;
const DAY_SECS: u64 = 24 * 60 * 60;

/// Skill install locations (shared with the installer; single source here).
pub const SKILL_NAME: &str = "cxcap-development";
pub const SKILL_FILE: &str = "SKILL.md";
/// Marker proving this installation opted into skill management: updates
/// refresh the skill only when it exists; unrelated content is never touched.
pub const MANAGED_MARKER: &str = ".cxcap-managed";

/// Minimal manifest.json schema-1 subset (unknown fields ignored).
#[derive(Debug, Deserialize)]
struct Manifest {
    schema: u64,
    #[serde(default)]
    releases: Vec<ManifestRelease>,
}

#[derive(Debug, Deserialize)]
struct ManifestRelease {
    version: String,
    #[serde(default)]
    assets: Vec<ManifestAsset>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ManifestAsset {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CheckState {
    /// Unix seconds of the last completed check attempt.
    last_check: u64,
    /// Newest release version observed (informational notice source).
    latest: Option<String>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Update source URL, if configured (env override wins).
pub fn manifest_url() -> Option<String> {
    match std::env::var(MANIFEST_ENV).ok() {
        Some(v) if !v.trim().is_empty() => Some(v),
        _ => DEFAULT_MANIFEST_URL.map(|s| s.to_string()),
    }
}

pub fn check_disabled() -> bool {
    matches!(
        std::env::var(NO_CHECK_ENV).ok().as_deref().map(str::trim),
        Some("1") | Some("yes") | Some("true")
    )
}

fn state_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("cxcap").join("update-check.json"))
}

fn read_state() -> CheckState {
    let p = match state_path() {
        Some(p) => p,
        None => return CheckState::default(),
    };
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_state(st: &CheckState) {
    let Some(p) = state_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(s) = serde_json::to_string(st) {
        let _ = std::fs::write(p, s);
    }
}

fn download(url: &str, timeout: Duration, max_bytes: u64) -> Result<Vec<u8>, String> {
    let mut dl = self_update::Download::from_url(url);
    dl.timeout(timeout);
    dl.max_download_size(max_bytes);
    let mut buf: Vec<u8> = Vec::new();
    dl.download_to(&mut buf)
        .map_err(|e| format!("download failed: {e}"))?;
    Ok(buf)
}

fn parse_manifest(body: &[u8]) -> Result<Vec<(semver::Version, Vec<ManifestAsset>)>, String> {
    let m: Manifest =
        serde_json::from_slice(body).map_err(|e| format!("invalid manifest: {e}"))?;
    if m.schema != 1 {
        return Err(format!("unsupported manifest schema {}", m.schema));
    }
    let mut out = Vec::new();
    for r in m.releases {
        let Ok(v) = semver::Version::parse(&r.version) else {
            continue; // non-semver entries (e.g. nightly) are skipped
        };
        out.push((v, r.assets));
    }
    Ok(out)
}

/// Newest release newer than `current` that carries a usable asset for
/// this platform. Returns (version, asset download URL, hex digest).
/// A missing digest is a hard refusal (fail-closed checksum rule).
pub fn pick_update(
    current: &semver::Version,
    releases: &[(semver::Version, Vec<ManifestAsset>)],
    manifest_url: &str,
    target: &str,
) -> Option<(semver::Version, String, String)> {
    let mut best: Option<(semver::Version, String, String)> = None;
    for (v, assets) in releases {
        if v <= current {
            continue;
        }
        let hit = assets
            .iter()
            .filter(|a| a.name.contains(target) && a.name.contains("cxcap"))
            .filter_map(|a| {
                let digest = Checksum::parse_ok(a.digest.as_deref()?)?;
                Some((a, digest))
            })
            .next();
        let Some((a, digest)) = hit else { continue };
        let url = resolve_asset_url(manifest_url, &a.url);
        match &best {
            Some((bv, _, _)) if bv >= v => {}
            _ => best = Some((v.clone(), url, digest)),
        }
    }
    best
}

struct Checksum;

impl Checksum {
    /// Parse `sha256:<hex>` / `sha512:<hex>`; `None` on missing/unsupported.
    fn parse_ok(digest: &str) -> Option<String> {
        let (algo, hex) = digest.trim().split_once(':')?;
        match algo.trim().to_ascii_lowercase().as_str() {
            "sha256" if hex.len() == 64 => Some(hex.to_string()),
            "sha512" if hex.len() == 128 => Some(hex.to_string()),
            _ => None,
        }
    }

    fn verify_sha256(bytes: &[u8], hex: &str) -> bool {
        use sha2::Digest;
        let sum = sha2::Sha256::digest(bytes);
        hex::encode(sum) == hex.to_lowercase()
    }
}

/// Stub: hex encoding without a new dependency (sha2 is already aboard).
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        const H: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(bytes.as_ref().len() * 2);
        for b in bytes.as_ref() {
            s.push(H[(b >> 4) as usize] as char);
            s.push(H[(b & 15) as usize] as char);
        }
        s
    }
}

fn resolve_asset_url(manifest_url: &str, asset_url: &str) -> String {
    if asset_url.starts_with("http://") || asset_url.starts_with("https://") {
        return asset_url.to_string();
    }
    match manifest_url.rfind('/') {
        Some(idx) => format!("{}{}", &manifest_url[..=idx], asset_url),
        None => asset_url.to_string(),
    }
}

fn current_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(semver::Version::new(0, 0, 0))
}

/// True when `latest` parses and is strictly newer than `current`.
/// Single source for the cached-notice and fresh-check decisions.
fn is_newer(current: &str, latest: &str) -> bool {
    semver::Version::parse(latest).is_ok_and(|l| {
        semver::Version::parse(current).is_ok_and(|c| l > c)
    })
}

/// Notice text when `latest` is newer than the running binary.
pub fn update_notice(current: &str, latest: &str) -> String {
    format!("cxcap: v{latest} available (running v{current}) — run `cxcap update` to upgrade.")
}

/// Fetch the newest *installable* release version: newer than the running
/// binary with a digest-carrying asset for this platform. Notices and
/// `--check` share this so they can never disagree.
fn fetch_installable(manifest_url: &str) -> Option<String> {
    let body = download(manifest_url, CHECK_TIMEOUT, MANIFEST_MAX_BYTES).ok()?;
    let releases = parse_manifest(&body).ok()?;
    let current = current_version();
    pick_update(&current, &releases, manifest_url, self_update::get_target())
        .map(|(v, _, _)| v.to_string())
}

/// Daily check entry point for `audit`: at most one network attempt per
/// day; offline/errors are silent; notices go to stderr only.
pub fn maybe_check() {
    if check_disabled() {
        return;
    }
    let Some(url) = manifest_url() else { return };
    let current = env!("CARGO_PKG_VERSION").to_string();
    let mut st = read_state();
    let now = now_secs();
    // Cached newer release: notice without network.
    if let Some(ref latest) = st.latest.clone() {
        if is_newer(&current, latest) {
            eprintln!("{}", update_notice(&current, latest));
        }
    }
    if now.saturating_sub(st.last_check) < DAY_SECS {
        return;
    }
    // One network attempt per day, bounded by the join budget; a slow
    // network never blocks the analysis.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let latest = fetch_installable(&url);
        let mut st = read_state();
        st.last_check = now_secs();
        if latest.is_some() {
            st.latest = latest.clone();
        }
        write_state(&st);
        let _ = tx.send(latest);
    });
    if let Ok(Some(latest)) = rx.recv_timeout(CHECK_JOIN_BUDGET) {
        if is_newer(&current, &latest) {
            eprintln!("{}", update_notice(&current, &latest));
        }
        st.last_check = now;
        st.latest = Some(latest);
        write_state(&st);
    }
}

/// Skill source shipped inside a release tarball (`skill/<name>/SKILL.md`).
fn staged_skill(staging: &Path) -> PathBuf {
    staging
        .join("skill")
        .join(SKILL_NAME)
        .join(SKILL_FILE)
}

/// Post-replace check: the installed binary must answer anew.
fn installed_ok(exe: &Path) -> bool {
    std::process::Command::new(exe)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Skill refresh for opted-in installs only; unrelated content untouched.
/// Returns the parenthetical for the success message (or empty).
fn refresh_skill(staging: &Path) -> &'static str {
    if !skill_opted_in() {
        return "";
    }
    if !staged_skill(staging).is_file() {
        return "";
    }
    let Some(dir) = skill_home() else { return "" };
    match std::fs::read(staged_skill(staging)) {
        Ok(bytes) => match std::fs::write(dir.join(SKILL_FILE), &bytes) {
            Ok(()) => " (skill refreshed)",
            Err(_) => " (skill refresh failed; binary updated)",
        },
        Err(_) => " (skill refresh failed; binary updated)",
    }
}

fn skill_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join(".agents").join("skills").join(SKILL_NAME))
}

fn skill_opted_in() -> bool {
    skill_home()
        .map(|d| d.join(MANAGED_MARKER).is_file())
        .unwrap_or(false)
}

/// Run a binary with `--version`; require exit 0 and a matching version.
fn smoke_test(bin: &Path, expect: &semver::Version) -> Result<(), String> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("smoke test failed to run staged binary: {e}"))?;
    if !out.status.success() {
        return Err("smoke test failed: staged binary exited nonzero".to_string());
    }
    let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if semver::Version::parse(&got).is_ok_and(|v| &v == expect) {
        Ok(())
    } else {
        Err(format!("smoke test failed: staged binary reports {got:?}"))
    }
}

/// Explicit update flow: fetch → checksum → stage → smoke-test → atomic
/// replace (with rollback) → skill refresh for opted-in installs.
/// Never touches analyzed repositories; only the install itself.
pub fn run_update() -> Result<String, String> {
    let url = manifest_url()
        .ok_or_else(|| format!("no update source configured (set {MANIFEST_ENV})"))?;
    let current = current_version();
    let body = download(&url, MANIFEST_TIMEOUT, MANIFEST_MAX_BYTES)?;
    let releases = parse_manifest(&body)?;
    let target = self_update::get_target();
    let Some((version, asset_url, digest)) = pick_update(&current, &releases, &url, target)
    else {
        let mut st = read_state();
        st.last_check = now_secs();
        write_state(&st);
        return Ok(format!("cxcap v{} is current", env!("CARGO_PKG_VERSION")));
    };
    if digest.len() == 128 {
        return Err("sha512 assets are not supported by this updater".to_string());
    }
    let bytes = download(&asset_url, ASSET_TIMEOUT, ASSET_MAX_BYTES)?;
    if !Checksum::verify_sha256(&bytes, &digest) {
        return Err("checksum mismatch: refusing to install".to_string());
    }
    let staging = std::env::temp_dir().join(format!("cxcap-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("staging failed: {e}"))?;
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&staging);
    };
    let asset_file = staging.join("asset.tar.gz");
    std::fs::write(&asset_file, &bytes).map_err(|e| format!("staging failed: {e}"))?;
    let staged = staging.join("staged");
    std::fs::create_dir_all(&staged).map_err(|e| format!("staging failed: {e}"))?;
    self_update::Extract::from_source(&asset_file)
        .archive(self_update::ArchiveKind::Tar(Some(
            self_update::Compression::Gz,
        )))
        .extract_into(&staged)
        .map_err(|e| {
            cleanup();
            format!("extract failed: {e}")
        })?;
    let staged_bin = staged.join("cxcap");
    if !staged_bin.is_file() {
        cleanup();
        return Err("release archive has no cxcap binary".to_string());
    }
    if let Err(e) = smoke_test(&staged_bin, &version) {
        cleanup();
        return Err(e);
    }
    let exe =
        std::env::current_exe().map_err(|e| format!("cannot locate install: {e}"))?;
    let backup = staging.join("cxcap-backup");
    if let Err(e) = self_update::Move::from_source(&staged_bin)
        .replace_using_temp(&backup)
        .to_dest(&exe)
    {
        cleanup();
        return Err(format!("replace failed, working binary preserved: {e}"));
    }
    // Post-replace verification: the installed binary must answer anew.
    // On failure, restore the backup so no broken install is left behind.
    if !installed_ok(&exe) {
        let _ = self_update::Move::from_source(&backup)
            .replace_using_temp(staging.join("rollback-tmp"))
            .to_dest(&exe);
        cleanup();
        return Err("installed binary failed verification; previous binary restored"
            .to_string());
    }
    let skill_msg = refresh_skill(&staged);
    let mut st = read_state();
    st.last_check = now_secs();
    st.latest = Some(version.to_string());
    write_state(&st);
    cleanup();
    Ok(format!(
        "cxcap updated to v{version}{skill_msg} — restarted CLI picks it up on next invocation"
    ))
}

/// `cxcap update --check`: report availability without installing.
pub fn check_only() -> Result<String, String> {
    let url = manifest_url()
        .ok_or_else(|| format!("no update source configured (set {MANIFEST_ENV})"))?;
    let current = current_version();
    let body = download(&url, MANIFEST_TIMEOUT, MANIFEST_MAX_BYTES)?;
    let releases = parse_manifest(&body)?;
    let target = self_update::get_target();
    match pick_update(&current, &releases, &url, target) {
        Some((v, _, _)) => Ok(update_notice(env!("CARGO_PKG_VERSION"), &v.to_string())),
        None => Ok(format!("cxcap v{} is current", env!("CARGO_PKG_VERSION"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rels() -> Vec<(semver::Version, Vec<ManifestAsset>)> {
        vec![
            (
                semver::Version::new(0, 11, 0),
                vec![ManifestAsset {
                    name: "cxcap-0.11.0-x86_64-unknown-linux-gnu.tar.gz".into(),
                    url: "cxcap-0.11.0-x86_64-unknown-linux-gnu.tar.gz".into(),
                    digest: Some(format!("sha256:{}", "a".repeat(64))),
                }],
            ),
            (
                semver::Version::new(0, 12, 1),
                vec![
                    ManifestAsset {
                        name: "cxcap-0.12.1-aarch64-apple-darwin.tar.gz".into(),
                        url: "https://cdn.example/cxcap-0.12.1-aarch64-apple-darwin.tar.gz".into(),
                        digest: Some(format!("sha256:{}", "b".repeat(64))),
                    },
                    ManifestAsset {
                        name: "cxcap-0.12.1-x86_64-unknown-linux-gnu.tar.gz".into(),
                        url: "cxcap-0.12.1-x86_64-unknown-linux-gnu.tar.gz".into(),
                        digest: None, // no digest: must be skipped, not installed
                    },
                ],
            ),
            (
                semver::Version::new(0, 13, 0),
                vec![ManifestAsset {
                    name: "cxcap-0.13.0-x86_64-unknown-linux-gnu.tar.gz".into(),
                    url: "cxcap-0.13.0-x86_64-unknown-linux-gnu.tar.gz".into(),
                    digest: Some("md5:deadbeef".into()), // unsupported: skipped
                }],
            ),
        ]
    }

    #[test]
    fn picks_newest_compatible_with_digest() {
        let base = "https://example.net/r/manifest.json";
        let cur = semver::Version::new(0, 12, 0);
        // macOS arm: absolute URL asset with digest wins.
        let got = pick_update(&cur, &rels(), base, "aarch64-apple-darwin").unwrap();
        assert_eq!(got.0, semver::Version::new(0, 12, 1));
        assert!(got.1.starts_with("https://cdn.example/"));
        // linux gnu: 0.12.1 asset lacks a digest (skipped), 0.13.0 digest
        // unsupported (skipped) → no update, fail-closed.
        assert!(pick_update(&cur, &rels(), base, "x86_64-unknown-linux-gnu").is_none());
    }

    #[test]
    fn current_or_newer_never_updates() {
        let base = "https://example.net/r/manifest.json";
        assert!(pick_update(&semver::Version::new(0, 12, 1), &rels(), base, "aarch64-apple-darwin")
            .is_none());
        assert!(pick_update(&semver::Version::new(9, 9, 9), &rels(), base, "aarch64-apple-darwin")
            .is_none());
    }

    #[test]
    fn manifest_rejects_bad_schema_and_parses_minimal() {
        assert!(parse_manifest(br#"{"schema": 2, "releases": []}"#).is_err());
        assert!(parse_manifest(br#"not json"#).is_err());
        let ok = parse_manifest(
            br#"{"schema": 1, "releases": [{"version": "1.0.0", "assets": []}, {"version": "nightly"}]}"#,
        )
        .unwrap();
        assert_eq!(ok.len(), 1);
        // relative asset URLs resolve against the manifest directory.
        let rels = vec![(
            semver::Version::new(1, 0, 0),
            vec![ManifestAsset {
                name: "cxcap-1.0.0-x86_64-unknown-linux-gnu.tar.gz".into(),
                url: "cxcap-1.0.0-x86_64-unknown-linux-gnu.tar.gz".into(),
                digest: Some(format!("sha256:{}", "c".repeat(64))),
            }],
        )];
        let got = pick_update(
            &semver::Version::new(0, 9, 0),
            &rels,
            "https://example.net/r/manifest.json",
            "x86_64-unknown-linux-gnu",
        )
        .unwrap();
        assert_eq!(
            got.1,
            "https://example.net/r/cxcap-1.0.0-x86_64-unknown-linux-gnu.tar.gz"
        );
    }

    #[test]
    fn notice_names_both_versions() {
        let n = update_notice("0.11.6", "0.12.0");
        assert!(n.contains("0.12.0") && n.contains("0.11.6") && n.contains("cxcap update"));
    }

    #[test]
    fn checksum_parses_supported_digests_only() {
        assert!(Checksum::parse_ok(&format!("sha256:{}", "a".repeat(64))).is_some());
        assert!(Checksum::parse_ok("md5:deadbeef").is_none());
        assert!(Checksum::parse_ok("sha256:xyz").is_none());
        assert!(Checksum::parse_ok("").is_none());
    }
}
