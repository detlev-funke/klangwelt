//! Chat between the UI and the agent.
//!
//! The UI posts tasks; the agent picks them up with the MCP tool `chat_poll`
//! (long poll: the call waits until a task is available) and replies with
//! `chat_reply`. This way an agent can "listen" permanently.
//!
//! Picking up reserves a task for the client that polled ("claimed by …"); it
//! is not used up. If no `chat_reply` for it arrives within the lease, the task
//! is offered again – nothing is lost when a poller crashes or a second poller
//! snatches it. `chat_peek` reads new messages without picking anything up.
//!
//! A message like "Stopp" (or "Stopp #3") stops tasks: tasks that nobody
//! picked up yet are cancelled, tasks in progress get `cancel_requested` –
//! every tool response then carries `cancelRequested: [ids]`.

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
    /// Picked up (reserved or confirmed) by an agent.
    InArbeit,
    Erledigt,
    /// Stopped by the person before an agent picked it up.
    Abgebrochen,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
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
    /// Client that picked up the task (`client` of `chat_poll`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    /// The agent replied to the task (`chat_reply`) – it is no longer offered again.
    #[serde(skip_serializing_if = "is_false")]
    pub confirmed: bool,
    /// The person asked to stop this task.
    #[serde(skip_serializing_if = "is_false")]
    pub cancel_requested: bool,
    /// Stop message: the tasks it stops.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cancels: Vec<u64>,
    /// How often the task was offered again because its lease ran out.
    #[serde(skip_serializing_if = "is_zero")]
    pub redelivered: u32,
    /// Picked up at / last sign of work (reply with done=false).
    #[serde(skip)]
    touched: Option<Instant>,
    /// How long the reservation holds without a reply.
    #[serde(skip)]
    lease: Duration,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatView {
    pub messages: Vec<ChatMessage>,
    /// An agent is currently waiting in `chat_poll` or did so a moment ago.
    pub agent_listening: bool,
    /// An agent is connected via MCP (active session), listening or working –
    /// set by `AppState::chat_view`, the chat itself knows nothing about MCP sessions.
    pub agent_connected: bool,
    /// An agent is working: a tool call within the last minute or a task in progress.
    pub agent_working: bool,
    /// Tasks in progress (picked up and not done yet).
    pub working_on: Vec<u64>,
}

#[derive(Default)]
struct Inner {
    messages: Vec<ChatMessage>,
    next_id: u64,
    pollers: usize,
    last_poll: Option<Instant>,
    /// Last tool call apart from `chat_poll` (an agent at work).
    last_tool: Option<Instant>,
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

/// How long an agent still counts as "listening" after its last poll (the gap
/// between two polls of a listener: one model round, or a short task).
const LISTEN_GRACE: Duration = Duration::from_secs(30);
/// A tool call counts as "agent working" for this long.
pub const ACTIVE_WINDOW: Duration = Duration::from_secs(60);
/// A confirmed task (reply with done=false) counts as in progress for this long
/// after the last sign of work – long compositions take several minutes.
const WORK_GRACE: Duration = Duration::from_secs(10 * 60);
/// Default lease of a picked-up task until the first `chat_reply`.
pub const DEFAULT_LEASE: Duration = Duration::from_secs(60);
const MAX_MESSAGES: usize = 500;

/// Words that make a short message a stop request (all UI languages).
const STOP_WORDS: &[&str] = &[
    "stopp", "stop", "stoppen", "halt", "abbrechen", "abbruch", "cancel", "abort", "arrête", "arrete", "arrêter", "arreter",
    "annuler", "stoppa", "ferma", "basta", "annulla", "para", "parar", "detener", "cancelar", "zatrzymaj", "przestań", "przestan",
    "anuluj", "annuleer", "ophouden",
];
/// Words that may stand next to a stop word ("bitte stopp", "stop all").
const STOP_FILLER: &[&str] = &[
    "bitte", "sofort", "jetzt", "alles", "alle", "auftrag", "please", "now", "all", "task", "everything", "tout", "maintenant",
    "subito", "tutto", "ya", "todo", "teraz", "wszystko", "nu", "alstublieft", "graag",
];

/// Is `text` a stop request? `Some(ids)` with the task numbers it names
/// (`#3`, `3`), empty = all open tasks.
pub fn stop_request(text: &str) -> Option<Vec<u64>> {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '.' | '!' | '?' | ':' | '·' | '(' | ')' | '"' | '„' | '“'))
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() || words.len() > 5 {
        return None;
    }
    let mut ids = Vec::new();
    let mut stop = false;
    for w in words {
        if let Ok(n) = w.trim_start_matches('#').parse::<u64>() {
            ids.push(n);
        } else if STOP_WORDS.contains(&w) {
            stop = true;
        } else if !STOP_FILLER.contains(&w) {
            return None;
        }
    }
    stop.then_some(ids)
}

impl Chat {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push(g: &mut Inner, from: Author, text: String, status: Option<Status>, reply_to: Option<u64>) -> ChatMessage {
        g.next_id += 1;
        let m = ChatMessage {
            id: g.next_id,
            from,
            text,
            time: now_ms(),
            status,
            reply_to,
            kontext: None,
            claimed_by: None,
            confirmed: false,
            cancel_requested: false,
            cancels: Vec::new(),
            redelivered: 0,
            touched: None,
            lease: DEFAULT_LEASE,
        };
        g.messages.push(m.clone());
        if g.messages.len() > MAX_MESSAGES {
            let drop = g.messages.len() - MAX_MESSAGES;
            g.messages.drain(..drop);
        }
        m
    }

    pub fn view(&self) -> ChatView {
        let g = self.lock();
        let working_on = Self::in_progress(&g);
        ChatView {
            messages: g.messages.clone(),
            agent_listening: Self::is_listening(&g),
            agent_connected: false,
            agent_working: !working_on.is_empty() || Self::tool_recently(&g),
            working_on,
        }
    }

    fn is_listening(g: &Inner) -> bool {
        g.pollers > 0 || g.last_poll.map(|t| t.elapsed() < LISTEN_GRACE).unwrap_or(false)
    }

    fn tool_recently(g: &Inner) -> bool {
        g.last_tool.is_some_and(|t| t.elapsed() < ACTIVE_WINDOW)
    }

    /// Reservation of a picked-up task still holds (no reply yet).
    fn reserved(m: &ChatMessage) -> bool {
        m.touched.is_some_and(|t| t.elapsed() < m.lease)
    }

    /// Tasks in progress: reserved and not run out, or confirmed with a recent sign of work.
    fn in_progress(g: &Inner) -> Vec<u64> {
        g.messages
            .iter()
            .filter(|m| m.status == Some(Status::InArbeit))
            .filter(|m| if m.confirmed { m.touched.is_some_and(|t| t.elapsed() < WORK_GRACE) } else { Self::reserved(m) })
            .map(|m| m.id)
            .collect()
    }

    /// Task that `chat_poll` hands out: new, or reserved without a reply and the lease ran out.
    fn offered(m: &ChatMessage) -> bool {
        m.from == Author::User
            && match m.status {
                Some(Status::Neu) => true,
                Some(Status::InArbeit) => !m.confirmed && !Self::reserved(m),
                _ => false,
            }
    }

    /// Is an agent currently listening (without copying the messages)?
    pub fn listening(&self) -> bool {
        Self::is_listening(&self.lock())
    }

    /// Short key of the agent state (listening, working, tasks in progress) –
    /// changes when the indicator in the UI has to change.
    pub fn agent_key(&self) -> (bool, bool, Vec<u64>) {
        let g = self.lock();
        let working_on = Self::in_progress(&g);
        (Self::is_listening(&g), !working_on.is_empty() || Self::tool_recently(&g), working_on)
    }

    /// A tool call (apart from `chat_poll`): the agent is working.
    pub fn touch(&self) {
        self.lock().last_tool = Some(Instant::now());
    }

    /// New task from the UI; wakes waiting `chat_poll` calls.
    /// `kontext`: situation in the project at the time of sending (for the agent).
    /// A stop request ("Stopp", "Stopp #3") stops the tasks it names, or all open ones.
    pub fn send_user(&self, text: &str, kontext: Option<String>) -> Result<ChatMessage, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(tr!("Empty message"));
        }
        let mut g = self.lock();
        let cancels = match stop_request(text) {
            Some(ids) => Self::stop(&mut g, &ids),
            None => Vec::new(),
        };
        let m = Self::post(&mut g, text, kontext, cancels);
        drop(g);
        self.cond.notify_all();
        Ok(m)
    }

    /// The person stops task `id` (stop button at the task): `text` is shown
    /// in the chat and goes to the agent like a task.
    pub fn stop_task(&self, id: u64, text: &str) -> Result<ChatMessage, String> {
        let mut g = self.lock();
        if !g.messages.iter().any(|m| m.id == id && m.from == Author::User) {
            return Err(tr!("Task {id} doesn't exist", id = id));
        }
        let cancels = Self::stop(&mut g, &[id]);
        let m = Self::post(&mut g, text.trim(), None, cancels);
        drop(g);
        self.cond.notify_all();
        Ok(m)
    }

    fn post(g: &mut Inner, text: &str, kontext: Option<String>, cancels: Vec<u64>) -> ChatMessage {
        let mut m = Self::push(g, Author::User, text.to_string(), Some(Status::Neu), None);
        if let Some(last) = g.messages.last_mut() {
            last.kontext = kontext.clone();
            last.cancels = cancels.clone();
        }
        m.kontext = kontext;
        m.cancels = cancels;
        m
    }

    /// Stops tasks (`ids`, empty = all open ones): not picked up yet → cancelled,
    /// in progress → `cancel_requested`. Returns the ids it stopped.
    fn stop(g: &mut Inner, ids: &[u64]) -> Vec<u64> {
        let mut stopped = Vec::new();
        for m in g.messages.iter_mut().filter(|m| m.from == Author::User && (ids.is_empty() || ids.contains(&m.id))) {
            match m.status {
                Some(Status::Neu) => m.status = Some(Status::Abgebrochen),
                Some(Status::InArbeit) => m.cancel_requested = true,
                _ => continue,
            }
            stopped.push(m.id);
        }
        stopped
    }

    /// Tasks in progress the person asked to stop.
    pub fn cancel_requested(&self) -> Vec<u64> {
        let g = self.lock();
        g.messages.iter().filter(|m| m.cancel_requested && m.status == Some(Status::InArbeit)).map(|m| m.id).collect()
    }

    /// Long poll: replies immediately if a task is available or arrives, otherwise
    /// after `timeout` with an empty list. Tasks picked up are reserved for
    /// `client` for `lease` – without a `chat_reply` within it they are offered
    /// again. `on_change` reports state changes to the UI (agent listening /
    /// tasks picked up). `gone` checks whether the caller has given up the
    /// connection (client timeout) – then nothing is picked up, so that no
    /// task is lost to a dead connection.
    pub fn poll(&self, client: &str, lease: Duration, timeout: Duration, on_change: impl Fn(), gone: impl Fn() -> bool) -> PollResult {
        let mut g = self.lock();
        g.pollers += 1;
        g.last_poll = Some(Instant::now());
        let abandoned_before = g.abandoned_after.take();
        drop(g);
        on_change();

        let start = Instant::now();
        let has_new = |s: &mut Inner| s.messages.iter().any(Self::offered);
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
            // Wait in slices to check the connection (and leases running out) in between.
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
            for m in g.messages.iter_mut().filter(|m| Self::offered(m)) {
                if m.claimed_by.is_some() {
                    m.redelivered += 1;
                }
                m.status = Some(Status::InArbeit);
                m.claimed_by = Some(client.to_string());
                m.touched = Some(Instant::now());
                m.lease = lease;
                taken.push(m.clone());
            }
        }
        g.pollers = g.pollers.saturating_sub(1);
        g.last_poll = Some(Instant::now());
        drop(g);
        on_change();
        PollResult { tasks: taken, abandoned_before }
    }

    /// Messages after `since_id` (tasks and replies) – without picking anything up.
    pub fn peek(&self, since_id: u64) -> Vec<ChatMessage> {
        let g = self.lock();
        g.messages.iter().filter(|m| m.id > since_id).cloned().collect()
    }

    /// Reply from the agent. A reply to a task confirms it (no longer offered
    /// again); with `done` the task (or, without `reply_to`, all tasks in
    /// progress – of `client`, if given) is completed.
    pub fn reply(&self, text: &str, reply_to: Option<u64>, done: bool, client: Option<&str>) -> Result<ChatMessage, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(tr!("Empty reply"));
        }
        let mut g = self.lock();
        if let Some(id) = reply_to {
            let Some(m) = g.messages.iter_mut().find(|m| m.id == id && m.from == Author::User) else {
                return Err(tr!("Task {id} doesn't exist", id = id));
            };
            if matches!(m.status, Some(Status::Neu) | Some(Status::InArbeit)) {
                m.confirmed = true;
                m.touched = Some(Instant::now());
                if m.claimed_by.is_none() || client.is_some() {
                    m.claimed_by = Some(client.unwrap_or("agent").to_string());
                }
                m.status = Some(if done { Status::Erledigt } else { Status::InArbeit });
                if done {
                    m.cancel_requested = false;
                }
            }
        } else if done {
            for m in g.messages.iter_mut().filter(|m| m.from == Author::User && m.status == Some(Status::InArbeit)) {
                if client.is_none_or(|c| m.claimed_by.as_deref() == Some(c)) {
                    m.status = Some(Status::Erledigt);
                    m.cancel_requested = false;
                }
            }
        }
        g.last_tool = Some(Instant::now());
        Ok(Self::push(&mut g, Author::Agent, text.to_string(), None, reply_to))
    }

    /// Latest messages as context for the agent.
    pub fn recent(&self, n: usize) -> Vec<ChatMessage> {
        let g = self.lock();
        g.messages.iter().rev().take(n).rev().cloned().collect()
    }

    /// Highest message id so far (for `chat_peek` with `sinceId`).
    pub fn last_id(&self) -> u64 {
        self.lock().messages.last().map(|m| m.id).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn poll(c: &Chat, client: &str, lease: Duration) -> Vec<ChatMessage> {
        c.poll(client, lease, Duration::from_millis(50), || {}, || false).tasks
    }

    #[test]
    fn poll_waits_for_a_task_and_reply_finishes_it() {
        let chat = Arc::new(Chat::default());
        let c2 = chat.clone();
        let waiter = std::thread::spawn(move || c2.poll("zuhoerer", DEFAULT_LEASE, Duration::from_secs(5), || {}, || false).tasks);
        // wait until the poll is really listening (can take a while under load)
        let t0 = Instant::now();
        while !chat.view().agent_listening && t0.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(chat.view().agent_listening);
        let started = Instant::now();
        let task = chat.send_user("Mach die Kick tiefer", Some("Spuren: hörbar Bass (3)".into())).unwrap();
        let got = waiter.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2), "the poll must wake up at once");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "Mach die Kick tiefer");
        let v = chat.view();
        assert_eq!(v.messages[0].status, Some(Status::InArbeit));
        assert_eq!(v.messages[0].claimed_by.as_deref(), Some("zuhoerer"));
        assert_eq!(v.working_on, vec![task.id]);
        assert!(v.agent_working);

        chat.reply("Erledigt: Grundton jetzt E1", Some(task.id), true, None).unwrap();
        let v = chat.view();
        assert_eq!(v.messages[0].status, Some(Status::Erledigt));
        assert_eq!(v.messages[1].from, Author::Agent);
        assert_eq!(v.messages[1].reply_to, Some(task.id));
        assert!(v.working_on.is_empty());
    }

    #[test]
    fn poll_times_out_empty_and_reserved_tasks_are_not_offered_twice() {
        let chat = Chat::default();
        assert!(poll(&chat, "a", DEFAULT_LEASE).is_empty());
        chat.send_user("eins", None).unwrap();
        chat.send_user("zwei", None).unwrap();
        assert_eq!(poll(&chat, "a", DEFAULT_LEASE).len(), 2);
        assert!(poll(&chat, "b", DEFAULT_LEASE).is_empty(), "reserved for a");
        assert!(chat.send_user("   ", None).is_err());
        assert!(chat.reply("x", Some(999), true, None).is_err());
    }

    #[test]
    fn lease_runs_out_without_reply_and_the_task_is_offered_again() {
        let chat = Chat::default();
        let t = chat.send_user("Bass wärmer", None).unwrap();
        let lease = Duration::from_millis(80);
        assert_eq!(poll(&chat, "test", lease).len(), 1);
        assert!(poll(&chat, "zuhoerer", lease).is_empty());
        std::thread::sleep(Duration::from_millis(120));
        let again = poll(&chat, "zuhoerer", lease);
        assert_eq!(again.len(), 1, "offered again after the lease");
        assert_eq!(again[0].id, t.id);
        assert_eq!(again[0].redelivered, 1);
        assert_eq!(again[0].claimed_by.as_deref(), Some("zuhoerer"));
        // a reply confirms it: never offered again, even after the lease
        chat.reply("Bin dran", Some(t.id), false, Some("zuhoerer")).unwrap();
        std::thread::sleep(Duration::from_millis(120));
        assert!(poll(&chat, "other", lease).is_empty());
        assert_eq!(chat.view().working_on, vec![t.id]);
    }

    #[test]
    fn peek_reads_without_picking_up() {
        let chat = Chat::default();
        let a = chat.send_user("eins", None).unwrap();
        let since = chat.last_id();
        let b = chat.send_user("zwei", None).unwrap();
        let all = chat.peek(0);
        assert_eq!(all.len(), 2);
        let new = chat.peek(since);
        assert_eq!(new.iter().map(|m| m.id).collect::<Vec<_>>(), vec![b.id]);
        // still offered to the listener
        assert_eq!(poll(&chat, "zuhoerer", DEFAULT_LEASE).iter().map(|m| m.id).collect::<Vec<_>>(), vec![a.id, b.id]);
    }

    #[test]
    fn stop_cancels_new_tasks_and_flags_running_ones() {
        let chat = Chat::default();
        let running = chat.send_user("Jazz-Stück bauen", None).unwrap();
        poll(&chat, "arbeiter", DEFAULT_LEASE);
        let waiting = chat.send_user("Danach Tempo 90", None).unwrap();
        let stop = chat.send_user("Stopp!", None).unwrap();
        assert_eq!(stop.cancels, vec![running.id, waiting.id]);
        assert_eq!(chat.cancel_requested(), vec![running.id]);
        let v = chat.view();
        assert_eq!(v.messages[1].status, Some(Status::Abgebrochen));
        // the stop message itself goes to the listener; the cancelled task does not
        let got = poll(&chat, "zuhoerer", DEFAULT_LEASE);
        assert_eq!(got.iter().map(|m| m.id).collect::<Vec<_>>(), vec![stop.id]);
        // done → no longer reported
        chat.reply("Abgebrochen", Some(running.id), true, None).unwrap();
        assert!(chat.cancel_requested().is_empty());
    }

    #[test]
    fn stop_names_one_task() {
        let chat = Chat::default();
        let a = chat.send_user("Bass", None).unwrap();
        let b = chat.send_user("Schlagzeug", None).unwrap();
        poll(&chat, "x", DEFAULT_LEASE);
        let s = chat.send_user(&format!("stopp #{}", b.id), None).unwrap();
        assert_eq!(s.cancels, vec![b.id]);
        assert_eq!(chat.cancel_requested(), vec![b.id]);
        let s2 = chat.stop_task(a.id, "Stopp #1").unwrap();
        assert_eq!(s2.cancels, vec![a.id]);
        assert_eq!(chat.cancel_requested(), vec![a.id, b.id]);
        assert!(chat.stop_task(999, "Stopp").is_err());
    }

    #[test]
    fn stop_words_only_in_short_messages() {
        assert_eq!(stop_request("Stopp"), Some(vec![]));
        assert_eq!(stop_request("stop #4"), Some(vec![4]));
        assert_eq!(stop_request("Bitte sofort stoppen!"), Some(vec![]));
        assert_eq!(stop_request("Arrête 3"), Some(vec![3]));
        assert_eq!(stop_request("Stopp die Hi-Hat bei Takt 4"), None);
        assert_eq!(stop_request("Mach einen Stop-and-go-Rhythmus"), None);
        assert_eq!(stop_request("Tempo 90"), None);
    }

    #[test]
    fn tool_calls_count_as_working() {
        let chat = Chat::default();
        assert!(!chat.view().agent_working);
        chat.touch();
        let v = chat.view();
        assert!(v.agent_working);
        assert!(v.working_on.is_empty());
        assert!(!v.agent_listening);
    }

    #[test]
    fn abandoned_poll_leaves_tasks_for_the_next_one() {
        // The client gave up (timeout): the task must not be lost.
        let chat = Arc::new(Chat::default());
        let c2 = chat.clone();
        let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let g2 = gone.clone();
        let waiter = std::thread::spawn(move || {
            c2.poll("a", DEFAULT_LEASE, Duration::from_secs(10), || {}, || g2.load(std::sync::atomic::Ordering::SeqCst))
        });
        std::thread::sleep(Duration::from_millis(100));
        gone.store(true, std::sync::atomic::Ordering::SeqCst);
        let r = waiter.join().unwrap();
        assert!(r.tasks.is_empty());
        chat.send_user("Bass wärmer", None).unwrap();
        let next = chat.poll("a", DEFAULT_LEASE, Duration::from_millis(50), || {}, || false);
        assert_eq!(next.tasks.len(), 1, "the task arrives with the next poll");
        assert!(next.abandoned_before.is_some(), "the abort is reported");
    }
}
