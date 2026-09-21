//! Updater release matrix: check/notice, success, checksum-rollback.
//!
//! All network is loopback-only (`python3 -m http.server` serving a temp
//! dir). The "installed" binary under test is a *copy* of the real binary
//! in a temp dir, so self-replacement can never touch the build tree.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

struct Ctx {
    root: PathBuf,
    server: Child,
    base_url: String,
}

fn target_triple() -> String {
    let arch = std::env::consts::ARCH;
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        other => panic!("unsupported test platform {other}"),
    };
    format!("{arch}-{os}")
}

fn sha256(path: &Path) -> String {
    let out = Command::new("python3")
        .args(["-c", "import hashlib,sys;print(hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())"])
        .arg(path)
        .output()
        .expect("python3 for sha256");
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn serve_dir(dir: &Path, port: u16) -> Child {
    Command::new("python3")
        .args(["-m", "http.server", &port.to_string(), "--bind", "127.0.0.1"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn http.server")
}

/// Build a fake release: tarball with `cxcap` (a script answering
/// `--version` with `version`) plus an optional skill file, and a
/// manifest.json offering it. Returns (manifest_url, tarball_path).
fn fake_release(server_dir: &Path, version: &str, with_skill: bool, corrupt_digest: bool) -> String {
    let stage = server_dir.join(format!("stage-{version}"));
    std::fs::create_dir_all(&stage).unwrap();
    let script = format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo {version}; else echo fake-cxcap; fi\n");
    std::fs::write(stage.join("cxcap"), &script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(stage.join("cxcap"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    if with_skill {
        let d = stage.join("skill").join("cxcap-development");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), format!("# skill v{version}\n")).unwrap();
    }
    let tarball = format!("cxcap-{version}-{}.tar.gz", target_triple());
    let mut args = vec!["-czf".to_string(), tarball.clone(), "cxcap".to_string()];
    if with_skill {
        args.push("skill".to_string());
    }
    let tar_status = Command::new("tar")
        .args(&args)
        .current_dir(&stage)
        .status()
        .expect("tar");
    assert!(tar_status.success());
    let tarball_path = server_dir.join(&tarball);
    std::fs::rename(stage.join(&tarball), &tarball_path).unwrap();
    let mut digest = format!("sha256:{}", sha256(&tarball_path));
    if corrupt_digest {
        digest = format!("sha256:{}", "0".repeat(64));
    }
    let tb_name = format!("cxcap-{version}-{}.tar.gz", target_triple());
    let manifest_text = format!(
        "{{\"schema\": 1, \"releases\": [{{\"version\": \"{version}\", \"assets\": [{{\"name\": \"{tb}\", \"url\": \"{tb}\", \"digest\": \"{digest}\"}}]}}]}}",
        tb = tb_name
    );
    std::fs::write(server_dir.join("manifest.json"), manifest_text).unwrap();
    tarball_path.to_string_lossy().into_owned()
}

fn setup(port: u16, version: &str, with_skill: bool, corrupt: bool) -> Ctx {
    let root = std::env::temp_dir().join(format!("cxcap-upd-test-{}-{port}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("server")).unwrap();
    std::fs::create_dir_all(root.join("install")).unwrap();
    std::fs::create_dir_all(root.join("home")).unwrap();
    std::fs::create_dir_all(root.join("cache")).unwrap();
    fake_release(&root.join("server"), version, with_skill, corrupt);
    let bin = common::bin();
    std::fs::copy(&bin, root.join("install").join("cxcap")).unwrap();
    let server = serve_dir(&root.join("server"), port);
    // Wait for readiness via the real check path.
    let base_url = format!("http://127.0.0.1:{port}/manifest.json");
    for _ in 0..40 {
        let out = Command::new(root.join("install").join("cxcap"))
            .args(["update", "--check"])
            .env("CXCAP_UPDATE_MANIFEST", &base_url)
            .env("HOME", root.join("home"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .output()
            .expect("run update --check");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if text.contains(version) || text.contains("is current") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Ctx { root, server, base_url }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn installed(ctx: &Ctx) -> Command {
    let mut c = Command::new(ctx.root.join("install").join("cxcap"));
    c.env("CXCAP_UPDATE_MANIFEST", &ctx.base_url)
        .env("HOME", ctx.root.join("home"))
        .env("XDG_CACHE_HOME", ctx.root.join("cache"));
    c
}

fn version_of(bin: &Path) -> String {
    let out = Command::new(bin).arg("--version").output().expect("run --version");
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

mod common;

#[test]
fn check_reports_newer_without_installing() {
    let ctx = setup(18931, "9.9.9", false, false);
    let out = installed(&ctx).args(["update", "--check"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("9.9.9") && text.contains("cxcap update"), "{text}");
    // Nothing installed: still the build version.
    assert_eq!(version_of(&ctx.root.join("install").join("cxcap")), env!("CARGO_PKG_VERSION"));
}

#[test]
fn update_success_replaces_and_verifies() {
    let ctx = setup(18932, "9.9.9", false, false);
    let out = installed(&ctx).arg("update").output().unwrap();
    assert!(out.status.success(), "{:?}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(version_of(&ctx.root.join("install").join("cxcap")), "9.9.9");
}

/// One-shot staged binary: answers `--version` with `version` exactly once
/// (smoke passes), then fails every later invocation, forcing the
/// post-replace verification to restore the backup.
fn fake_oneshot_release(server_dir: &Path, version: &str, flag: &Path) {
    let stage = server_dir.join(format!("stage-{version}-oneshot"));
    std::fs::create_dir_all(&stage).unwrap();
    let script = format!(
        "#!/bin/sh\nFLAG=\"{}\"\nif [ \"$1\" = \"--version\" ]; then if [ -f \"$FLAG\" ]; then exit 1; else touch \"$FLAG\"; echo {version}; fi; else echo fake-cxcap; fi\n",
        flag.display()
    );
    std::fs::write(stage.join("cxcap"), &script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(stage.join("cxcap"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let tb = format!("cxcap-{version}-{}.tar.gz", target_triple());
    let tar_status = Command::new("tar")
        .args(["-czf", &tb, "cxcap"])
        .current_dir(&stage)
        .status()
        .expect("tar");
    assert!(tar_status.success());
    let tarball_path = server_dir.join(&tb);
    let _ = std::fs::remove_file(&tarball_path);
    std::fs::rename(stage.join(&tb), &tarball_path).unwrap();
    let digest = format!("sha256:{}", sha256(&tarball_path));
    let manifest_text = format!(
        "{{\"schema\": 1, \"releases\": [{{\"version\": \"{version}\", \"assets\": [{{\"name\": \"{tb}\", \"url\": \"{tb}\", \"digest\": \"{digest}\"}}]}}]}}",
    );
    std::fs::write(server_dir.join("manifest.json"), manifest_text).unwrap();
}

#[test]
fn update_rollback_restores_working_binary() {
    // Normal harness first (dirs, server, installed copy), then swap the
    // served release for the one-shot variant. The loopback server serves
    // fresh reads, so no restart is needed; --check downloads no asset, so
    // the one-shot is still armed when `update` runs.
    let ctx = setup(18938, "9.9.9", false, false);
    let flag = ctx.root.join("oneshot-seen");
    fake_oneshot_release(&ctx.root.join("server"), "9.9.9", &flag);
    let before = version_of(&ctx.root.join("install").join("cxcap"));
    let out = installed(&ctx).arg("update").output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("previous binary restored"),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(version_of(&ctx.root.join("install").join("cxcap")), before);
}

#[test]
fn update_checksum_mismatch_preserves_working_binary() {
    let ctx = setup(18933, "9.9.9", false, true);
    let before = version_of(&ctx.root.join("install").join("cxcap"));
    let out = installed(&ctx).arg("update").output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("checksum"),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(version_of(&ctx.root.join("install").join("cxcap")), before);
}

#[test]
fn update_refreshes_skill_only_for_opted_in_installs() {
    let ctx = setup(18934, "9.9.9", true, false);
    // Opted-in install: marker + stale skill.
    let skill_dir = ctx.root.join("home").join(".agents").join("skills").join("cxcap-development");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# stale\n").unwrap();
    std::fs::write(skill_dir.join(".cxcap-managed"), "1").unwrap();
    // Unrelated user content that must survive.
    std::fs::write(skill_dir.join("notes.txt"), "mine\n").unwrap();
    let out = installed(&ctx).arg("update").output().unwrap();
    assert!(out.status.success(), "{:?}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(), "# skill v9.9.9\n");
    assert_eq!(std::fs::read_to_string(skill_dir.join("notes.txt")).unwrap(), "mine\n");
}

#[test]
fn update_leaves_skill_alone_without_opt_in() {
    let ctx = setup(18935, "9.9.9", true, false);
    // Skill dir exists but WITHOUT the managed marker (user-owned).
    let skill_dir = ctx.root.join("home").join(".agents").join("skills").join("cxcap-development");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# user version\n").unwrap();
    let out = installed(&ctx).arg("update").output().unwrap();
    assert!(out.status.success(), "{:?}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
        "# user version\n"
    );
}

#[test]
fn daily_check_notice_and_offline_silence() {
    // Fresh cache + offering manifest: audit prints a stderr notice, JSON stays pure.
    let ctx = setup(18936, "9.9.9", false, false);
    let proj = ctx.root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("a.py"), "def a():\n    return 1\n").unwrap();
    let out = installed(&ctx).args(["audit", &proj.to_string_lossy(), "--json"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&stdout).is_ok());
    assert!(stderr.contains("9.9.9"), "{stderr:?}");
    assert!(!stdout.contains("9.9.9"));
    // Offline (discard port) with no cached knowledge: silent, analysis unaffected.
    let out2 = installed(&ctx)
        .env("CXCAP_UPDATE_MANIFEST", "http://127.0.0.1:9/manifest.json")
        .env("HOME", ctx.root.join("home-offline"))
        .env("XDG_CACHE_HOME", ctx.root.join("cache-offline"))
        .args(["audit", &proj.to_string_lossy()])
        .output()
        .unwrap();
    assert!(out2.status.success());
    assert!(!String::from_utf8(out2.stderr).unwrap().contains("available"));
    // Disabled: silent even with a live newer release and no cache.
    let out3 = installed(&ctx)
        .env("CXCAP_NO_UPDATE_CHECK", "1")
        .env("HOME", ctx.root.join("home-disabled"))
        .env("XDG_CACHE_HOME", ctx.root.join("cache-disabled"))
        .args(["audit", &proj.to_string_lossy()])
        .output()
        .unwrap();
    assert!(out3.status.success());
    assert!(!String::from_utf8(out3.stderr).unwrap().contains("available"));
}

#[test]
fn read_only_target_tree_untouched() {
    let ctx = setup(18937, "9.9.9", false, false);
    let proj = ctx.root.join("ro");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("a.py"), "def a():\n    return 1\n").unwrap();
    let mut before = Vec::new();
    for e in walk(&proj) {
        before.push((e.clone(), std::fs::read(&e).unwrap()));
    }
    let _ = installed(&ctx).args(["audit", &proj.to_string_lossy()]).output().unwrap();
    for (p, content) in before {
        assert_eq!(&std::fs::read(&p).unwrap(), &content, "{p:?} modified");
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}
