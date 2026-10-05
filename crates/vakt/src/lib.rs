//! Vakt — the NorOS privacy guard.
//!
//! `vaktd` runs as root and sees every new outgoing connection made by a user's
//! apps (via an nftables queue). Each app is allowed, blocked, or — the first time —
//! the user is asked. Desktop programs talk to it over a Unix socket with one JSON
//! object per line; this crate holds that protocol and a small client.
//!
//! System services (user IDs below 1000: address configuration, name lookups,
//! package updates you start yourself) are allowed and listed openly in Privacy Center.

use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    time::Duration,
};

use serde::{Deserialize, Serialize};

pub const SOCKET: &str = "/run/noros/vakt.sock";

/// An app as Vakt identifies it: by the program file it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct App {
    pub exe: String,
    pub name: String,
    pub pid: u32,
    pub uid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination {
    pub ip: String,
    pub port: u16,
    pub protocol: String,
    /// The name the app looked up, when Vakt saw the lookup.
    pub host: Option<String>,
}

impl Destination {
    pub fn label(&self) -> String {
        let host = self.host.as_deref().unwrap_or(&self.ip);
        format!("{host}:{}", self.port)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Allow,
    Block,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// Allow for the next few minutes.
    AllowOnce,
    AllowAlways,
    /// Block this attempt.
    BlockOnce,
    BlockAlways,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Ask about each new app (the default).
    Ask,
    /// Let every app connect, but still log it.
    AllowAll,
    /// Block every app; nothing leaves the machine.
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub exe: String,
    pub name: String,
    pub policy: Policy,
    pub created: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub time: u64,
    pub app: App,
    pub destination: Destination,
    pub allowed: bool,
    /// Why: "rule", "you allowed it", "offline mode", …
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    pub id: u64,
    pub app: App,
    pub destination: Destination,
    /// Seconds before an unanswered question counts as "block".
    pub timeout: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Status {
    pub mode: Option<Mode>,
    pub rules: usize,
    pub allowed_today: u64,
    pub blocked_today: u64,
    pub camera_enabled: bool,
    pub camera_present: bool,
    /// Apps that have a camera open right now.
    pub camera_in_use: Vec<App>,
    #[serde(default)]
    pub microphone_enabled: bool,
    #[serde(default)]
    pub microphone_present: bool,
    /// Something is recording right now.
    #[serde(default)]
    pub microphone_in_use: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Receive `ask` and `connection` events as they happen.
    Subscribe,
    Answer { id: u64, answer: Answer },
    Status,
    Rules,
    SetRule { exe: String, name: String, policy: Policy },
    DeleteRule { exe: String },
    SetMode { mode: Mode },
    Log,
    SetCamera { enabled: bool },
    SetMicrophone { enabled: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Ask(Ask),
    /// A question was answered (here or elsewhere) or timed out.
    Answered { id: u64 },
    Connection(Connection),
    Status(Status),
    Rules { rules: Vec<Rule> },
    Log { entries: Vec<Connection> },
    Ok,
    Error { message: String },
}

/// A connection to `vaktd`.
pub struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    pub fn connect() -> io::Result<Self> {
        let stream = UnixStream::connect(SOCKET)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        })
    }

    pub fn send(&mut self, request: &Request) -> io::Result<()> {
        let mut line = serde_json::to_string(request).map_err(io::Error::other)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())
    }

    pub fn next_event(&mut self) -> io::Result<Event> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "vaktd closed the connection"));
        }
        serde_json::from_str(&line).map_err(io::Error::other)
    }

    /// Send a request and wait for its reply.
    pub fn call(&mut self, request: &Request) -> io::Result<Event> {
        self.send(request)?;
        self.next_event()
    }

    /// For subscribers: block until events arrive.
    pub fn wait_forever(&mut self) -> io::Result<()> {
        self.reader.get_ref().set_read_timeout(None)
    }
}
