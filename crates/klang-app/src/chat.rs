//! Chat between the UI and the agent.
//!
//! The UI posts tasks; the agent picks them up with the MCP tool `chat_poll`
//! (long poll: the call waits until a task is available) and replies with
//! `chat_reply`. This way an agent can "listen" permanently.

use klang_i18n::tr;
use serde::Serialize;
use std::sync::{Condvar, Mutex, MutexGuard};
use crate::clock::Instant;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Author {
    User,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Waiting to be picked up.
    Neu,
    /// Picked up by the agent.
    InArbeit,
    Erledigt,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: u64,
    pub from: Author,
    pub text: String,
    /// Unix time in milliseconds.
    pub time: u64,
    /// Only for tasks (messages from the user).
    pub status: Option<Status>,
    pub reply_to: Option<u64>,
    /// Situation in the project at the time of sending (audible/muted/solo
    /// tracks, selected time range) – goes to the agent along with the task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kontext: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatView {
    pub messages: Vec<ChatMessage>,
    /// An agent is currently waiting in `chat_poll` or did so a moment ago.
    pub agent_listening: bool,
    /// An agent is connected via MCP (active session) or is listening –
    /// set by `AppState::chat_view`, the chat itself knows nothing about it.
    pub agent_connected: bool,
}

#[derive(Default)]
struct Inner {
    messages: Vec<ChatMessage>,
    next_id: u64,
    pollers: usize,
    last_poll: Option<Instant>,
    /// A poll was given up by the client after this many seconds (reported at
    /// the next poll).
    abandoned_after: Option<f64>,
}

/// Result of a long poll.
pub struct PollResult {
    pub tasks: Vec<ChatMessage>,
    /// An earlier poll was aborted by the client after this many seconds.
    pub abandoned_before: Option<f64>,
}

#[derive(Default)]
pub struct Chat {
    inner: Mutex<Inner>,
    cond: Condvar,
}

fn now_ms() -> u64 {
    crate::clock::now_ms()
}

/// How long an agent still counts as "listening" after its last poll.
const LISTEN_GRACE: Duration = Duration::from_secs(90);
const MAX_MESSAGES: usize = 500;

impl Chat {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push(g: &mut Inner, from: Author, text: String, status: Option<Status>, reply_to: Option<u64>) -> ChatMessage {
        g.next_id += 1;
        let m = ChatMessage { id: g.next_id, from, text, time: now_ms(), status, reply_to, kontext: None };
        g.messages.push(m.clone());
        if g.messages.len() > MAX_MESSAGES {
            let drop = g.messages.len() - MAX_MESSAGES;
            g.messages.drain(..drop);
        }
        m
    }

    pub fn view(&self) -> ChatView {
        let g = self.lock();
        ChatView { messages: g.messages.clone(), agent_listening: Self::is_listening(&g), agent_connected: false }
    }

    fn is_listening(g: &Inner) -> bool {
        g.pollers > 0 || g.last_poll.map(|t| t.elapsed() < LISTEN_GRACE).unwrap_or(false)
    }

    /// Is an agent currently listening (without copying the messages)?
    pub fn listening(&self) -> bool {
        Self::is_listening(&self.lock())
    }

    /// New task from the UI; wakes waiting `chat_poll` calls.
    /// `kontext`: situation in the project at the time of sending (for the agent).
    pub fn send_user(&self, text: &str, kontext: Option<String>) -> Result<ChatMessage, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(tr!("Empty message"));
        }
        let mut g = self.lock();
        let mut m = Self::push(&mut g, Author::User, text.to_string(), Some(Status::Neu), None);
        if let Some(last) = g.messages.last_mut() {
            last.kontext = kontext.clone();
        }
        m.kontext = kontext;
        drop(g);
        self.cond.notify_all();
        Ok(m)
    }

    /// Long poll: replies immediately if a task is available or arrives, otherwise
    /// after `timeout` with an empty list. Tasks picked up are then set to
    /// "in progress". `on_change` reports state changes to the UI
    /// (agent listening / tasks picked up). `gone` checks whether the caller
    /// has given up the connection (client timeout) – then nothing is picked up,
    /// so that no task is lost to a dead connection.
    pub fn poll(&self, timeout: Duration, on_change: impl Fn(), gone: impl Fn() -> bool) -> PollResult {
        let mut g = self.lock();
        g.pollers += 1;
        g.last_poll = Some(Instant::now());
        let abandoned_before = g.abandoned_after.take();
        drop(g);
        on_change();

        let start = Instant::now();
        let has_new = |s: &mut Inner| s.messages.iter().any(|m| m.status == Some(Status::Neu));
        let mut g = self.lock();
        let mut abandoned = false;
        loop {
            if has_new(&mut g) {
                break;
            }
            let left = timeout.saturating_sub(start.elapsed());
            if left.is_zero() {
                break;
            }
            // In the browser there is only one thread, the core must not wait:
            // there the web worker keeps the reply open and asks again as soon as
            // a task arrives (ui/src/web/worker.ts, "waiting chat_poll").
            if cfg!(target_arch = "wasm32") {
                break;
            }
            // Wait in slices to check the connection in between.
            let (next, _) = self
                .cond
                .wait_timeout_while(g, left.min(Duration::from_secs(1)), |s| !has_new(s))
                .unwrap_or_else(|e| e.into_inner());
            g = next;
            if !has_new(&mut g) && gone() {
                abandoned = true;
                break;
            }
        }
        if !abandoned && gone() {
            abandoned = true;
        }
        let mut taken = Vec::new();
        if abandoned {
            g.abandoned_after = Some(start.elapsed().as_secs_f64());
        } else {
            for m in g.messages.iter_mut().filter(|m| m.status == Some(Status::Neu)) {
                m.status = Some(Status::InArbeit);
                taken.push(m.clone());
            }
        }
        g.pollers = g.pollers.saturating_sub(1);
        g.last_poll = Some(Instant::now());
        drop(g);
        on_change();
        PollResult { tasks: taken, abandoned_before }
    }

    /// Reply from the agent. With `done` the task (or all open ones) is completed.
    pub fn reply(&self, text: &str, reply_to: Option<u64>, done: bool) -> Result<ChatMessage, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(tr!("Empty reply"));
        }
        let mut g = self.lock();
        if let Some(id) = reply_to {
            if !g.messages.iter().any(|m| m.id == id && m.from == Author::User) {
                return Err(tr!("Task {id} doesn't exist", id = id));
            }
        }
        if done {
            for m in g.messages.iter_mut() {
                let target = match reply_to {
                    Some(id) => m.id == id,
                    None => m.status == Some(Status::InArbeit),
                };
                if target && m.from == Author::User {
                    m.status = Some(Status::Erledigt);
                }
            }
        }
        g.last_poll = Some(Instant::now());
        Ok(Self::push(&mut g, Author::Agent, text.to_string(), None, reply_to))
    }

    /// Latest messages as context for the agent.
    pub fn recent(&self, n: usize) -> Vec<ChatMessage> {
        let g = self.lock();
        g.messages.iter().rev().take(n).rev().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn poll_waits_for_a_task_and_reply_finishes_it() {
        let chat = Arc::new(Chat::default());
        let c2 = chat.clone();
        let waiter = std::thread::spawn(move || c2.poll(Duration::from_secs(5), || {}, || false).tasks);
        // wait until the poll is really listening (can take a while under load)
        let t0 = Instant::now();
        while !chat.view().agent_listening && t0.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(chat.view().agent_listening);
        let started = Instant::now();
        let task = chat.send_user("Mach die Kick tiefer", Some("Spuren: hörbar Bass (3)".into())).unwrap();
        let got = waiter.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2), "Poll muss sofort aufwachen");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "Mach die Kick tiefer");
        assert_eq!(chat.view().messages[0].status, Some(Status::InArbeit));

        chat.reply("Erledigt: Grundton jetzt E1", Some(task.id), true).unwrap();
        let v = chat.view();
        assert_eq!(v.messages[0].status, Some(Status::Erledigt));
        assert_eq!(v.messages[1].from, Author::Agent);
        assert_eq!(v.messages[1].reply_to, Some(task.id));
    }

    #[test]
    fn poll_times_out_empty_and_tasks_are_taken_once() {
        let chat = Chat::default();
        let poll = |c: &Chat| c.poll(Duration::from_millis(50), || {}, || false).tasks;
        assert!(poll(&chat).is_empty());
        chat.send_user("eins", None).unwrap();
        chat.send_user("zwei", None).unwrap();
        assert_eq!(poll(&chat).len(), 2);
        assert!(poll(&chat).is_empty());
        assert!(chat.send_user("   ", None).is_err());
        assert!(chat.reply("x", Some(999), true).is_err());
    }

    #[test]
    fn abandoned_poll_leaves_tasks_for_the_next_one() {
        // The client gave up (timeout): the task must not be lost.
        let chat = Arc::new(Chat::default());
        let c2 = chat.clone();
        let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let g2 = gone.clone();
        let waiter = std::thread::spawn(move || c2.poll(Duration::from_secs(10), || {}, || g2.load(std::sync::atomic::Ordering::SeqCst)));
        std::thread::sleep(Duration::from_millis(100));
        gone.store(true, std::sync::atomic::Ordering::SeqCst);
        let r = waiter.join().unwrap();
        assert!(r.tasks.is_empty());
        chat.send_user("Bass wärmer", None).unwrap();
        let next = chat.poll(Duration::from_millis(50), || {}, || false);
        assert_eq!(next.tasks.len(), 1, "Auftrag kommt beim nächsten Poll an");
        assert!(next.abandoned_before.is_some(), "Abbruch wird gemeldet");
    }
}
