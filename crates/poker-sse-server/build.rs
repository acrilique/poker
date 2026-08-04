// Build scripts are build-time tooling, not shipped server logic, so the
// crate's strict clippy set (pedantic/nursery/deny-on-panics) doesn't apply.
#![allow(clippy::all)]

//! Emits `POKER_CACHE_VERSION`, used to stamp the service worker's
//! `CACHE_VERSION` so each deploy invalidates old caches without a manual bump.
//!
//! Prefers the short git SHA of the working copy (meaningful, deterministic per
//! commit). Falls back to a build timestamp when git isn't available — notably
//! the Docker build, where the `poker` submodule's `.git` isn't in the build
//! context, so `git rev-parse` fails and the timestamp guarantees a fresh value
//! every build.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let version = git_short_sha().unwrap_or_else(|| format!("build-{}", unix_seconds()));
    println!("cargo:rustc-env=POKER_CACHE_VERSION={version}");
}

/// Short SHA of `HEAD`, or `None` if git is missing / not a repo / fails.
fn git_short_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?;
    let trimmed = sha.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Seconds since the Unix epoch, or `0` if the clock is before the epoch.
fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
