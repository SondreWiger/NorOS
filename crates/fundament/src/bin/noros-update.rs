//! noros-update — snapshots, rollback and system updates.
//!
//!   noros-update status [--json]       what's installed and which snapshots exist
//!   noros-update snapshot [TEXT]       take a snapshot now
//!   noros-update rollback ID           make snapshot ID the system (after a restart)
//!   noros-update delete ID             remove a snapshot
//!   noros-update upgrade               snapshot, then fetch and install updates
//!                                      (the only command that goes online, and only when you run it)
//!   noros-update cleanup               housekeeping at boot

use fundament::{VERSION, snapshots, sys};

fn fail(err: impl std::fmt::Display) -> ! {
    eprintln!("noros-update: {err}");
    std::process::exit(1)
}

fn date(secs: u64) -> String {
    sys::run("date", &["-d", &format!("@{secs}"), "+%e %b %Y %H:%M"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| secs.to_string())
}

fn parse_id(arg: Option<&String>) -> u32 {
    arg.and_then(|a| a.trim_start_matches('#').parse().ok())
        .unwrap_or_else(|| fail("give the snapshot number, e.g. `noros-update rollback 3`"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("status");

    match command {
        "status" => {
            let list = snapshots::list();
            if args.iter().any(|a| a == "--json") {
                let status = serde_json::json!({
                    "version": VERSION,
                    "live": fundament::is_live(),
                    "reboot_required": snapshots::reboot_required(),
                    "snapshots": list,
                });
                println!("{}", serde_json::to_string_pretty(&status).unwrap());
                return;
            }
            println!("NorOS {VERSION}{}", if fundament::is_live() { " (live session)" } else { "" });
            if snapshots::reboot_required() {
                println!("A rollback is waiting: restart to finish it.");
            }
            if list.is_empty() {
                println!("No snapshots.");
            }
            for s in list {
                let kind = if s.automatic { " (automatic)" } else { "" };
                println!("  #{:<4} {}  {}{kind}", s.id, date(s.created), s.description);
            }
        }
        "snapshot" => {
            let automatic = args.iter().any(|a| a == "--auto");
            let text: Vec<&str> = args.iter().skip(1).filter(|a| *a != "--auto").map(String::as_str).collect();
            let description = if text.is_empty() { "Manual snapshot".to_string() } else { text.join(" ") };
            match snapshots::create(&description, automatic) {
                Ok(Some(s)) => println!("Snapshot #{} created.", s.id),
                Ok(None) => println!("A recent snapshot already covers this."),
                // Automatic snapshots must never block package installs.
                Err(_) if automatic => {}
                Err(err) => fail(err),
            }
        }
        "rollback" => {
            let id = parse_id(args.get(1));
            match snapshots::rollback(id) {
                Ok(_) => println!("Done. Restart to start snapshot #{id}. Your current system was kept as a snapshot."),
                Err(err) => fail(err),
            }
        }
        "delete" => {
            let id = parse_id(args.get(1));
            match snapshots::delete(id) {
                Ok(()) => println!("Snapshot #{id} deleted."),
                Err(err) => fail(err),
            }
        }
        "upgrade" => {
            if let Err(err) = sys::require_root() {
                fail(err);
            }
            match snapshots::create("Before system update", false) {
                Ok(Some(s)) => println!("Snapshot #{} taken; you can roll back to it if anything goes wrong.", s.id),
                Ok(None) => {}
                Err(err) => fail(format!("not updating without a snapshot: {err}")),
            }
            for step in [vec!["update"], vec!["-y", "full-upgrade"]] {
                let status = std::process::Command::new("apt-get").args(&step).status();
                if !status.map(|s| s.success()).unwrap_or(false) {
                    fail("the update didn't finish; nothing is lost: roll back with `noros-update rollback <id>`");
                }
            }
            println!("Up to date.");
        }
        "cleanup" => {
            if let Err(err) = snapshots::cleanup() {
                fail(err);
            }
        }
        "--version" => println!("noros-update {VERSION}"),
        _ => {
            eprintln!("usage: noros-update [status|snapshot|rollback ID|delete ID|upgrade]");
            std::process::exit(2);
        }
    }
}
