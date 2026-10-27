//! Names the build for the settings page: AINKI_BUILD when the deploy sets
//! it, else the git commit when built from a checkout.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

fn main() {
    println!("cargo:rerun-if-env-changed=AINKI_BUILD");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    let build = std::env::var("AINKI_BUILD")
        .ok()
        .filter(|b| !b.trim().is_empty())
        .or_else(|| {
            let commit = git(&["log", "-1", "--format=%h, %cd", "--date=format:%Y-%m-%d %H:%M"])?;
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some();
            Some(if dirty { format!("{commit} (with local changes)") } else { commit })
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=AINKI_BUILD={build}");
}
