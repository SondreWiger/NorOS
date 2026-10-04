//! Small helpers for running system tools.

use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use crate::{Error, Result};

/// Run a command; on failure the error includes what it printed.
pub fn run(program: &str, args: &[&str]) -> Result<String> {
    run_with_input(program, args, None)
}

/// Run a command, feeding `input` on stdin (used for passwords, never on the command line).
pub fn run_with_input(program: &str, args: &[&str], input: Option<&str>) -> Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error(format!("could not run {program}: {e}")))?;
    if let Some(input) = input {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error(format!("{program} {} failed: {}", args.join(" "), stderr.trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a command inside the installed system.
pub fn chroot(root: &Path, program: &str, args: &[&str], input: Option<&str>) -> Result<String> {
    let root = root.to_str().unwrap();
    let mut full = vec![root, program];
    full.extend_from_slice(args);
    run_with_input("chroot", &full, input)
}

/// Write a line to the system journal (and the kernel log as a fallback).
pub fn log(message: &str) {
    let logged = Command::new("logger").args(["-t", "noros", message]).status().map(|s| s.success()).unwrap_or(false);
    if !logged {
        if let Ok(mut kmsg) = std::fs::OpenOptions::new().write(true).open("/dev/kmsg") {
            let _ = writeln!(kmsg, "{message}");
        }
    }
}

pub fn require_root() -> Result<()> {
    let uid = run("id", &["-u"]).unwrap_or_default();
    if uid.trim() != "0" {
        return Err(Error("this needs administrator rights; run it with sudo".into()));
    }
    Ok(())
}

/// The block device's UUID, e.g. for fstab.
pub fn uuid_of(device: &str) -> Result<String> {
    let uuid = run("blkid", &["-s", "UUID", "-o", "value", device])?;
    let uuid = uuid.trim();
    if uuid.is_empty() {
        return Err(Error(format!("{device} has no UUID")));
    }
    Ok(uuid.to_string())
}
