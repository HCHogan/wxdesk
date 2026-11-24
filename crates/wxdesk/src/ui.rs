//! WeChat's main window as the driver reads and drives it. One task owns
//! it: polls, private chats and commands take turns, so no two clicks
//! interleave.
//!
//! What the window shows, as the probe found it (WeChat 4.1.13 for Linux):
//! the session list is a `[list] '会话'` whose items are named
//! `title\n[N条] \n[有人@我]\nsender: preview\nHH:MM\n`; the open chat's
//! messages are a `[list] '消息'` with an item per visible row, named by
//! its content alone; the chat's input is a `[text]` named after the chat.
//! Who sent a row shows only in the widgets WeChat builds to draw it: an
//! avatar button named after the sender, a child of the message list,
//! 8 pixels below the row's top. [`watch`] pairs those with rows.

use crate::a11y::{A11y, Obj, Rect};
use crate::input;
use anyhow::{Context, Result, anyhow, bail};
use atspi_common::Role;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::StreamExt;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::{Receiver, Sender, UnboundedReceiver, UnboundedSender};
use tracing::{debug, info, warn};
use wxdesk_proto::{Attachment, FromDriver, Kind, Message, Quote, ToDriver};

pub struct Config {
    pub group: String,
    pub me: Option<String>,
    pub home: PathBuf,
}

/// What the server tells a client as it connects.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub me: Option<String>,
    pub logged_in: bool,
}

/// A row of the open chat, drawn by WeChat with its sender's avatar.
#[derive(Clone, Debug)]
pub struct Drawn {
    /// The chat open when it was drawn.
    chat: Option<String>,
    text: String,
    /// Where the row's top was.
    y: i32,
    sender: String,
    mine: bool,
    at: Instant,
}

/// What [`watch`] shares with the window's task.
#[derive(Default)]
pub struct Shared {
    /// The message list now, which [`watch`] looks under.
    pub list: Option<Obj>,
    /// The chat open now.
    pub open: Option<String>,
    pub drawn: VecDeque<Drawn>,
}

#[derive(Clone, Debug)]
struct Row {
    text: String,
    rect: Rect,
}

#[derive(Debug)]
struct Session {
    title: String,
    /// The item's whole name, which changes with each message.
    name: String,
    unread: u32,
    preview: String,
    rect: Rect,
}

/// The avatar a row is drawn with sits this far below the row's top.
const AVATAR_OFFSET: i32 = 8;

/// How long a drawn row waits to be read.
const DRAWN_FOR: Duration = Duration::from_secs(120);

/// Pairs the avatars WeChat draws rows with with the rows, as they're
/// drawn, and wakes the window's task.
pub async fn watch(a: A11y, app: Obj, shared: Arc<Mutex<Shared>>, poke: Sender<()>) -> Result<()> {
    a.register(&[
        "object:state-changed:showing",
        "object:property-change:accessible-name",
    ])
    .await?;
    let mut signals = std::pin::pin!(a.signals().await?);
    let mut recent: VecDeque<(String, Instant)> = VecDeque::new();
    while let Some(s) = signals.next().await {
        if s.source.bus != app.bus {
            continue;
        }
        let wanted = (s.member == "StateChanged" && s.kind == "showing" && s.detail1 == 1)
            || (s.member == "PropertyChange" && s.kind == "accessible-name");
        if !wanted {
            continue;
        }
        let now = Instant::now();
        while recent
            .front()
            .is_some_and(|(_, t)| now - *t > Duration::from_secs(2))
        {
            recent.pop_front();
        }
        if recent.iter().any(|(p, _)| *p == s.source.path) {
            continue;
        }
        let Some(list) = shared.lock().unwrap().list.clone() else {
            continue;
        };
        if !matches!(a.parent(&s.source).await, Ok(Some(p)) if p == list) {
            continue;
        }
        if !matches!(a.role(&s.source).await, Ok(Role::Button)) {
            continue;
        }
        let (Ok(sender), Ok(rect)) = (a.name(&s.source).await, a.extents(&s.source).await) else {
            continue;
        };
        if sender.is_empty() || rect.is_empty() || rect.w != rect.h {
            continue;
        }
        recent.push_back((s.source.path.clone(), now));
        let Ok(rows) = rows(&a, &list).await else {
            continue;
        };
        let Some(row) = rows
            .iter()
            .find(|r| (r.rect.y + AVATAR_OFFSET - rect.y).abs() <= 3)
        else {
            debug!(sender, ?rect, "an avatar with no row");
            continue;
        };
        let mine = rect.x > row.rect.x + row.rect.w / 2;
        debug!(sender, mine, text = row.text, "drawn");
        let mut sh = shared.lock().unwrap();
        let chat = sh.open.clone();
        sh.drawn.push_back(Drawn {
            chat,
            text: row.text.clone(),
            y: row.rect.y,
            sender,
            mine,
            at: now,
        });
        while sh.drawn.len() > 500 {
            sh.drawn.pop_front();
        }
        drop(sh);
        let _ = poke.try_send(());
    }
    Err(anyhow!("the accessibility bus closed"))
}

async fn rows(a: &A11y, list: &Obj) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for o in a.children(list).await? {
        let (Ok(text), Ok(rect)) = (a.name(&o).await, a.extents(&o).await) else {
            continue;
        };
        if rect.is_empty() {
            continue;
        }
        rows.push(Row { text, rect });
    }
    rows.sort_by_key(|r| r.rect.y);
    Ok(rows)
}

/// The drawn rows that drew `fresh` in `chat`, newest last on both sides:
/// each row takes the newest drawing of its text where it is now, or else
/// the newest drawing of its text, not taken by a later row. Older
/// drawings of a text taken are of rows read before, drawn again, and go.
fn take_drawn(drawn: &mut VecDeque<Drawn>, chat: &str, fresh: &[&Row]) -> Vec<Option<Drawn>> {
    let horizon = Instant::now().checked_sub(DRAWN_FOR);
    drawn.retain(|d| horizon.is_none_or(|h| d.at >= h));
    let mut taken = vec![false; drawn.len()];
    let mut out = vec![None; fresh.len()];
    for (i, row) in fresh.iter().enumerate().rev() {
        let candidate = |j: usize| {
            !taken[j] && drawn[j].chat.as_deref() == Some(chat) && drawn[j].text == row.text
        };
        let there = (0..drawn.len())
            .rev()
            .find(|&j| candidate(j) && (drawn[j].y - row.rect.y).abs() <= 3);
        if let Some(j) = there.or_else(|| (0..drawn.len()).rev().find(|&j| candidate(j))) {
            taken[j] = true;
            out[i] = Some(drawn[j].clone());
        }
    }
    let mut newest_taken: HashMap<String, usize> = HashMap::new();
    for (j, d) in drawn.iter().enumerate() {
        if taken[j] {
            newest_taken.insert(d.text.clone(), j);
        }
    }
    let mut j = 0;
    drawn.retain(|d| {
        let keep =
            d.chat.as_deref() != Some(chat) || newest_taken.get(&d.text).is_none_or(|&n| j > n);
        j += 1;
        keep
    });
    out
}

pub struct Ui {
    a: A11y,
    app: Obj,
    cfg: Config,
    shared: Arc<Mutex<Shared>>,
    status: Arc<Mutex<Status>>,
    out: UnboundedSender<FromDriver>,
    frame: Option<Obj>,
    sessions: Option<Obj>,
    input: Option<Obj>,
    open: Option<String>,
    /// Each chat's rows as far as the driver has read them, oldest first.
    tails: HashMap<String, Vec<String>>,
    /// Each session item's name at the last look.
    seen: HashMap<String, String>,
    /// What the driver just sent, to know it again when it shows.
    sent: Vec<(String, String)>,
    seq: u64,
}

impl Ui {
    pub fn new(
        a: A11y,
        app: Obj,
        cfg: Config,
        shared: Arc<Mutex<Shared>>,
        status: Arc<Mutex<Status>>,
        out: UnboundedSender<FromDriver>,
    ) -> Self {
        if let Some(me) = &cfg.me {
            status.lock().unwrap().me = Some(me.clone());
        }
        Self {
            a,
            app,
            cfg,
            shared,
            status,
            out,
            frame: None,
            sessions: None,
            input: None,
            open: None,
            tails: HashMap::new(),
            seen: HashMap::new(),
            sent: Vec::new(),
            seq: 0,
        }
    }

    /// Polls, reads private chats and carries out commands, until the
    /// commands' sender is gone.
    pub async fn run(
        mut self,
        mut commands: UnboundedReceiver<ToDriver>,
        mut pokes: Receiver<()>,
    ) -> Result<()> {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                c = commands.recv() => {
                    let Some(c) = c else { return Ok(()) };
                    self.command(c).await;
                }
                Some(()) = pokes.recv() => {
                    input::pause(150).await;
                    while pokes.try_recv().is_ok() {}
                    if let Err(e) = self.poll_group().await {
                        warn!("polling the group: {e:#}");
                    }
                }
                _ = tick.tick() => {
                    if let Err(e) = self.tick().await {
                        warn!("{e:#}");
                        self.frame = None;
                    }
                }
            }
        }
    }

    async fn tick(&mut self) -> Result<()> {
        let logged_in = self.locate().await.is_ok();
        let was = std::mem::replace(&mut self.status.lock().unwrap().logged_in, logged_in);
        if was != logged_in {
            info!(logged_in, "the client's state");
            let _ = self.out.send(FromDriver::State { logged_in });
        }
        if !logged_in {
            return Ok(());
        }
        let group = self.cfg.group.clone();
        let first = self.seen.is_empty();
        let mut read = false;
        for s in self.session_items().await? {
            let before = self.seen.insert(s.title.clone(), s.name.clone());
            if first || s.title == group {
                continue;
            }
            let changed = before.as_ref().is_some_and(|b| *b != s.name);
            if s.unread > 0 || changed || before.is_none() {
                self.read_private(&s.title, s.unread.max(1)).await?;
                read = true;
            }
        }
        if read {
            // Reading a chat changes its item; that isn't news.
            for s in self.session_items().await? {
                self.seen.insert(s.title, s.name);
            }
        }
        self.open_chat(&group).await?;
        self.poll_group().await
    }

    /// Finds the main window and its session list.
    async fn locate(&mut self) -> Result<()> {
        if let (Some(f), Some(s)) = (&self.frame, &self.sessions)
            && self.a.name(f).await.is_ok_and(|n| n == "微信")
            && self.a.name(s).await.is_ok_and(|n| n == "会话")
        {
            return Ok(());
        }
        self.frame = None;
        self.sessions = None;
        self.input = None;
        self.open = None;
        self.shared.lock().unwrap().list = None;
        let mut frame = None;
        for o in self.a.children(&self.app).await? {
            if self.a.role(&o).await.is_ok_and(|r| r == Role::Frame)
                && self.a.name(&o).await.is_ok_and(|n| n == "微信")
            {
                frame = Some(o);
            }
        }
        let frame = frame.ok_or_else(|| anyhow!("no main window: not logged in"))?;
        let sessions = self
            .find_showing(&frame, Role::List, "会话")
            .await
            .ok_or_else(|| anyhow!("no session list: not logged in"))?;
        info!("found the main window");
        // As large as the display, to show as many rows as it can.
        if let Err(e) = input::fill_screen("^微信$").await {
            warn!("resizing the main window: {e:#}");
        }
        self.frame = Some(frame);
        self.sessions = Some(sessions);
        Ok(())
    }

    /// A showing object under `root` with this role and name.
    async fn find_showing(&self, root: &Obj, role: Role, name: &str) -> Option<Obj> {
        let mut queue = VecDeque::from([root.clone()]);
        let mut seen = 0;
        while let Some(o) = queue.pop_front() {
            seen += 1;
            if seen > 5000 {
                return None;
            }
            let (Ok(r), Ok(n)) = (self.a.role(&o).await, self.a.name(&o).await) else {
                continue;
            };
            if r == role && n == name && self.a.extents(&o).await.is_ok_and(|e| !e.is_empty()) {
                return Some(o);
            }
            if let Ok(children) = self.a.children(&o).await {
                queue.extend(children);
            }
        }
        None
    }

    async fn session_items(&mut self) -> Result<Vec<Session>> {
        let list = self.sessions.clone().context("no session list")?;
        let mut out = Vec::new();
        for o in self.a.children(&list).await? {
            let (Ok(name), Ok(rect)) = (self.a.name(&o).await, self.a.extents(&o).await) else {
                continue;
            };
            let mut lines = name.split('\n');
            let title = lines.next().unwrap_or_default().to_owned();
            let mut unread = 0;
            let mut preview = String::new();
            for l in lines {
                let l = l.trim();
                if let Some(n) = l.strip_prefix('[').and_then(|l| l.strip_suffix("条]")) {
                    unread = n.parse().unwrap_or(1);
                } else if l.starts_with("[有人@我]") || l.is_empty() {
                } else if preview.is_empty() {
                    preview = l.to_owned();
                }
            }
            out.push(Session {
                title,
                name,
                unread,
                preview,
                rect,
            });
        }
        Ok(out)
    }

    /// The message list of the chat open now.
    async fn message_list(&mut self) -> Result<Obj> {
        let cached = self.shared.lock().unwrap().list.clone();
        if let Some(l) = cached
            && self.a.name(&l).await.is_ok_and(|n| n == "消息")
        {
            return Ok(l);
        }
        let frame = self.frame.clone().context("no main window")?;
        let l = self
            .find_showing(&frame, Role::List, "消息")
            .await
            .context("no message list")?;
        self.shared.lock().unwrap().list = Some(l.clone());
        Ok(l)
    }

    /// Where the chat's input is: a `[text]` named after the chat.
    async fn input_rect(&mut self, title: &str) -> Result<Rect> {
        if let Some(o) = &self.input
            && self.a.name(o).await.is_ok_and(|n| n == title)
        {
            let r = self.a.extents(o).await?;
            if !r.is_empty() {
                return Ok(r);
            }
        }
        let frame = self.frame.clone().context("no main window")?;
        let o = self
            .find_showing(&frame, Role::Text, title)
            .await
            .with_context(|| format!("no input for {title}"))?;
        let r = self.a.extents(&o).await?;
        self.input = Some(o);
        Ok(r)
    }

    async fn open_chat(&mut self, title: &str) -> Result<()> {
        if self.open.as_deref() == Some(title) {
            return Ok(());
        }
        let s = self
            .session_items()
            .await?
            .into_iter()
            .find(|s| s.title == title)
            .with_context(|| format!("{title} isn't in the session list"))?;
        let (x, y) = s.rect.center();
        input::click(x, y).await?;
        input::pause(700).await;
        self.open = Some(title.to_owned());
        {
            let mut sh = self.shared.lock().unwrap();
            sh.list = None;
            sh.open = Some(title.to_owned());
        }
        self.message_list().await?;
        debug!(title, "opened");
        Ok(())
    }

    async fn poll_group(&mut self) -> Result<()> {
        if self.open.as_deref() != Some(self.cfg.group.as_str()) {
            return Ok(());
        }
        let group = self.cfg.group.clone();
        self.poll(&group, true, None).await
    }

    /// Reads a private chat with `unread` new messages, and goes back to
    /// the group.
    async fn read_private(&mut self, title: &str, unread: u32) -> Result<()> {
        info!(title, unread, "a private chat has news");
        self.open_chat(title).await?;
        let r = self.poll(title, false, Some(unread)).await;
        let group = self.cfg.group.clone();
        self.open_chat(&group).await?;
        r
    }

    /// The open chat's new rows, as messages. A chat read for the first
    /// time has `unread` new rows, or none.
    async fn poll(&mut self, title: &str, group: bool, unread: Option<u32>) -> Result<()> {
        let list = self.message_list().await?;
        let rows = rows(&self.a, &list).await?;
        let texts: Vec<String> = rows.iter().map(|r| r.text.clone()).collect();
        let fresh = match self.tails.get_mut(title) {
            Some(tail) => {
                let m = overlap(tail, &texts);
                let mut fresh = texts.len() - m;
                if m == 0 && !tail.is_empty() {
                    // Nothing in common: the window moved, or more came
                    // than it shows. What was read lately isn't news.
                    warn!(title, "no rows in common with the last look");
                    let recent: HashSet<String> =
                        tail.iter().rev().take(50).map(|t| key(t)).collect();
                    fresh -= texts
                        .iter()
                        .take_while(|t| recent.contains(&key(t)))
                        .count();
                }
                // Rows change as they go: a file's state, a voice played.
                let start = tail.len() - m;
                tail.truncate(start);
                tail.extend(texts.iter().cloned());
                let drop = tail.len().saturating_sub(300);
                tail.drain(..drop);
                fresh
            }
            None => {
                let n = unread.unwrap_or(0) as usize;
                let mut seen = 0;
                let mut start = rows.len();
                while start > 0 && seen < n {
                    start -= 1;
                    if !is_time(&rows[start]) {
                        seen += 1;
                    }
                }
                self.tails.insert(title.to_owned(), texts.clone());
                rows.len() - start
            }
        };
        let fresh: Vec<&Row> = rows[rows.len() - fresh..]
            .iter()
            .filter(|r| !is_time(r))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        let drawn = take_drawn(&mut self.shared.lock().unwrap().drawn, title, &fresh);
        for (row, drawn) in fresh.into_iter().zip(drawn) {
            self.emit(title, group, row, drawn).await;
        }
        Ok(())
    }

    async fn emit(&mut self, chat: &str, group: bool, row: &Row, drawn: Option<Drawn>) {
        let (kind, text, quote) = parse(&row.text, row.rect.h, group, drawn.is_some());
        let sent = self
            .sent
            .iter()
            .position(|(c, t)| c == chat && row.text.ends_with(t.as_str()));
        let sent = sent.map(|i| self.sent.remove(i)).is_some();
        let mine = sent || drawn.as_ref().is_some_and(|d| d.mine);
        let me = self.status.lock().unwrap().me.clone();
        let sender = if mine {
            match (&drawn, &me) {
                (Some(d), None) => {
                    self.status.lock().unwrap().me = Some(d.sender.clone());
                    Some(d.sender.clone())
                }
                (_, Some(me)) => Some(me.clone()),
                (None, None) => None,
            }
        } else if !group {
            Some(chat.to_owned())
        } else if let Some(d) = &drawn {
            Some(d.sender.clone())
        } else {
            self.sender_from_preview(chat, &row.text).await
        };
        let mentions_me = me
            .as_ref()
            .is_some_and(|me| text.contains(&format!("@{me}\u{2005}")));
        self.seq += 1;
        let mut message = Message {
            seq: self.seq,
            chat: chat.to_owned(),
            group,
            sender,
            mine,
            kind,
            text,
            quote,
            mentions_me,
            attachment: None,
            at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64),
        };
        info!(seq = message.seq, mine, ?kind, "message");
        debug!(
            seq = message.seq,
            chat,
            sender = message.sender,
            text = message.text,
            "what it says"
        );
        if !mine && kind == Kind::Image {
            match self.copy_image(row, group).await {
                Ok(png) => {
                    message.attachment = Some(Attachment {
                        name: format!("{}.png", message.seq),
                        mime: "image/png".into(),
                        data: STANDARD.encode(png),
                    });
                }
                Err(e) => warn!("copying image {}: {e:#}", message.seq),
            }
        }
        if !mine && kind == Kind::File {
            let name = file_name(&message.text).unwrap_or_default().to_owned();
            let came = SystemTime::now();
            let home = self.cfg.home.clone();
            let out = self.out.clone();
            tokio::spawn(async move {
                match fetch_file(&home, &name, came).await {
                    Ok(data) => {
                        message.attachment = Some(Attachment {
                            name,
                            mime: "application/octet-stream".into(),
                            data: STANDARD.encode(data),
                        });
                    }
                    Err(e) => warn!("fetching file {name}: {e:#}"),
                }
                let _ = out.send(FromDriver::Message(message));
            });
            return;
        }
        let _ = self.out.send(FromDriver::Message(message));
    }

    /// The sender of the group's last message, from its session item's
    /// preview, when the preview is that message.
    async fn sender_from_preview(&mut self, chat: &str, text: &str) -> Option<String> {
        let items = self.session_items().await.ok()?;
        let s = items.into_iter().find(|s| s.title == chat)?;
        let (sender, said) = s.preview.split_once(": ")?;
        let first = text.lines().next().unwrap_or_default();
        (first.starts_with(said.trim_end_matches('…')) || said.starts_with('['))
            .then(|| sender.to_owned())
    }

    /// Right-clicks the image in `row`, 复制, and reads the clipboard. A
    /// row just come in may still be moving, so it's looked for again; an
    /// image still downloading has no menu, so it's tried for half a minute.
    async fn copy_image(&mut self, row: &Row, group: bool) -> Result<Vec<u8>> {
        let mut last = anyhow!("not tried");
        for wait in [1000, 2000, 3000, 4000, 5000, 5000, 5000, 5000] {
            input::pause(wait).await;
            let list = self.message_list().await?;
            let Some(now) = rows(&self.a, &list)
                .await?
                .into_iter()
                .filter(|r| r.text == row.text)
                .min_by_key(|r| (r.rect.y - row.rect.y).abs())
            else {
                bail!("the image's row is gone");
            };
            let top = now.rect.y + if group { 26 } else { AVATAR_OFFSET };
            let bottom = now.rect.y + now.rect.h - 8;
            input::right_click(now.rect.x + 110, (top + bottom) / 2).await?;
            let mut copy = None;
            for _ in 0..10 {
                input::pause(150).await;
                copy = self.menu_item("复制").await;
                if copy.is_some() {
                    break;
                }
            }
            let Some(copy) = copy else {
                input::key("Escape").await?;
                last = anyhow!("no 复制 in the menu");
                continue;
            };
            let (x, y) = copy.center();
            input::click(x, y).await?;
            input::pause(400).await;
            input::park().await?;
            match input::clipboard("image/png").await {
                Ok(png) => return Ok(png),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// A showing menu item called `name`, in a popup menu.
    async fn menu_item(&self, name: &str) -> Option<Rect> {
        for top in self.a.children(&self.app).await.ok()? {
            if !self.a.role(&top).await.is_ok_and(|r| r == Role::PopupMenu) {
                continue;
            }
            for item in self.a.children(&top).await.ok()? {
                if self.a.name(&item).await.is_ok_and(|n| n == name) {
                    let rect = self.a.extents(&item).await.ok()?;
                    if !rect.is_empty() {
                        return Some(rect);
                    }
                }
            }
        }
        None
    }

    /// A showing list item called `name` outside the main window: an
    /// entry of the @ popup.
    async fn popup_entry(&self, name: &str) -> Option<Rect> {
        for top in self.a.children(&self.app).await.ok()? {
            if Some(&top) == self.frame.as_ref() {
                continue;
            }
            if let Some(o) = self
                .a
                .find(&top, 300, |r, n| r == Role::ListItem && n == name)
                .await
            {
                let rect = self.a.extents(&o).await.ok()?;
                if !rect.is_empty() {
                    return Some(rect);
                }
            }
        }
        None
    }

    async fn command(&mut self, c: ToDriver) {
        let (id, r) = match c {
            ToDriver::Auth { .. } => return,
            ToDriver::SendText {
                id,
                chat,
                text,
                mentions,
            } => (id, self.send_text(&chat, &text, &mentions).await),
            ToDriver::SendImage { id, chat, png } => (id, self.send_image(&chat, &png).await),
            ToDriver::SendFile {
                id,
                chat,
                name,
                data,
            } => (id, self.send_file(&chat, &name, &data).await),
        };
        if let Err(e) = &r {
            warn!(id, "command: {e:#}");
            let _ = input::key("Escape").await;
        }
        let _ = self.out.send(FromDriver::Done {
            id,
            error: r.err().map(|e| format!("{e:#}")),
        });
    }

    /// Opens `chat` and clicks into its input.
    async fn focus(&mut self, chat: &str) -> Result<()> {
        self.locate().await?;
        if chat != self.cfg.group && !self.tails.contains_key(chat) {
            // Read it first, so what's there already isn't news.
            self.open_chat(chat).await?;
            self.poll(chat, false, Some(0)).await?;
        }
        self.open_chat(chat).await?;
        let (x, y) = self.input_rect(chat).await?.center();
        input::click(x, y).await?;
        input::pause(150).await;
        Ok(())
    }

    /// Sends what's in the input, reads it back, and goes back to the group.
    async fn finish(&mut self, chat: &str, sent: String) -> Result<()> {
        self.sent.push((chat.to_owned(), sent));
        let drop = self.sent.len().saturating_sub(20);
        self.sent.drain(..drop);
        input::key("Return").await?;
        input::pause(800).await;
        input::park().await?;
        let group = chat == self.cfg.group;
        self.poll(chat, group, None).await?;
        let home = self.cfg.group.clone();
        self.open_chat(&home).await
    }

    async fn send_text(&mut self, chat: &str, text: &str, mentions: &[String]) -> Result<()> {
        self.focus(chat).await?;
        for m in mentions {
            input::key("at").await?;
            input::pause(300).await;
            input::paste_text(m).await?;
            input::pause(600).await;
            if self.popup_entry(m).await.is_some() {
                input::key("Return").await?;
            } else {
                warn!(m, "no one by that name to mention");
                input::key("Escape").await?;
                input::paste_text(" ").await?;
            }
            input::pause(200).await;
        }
        input::paste_text(text).await?;
        input::pause(200).await;
        self.finish(chat, text.trim_end().to_owned()).await
    }

    async fn send_image(&mut self, chat: &str, png: &str) -> Result<()> {
        let bytes = STANDARD.decode(png).context("the image's base64")?;
        self.focus(chat).await?;
        input::set_clipboard("image/png", &bytes).await?;
        input::pause(150).await;
        input::key("ctrl+v").await?;
        input::pause(800).await;
        self.finish(chat, "图片".into()).await
    }

    async fn send_file(&mut self, chat: &str, name: &str, data: &str) -> Result<()> {
        let bytes = STANDARD.decode(data).context("the file's base64")?;
        let name: String = name
            .chars()
            .map(|c| if c == '/' || c == '\0' { '_' } else { c })
            .collect();
        let name = if name.is_empty() { "file".into() } else { name };
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let dir = self.cfg.home.join(".wxdesk/out").join(stamp.to_string());
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join(&name);
        tokio::fs::write(&path, bytes).await?;
        self.focus(chat).await?;
        input::paste_file(&path).await?;
        input::pause(800).await;
        self.finish(chat, format!("{name}\n")).await
    }
}

/// How many of `rows` the end of `tail` already has: the longest prefix
/// of `rows` that `tail` ends with, row by row as [`key`] has them. A row
/// in four may differ, as one does that is recalled.
fn overlap(tail: &[String], rows: &[String]) -> usize {
    (1..=rows.len().min(tail.len()))
        .rev()
        .find(|&m| {
            let misses = rows[..m]
                .iter()
                .zip(&tail[tail.len() - m..])
                .filter(|(a, b)| key(a) != key(b))
                .count();
            misses == 0 || (m >= 4 && misses * 4 <= m)
        })
        .unwrap_or(0)
}

/// What stays of a row's text as its state changes: a file's name without
/// its size, progress or whether it's downloaded; a voice without whether
/// it's been played.
fn key(text: &str) -> String {
    if let Some(name) = file_name(text) {
        return format!("文件\n{name}");
    }
    text.strip_suffix("未播放").unwrap_or(text).to_owned()
}

/// The name in a file's row: `文件\nname\nsize\nstate`, or, while it
/// downloads, `文件\n进度: 3%\nname`.
fn file_name(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("文件\n")?;
    rest.lines().find(|l| !l.starts_with("进度"))
}

/// A time between messages, such as `18:52` or `昨天 18:52`.
fn is_time(row: &Row) -> bool {
    row.rect.h < 50 && row.text.chars().count() <= 24 && row.text.contains(':')
}

/// What a row is, its text, and the message it quotes.
fn parse(text: &str, height: i32, group: bool, drawn: bool) -> (Kind, String, Option<Quote>) {
    let tall = height > if group { 100 } else { 80 };
    let kind = match text {
        "图片" if tall => Kind::Image,
        "动画表情" if tall => Kind::Sticker,
        t if t.starts_with("文件\n") => Kind::File,
        t if t.starts_with("语音") && t.contains("\"秒") => Kind::Voice,
        _ if !drawn && height < 50 => Kind::System,
        _ => Kind::Text,
    };
    if let Some((body, quoted)) = text.split_once("\n引用 ")
        && let Some((sender, said)) = quoted.split_once(" 的消息 : ")
    {
        return (
            kind,
            body.to_owned(),
            Some(Quote {
                sender: sender.to_owned(),
                text: said.to_owned(),
            }),
        );
    }
    if kind == Kind::File {
        return (kind, key(text), None);
    }
    (kind, text.to_owned(), None)
}

/// A received file, once WeChat has downloaded it: WeChat makes a file
/// read-only when it has all of it. A file of a name taken is saved as
/// `name(1).ext`; one modified since the message came is taken first, and
/// after a few seconds without one, the newest, which WeChat may have
/// linked to a file it had of the same content.
async fn fetch_file(home: &std::path::Path, name: &str, came: SystemTime) -> Result<Vec<u8>> {
    const LIMIT: u64 = 25 << 20;
    let since = came - Duration::from_secs(60);
    for i in 0..600 {
        let files = downloads(home, name);
        let newest = |since: SystemTime| {
            files
                .iter()
                .filter(|(m, _, _)| *m >= since)
                .max_by_key(|(m, _, _)| *m)
        };
        let pick = newest(since).or_else(|| (i >= 5).then(|| newest(UNIX_EPOCH)).flatten());
        if let Some((_, path, meta)) = pick {
            if meta.len() > LIMIT {
                bail!("{} bytes, more than {LIMIT}", meta.len());
            }
            if meta.permissions().readonly() && meta.len() > 0 {
                return Ok(tokio::fs::read(path).await?);
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!("not downloaded within ten minutes")
}

/// `~/xwechat_files/<account>/msg/file/<month>/<name>`, and `name(N)`.
fn downloads(home: &std::path::Path, name: &str) -> Vec<(SystemTime, PathBuf, std::fs::Metadata)> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) => (stem, format!(".{ext}")),
        None => (name, String::new()),
    };
    let ours = |f: &str| {
        f == name
            || f.strip_prefix(stem)
                .and_then(|r| r.strip_suffix(ext.as_str()))
                .and_then(|r| r.strip_prefix('('))
                .and_then(|r| r.strip_suffix(')'))
                .is_some_and(|n| n.parse::<u32>().is_ok())
    };
    let mut out = Vec::new();
    let Ok(accounts) = std::fs::read_dir(home.join("xwechat_files")) else {
        return out;
    };
    for account in accounts.flatten() {
        let Ok(months) = std::fs::read_dir(account.path().join("msg/file")) else {
            continue;
        };
        for month in months.flatten() {
            let Ok(files) = std::fs::read_dir(month.path()) else {
                continue;
            };
            for f in files.flatten() {
                if !f.file_name().to_str().is_some_and(ours) {
                    continue;
                }
                if let Ok(meta) = f.metadata()
                    && let Ok(m) = meta.modified()
                {
                    out.push((m, f.path(), meta));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn new_rows_follow_what_the_tail_ends_with() {
        assert_eq!(overlap(&s(&["a", "b", "c"]), &s(&["b", "c", "d"])), 2);
        assert_eq!(overlap(&s(&["a", "1"]), &s(&["1", "1"])), 1);
        assert_eq!(overlap(&s(&["a", "b"]), &s(&["a", "b"])), 2);
        assert_eq!(overlap(&s(&["a"]), &s(&["x", "y"])), 0);
    }

    #[test]
    fn a_file_row_changing_state_is_the_same_row() {
        let tail = s(&["a", "b", "文件\nx.pdf\n1K\n对方上传中"]);
        let rows = s(&["a", "b", "文件\nx.pdf\n1K\n未下载\n微信电脑版"]);
        assert_eq!(overlap(&tail, &rows), 3);
    }

    #[test]
    fn a_file_row_downloading_is_the_same_row() {
        let tail = s(&["a", "文件\n进度: 0%\nx.pdf"]);
        let rows = s(&["a", "文件\n进度: 3%\nx.pdf"]);
        assert_eq!(overlap(&tail, &rows), 2);
        assert_eq!(key("文件\nx.pdf\n3.2M"), key(&rows[1]));
    }

    #[test]
    fn a_recalled_row_in_four_is_still_read_before() {
        let tail = s(&["a", "b", "c", "d"]);
        let rows = s(&["a", "某人撤回了一条消息", "c", "d", "e"]);
        assert_eq!(overlap(&tail, &rows), 4);
    }

    fn row(text: &str) -> Row {
        Row {
            text: text.into(),
            rect: Rect::default(),
        }
    }

    fn drawn(text: &str, sender: &str) -> Drawn {
        Drawn {
            chat: Some("g".into()),
            text: text.into(),
            y: -100,
            sender: sender.into(),
            mine: false,
            at: Instant::now(),
        }
    }

    #[test]
    fn a_row_takes_its_newest_drawing() {
        // "ok" by A, read before and drawn again; then "ok" by B, new.
        let mut d = VecDeque::from([drawn("ok", "A"), drawn("hi", "C"), drawn("ok", "B")]);
        let r = row("ok");
        let got = take_drawn(&mut d, "g", &[&r]);
        assert_eq!(got[0].as_ref().unwrap().sender, "B");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].text, "hi");
    }

    #[test]
    fn a_row_takes_the_drawing_where_it_is() {
        // Two rows "hi", the new one at the bottom, drawn in another order.
        let mut d = VecDeque::from([drawn("hi", "A"), drawn("hi", "B")]);
        d[0].y = 500;
        d[1].y = 300;
        let mut r = row("hi");
        r.rect.y = 500;
        let got = take_drawn(&mut d, "g", &[&r]);
        assert_eq!(got[0].as_ref().unwrap().sender, "A");
    }

    #[test]
    fn a_burst_of_the_same_text_keeps_its_order() {
        let mut d = VecDeque::from([drawn("1", "A"), drawn("1", "B")]);
        let (r1, r2) = (row("1"), row("1"));
        let got = take_drawn(&mut d, "g", &[&r1, &r2]);
        assert_eq!(got[0].as_ref().unwrap().sender, "A");
        assert_eq!(got[1].as_ref().unwrap().sender, "B");
        assert!(d.is_empty());
    }

    #[test]
    fn quotes_split_off() {
        let (kind, text, quote) = parse(
            "ok\n引用 Alice 的消息 : [聊天记录]Group Chat History",
            94,
            true,
            true,
        );
        assert_eq!(kind, Kind::Text);
        assert_eq!(text, "ok");
        let q = quote.unwrap();
        assert_eq!(q.sender, "Alice");
        assert_eq!(q.text, "[聊天记录]Group Chat History");
    }
}
