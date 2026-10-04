//! noros-install — install NorOS onto a disk. Used by the Install NorOS app, and
//! usable on its own from a terminal in the live session.
//!
//!   noros-install --list-disks
//!   noros-install --disk /dev/nvme0n1 --user ada --name "Ada Lovelace" [--hostname noros]
//!                 [--no-encryption] [--yes] [--progress-lines]
//!
//! Passwords are never taken as arguments: they're read from stdin, one per line
//! (account password, then the disk encryption password).

use std::io::{BufRead, IsTerminal, Write};

use fundament::{disks, install};

fn usage() -> ! {
    eprintln!(
        "usage:\n  noros-install --list-disks\n  noros-install --disk DISK --user NAME [--name \"Full Name\"] [--hostname HOST] [--no-encryption] [--yes] [--progress-lines]"
    );
    std::process::exit(2)
}

fn read_secret(prompt: &str, lines: &mut impl Iterator<Item = std::io::Result<String>>) -> String {
    let tty = std::io::stdin().is_terminal();
    if tty {
        eprint!("{prompt}: ");
        let _ = std::io::stderr().flush();
        let _ = std::process::Command::new("stty").arg("-echo").status();
    }
    let line = lines.next().and_then(|l| l.ok()).unwrap_or_default();
    if tty {
        let _ = std::process::Command::new("stty").arg("echo").status();
        eprintln!();
    }
    line.trim_end_matches(['\r', '\n']).to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list-disks") {
        match disks::list() {
            Ok(list) => println!("{}", serde_json::to_string_pretty(&list).unwrap()),
            Err(err) => {
                eprintln!("noros-install: {err}");
                std::process::exit(1);
            }
        }
        return;
    }

    let mut disk = None;
    let mut user = None;
    let mut name = None;
    let mut hostname = "noros".to_string();
    let mut encrypt = true;
    let mut yes = false;
    let mut machine = false;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--disk" => disk = it.next(),
            "--user" => user = it.next(),
            "--name" => name = it.next(),
            "--hostname" => hostname = it.next().unwrap_or_else(|| usage()),
            "--no-encryption" => encrypt = false,
            "--yes" => yes = true,
            "--progress-lines" => machine = true,
            _ => usage(),
        }
    }
    let (Some(disk), Some(username)) = (disk, user) else { usage() };

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let password = read_secret("Password for your account", &mut lines);
    let encryption_password = encrypt.then(|| read_secret("Disk encryption password", &mut lines));

    if !yes {
        if !std::io::stdin().is_terminal() {
            eprintln!("noros-install: refusing to erase {disk} without --yes");
            std::process::exit(2);
        }
        eprint!("Everything on {disk} will be erased. Type ERASE to continue: ");
        let answer = lines.next().and_then(|l| l.ok()).unwrap_or_default();
        if answer.trim() != "ERASE" {
            eprintln!("Cancelled.");
            std::process::exit(1);
        }
    }

    let opts = install::Options {
        disk,
        full_name: name.unwrap_or_else(|| username.clone()),
        username,
        password,
        encryption_password,
        hostname,
    };

    let mut report = |pct: u8, what: &str| {
        if machine {
            println!("PROGRESS {pct} {what}");
        } else {
            eprint!("\r[{pct:3}%] {what:<40}");
        }
        let _ = std::io::stdout().flush();
    };
    match install::run(&opts, &mut report) {
        Ok(()) => {
            if machine {
                println!("DONE");
            } else {
                eprintln!("\nNorOS is installed. Remove the installation medium and restart.");
            }
        }
        Err(err) => {
            if machine {
                println!("ERROR {err}");
            } else {
                eprintln!("\nnoros-install: {err}");
            }
            std::process::exit(1);
        }
    }
}
