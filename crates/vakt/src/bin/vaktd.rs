//! vaktd — the NorOS privacy guard daemon (runs as root).
//!
//! * nftables sends the first packet of every new outgoing connection made by a
//!   user's app (UID ≥ 1000) to queue 42. Loopback, established connections and
//!   system services pass untouched.
//! * For each queued packet vaktd finds the process behind it, then allows it,
//!   blocks it, or holds it while asking the user.
//! * If vaktd isn't running, the queue has no listener and those packets are
//!   dropped: when the guard is down, apps stay offline rather than unguarded.

use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{BufRead, BufReader, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::unix::{fs::PermissionsExt, io::AsRawFd, net::{UnixListener, UnixStream}},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use nfq::{Message, Queue, Verdict};
use serde::{Deserialize, Serialize};
use vakt::{Answer, App, Ask, Connection, Destination, Event, Mode, Policy, Request, Rule, SOCKET, Status};

const QUEUE: u16 = 42;
const STATE_DIR: &str = "/etc/noros/vakt";
const CAMERA_OFF_FLAG: &str = "/var/lib/noros/vakt/camera-off";
const MIC_OFF_FLAG: &str = "/var/lib/noros/vakt/microphone-off";
const ASK_TIMEOUT: Duration = Duration::from_secs(60);
const ALLOW_ONCE_FOR: Duration = Duration::from_secs(10 * 60);
const LOG_SIZE: usize = 1000;

const RULESET: &str = r#"
table inet noros_vakt {
    chain output {
        type filter hook output priority filter; policy accept;
        oifname "lo" accept
        ct state established,related accept
        meta skuid < 1000 accept
        ct state new queue num 42
    }
}
"#;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn log(message: &str) {
    eprintln!("vaktd: {message}");
}

// ── Persistent settings ──────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct Settings {
    mode: Mode,
    camera_enabled: bool,
    #[serde(default = "yes")]
    microphone_enabled: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self { mode: Mode::Ask, camera_enabled: true, microphone_enabled: true }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct RuleFile {
    #[serde(default)]
    rule: Vec<Rule>,
}

fn load<T: for<'de> Deserialize<'de> + Default>(name: &str) -> T {
    fs::read_to_string(Path::new(STATE_DIR).join(name))
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

fn save<T: Serialize>(name: &str, value: &T) {
    let _ = fs::create_dir_all(STATE_DIR);
    let path = Path::new(STATE_DIR).join(name);
    if let Ok(text) = toml::to_string_pretty(value) {
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(tmp, path);
        }
    }
}

// ── Shared state ─────────────────────────────────────────────────────

struct State {
    settings: Settings,
    rules: HashMap<String, Rule>,
    /// "Allow once" grants, until the given moment.
    temporary: HashMap<String, Instant>,
    log: VecDeque<Connection>,
    pending: HashMap<u64, Ask>,
    allowed_today: u64,
    blocked_today: u64,
    day: u64,
}

impl State {
    fn record(&mut self, connection: Connection) {
        let today = now() / 86_400;
        if today != self.day {
            self.day = today;
            self.allowed_today = 0;
            self.blocked_today = 0;
        }
        if connection.allowed {
            self.allowed_today += 1;
        } else {
            self.blocked_today += 1;
        }
        if self.log.len() >= LOG_SIZE {
            self.log.pop_back();
        }
        self.log.push_front(connection);
    }

    fn save_rules(&self) {
        let mut rules: Vec<Rule> = self.rules.values().cloned().collect();
        rules.sort_by(|a, b| a.name.cmp(&b.name));
        save("rules.toml", &RuleFile { rule: rules });
    }
}

struct Subscriber {
    uid: u32,
    stream: Arc<Mutex<UnixStream>>,
}

struct Shared {
    state: Mutex<State>,
    subscribers: Mutex<Vec<Subscriber>>,
    dns: Mutex<HashMap<IpAddr, String>>,
    answers: Mutex<Sender<(u64, Answer)>>,
}

impl Shared {
    fn broadcast(&self, event: &Event, only_uid: Option<u32>) {
        let Ok(mut line) = serde_json::to_string(event) else { return };
        line.push('\n');
        let mut subscribers = self.subscribers.lock().unwrap();
        subscribers.retain(|s| {
            if only_uid.is_some_and(|uid| uid != s.uid && s.uid != 0) {
                return true;
            }
            s.stream.lock().unwrap().write_all(line.as_bytes()).is_ok()
        });
    }

    fn has_subscriber(&self, uid: u32) -> bool {
        self.subscribers.lock().unwrap().iter().any(|s| s.uid == uid)
    }
}

// ── Packets ──────────────────────────────────────────────────────────

struct Packet {
    protocol: &'static str,
    destination: IpAddr,
    source_port: u16,
    destination_port: u16,
}

fn parse_packet(data: &[u8]) -> Option<Packet> {
    let version = data.first()? >> 4;
    let (protocol, destination, header_len) = match version {
        4 => {
            let ihl = ((data[0] & 0x0f) as usize) * 4;
            let dst = Ipv4Addr::new(*data.get(16)?, data[17], data[18], data[19]);
            (*data.get(9)?, IpAddr::V4(dst), ihl)
        }
        6 => {
            let bytes: [u8; 16] = data.get(24..40)?.try_into().ok()?;
            (*data.get(6)?, IpAddr::V6(Ipv6Addr::from(bytes)), 40)
        }
        _ => return None,
    };
    let l4 = data.get(header_len..).unwrap_or(&[]);
    let port = |offset: usize| l4.get(offset..offset + 2).map(|b| u16::from_be_bytes([b[0], b[1]])).unwrap_or(0);
    let name = match protocol {
        6 => "tcp",
        17 => "udp",
        1 | 58 => "icmp",
        _ => "other",
    };
    let (source_port, destination_port) = if matches!(protocol, 6 | 17) { (port(0), port(2)) } else { (0, 0) };
    Some(Packet { protocol: name, destination, source_port, destination_port })
}

/// Find the socket inode for a local port in /proc/net/{tcp,udp}{,6}.
fn socket_inode(protocol: &str, port: u16) -> Option<u64> {
    for table in [protocol.to_string(), format!("{protocol}6")] {
        let Ok(text) = fs::read_to_string(format!("/proc/net/{table}")) else { continue };
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let local = fields.get(1)?;
            let local_port = local.rsplit(':').next().and_then(|p| u16::from_str_radix(p, 16).ok());
            if local_port == Some(port) {
                if let Some(inode) = fields.get(9).and_then(|i| i.parse::<u64>().ok()).filter(|i| *i != 0) {
                    return Some(inode);
                }
            }
        }
    }
    None
}

fn process_owning(inode: u64, uid: Option<u32>) -> Option<u32> {
    let target = format!("socket:[{inode}]");
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if let Some(uid) = uid {
            use std::os::unix::fs::MetadataExt;
            if entry.metadata().map(|m| m.uid() != uid).unwrap_or(true) {
                continue;
            }
        }
        let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        for fd in fds.flatten() {
            if fs::read_link(fd.path()).map(|l| l.to_string_lossy() == target).unwrap_or(false) {
                return Some(pid);
            }
        }
    }
    None
}

/// Program names from installed .desktop files, keyed by program path.
fn desktop_names() -> HashMap<String, String> {
    let mut names = HashMap::new();
    // Flatpak apps are named by their app ID (the .desktop file name).
    let mut flatpak_dirs = vec!["/var/lib/flatpak/exports/share/applications".to_string()];
    for home in fs::read_dir("/home").into_iter().flatten().flatten() {
        flatpak_dirs.push(format!("{}/.local/share/flatpak/exports/share/applications", home.path().display()));
    }
    for dir in &flatpak_dirs {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
            if let Some(name) = fs::read_to_string(&path).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("Name=")).map(|n| n.trim().to_string())) {
                names.insert(format!("flatpak:{id}"), name);
            }
        }
    }
    for dir in ["/usr/share/applications", "/usr/local/share/applications"] {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let Ok(text) = fs::read_to_string(entry.path()) else { continue };
            let field = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)).map(str::trim).map(str::to_string);
            let (Some(name), Some(exec)) = (field("Name="), field("Exec=")) else { continue };
            let Some(program) = exec.split_whitespace().next() else { continue };
            let path = if program.starts_with('/') {
                program.to_string()
            } else {
                ["/usr/bin", "/usr/local/bin", "/bin"]
                    .iter()
                    .map(|d| format!("{d}/{program}"))
                    .find(|p| Path::new(p).exists())
                    .unwrap_or_default()
            };
            if !path.is_empty() {
                names.entry(path).or_insert(name);
            }
        }
    }
    names
}

fn parent_of(pid: u32) -> Option<u32> {
    fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("PPid:"))?
        .trim()
        .parse()
        .ok()
}

fn exe_of(pid: u32) -> Option<String> {
    fs::read_link(format!("/proc/{pid}/exe")).ok().map(|p| p.to_string_lossy().into_owned())
}

/// The program to hold responsible for a process: Flatpak apps by their app ID,
/// and browser helper processes (WebKit's network process) by the browser itself.
fn responsible(pid: u32) -> Option<(u32, String)> {
    let info = fs::read_to_string(format!("/proc/{pid}/root/.flatpak-info")).unwrap_or_default();
    if let Some(id) = info.lines().find_map(|l| l.strip_prefix("name=")) {
        return Some((pid, format!("flatpak:{}", id.trim())));
    }
    let exe = exe_of(pid)?;
    let file = Path::new(&exe).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if file.starts_with("WebKit") {
        if let Some(parent) = parent_of(pid).filter(|p| *p > 1) {
            if let Some(found) = responsible(parent) {
                return Some(found);
            }
        }
    }
    Some((pid, exe))
}

fn identify(packet: &Packet, uid: Option<u32>, names: &HashMap<String, String>) -> App {
    let pid = socket_inode(if packet.protocol == "udp" { "udp" } else { "tcp" }, packet.source_port)
        .filter(|_| packet.source_port != 0)
        .and_then(|inode| process_owning(inode, uid));
    let (pid, exe) = match pid.and_then(responsible) {
        Some((pid, exe)) => (Some(pid), exe),
        None => (pid, "unknown".to_string()),
    };
    let name = names.get(&exe).cloned().unwrap_or_else(|| {
        if exe == "unknown" {
            "Unknown program".into()
        } else if let Some(id) = exe.strip_prefix("flatpak:") {
            id.rsplit('.').next().unwrap_or(id).to_string()
        } else {
            Path::new(&exe).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| exe.clone())
        }
    });
    App { exe, name, pid: pid.unwrap_or(0), uid: uid.unwrap_or(u32::MAX) }
}

enum Decision {
    Accept(&'static str),
    Drop(&'static str),
    Ask,
}

fn decide(state: &mut State, app: &App) -> Decision {
    match state.settings.mode {
        Mode::Offline => return Decision::Drop("offline mode"),
        Mode::AllowAll => return Decision::Accept("allow-everything mode"),
        Mode::Ask => {}
    }
    match state.rules.get(&app.exe).map(|r| r.policy) {
        Some(Policy::Allow) => return Decision::Accept("allowed by your rule"),
        Some(Policy::Block) => return Decision::Drop("blocked by your rule"),
        _ => {}
    }
    if let Some(until) = state.temporary.get(&app.exe) {
        if Instant::now() < *until {
            return Decision::Accept("allowed for now");
        }
        state.temporary.remove(&app.exe);
    }
    Decision::Ask
}

/// Packets held while the user decides about one app.
struct Held {
    id: u64,
    app: App,
    destination: Destination,
    messages: Vec<Message>,
    deadline: Instant,
}

fn packet_loop(shared: Arc<Shared>, mut queue: Queue, answers: Receiver<(u64, Answer)>) {
    let mut names = desktop_names();
    let mut names_loaded = Instant::now();
    let mut held: HashMap<String, Held> = HashMap::new();
    let mut next_id = 1u64;
    let mut recent: HashMap<(String, String), Instant> = HashMap::new();

    let verdict = |queue: &mut Queue, mut msg: Message, accept: bool| {
        msg.set_verdict(if accept { Verdict::Accept } else { Verdict::Drop });
        let _ = queue.verdict(msg);
    };

    loop {
        let mut idle = true;

        // New packets.
        match queue.recv() {
            Ok(msg) => {
                idle = false;
                if names_loaded.elapsed() > Duration::from_secs(60) {
                    names = desktop_names();
                    names_loaded = Instant::now();
                }
                let Some(packet) = parse_packet(msg.get_payload()) else {
                    verdict(&mut queue, msg, true);
                    continue;
                };
                let app = identify(&packet, msg.get_uid(), &names);
                let destination = Destination {
                    ip: packet.destination.to_string(),
                    port: packet.destination_port,
                    protocol: packet.protocol.to_string(),
                    host: shared.dns.lock().unwrap().get(&packet.destination).cloned(),
                };

                // Already asking about this app? Hold this packet with the others.
                if let Some(h) = held.get_mut(&app.exe) {
                    h.messages.push(msg);
                    continue;
                }

                let decision = decide(&mut shared.state.lock().unwrap(), &app);
                let (accept, reason) = match decision {
                    Decision::Accept(reason) => (true, reason),
                    Decision::Drop(reason) => (false, reason),
                    Decision::Ask if !shared.has_subscriber(app.uid) => (false, "nobody signed in to ask"),
                    Decision::Ask => {
                        let ask = Ask { id: next_id, app: app.clone(), destination: destination.clone(), timeout: ASK_TIMEOUT.as_secs() };
                        next_id += 1;
                        shared.state.lock().unwrap().pending.insert(ask.id, ask.clone());
                        shared.broadcast(&Event::Ask(ask.clone()), Some(app.uid));
                        held.insert(
                            app.exe.clone(),
                            Held { id: ask.id, app, destination, messages: vec![msg], deadline: Instant::now() + ASK_TIMEOUT },
                        );
                        continue;
                    }
                };
                verdict(&mut queue, msg, accept);

                // Log each connection once, not every retransmitted packet.
                let key = (app.exe.clone(), destination.label());
                let fresh = recent.get(&key).is_none_or(|t| t.elapsed() > Duration::from_secs(5));
                if fresh {
                    if recent.len() > 10_000 {
                        recent.clear();
                    }
                    recent.insert(key, Instant::now());
                    let connection = Connection { time: now(), app, destination, allowed: accept, reason: reason.into() };
                    shared.state.lock().unwrap().record(connection.clone());
                    shared.broadcast(&Event::Connection(connection), None);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) => {
                log(&format!("queue error: {err}"));
                thread::sleep(Duration::from_millis(100));
            }
        }

        // Answers from the user, and questions nobody answered in time.
        let mut resolved: Vec<(String, Answer, &'static str)> = Vec::new();
        while let Ok((id, answer)) = answers.try_recv() {
            idle = false;
            if let Some(exe) = held.iter().find(|(_, h)| h.id == id).map(|(exe, _)| exe.clone()) {
                resolved.push((exe, answer, "you decided"));
            }
        }
        for (exe, h) in &held {
            if Instant::now() > h.deadline && !resolved.iter().any(|(e, _, _)| e == exe) {
                resolved.push((exe.clone(), Answer::BlockOnce, "no answer"));
            }
        }
        for (exe, answer, why) in resolved {
            let Some(h) = held.remove(&exe) else { continue };
            let allow = matches!(answer, Answer::AllowOnce | Answer::AllowAlways);
            {
                let mut state = shared.state.lock().unwrap();
                state.pending.remove(&h.id);
                match answer {
                    Answer::AllowOnce => {
                        state.temporary.insert(exe.clone(), Instant::now() + ALLOW_ONCE_FOR);
                    }
                    Answer::AllowAlways | Answer::BlockAlways => {
                        let policy = if allow { Policy::Allow } else { Policy::Block };
                        state.rules.insert(exe.clone(), Rule { exe: exe.clone(), name: h.app.name.clone(), policy, created: now() });
                        state.save_rules();
                    }
                    Answer::BlockOnce => {}
                }
                let reason = if why == "no answer" { "no answer" } else if allow { "you allowed it" } else { "you blocked it" };
                state.record(Connection { time: now(), app: h.app.clone(), destination: h.destination.clone(), allowed: allow, reason: reason.into() });
            }
            for msg in h.messages {
                verdict(&mut queue, msg, allow);
            }
            shared.broadcast(&Event::Answered { id: h.id }, None);
        }

        if idle {
            thread::sleep(Duration::from_millis(5));
        }
    }
}

// ── Camera ───────────────────────────────────────────────────────────

fn camera_devices() -> Vec<String> {
    fs::read_dir("/dev")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().to_string_lossy().into_owned())
        .filter(|p| p.starts_with("/dev/video"))
        .collect()
}

fn camera_users(names: &HashMap<String, String>) -> Vec<App> {
    let mut users = Vec::new();
    for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        let uses = fds.flatten().any(|fd| fs::read_link(fd.path()).map(|l| l.to_string_lossy().starts_with("/dev/video")).unwrap_or(false));
        if uses {
            use std::os::unix::fs::MetadataExt;
            let exe = fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let name = names.get(&exe).cloned().unwrap_or_else(|| Path::new(&exe).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            users.push(App { exe, name, pid, uid: entry.metadata().map(|m| m.uid()).unwrap_or(0) });
        }
    }
    users
}

/// Switching the camera off removes everyone's access to the device; udev keeps it
/// that way for cameras plugged in later (see 70-noros-camera.rules).
fn apply_camera(enabled: bool) {
    if enabled {
        let _ = fs::remove_file(CAMERA_OFF_FLAG);
    } else {
        let _ = fs::create_dir_all(Path::new(CAMERA_OFF_FLAG).parent().unwrap());
        let _ = fs::write(CAMERA_OFF_FLAG, "");
    }
    let _ = Command::new("udevadm").args(["trigger", "--subsystem-match=video4linux", "--action=change"]).status();
    let _ = Command::new("udevadm").args(["settle", "--timeout=5"]).status();
    if !enabled {
        for device in camera_devices() {
            let _ = Command::new("setfacl").args(["-b", &device]).status();
            let _ = fs::set_permissions(&device, fs::Permissions::from_mode(0o600));
        }
    }
}

// ── Microphone ───────────────────────────────────────────────────────

/// ALSA capture devices (the kernel side of every microphone).
fn capture_devices() -> Vec<String> {
    fs::read_dir("/dev/snd")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().to_string_lossy().into_owned())
        .filter(|p| p.rsplit('/').next().is_some_and(|n| n.starts_with("pcmC") && n.ends_with('c')))
        .collect()
}

fn microphone_in_use() -> bool {
    let Ok(cards) = fs::read_dir("/proc/asound") else { return false };
    for card in cards.flatten().filter(|c| c.file_name().to_string_lossy().starts_with("card")) {
        for pcm in fs::read_dir(card.path()).into_iter().flatten().flatten() {
            if !pcm.file_name().to_string_lossy().ends_with('c') {
                continue;
            }
            for sub in fs::read_dir(pcm.path()).into_iter().flatten().flatten() {
                if fs::read_to_string(sub.path().join("status")).is_ok_and(|s| s.contains("RUNNING")) {
                    return true;
                }
            }
        }
    }
    false
}

/// Switching the microphone off removes access to every capture device and
/// restarts the sound servers holding one open, so recording stops right away
/// (sound playback resumes by itself a moment later).
fn apply_microphone(enabled: bool) {
    if enabled {
        let _ = fs::remove_file(MIC_OFF_FLAG);
    } else {
        let _ = fs::create_dir_all(Path::new(MIC_OFF_FLAG).parent().unwrap());
        let _ = fs::write(MIC_OFF_FLAG, "");
    }
    let _ = Command::new("udevadm").args(["trigger", "--subsystem-match=sound", "--action=change"]).status();
    let _ = Command::new("udevadm").args(["settle", "--timeout=5"]).status();
    if enabled {
        return;
    }
    let devices = capture_devices();
    for device in &devices {
        let _ = Command::new("setfacl").args(["-b", device]).status();
        let _ = fs::set_permissions(device, fs::Permissions::from_mode(0o600));
    }
    for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
        let holds = fs::read_dir(format!("/proc/{pid}/fd"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|fd| fs::read_link(fd.path()).is_ok_and(|l| devices.iter().any(|d| l.to_string_lossy() == d.as_str())));
        let is_sound_server = exe_of(pid as u32).is_some_and(|e| e.ends_with("/pipewire") || e.ends_with("/wireplumber"));
        if holds && is_sound_server {
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
    }
}

// ── Names from DNS lookups ───────────────────────────────────────────

/// Watch systemd-resolved's answers (locally; nothing extra is looked up) so
/// connections can show "example.com" instead of a bare address.
fn dns_watch(shared: Arc<Shared>) {
    loop {
        let child = Command::new("resolvectl").args(["monitor", "--json=short"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        if let Ok(mut child) = child {
            let reader = BufReader::new(child.stdout.take().unwrap());
            for line in reader.lines().map_while(Result::ok) {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                let Some(answers) = value.get("answer").and_then(|a| a.as_array()) else { continue };
                let mut dns = shared.dns.lock().unwrap();
                for answer in answers {
                    let rr = answer.get("rr").unwrap_or(answer);
                    let name = rr.pointer("/key/name").and_then(|n| n.as_str());
                    let bytes: Option<Vec<u8>> = rr.get("address").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|b| b.as_u64().map(|b| b as u8)).collect());
                    let ip = match bytes.as_deref() {
                        Some(b) if b.len() == 4 => Some(IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3]))),
                        Some(b) if b.len() == 16 => <[u8; 16]>::try_from(b).ok().map(|a| IpAddr::V6(Ipv6Addr::from(a))),
                        _ => None,
                    };
                    if let (Some(name), Some(ip)) = (name, ip) {
                        if dns.len() > 5000 {
                            dns.clear();
                        }
                        dns.insert(ip, name.trim_end_matches('.').to_string());
                    }
                }
            }
            let _ = child.wait();
        }
        thread::sleep(Duration::from_secs(30));
    }
}

// ── Control socket ───────────────────────────────────────────────────

fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len)
    };
    (rc == 0).then_some(cred.uid)
}

/// Changing rules and modes is for administrators (the "sudo" group: every
/// account the installer creates, and the live user).
fn is_admin(uid: u32) -> bool {
    if uid == 0 {
        return true;
    }
    let Some(user) = fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|p| p.lines().find(|l| l.split(':').nth(2) == Some(&uid.to_string())).and_then(|l| l.split(':').next().map(str::to_string)))
    else {
        return false;
    };
    fs::read_to_string("/etc/group")
        .map(|g| g.lines().any(|l| l.starts_with("sudo:") && l.rsplit(':').next().is_some_and(|m| m.split(',').any(|u| u == user))))
        .unwrap_or(false)
}

fn status(shared: &Shared) -> Status {
    let state = shared.state.lock().unwrap();
    let names = desktop_names();
    Status {
        mode: Some(state.settings.mode),
        rules: state.rules.len(),
        allowed_today: state.allowed_today,
        blocked_today: state.blocked_today,
        camera_enabled: state.settings.camera_enabled,
        camera_present: !camera_devices().is_empty(),
        camera_in_use: camera_users(&names),
        microphone_enabled: state.settings.microphone_enabled,
        microphone_present: !capture_devices().is_empty(),
        microphone_in_use: microphone_in_use(),
    }
}

fn handle_client(shared: Arc<Shared>, stream: UnixStream) {
    let Some(uid) = peer_uid(&stream) else { return };
    let writer = Arc::new(Mutex::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    }));
    let reply = |event: Event| {
        let mut line = serde_json::to_string(&event).unwrap_or_default();
        line.push('\n');
        writer.lock().unwrap().write_all(line.as_bytes()).is_ok()
    };
    let denied = || Event::Error { message: "only administrators can change this".into() };

    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        let request: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(err) => {
                reply(Event::Error { message: format!("bad request: {err}") });
                continue;
            }
        };
        let response = match request {
            Request::Subscribe => {
                shared.subscribers.lock().unwrap().push(Subscriber { uid, stream: writer.clone() });
                // Questions that were already waiting.
                let waiting: Vec<Ask> = shared.state.lock().unwrap().pending.values().filter(|a| a.app.uid == uid || uid == 0).cloned().collect();
                for ask in waiting {
                    reply(Event::Ask(ask));
                }
                Event::Ok
            }
            Request::Answer { id, answer } => {
                let owner = shared.state.lock().unwrap().pending.get(&id).map(|a| a.app.uid);
                match owner {
                    Some(owner) if owner == uid || uid == 0 => {
                        let _ = shared.answers.lock().unwrap().send((id, answer));
                        Event::Ok
                    }
                    Some(_) => Event::Error { message: "that question belongs to another user".into() },
                    None => Event::Error { message: "that question was already answered".into() },
                }
            }
            Request::Status => Event::Status(status(&shared)),
            Request::Rules => {
                let mut rules: Vec<Rule> = shared.state.lock().unwrap().rules.values().cloned().collect();
                rules.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                Event::Rules { rules }
            }
            Request::Log => Event::Log { entries: shared.state.lock().unwrap().log.iter().cloned().collect() },
            Request::SetRule { exe, name, policy } if is_admin(uid) => {
                let mut state = shared.state.lock().unwrap();
                state.rules.insert(exe.clone(), Rule { exe, name, policy, created: now() });
                state.save_rules();
                Event::Ok
            }
            Request::DeleteRule { exe } if is_admin(uid) => {
                let mut state = shared.state.lock().unwrap();
                state.rules.remove(&exe);
                state.temporary.remove(&exe);
                state.save_rules();
                Event::Ok
            }
            Request::SetMode { mode } if is_admin(uid) => {
                let mut state = shared.state.lock().unwrap();
                state.settings.mode = mode;
                save("settings.toml", &state.settings);
                log(&format!("mode set to {mode:?}"));
                Event::Ok
            }
            Request::SetCamera { enabled } if is_admin(uid) => {
                {
                    let mut state = shared.state.lock().unwrap();
                    state.settings.camera_enabled = enabled;
                    save("settings.toml", &state.settings);
                }
                apply_camera(enabled);
                log(&format!("camera {}", if enabled { "on" } else { "off" }));
                Event::Ok
            }
            Request::SetMicrophone { enabled } if is_admin(uid) => {
                {
                    let mut state = shared.state.lock().unwrap();
                    state.settings.microphone_enabled = enabled;
                    save("settings.toml", &state.settings);
                }
                apply_microphone(enabled);
                log(&format!("microphone {}", if enabled { "on" } else { "off" }));
                Event::Ok
            }
            Request::SetRule { .. }
            | Request::DeleteRule { .. }
            | Request::SetMode { .. }
            | Request::SetCamera { .. }
            | Request::SetMicrophone { .. } => denied(),
        };
        if !reply(response) {
            break;
        }
    }
}

fn serve(shared: Arc<Shared>) {
    let _ = fs::create_dir_all(Path::new(SOCKET).parent().unwrap());
    let _ = fs::remove_file(SOCKET);
    let listener = match UnixListener::bind(SOCKET) {
        Ok(l) => l,
        Err(err) => {
            log(&format!("cannot listen on {SOCKET}: {err}"));
            std::process::exit(1);
        }
    };
    // Anyone may connect; what they may do is checked per request.
    let _ = fs::set_permissions(SOCKET, fs::Permissions::from_mode(0o666));
    for stream in listener.incoming().flatten() {
        let shared = shared.clone();
        thread::spawn(move || handle_client(shared, stream));
    }
}

fn install_ruleset() -> std::io::Result<()> {
    let _ = Command::new("nft").args(["delete", "table", "inet", "noros_vakt"]).stderr(Stdio::null()).status();
    let mut child = Command::new("nft").args(["-f", "-"]).stdin(Stdio::piped()).spawn()?;
    child.stdin.take().unwrap().write_all(RULESET.as_bytes())?;
    if !child.wait()?.success() {
        return Err(std::io::Error::other("nft rejected the ruleset"));
    }
    Ok(())
}

fn main() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("vaktd must run as root");
        std::process::exit(1);
    }

    let settings: Settings = load("settings.toml");
    let rules: RuleFile = load("rules.toml");
    apply_camera(settings.camera_enabled);
    apply_microphone(settings.microphone_enabled);

    let mut queue = match Queue::open() {
        Ok(q) => q,
        Err(err) => {
            log(&format!("cannot open the packet queue: {err}"));
            std::process::exit(1);
        }
    };
    let setup = (|| {
        queue.bind(QUEUE)?;
        queue.set_recv_uid_gid(QUEUE, true)?;
        queue.set_fail_open(QUEUE, false)?;
        queue.set_copy_range(QUEUE, 128)?;
        queue.set_queue_max_len(QUEUE, 4096)?;
        Ok::<(), std::io::Error>(())
    })();
    if let Err(err) = setup {
        log(&format!("cannot set up the packet queue: {err}"));
        std::process::exit(1);
    }
    queue.set_nonblocking(true);

    if let Err(err) = install_ruleset() {
        log(&format!("cannot install firewall rules: {err}"));
        std::process::exit(1);
    }

    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            settings,
            rules: rules.rule.into_iter().map(|r| (r.exe.clone(), r)).collect(),
            temporary: HashMap::new(),
            log: VecDeque::new(),
            pending: HashMap::new(),
            allowed_today: 0,
            blocked_today: 0,
            day: now() / 86_400,
        }),
        subscribers: Mutex::new(Vec::new()),
        dns: Mutex::new(HashMap::new()),
        answers: Mutex::new(tx),
    });

    {
        let shared = shared.clone();
        thread::spawn(move || serve(shared));
    }
    {
        let shared = shared.clone();
        thread::spawn(move || dns_watch(shared));
    }
    log("guarding outgoing connections");
    packet_loop(shared, queue, rx);
}
