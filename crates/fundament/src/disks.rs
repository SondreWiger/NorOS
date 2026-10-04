//! Disks NorOS could be installed on.

use serde::{Deserialize, Serialize};

use crate::{Result, sys};

/// Smaller than this and NorOS won't fit with room to spare.
pub const MIN_SIZE: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct Disk {
    pub path: String,
    pub size: u64,
    pub model: String,
    pub transport: String,
    pub removable: bool,
    /// Why this disk can't be used, if it can't.
    pub unusable: Option<String>,
}

impl Disk {
    pub fn size_label(&self) -> String {
        human_size(self.size)
    }
}

pub fn human_size(bytes: u64) -> String {
    let gb = bytes as f64 / 1_000_000_000.0;
    if gb >= 1000.0 {
        format!("{:.1} TB", gb / 1000.0)
    } else {
        format!("{gb:.0} GB")
    }
}

#[derive(Deserialize)]
struct Lsblk {
    blockdevices: Vec<Device>,
}

#[derive(Deserialize)]
struct Device {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    size: Option<u64>,
    model: Option<String>,
    tran: Option<String>,
    rm: Option<bool>,
    ro: Option<bool>,
}

/// The whole disk the live system booted from (never offered for installing).
pub fn live_disk() -> Option<String> {
    let partition = sys::run("blkid", &["-L", "NOROS_LIVE"]).ok()?;
    let partition = partition.trim();
    if partition.is_empty() {
        return None;
    }
    let parent = sys::run("lsblk", &["-no", "PKNAME", partition]).ok()?;
    let parent = parent.lines().next().unwrap_or("").trim();
    Some(if parent.is_empty() { partition.to_string() } else { format!("/dev/{parent}") })
}

pub fn list() -> Result<Vec<Disk>> {
    let json = sys::run("lsblk", &["-J", "-b", "-d", "-o", "PATH,TYPE,SIZE,MODEL,TRAN,RM,RO"])?;
    let parsed: Lsblk = serde_json::from_str(&json).map_err(|e| crate::Error(format!("could not read disk list: {e}")))?;
    let live = live_disk();

    Ok(parsed
        .blockdevices
        .into_iter()
        .filter(|d| d.kind == "disk" && !d.path.contains("zram") && !d.path.starts_with("/dev/loop"))
        .map(|d| {
            let size = d.size.unwrap_or(0);
            let unusable = if Some(&d.path) == live.as_ref() {
                Some("NorOS is running from this disk".to_string())
            } else if d.ro.unwrap_or(false) {
                Some("read-only".to_string())
            } else if size < MIN_SIZE {
                Some(format!("too small (needs at least {})", human_size(MIN_SIZE)))
            } else {
                None
            };
            Disk {
                path: d.path,
                size,
                model: d.model.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).unwrap_or_else(|| "Disk".into()),
                transport: d.tran.unwrap_or_default(),
                removable: d.rm.unwrap_or(false),
                unusable,
            }
        })
        .collect())
}

/// Partition device names: /dev/sda → /dev/sda1, /dev/nvme0n1 → /dev/nvme0n1p1.
pub fn partition_path(disk: &str, number: u32) -> String {
    if disk.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        format!("{disk}p{number}")
    } else {
        format!("{disk}{number}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_names() {
        assert_eq!(partition_path("/dev/sda", 2), "/dev/sda2");
        assert_eq!(partition_path("/dev/vda", 1), "/dev/vda1");
        assert_eq!(partition_path("/dev/nvme0n1", 2), "/dev/nvme0n1p2");
        assert_eq!(partition_path("/dev/mmcblk0", 1), "/dev/mmcblk0p1");
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(256_060_514_304), "256 GB");
        assert_eq!(human_size(2_000_398_934_016), "2.0 TB");
    }
}
