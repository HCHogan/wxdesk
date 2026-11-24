//! What wxdesk's driver and its client say to each other: one JSON value
//! a line over a TCP stream. The client sends [`ToDriver`] and reads
//! [`FromDriver`]; [`connect`] is the client's end.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// What the driver says.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromDriver {
    /// First, on every connection; then the messages that came while no
    /// client was connected, at most 500.
    Hello {
        /// When the driver started, in Unix milliseconds: a message is
        /// known by this and its `seq`.
        run: u64,
        /// The bot's display name, once the driver has seen it.
        me: Option<String>,
        /// The group the driver keeps open.
        group: String,
        logged_in: bool,
    },
    /// The client logged in or out.
    State { logged_in: bool },
    /// A message, mine or someone else's.
    Message(Message),
    /// The end of the command with this id.
    Done { id: u64, error: Option<String> },
}

/// A message as the client showed it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    /// Increasing within one run of the driver (see `Hello`).
    pub seq: u64,
    /// The chat's title: the group's name, or the friend's remark or name.
    pub chat: String,
    pub group: bool,
    /// The sender's display name. The chat's title in a private chat;
    /// none where the driver couldn't tell.
    pub sender: Option<String>,
    pub mine: bool,
    pub kind: Kind,
    /// The message as the client names it: the text, `图片`, `文件\n…`.
    pub text: String,
    pub quote: Option<Quote>,
    /// Whether it mentions the bot.
    pub mentions_me: bool,
    pub attachment: Option<Attachment>,
    /// When the driver saw it, in Unix seconds.
    pub at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Text,
    Image,
    File,
    Voice,
    Sticker,
    /// A line from the client itself, such as someone joining.
    System,
}

/// The message a reply quotes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Quote {
    pub sender: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    /// The bytes, in standard base64.
    pub data: String,
}

/// What the client asks of the driver. Each is answered with a
/// [`FromDriver::Done`] of its id.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ToDriver {
    /// The first line of a connection when the driver wants a token.
    Auth { token: String },
    /// Text, after a real mention of each display name in `mentions`.
    SendText {
        id: u64,
        chat: String,
        text: String,
        #[serde(default)]
        mentions: Vec<String>,
    },
    /// A PNG, in standard base64.
    SendImage { id: u64, chat: String, png: String },
    /// A file, in standard base64.
    SendFile {
        id: u64,
        chat: String,
        name: String,
        data: String,
    },
}

/// Connects to the driver at `addr` and, with a token, authenticates: what
/// it says, and where to send commands.
///
/// # Errors
///
/// If the connection fails.
pub async fn connect(addr: &str, token: Option<&str>) -> std::io::Result<(Events, Commands)> {
    let stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    let (read, write) = stream.into_split();
    let mut commands = Commands { write };
    if let Some(token) = token {
        commands
            .send(&ToDriver::Auth {
                token: token.to_owned(),
            })
            .await?;
    }
    let events = Events {
        lines: BufReader::new(read).lines(),
    };
    Ok((events, commands))
}

/// What the driver says on a connection.
pub struct Events {
    lines: tokio::io::Lines<BufReader<OwnedReadHalf>>,
}

impl Events {
    /// The next thing the driver says; none when it hangs up.
    ///
    /// # Errors
    ///
    /// If reading fails or the line isn't one of ours.
    pub async fn next(&mut self) -> std::io::Result<Option<FromDriver>> {
        match self.lines.next_line().await? {
            None => Ok(None),
            Some(line) => Ok(Some(serde_json::from_str(&line)?)),
        }
    }
}

/// Where commands go on a connection.
pub struct Commands {
    write: OwnedWriteHalf,
}

impl Commands {
    /// # Errors
    ///
    /// If writing fails.
    pub async fn send(&mut self, command: &ToDriver) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(command)?;
        line.push(b'\n');
        self.write.write_all(&line).await
    }
}
