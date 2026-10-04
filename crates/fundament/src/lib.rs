//! Fundament — how NorOS gets onto a disk and stays healthy there.
//!
//! * [`disks`]: which disks NorOS could be installed on.
//! * [`install`]: partition, encrypt, copy the system, set up users and booting.
//! * [`snapshots`]: btrfs snapshots before every change, and rolling back to them.
//!
//! Disk layout of an installed system:
//!
//! ```text
//! GPT
//! ├─ NOROS_EFI   1 GiB FAT32   systemd-boot, kernel, initrd, boot entries
//! └─ noros-root  rest          LUKS2 (optional) → btrfs "NOROS"
//!                              ├─ @           the system, mounted at /
//!                              ├─ @home       your files
//!                              └─ @snapshots  read-only copies of @, at /.snapshots
//! ```

pub mod disks;
pub mod install;
pub mod snapshots;
pub mod sys;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Running from the live ISO (nothing is saved)?
pub fn is_live() -> bool {
    std::fs::read_to_string("/proc/cmdline")
        .map(|cmdline| cmdline.contains("NOROS_LIVE"))
        .unwrap_or(false)
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error(err.to_string())
    }
}

#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => { return Err($crate::Error(format!($($arg)*))) };
}
