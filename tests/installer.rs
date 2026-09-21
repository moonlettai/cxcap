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
