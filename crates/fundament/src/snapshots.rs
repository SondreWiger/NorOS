//! btrfs snapshots of the system, and rolling back to them.
//!
//! Snapshots are read-only copies of the `@` subvolume, kept in `@snapshots/<id>`
//! with a small `<id>.toml` beside each. Every snapshot also gets a boot menu entry
//! that starts it read-only (changes go to RAM), so you can look before you roll back.
//! Rolling back swaps `@` for a writable copy of the snapshot; the old system is
//! kept as a snapshot too, so a rollback can itself be undone.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{Error, Result, VERSION, bail, sys};

const TOP: &str = "/run/noros/btrfs-top";
const SNAPSHOT_DIR: &str = "/.snapshots";
const ESP: &str = "/efi";
/// Automatic snapshots (before package changes) beyond this many are removed, oldest first.
const KEEP_AUTO: usize = 8;
/// Don't take another automatic snapshot within this many seconds of the last one.
const AUTO_INTERVAL: u64 = 10 * 60;
pub const REBOOT_FLAG: &str = "/run/noros/reboot-required";
/// Where the replaced system waits for deletion after a rollback.
const OLD_ROOT: &str = "@replaced";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: u32,
    /// Seconds since the Unix epoch.
    pub created: u64,
    pub description: String,
    pub version: String,
    #[serde(default)]
    pub automatic: bool,
}

/// The snapshots on this system, newest first. Needs no special rights.
pub fn list() -> Vec<Snapshot> {
    list_in(Path::new(SNAPSHOT_DIR))
}

fn list_in(dir: &Path) -> Vec<Snapshot> {
    let mut snapshots: Vec<Snapshot> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| toml::from_str(&fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    snapshots.sort_by(|a, b| b.id.cmp(&a.id));
    snapshots
}

/// Unmounts the btrfs top level again when dropped.
struct TopLevel(PathBuf);

impl TopLevel {
    fn mount() -> Result<Self> {
        let device = sys::run("blkid", &["-L", "NOROS"]).map_err(|_| Error("NorOS isn't installed on a btrfs disk here".into()))?;
        let device = device.trim();
        if device.is_empty() {
            bail!("NorOS isn't installed on a btrfs disk here");
        }
        fs::create_dir_all(TOP)?;
        // Already mounted by an earlier call that didn't clean up? Reuse it.
        let _ = sys::run("umount", &[TOP]);
        sys::run("mount", &["-o", "subvolid=5", device, TOP])?;
        Ok(Self(PathBuf::from(TOP)))
    }

    fn snapshots(&self) -> PathBuf {
        self.0.join("@snapshots")
    }
}

impl Drop for TopLevel {
    fn drop(&mut self) {
        let _ = sys::run("umount", &[TOP]);
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn next_id(dir: &Path) -> u32 {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.split('.').next()?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

fn write_meta(dir: &Path, snapshot: &Snapshot) -> Result<()> {
    let text = toml::to_string(snapshot).map_err(|e| Error(e.to_string()))?;
    fs::write(dir.join(format!("{}.toml", snapshot.id)), text)?;
    Ok(())
}

/// Snapshot the running system.
pub fn create(description: &str, automatic: bool) -> Result<Option<Snapshot>> {
    if crate::is_live() {
        bail!("this is the live session; nothing here is kept, so there is nothing to snapshot");
    }
    sys::require_root()?;
    if automatic {
        let recent = list().into_iter().filter(|s| s.automatic).any(|s| now().saturating_sub(s.created) < AUTO_INTERVAL);
        if recent {
            return Ok(None);
        }
    }
    create_at(Path::new("/"), Path::new(ESP), description, automatic).map(Some)
}

/// Snapshot the system whose root is mounted at `root` (used by the installer too).
pub fn create_at(_root: &Path, esp: &Path, description: &str, automatic: bool) -> Result<Snapshot> {
    let top = TopLevel::mount()?;
    let dir = top.snapshots();
    let snapshot = Snapshot {
        id: next_id(&dir),
        created: now(),
        description: description.to_string(),
        version: VERSION.to_string(),
        automatic,
    };
    let source = top.0.join("@");
    let target = dir.join(snapshot.id.to_string());
    sys::run("btrfs", &["subvolume", "snapshot", "-r", source.to_str().unwrap(), target.to_str().unwrap()])?;
    write_meta(&dir, &snapshot)?;
    write_boot_entry(esp, &snapshot)?;
    if automatic {
        prune(&top, esp)?;
    }
    sys::log(&format!("noros-update: snapshot #{} created ({description})", snapshot.id));
    Ok(snapshot)
}

/// A boot entry that starts the snapshot read-only, with changes kept in RAM.
fn write_boot_entry(esp: &Path, snapshot: &Snapshot) -> Result<()> {
    let current = fs::read_to_string(esp.join("loader/entries/noros-current.conf"))
        .map_err(|_| Error("no NorOS boot entry found on the EFI partition".into()))?;
    let mut entry = format!("title   NorOS snapshot #{} — {}\nsort-key 1-snapshot-{:06}\n", snapshot.id, snapshot.description, u32::MAX - snapshot.id);
    for line in current.lines() {
        if line.starts_with("linux") || line.starts_with("initrd") {
            entry.push_str(line);
            entry.push('\n');
        } else if let Some(options) = line.strip_prefix("options") {
            let options = options
                .trim()
                .replace("subvol=@,", &format!("subvol=@snapshots/{},", snapshot.id))
                .replace(" rw ", " ro ");
            entry.push_str(&format!("options {options} systemd.volatile=overlay\n"));
        }
    }
    fs::write(esp.join(format!("loader/entries/noros-snapshot-{}.conf", snapshot.id)), entry)?;
    Ok(())
}

fn prune(top: &TopLevel, esp: &Path) -> Result<()> {
    let dir = top.snapshots();
    let autos: Vec<Snapshot> = list_in(&dir).into_iter().filter(|s| s.automatic).collect();
    for old in autos.into_iter().skip(KEEP_AUTO) {
        remove(top, esp, old.id)?;
    }
    Ok(())
}

fn remove(top: &TopLevel, esp: &Path, id: u32) -> Result<()> {
    let dir = top.snapshots();
    let path = dir.join(id.to_string());
    if path.exists() {
        sys::run("btrfs", &["subvolume", "delete", path.to_str().unwrap()])?;
    }
    let _ = fs::remove_file(dir.join(format!("{id}.toml")));
    let _ = fs::remove_file(esp.join(format!("loader/entries/noros-snapshot-{id}.conf")));
    Ok(())
}

/// Delete one snapshot.
pub fn delete(id: u32) -> Result<()> {
    sys::require_root()?;
    let top = TopLevel::mount()?;
    if !top.snapshots().join(id.to_string()).exists() {
        bail!("there is no snapshot #{id}");
    }
    remove(&top, Path::new(ESP), id)
}

/// Make snapshot `id` the system from the next boot on. The current system is kept
/// as a new snapshot, so this can be undone the same way.
pub fn rollback(id: u32) -> Result<Snapshot> {
    if crate::is_live() {
        bail!("this is the live session; there is nothing to roll back");
    }
    sys::require_root()?;
    let top = TopLevel::mount()?;
    let dir = top.snapshots();
    let chosen = dir.join(id.to_string());
    if !chosen.exists() {
        bail!("there is no snapshot #{id}");
    }
    let target = list_in(&dir).into_iter().find(|s| s.id == id);

    // Keep what we're leaving behind, as an ordinary snapshot.
    let kept = Snapshot {
        id: next_id(&dir),
        created: now(),
        description: format!("Before rolling back to #{id}"),
        version: VERSION.to_string(),
        automatic: false,
    };
    let current = top.0.join("@");
    let kept_path = dir.join(kept.id.to_string());
    sys::run("btrfs", &["subvolume", "snapshot", "-r", current.to_str().unwrap(), kept_path.to_str().unwrap()])?;
    write_meta(&dir, &kept)?;
    write_boot_entry(Path::new(ESP), &kept)?;

    // Move the running system aside (it stays mounted until reboot) and put a
    // writable copy of the chosen snapshot in its place.
    let old = top.0.join(OLD_ROOT);
    if old.exists() {
        bail!("a previous rollback hasn't finished; restart first");
    }
    fs::rename(&current, &old)?;
    if let Err(err) = sys::run("btrfs", &["subvolume", "snapshot", chosen.to_str().unwrap(), current.to_str().unwrap()]) {
        let _ = fs::rename(&old, &current);
        return Err(err);
    }

    fs::create_dir_all("/run/noros")?;
    fs::write(REBOOT_FLAG, format!("{id}\n"))?;
    sys::log(&format!("noros-update: rolled back to snapshot #{id}; restart to finish"));
    Ok(target.unwrap_or(kept))
}

/// At boot: remove the system a rollback replaced (no longer mounted by then).
pub fn cleanup() -> Result<()> {
    if crate::is_live() {
        return Ok(());
    }
    sys::require_root()?;
    let Ok(top) = TopLevel::mount() else { return Ok(()) };
    let old = top.0.join(OLD_ROOT);
    if old.exists() {
        sys::run("btrfs", &["subvolume", "delete", old.to_str().unwrap()])?;
        sys::log("noros-update: removed the system replaced by a rollback");
    }
    Ok(())
}

pub fn reboot_required() -> bool {
    Path::new(REBOOT_FLAG).exists()
}
