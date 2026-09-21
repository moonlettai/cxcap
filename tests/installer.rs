//! Installer matrix: default, --no-skill, idempotence, user-content safety.
//!
//! Runs install.sh with an isolated HOME and prefix (never touches the
//! real home). Uses the already-built debug binary as the install source.

use std::path::PathBuf;
use std::process::Command;

mod common;

struct Env {
    home: PathBuf,
    prefix: PathBuf,
}

fn setup(tag: &str) -> Env {
    let home = std::env::temp_dir().join(format!("cxcap-inst-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let prefix = home.join("prefix");
    Env { home, prefix }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn pkg_root() -> PathBuf {
    let mut p = std::env::current_exe().expect("test exe");
    p.pop();
    if p.file_name().map(|n| n == "deps").unwrap_or(false) {
        p.pop();
    }
    p.pop();
    p.pop();
    p
}

fn install(env: &Env, args: &[&str]) -> (i32, String) {
    let binary = common::bin();
    let mut cmd = Command::new("sh");
    cmd.arg(pkg_root().join("install.sh"))
        .arg(format!("--prefix={}", env.prefix.display()))
        .arg(format!("--binary={}", binary.display()))
        .args(args)
        .env("HOME", &env.home)
        // Keep cargo/rustup shims out of the way; plain tool PATH suffices.
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    let out = cmd.output().expect("run install.sh");
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

fn skill_dir(env: &Env) -> PathBuf {
    env.home.join(".agents").join("skills").join("cxcap-development")
}

#[test]
fn default_installs_binary_skill_and_symlink() {
    let env = setup("default");
    let (code, log) = install(&env, &[]);
    assert_eq!(code, 0, "{log}");
    assert!(env.prefix.join("bin").join("cxcap").is_file());
    // Installed binary runs.
    let out = Command::new(env.prefix.join("bin").join("cxcap"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    // Skill + marker.
    let sk = std::fs::read_to_string(skill_dir(&env).join("SKILL.md")).expect("SKILL.md");
    assert!(sk.contains("cxcap audit . --intent"), "{sk}");
    assert!(skill_dir(&env).join(".cxcap-managed").is_file());
    // Claude symlink points at the canonical skill dir.
    let link = env.home.join(".claude").join("skills").join("cxcap-development");
    assert_eq!(std::fs::read_link(&link).unwrap(), skill_dir(&env));
}

#[test]
fn no_skill_stays_binary_only() {
    let env = setup("noskill");
    let (code, log) = install(&env, &["--no-skill"]);
    assert_eq!(code, 0, "{log}");
    assert!(env.prefix.join("bin").join("cxcap").is_file());
    assert!(!skill_dir(&env).exists());
    assert!(!env.home.join(".claude").exists());
}

#[test]
fn reinstall_is_idempotent() {
    let env = setup("idempotent");
    let (c1, l1) = install(&env, &[]);
    assert_eq!(c1, 0, "{l1}");
    let (c2, l2) = install(&env, &[]);
    assert_eq!(c2, 0, "{l2}");
    assert!(skill_dir(&env).join(".cxcap-managed").is_file());
    let link = env.home.join(".claude").join("skills").join("cxcap-development");
    assert_eq!(std::fs::read_link(&link).unwrap(), skill_dir(&env));
}

#[test]
fn user_owned_skill_and_foreign_symlink_untouched() {
    let env = setup("safe");
    // Pre-existing skill content without our marker: user-owned.
    std::fs::create_dir_all(skill_dir(&env)).unwrap();
    std::fs::write(skill_dir(&env).join("SKILL.md"), "# mine\n").unwrap();
    // Foreign symlink target.
    let link = env.home.join(".claude").join("skills").join("cxcap-development");
    std::fs::create_dir_all(env.home.join(".claude").join("skills")).unwrap();
    std::os::unix::fs::symlink("/tmp", &link).unwrap();
    let (code, log) = install(&env, &[]);
    assert_eq!(code, 0, "{log}");
    assert_eq!(std::fs::read_to_string(skill_dir(&env).join("SKILL.md")).unwrap(), "# mine\n");
    assert!(!skill_dir(&env).join(".cxcap-managed").exists());
    assert_eq!(std::fs::read_link(&link).unwrap(), PathBuf::from("/tmp"));
}

// --- download-mode tests (loopback only, never the real network) ---

fn fetch_triple() -> String {
    let arch = std::env::consts::ARCH;
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        other => panic!("unsupported test platform {other}"),
    };
    format!("{arch}-{os}")
}

fn sha256(path: &std::path::Path) -> String {
    let out = Command::new("python3")
        .args(["-c", "import hashlib,sys;print(hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())"])
        .arg(path)
        .output()
        .expect("python3 for sha256");
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn serve_dir(dir: &std::path::Path, port: u16) -> std::process::Child {
    Command::new("python3")
        .args(["-m", "http.server", &port.to_string(), "--bind", "127.0.0.1"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn http.server")
}

/// Loopback release fixture: tarball with a fake `cxcap` (answers
/// `--version` with `version`), the skill file, SHA256SUMS.txt and a
/// latest.json. `sums_mode`: "ok" | "corrupt" | "missing".
fn fetch_fixture(server_dir: &std::path::Path, version: &str, sums_mode: &str) {
    let stage = server_dir.join(format!("stage-{version}"));
    std::fs::create_dir_all(&stage).unwrap();
    let script = format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo {version}; else echo fake-cxcap; fi\n");
    std::fs::write(stage.join("cxcap"), &script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(stage.join("cxcap"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let d = stage.join("skill").join("cxcap-development");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("SKILL.md"), format!("# skill v{version}\n")).unwrap();
    let tb = format!("cxcap-{version}-{}.tar.gz", fetch_triple());
    let tar_status = Command::new("tar")
        .args(["-czf", &tb, "cxcap", "skill"])
        .current_dir(&stage)
        .status()
        .expect("tar");
    assert!(tar_status.success());
    // Mirror the release layout: assets live under v{version}/.
    let vdir = server_dir.join(format!("v{version}"));
    std::fs::create_dir_all(&vdir).unwrap();
    std::fs::rename(stage.join(&tb), vdir.join(&tb)).unwrap();
    let digest = match sums_mode {
        // Bare hex, exactly like the release SHA256SUMS.txt.
        "ok" => sha256(&vdir.join(&tb)),
        "corrupt" => "0".repeat(64),
        "missing" => String::new(),
        other => panic!("unknown sums_mode {other}"),
    };
    let sums = if digest.is_empty() {
        "deadbeef  unrelated-file.tar.gz\n".to_string()
    } else {
        format!("{digest}  {tb}\n")
    };
    std::fs::write(vdir.join("SHA256SUMS.txt"), sums).unwrap();
    std::fs::write(
        server_dir.join("latest.json"),
        format!("{{\"tag_name\": \"v{version}\"}}"),
    )
    .unwrap();
}

struct FetchCtx {
    env: Env,
    server: std::process::Child,
    base: String,
    tag: String,
}

fn fetch_setup(tag: &str, port: u16, version: &str, sums_mode: &str) -> FetchCtx {
    let env = setup(tag);
    let server_dir = env.home.join("server");
    std::fs::create_dir_all(&server_dir).unwrap();
    fetch_fixture(&server_dir, version, sums_mode);
    let server = serve_dir(&server_dir, port);
    // Readiness: the sums file answers over loopback.
    let base = format!("http://127.0.0.1:{port}");
    for _ in 0..40 {
        let probe = Command::new("curl")
            .args(["-fsSL", &format!("{base}/v{version}/SHA256SUMS.txt")])
            .output();
        if probe.is_ok_and(|o| o.status.success()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    FetchCtx {
        env,
        server,
        base,
        tag: format!("http://127.0.0.1:{port}/latest.json"),
    }
}

impl Drop for FetchCtx {
    fn drop(&mut self) {
        let _ = self.server.kill();
    }
}

fn install_fetch(ctx: &FetchCtx, args: &[&str], extra_env: &[(&str, String)]) -> (i32, String) {
    // Run a bare copy of install.sh: the package checkout contains
    // target/release/cxcap, which would (correctly) win over fetching.
    // A standalone copy proves the download path a curl user takes.
    let script_dir = ctx.env.home.join("iscript");
    std::fs::create_dir_all(&script_dir).unwrap();
    std::fs::copy(pkg_root().join("install.sh"), script_dir.join("install.sh")).unwrap();
    let mut cmd = Command::new("sh");
    cmd.arg(script_dir.join("install.sh"))
        .arg(format!("--prefix={}", ctx.env.prefix.display()))
        .args(args)
        .env("HOME", &ctx.env.home)
        .env("SHELL", "/bin/sh")
        .env("CXCAP_RELEASE_BASE", &ctx.base)
        .env("CXCAP_LATEST_URL", &ctx.tag)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run install.sh");
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

fn installed_version(env: &Env) -> String {
    let out = Command::new(env.prefix.join("bin").join("cxcap"))
        .arg("--version")
        .output()
        .expect("run installed --version");
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn fetch_pinned_installs_verified_binary_skill_and_path() {
    let ctx = fetch_setup("fetch-ok", 18941, "9.9.9", "ok");
    let (code, log) = install_fetch(
        &ctx,
        &[],
        &[("CXCAP_VERSION", "9.9.9".to_string())],
    );
    assert_eq!(code, 0, "{log}");
    assert_eq!(installed_version(&ctx.env), "9.9.9");
    assert!(skill_dir(&ctx.env).join(".cxcap-managed").is_file());
    assert_eq!(
        std::fs::read_to_string(skill_dir(&ctx.env).join("SKILL.md")).unwrap(),
        "# skill v9.9.9\n"
    );
    // PATH block landed in ~/.profile (SHELL=/bin/sh) exactly once,
    // and a second run stays idempotent.
    let rc = ctx.env.home.join(".profile");
    assert!(std::fs::read_to_string(&rc).unwrap().contains("cxcap installer PATH"), "{log}");
    let (c2, l2) = install_fetch(
        &ctx,
        &[],
        &[("CXCAP_VERSION", "9.9.9".to_string())],
    );
    assert_eq!(c2, 0, "{l2}");
    let body = std::fs::read_to_string(&rc).unwrap();
    assert_eq!(body.matches("cxcap installer PATH").count(), 2, "{body}");
}

#[test]
fn fetch_latest_lookup_resolves_tag() {
    // No CXCAP_VERSION: the installer parses tag_name from latest.json.
    let ctx = fetch_setup("fetch-latest", 18942, "9.9.9", "ok");
    let (code, log) = install_fetch(&ctx, &[], &[]);
    assert_eq!(code, 0, "{log}");
    assert_eq!(installed_version(&ctx.env), "9.9.9");
}

#[test]
fn fetch_checksum_mismatch_aborts_without_installing() {
    let ctx = fetch_setup("fetch-corrupt", 18943, "9.9.9", "corrupt");
    let (code, log) = install_fetch(
        &ctx,
        &[],
        &[("CXCAP_VERSION", "9.9.9".to_string())],
    );
    assert_ne!(code, 0, "{log}");
    assert!(log.contains("checksum"), "{log}");
    assert!(!ctx.env.prefix.join("bin").join("cxcap").exists());
}

#[test]
fn fetch_missing_checksum_entry_aborts() {
    let ctx = fetch_setup("fetch-missing", 18944, "9.9.9", "missing");
    let (code, log) = install_fetch(
        &ctx,
        &[],
        &[("CXCAP_VERSION", "9.9.9".to_string())],
    );
    assert_ne!(code, 0, "{log}");
    assert!(log.contains("no checksum entry"), "{log}");
    assert!(!ctx.env.prefix.join("bin").join("cxcap").exists());
}

#[test]
fn fetch_no_path_leaves_rc_alone() {
    let ctx = fetch_setup("fetch-nopath", 18945, "9.9.9", "ok");
    let (code, log) = install_fetch(
        &ctx,
        &["--no-path"],
        &[("CXCAP_VERSION", "9.9.9".to_string())],
    );
    assert_eq!(code, 0, "{log}");
    assert_eq!(installed_version(&ctx.env), "9.9.9");
    assert!(!ctx.env.home.join(".profile").exists());
}

#[test]
fn adjacent_tarball_binary_wins_offline() {
    // Extracted-tarball layout: ./cxcap next to install.sh wins without
    // any network (unreachable release base proves no fetch happens).
    let env = setup("adjacent");
    let stage = env.home.join("stage");
    let sk = stage.join("skill").join("cxcap-development");
    std::fs::create_dir_all(&sk).unwrap();
    std::fs::copy(pkg_root().join("install.sh"), stage.join("install.sh")).unwrap();
    let script = "#!/bin/sh\necho 9.9.9\n";
    std::fs::write(stage.join("cxcap"), script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(stage.join("cxcap"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(sk.join("SKILL.md"), "# skill\n").unwrap();
    let out = Command::new("sh")
        .arg(stage.join("install.sh"))
        .arg(format!("--prefix={}", env.prefix.display()))
        .arg("--no-path")
        .env("HOME", &env.home)
        .env("SHELL", "/bin/sh")
        .env("CXCAP_RELEASE_BASE", "http://127.0.0.1:9")
        .env("CXCAP_LATEST_URL", "http://127.0.0.1:9/latest.json")
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .output()
        .expect("run staged install.sh");
    assert_eq!(out.status.code().unwrap_or(-1), 0, "{:?}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(installed_version(&env), "9.9.9");
}
