//! Installing NorOS onto a disk.

use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{Error, Result, VERSION, bail, disks, snapshots, sys};

const TARGET: &str = "/run/noros-install/target";
const SOURCE: &str = "/run/noros-install/source";
const MAPPER_NAME: &str = "noros-root";
const LIVE_USER: &str = "noros";

pub struct Options {
    pub disk: String,
    pub full_name: String,
    pub username: String,
    pub password: String,
    /// `None` installs without disk encryption.
    pub encryption_password: Option<String>,
    pub hostname: String,
}

impl Options {
    pub fn validate(&self) -> Result<()> {
        let user_ok = !self.username.is_empty()
            && self.username.len() <= 32
            && self.username.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
            && self.username.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
        if !user_ok {
            bail!("usernames use lowercase letters, digits, - and _, and start with a letter");
        }
        if self.username == "root" || self.username == LIVE_USER {
            bail!("“{}” is reserved; pick another username", self.username);
        }
        if self.password.is_empty() {
            bail!("the account needs a password");
        }
        if let Some(key) = &self.encryption_password {
            if key.chars().count() < 8 {
                bail!("the disk encryption password must be at least 8 characters");
            }
        }
        let host_ok = !self.hostname.is_empty()
            && self.hostname.len() <= 63
            && self.hostname.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !self.hostname.starts_with('-');
        if !host_ok {
            bail!("computer names use letters, digits and -");
        }
        Ok(())
    }
}

/// Reports progress as (percent, what's happening).
pub type Progress<'a> = &'a mut dyn FnMut(u8, &str);

pub fn run(opts: &Options, progress: Progress) -> Result<()> {
    opts.validate()?;
    sys::require_root()?;
    let usable = disks::list()?
        .into_iter()
        .find(|d| d.path == opts.disk)
        .ok_or_else(|| Error(format!("{} is not a disk", opts.disk)))?;
    if let Some(reason) = usable.unusable {
        bail!("can't install on {}: {reason}", opts.disk);
    }

    sys::log(&format!("noros-install: installing NorOS {VERSION} on {}", opts.disk));
    let result = install(opts, progress);
    cleanup();
    match &result {
        Ok(()) => sys::log("noros-install: installation complete"),
        Err(err) => sys::log(&format!("noros-install: failed: {err}")),
    }
    result
}

fn install(opts: &Options, progress: Progress) -> Result<()> {
    let disk = opts.disk.as_str();
    let esp = disks::partition_path(disk, 1);
    let root_part = disks::partition_path(disk, 2);

    // 1. Partitions.
    progress(2, "Preparing the disk");
    let _ = sys::run("swapoff", &["-a"]);
    sys::run("wipefs", &["--all", "--force", disk])?;
    let root_type = match std::env::consts::ARCH {
        "x86_64" => "4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709",
        "aarch64" => "B921B045-1DF0-41C3-AF44-4C6F280D3FAE",
        _ => "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
    };
    let layout = format!(
        "label: gpt\nsize=1GiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=\"NOROS_EFI\"\ntype={root_type}, name=\"noros-root\"\n"
    );
    sys::run_with_input("sfdisk", &["--wipe", "always", "--wipe-partitions", "always", disk], Some(&layout))?;
    let _ = sys::run("partprobe", &[disk]);
    let _ = sys::run("udevadm", &["settle"]);
    sys::run("mkfs.vfat", &["-F", "32", "-n", "NOROS_EFI", &esp])?;

    // 2. Encryption.
    let fs_device = match &opts.encryption_password {
        Some(key) => {
            progress(5, "Encrypting the disk");
            sys::run_with_input(
                "cryptsetup",
                &["luksFormat", "--type", "luks2", "--batch-mode", "--label", "NOROS_CRYPT", "--pbkdf", "argon2id", "--key-file=-", &root_part],
                Some(key),
            )?;
            sys::run_with_input("cryptsetup", &["open", "--key-file=-", &root_part, MAPPER_NAME], Some(key))?;
            format!("/dev/mapper/{MAPPER_NAME}")
        }
        None => root_part.clone(),
    };

    // 3. Filesystem and subvolumes.
    progress(10, "Creating the file system");
    sys::run("mkfs.btrfs", &["--force", "--label", "NOROS", &fs_device])?;
    fs::create_dir_all(TARGET)?;
    sys::run("mount", &[&fs_device, TARGET])?;
    for subvolume in ["@", "@home", "@snapshots"] {
        sys::run("btrfs", &["subvolume", "create", &format!("{TARGET}/{subvolume}")])?;
    }
    sys::run("umount", &[TARGET])?;
    let options = |subvol: &str| format!("subvol={subvol},compress=zstd:1,noatime");
    sys::run("mount", &["-o", &options("@"), &fs_device, TARGET])?;
    for (subvol, dir) in [("@home", "home"), ("@snapshots", ".snapshots")] {
        let path = format!("{TARGET}/{dir}");
        fs::create_dir_all(&path)?;
        sys::run("mount", &["-o", &options(subvol), &fs_device, &path])?;
    }
    fs::create_dir_all(format!("{TARGET}/efi"))?;
    sys::run("mount", &[&esp, &format!("{TARGET}/efi")])?;

    // 4. Copy the system from the live medium (untouched by anything done in this session).
    progress(14, "Copying NorOS");
    let live = sys::run("blkid", &["-L", "NOROS_LIVE"])?;
    fs::create_dir_all(SOURCE)?;
    sys::run("mount", &["-o", "ro", live.trim(), SOURCE])?;
    copy_system(&mut |pct| progress(14 + (pct as u32 * 70 / 100) as u8, "Copying NorOS"))?;

    // 5. Configure the new system.
    progress(86, "Setting up your account");
    configure(opts, &fs_device, &esp, &root_part)?;

    // 6. Booting.
    progress(93, "Installing the boot loader");
    install_bootloader(opts, &fs_device, &root_part)?;

    // 7. A first snapshot to always be able to return to.
    progress(97, "Taking a first snapshot");
    snapshots::create_at(Path::new(TARGET), Path::new(&format!("{TARGET}/efi")), "As installed", false)?;

    progress(100, "Done");
    Ok(())
}

fn copy_system(progress: &mut dyn FnMut(u8)) -> Result<()> {
    let mut child = Command::new("rsync")
        .args(["-aHx", "--numeric-ids", "--info=progress2", "--no-inc-recursive", "--exclude=/live", "--exclude=/boot.catalog"])
        .arg(format!("{SOURCE}/"))
        .arg(format!("{TARGET}/"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error(format!("could not run rsync: {e}")))?;

    // rsync rewrites one status line with \r; pick the "NN%" field out of each update.
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut chunk = Vec::new();
    let mut last = 0u8;
    loop {
        chunk.clear();
        let mut byte = [0u8; 1];
        while stdout.read(&mut byte)? == 1 {
            if byte[0] == b'\r' || byte[0] == b'\n' {
                break;
            }
            chunk.push(byte[0]);
        }
        if chunk.is_empty() && stdout.fill_buf()?.is_empty() {
            break;
        }
        let line = String::from_utf8_lossy(&chunk);
        if let Some(pct) = line.split_whitespace().find_map(|t| t.strip_suffix('%')?.parse::<u8>().ok()) {
            if pct != last {
                last = pct;
                progress(pct);
            }
        }
    }
    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr)?;
    if !child.wait()?.success() {
        bail!("copying the system failed: {}", stderr.trim());
    }
    Ok(())
}

fn configure(opts: &Options, fs_device: &str, esp: &str, root_part: &str) -> Result<()> {
    let root = PathBuf::from(TARGET);
    let fs_uuid = sys::uuid_of(fs_device)?;
    let esp_uuid = sys::uuid_of(esp)?;

    let mut fstab = String::from("# NorOS file systems. / and /home are btrfs subvolumes.\n");
    for (subvol, dir) in [("@", "/"), ("@home", "/home"), ("@snapshots", "/.snapshots")] {
        fstab.push_str(&format!("UUID={fs_uuid} {dir} btrfs subvol={subvol},compress=zstd:1,noatime 0 0\n"));
    }
    fstab.push_str(&format!("UUID={esp_uuid} /efi vfat umask=0077 0 2\n"));
    fs::write(root.join("etc/fstab"), fstab)?;

    if opts.encryption_password.is_some() {
        let luks_uuid = sys::uuid_of(root_part)?;
        fs::write(root.join("etc/crypttab"), format!("{MAPPER_NAME} UUID={luks_uuid} none luks,discard\n"))?;
    }

    fs::write(root.join("etc/hostname"), format!("{}\n", opts.hostname))?;
    fs::write(root.join("etc/hosts"), format!("127.0.0.1 localhost\n127.0.1.1 {}\n::1 localhost\n", opts.hostname))?;
    // A fresh machine ID is generated on first boot.
    fs::write(root.join("etc/machine-id"), "")?;

    // Live-session leftovers.
    let _ = fs::remove_file(root.join("etc/sudoers.d/noros-live"));

    // Bind the kernel file systems so tools inside the new system work.
    for dir in ["dev", "proc", "sys", "run"] {
        sys::run("mount", &["--rbind", &format!("/{dir}"), root.join(dir).to_str().unwrap()])?;
    }
    let result = (|| {
        let _ = sys::chroot(&root, "userdel", &["--remove", LIVE_USER], None);
        sys::chroot(&root, "useradd", &["--create-home", "--shell", "/bin/bash", "--comment", &opts.full_name, &opts.username], None)?;
        for group in ["sudo", "video", "render", "input"] {
            let _ = sys::chroot(&root, "usermod", &["-aG", group, &opts.username], None);
        }
        sys::chroot(&root, "chpasswd", &[], Some(&format!("{}:{}\n", opts.username, opts.password)))?;
        sys::chroot(&root, "passwd", &["--lock", "root"], None)?;
        Ok::<(), Error>(())
    })();
    let _ = sys::run("umount", &["-R", &format!("{TARGET}/dev")]);
    let _ = sys::run("umount", &["-R", &format!("{TARGET}/proc")]);
    let _ = sys::run("umount", &["-R", &format!("{TARGET}/sys")]);
    let _ = sys::run("umount", &["-R", &format!("{TARGET}/run")]);
    result?;

    // The desktop starts for the new user. The disk password already proved who's here.
    let dropin = root.join("etc/systemd/system/noros-session.service.d");
    fs::create_dir_all(&dropin)?;
    fs::write(dropin.join("user.conf"), format!("[Service]\nUser={}\n", opts.username))?;

    fs::create_dir_all(root.join("etc/noros"))?;
    fs::write(
        root.join("etc/noros/installed.toml"),
        format!("version = \"{VERSION}\"\nencrypted = {}\n", opts.encryption_password.is_some()),
    )?;
    Ok(())
}

/// Kernel command line that finds and mounts the root subvolume `subvol`.
pub fn root_options(fs_device: &str, root_part: &str, subvol: &str, encrypted: bool) -> Result<String> {
    let mut options = Vec::new();
    if encrypted {
        options.push(format!("rd.luks.name={}={MAPPER_NAME}", sys::uuid_of(root_part)?));
        options.push(format!("root=/dev/mapper/{MAPPER_NAME}"));
    } else {
        options.push(format!("root=UUID={}", sys::uuid_of(fs_device)?));
    }
    options.push("rootfstype=btrfs".into());
    options.push(format!("rootflags=subvol={subvol},compress=zstd:1"));
    Ok(options.join(" "))
}

pub const QUIET: &str = "quiet loglevel=3 systemd.show_status=auto rd.udev.log_level=3";

fn install_bootloader(opts: &Options, fs_device: &str, root_part: &str) -> Result<()> {
    let esp = PathBuf::from(format!("{TARGET}/efi"));
    let (stub, fallback) = match std::env::consts::ARCH {
        "x86_64" => ("systemd-bootx64.efi", "BOOTX64.EFI"),
        _ => ("systemd-bootaa64.efi", "BOOTAA64.EFI"),
    };
    let boot = PathBuf::from(TARGET).join("usr/lib/systemd/boot/efi").join(stub);
    fs::create_dir_all(esp.join("EFI/systemd"))?;
    fs::create_dir_all(esp.join("EFI/BOOT"))?;
    fs::copy(&boot, esp.join("EFI/systemd").join(stub))?;
    // The fallback path works on every UEFI machine, even without boot variables.
    fs::copy(&boot, esp.join("EFI/BOOT").join(fallback))?;

    let kernel_dir = esp.join("noros").join(VERSION);
    fs::create_dir_all(&kernel_dir)?;
    fs::copy(format!("{SOURCE}/live/vmlinuz"), kernel_dir.join("vmlinuz"))?;
    fs::copy(format!("{SOURCE}/live/initrd"), kernel_dir.join("initrd"))?;

    fs::create_dir_all(esp.join("loader/entries"))?;
    fs::write(esp.join("loader/loader.conf"), "timeout 3\ndefault noros-current.conf\neditor no\nconsole-mode keep\n")?;
    let options = root_options(fs_device, root_part, "@", opts.encryption_password.is_some())?;
    fs::write(
        esp.join("loader/entries/noros-current.conf"),
        format!(
            "title   NorOS {VERSION}\nsort-key 0-noros\nlinux   /noros/{VERSION}/vmlinuz\ninitrd  /noros/{VERSION}/initrd\noptions {options} rw {QUIET}\n"
        ),
    )?;

    // Register with the firmware too, where that's possible (it's fine if not).
    let _ = sys::run("bootctl", &["--esp-path", esp.to_str().unwrap(), "--no-variables", "is-installed"]);
    Ok(())
}

fn cleanup() {
    let _ = sys::run("sync", &[]);
    let _ = sys::run("umount", &["-R", TARGET]);
    let _ = sys::run("umount", &[SOURCE]);
    if Path::new(&format!("/dev/mapper/{MAPPER_NAME}")).exists() {
        let _ = sys::run("cryptsetup", &["close", MAPPER_NAME]);
    }
}
