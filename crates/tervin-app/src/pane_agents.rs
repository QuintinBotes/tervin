//! Agents someone started themselves, in a pane.
//!
//! Tervin can launch an agent and drive it over a protocol — that is what the
//! runtime adapters do. But most people already have a habit: open a terminal, type
//! `claude`. Before this, such a session was invisible to the workspace. Its prompts
//! were not in prompt history, its edits were not in Review, and the Threads list
//! showed nothing, even though the interesting work was happening right there.
//!
//! ## How it works
//!
//! Claude Code announces its own lifecycle with `OSC 777;notify;warp://cli-agent`
//! and a JSON body — verified against 2.1.220 by capturing a real PTY. On the turn
//! that ends, the body carries `transcript_path`, which points at the session's
//! JSONL log. So Tervin reads the sequence agents already emit and then reads the
//! file they already write; nothing has to be configured, and no agent has to know
//! Tervin exists.
//!
//! ## What it deliberately does not claim
//!
//! An observed session is **read-only**. Tervin has no channel to a process it did
//! not spawn: it cannot send a prompt, cannot answer a permission request, and
//! cannot cancel a turn. So no [`ThreadRuntime`] is registered for it, and the UI
//! shows it without a composer rather than with one that silently does nothing.
//!
//! [`ThreadRuntime`]: crate::state::ThreadRuntime
//!
//! The tier is Enhanced CLI, which is the honest reading of the spec's own
//! definition — "the terminal remains authoritative and Tervin extracts only what
//! is reliable". Structured would imply a protocol that is not there.

use agent_runtime::claude::transcript::TranscriptReader;
use block_engine::Store;
use parking_lot::Mutex;
use std::collections::HashMap;
use terminal_core::{AgentActivity, AgentEvent};
use tervin_core::thread::Thread;
use tervin_core::{
    AgentIdentity, EventPayload, Link, PaneId, TervinEvent, ThreadId, ThreadState, Tier,
};

/// Sessions being observed, keyed by the agent's own session id.
///
/// Keyed by session id rather than pane so that a session survives being moved, and
/// so two agents in two panes never merge into one Thread.
#[derive(Default)]
pub struct PaneAgents {
    sessions: Mutex<HashMap<String, Observed>>,
}

struct Observed {
    thread_id: ThreadId,
    pane_id: PaneId,
    identity: AgentIdentity,
    /// Opened on the first `stop`, which is when the path is announced.
    ///
    /// Started from the end: a `claude --resume` reattaches to a transcript that may
    /// hold weeks of conversation, and replaying it would fill the timeline with rows
    /// that look like they just happened.
    reader: Option<TranscriptReader>,
    /// Prompts seen, used for the Thread's title and to tell a fresh session from a
    /// resumed one.
    prompts: usize,
}

/// What the caller should do with one notification.
pub struct Observation {
    /// Events to persist and forward to the UI, in order.
    pub events: Vec<TervinEvent>,
    /// A Thread to upsert, when this notification created or changed one.
    pub thread: Option<Thread>,
    /// Set when the Thread's state changed, so the UI can move it in the Deck.
    pub state: Option<(ThreadId, ThreadState)>,
}

impl PaneAgents {
    pub fn new() -> Self {
        Self::default()
    }

    /// The Thread an observed session maps to, if it is still being followed.
    pub fn thread_for(&self, session_id: &str) -> Option<ThreadId> {
        self.sessions
            .lock()
            .get(session_id)
            .map(|s| s.thread_id.clone())
    }

    /// Which pane a Thread is being observed in.
    pub fn pane_for(&self, thread_id: &ThreadId) -> Option<PaneId> {
        self.sessions
            .lock()
            .values()
            .find(|s| &s.thread_id == thread_id)
            .map(|s| s.pane_id.clone())
    }

    /// Forget the sessions belonging to a pane that has closed.
    ///
    /// The Threads stay on disk — the conversation happened, and it is exactly what
    /// prompt history is for. Only the live mapping goes.
    pub fn forget_pane(&self, pane_id: &PaneId) -> Vec<ThreadId> {
        let mut sessions = self.sessions.lock();
        let gone: Vec<String> = sessions
            .iter()
            .filter(|(_, s)| &s.pane_id == pane_id)
            .map(|(k, _)| k.clone())
            .collect();
        gone.iter()
            .filter_map(|k| sessions.remove(k))
            .map(|s| s.thread_id)
            .collect()
    }

    /// Take in one notification and say what changed.
    ///
    /// `store` is read to decide whether a `stop` for an unknown session should adopt
    /// an existing Thread — a restart of Tervin mid-session must not orphan it.
    pub fn observe(
        &self,
        activity: &AgentActivity,
        pane_id: &PaneId,
        store: &Store,
    ) -> Observation {
        let mut sessions = self.sessions.lock();

        // Any event can be the first one seen: Tervin may have started after the
        // agent, or the session_start notification may have been missed. Treating
        // only session_start as the opener would drop the whole session.
        let known = sessions.contains_key(&activity.session_id);
        let mut created = None;
        if !known {
            let (session, thread) = Observed::open(activity, pane_id, store);
            created = Some(thread);
            sessions.insert(activity.session_id.clone(), session);
        }

        // Either the key was already present, or the branch above inserted it under
        // this same id. `sessions` is held under one lock guard across both, so no
        // other observer can remove it in between.
        #[allow(
            clippy::expect_used,
            reason = "known or inserted above, under one lock"
        )]
        let session = sessions
            .get_mut(&activity.session_id)
            .expect("known or just inserted");
        // A session can move: `claude` resumed in a different pane keeps its id.
        session.pane_id = pane_id.clone();

        let mut out = Observation {
            events: Vec::new(),
            thread: created.clone(),
            state: None,
        };

        let base = |payload: EventPayload, session: &Observed| {
            let mut event = TervinEvent::new(
                session.identity.clone(),
                String::new(),
                // Replaced below; `summary` is set per-event because each one reads
                // differently in a timeline.
                payload,
            );
            event.thread_id = Some(session.thread_id.clone());
            event.project = activity.project.clone();
            event.cwd = activity.cwd.clone();
            event.links = vec![Link::Pane {
                pane_id: session.pane_id.clone(),
            }];
            event
        };

        if created.is_some() {
            let mut event = base(
                EventPayload::ThreadStarted {
                    tier: Tier::EnhancedCli,
                    task_title: None,
                    // The agent's session id really is a resume handle: this is what
                    // `claude --resume` takes.
                    resume_id: Some(activity.session_id.clone()),
                },
                session,
            );
            event.summary = format!(
                "{} is running in this pane — Tervin is watching, not driving",
                session.identity.display_name
            );
            out.events.push(event);
            out.state = Some((session.thread_id.clone(), ThreadState::Idle));
        }

        match &activity.event {
            AgentEvent::SessionStart => {
                // Handled by the creation branch above. Arriving for a session
                // already known means the agent restarted in place; there is
                // nothing new to record.
            }

            AgentEvent::PromptSubmit => {
                if let Some(text) = &activity.query {
                    session.prompts += 1;
                    let mut event =
                        base(EventPayload::UserPrompted { text: text.clone() }, session);
                    event.summary = first_line(text);
                    out.events.push(event);

                    // The first prompt names the Thread. A title taken from the
                    // prompt is what makes the Threads list readable, and the agent
                    // never sends one.
                    if session.prompts == 1 {
                        let mut thread = out
                            .thread
                            .take()
                            .unwrap_or_else(|| session.thread(activity));
                        thread.task_title = first_line(text);
                        out.thread = Some(thread);
                    }
                    out.state = Some((session.thread_id.clone(), ThreadState::Understanding));
                }
            }

            AgentEvent::Stop => {
                // The turn ended, so the transcript now holds everything the agent
                // said and did. This is where an observed session stops being a
                // status line and becomes a readable Thread.
                if session.reader.is_none() {
                    if let Some(path) = &activity.transcript_path {
                        // From the start when we saw this session begin, from the end
                        // otherwise: a resumed session's log predates us.
                        session.reader = Some(if session.prompts > 0 {
                            TranscriptReader::new(path)
                        } else {
                            TranscriptReader::from_end(path)
                        });
                    }
                }

                if let Some(reader) = &mut session.reader {
                    match reader.read_new() {
                        Ok(entries) => {
                            for entry in entries {
                                // The prompt already arrived over OSC, and recording
                                // it twice would double every row in prompt history.
                                if matches!(entry.payload, EventPayload::UserPrompted { .. }) {
                                    continue;
                                }
                                let summary = summarise(&entry.payload);
                                let mut event = base(entry.payload, session);
                                event.summary = summary;
                                out.events.push(event);
                            }
                        }
                        Err(e) => {
                            // The path came from the agent and may be gone, or
                            // unreadable. Worth one line, not a failed Thread.
                            tracing::debug!(
                                "could not read transcript {}: {e}",
                                reader.path().display()
                            );
                        }
                    }
                }

                // Idle, not Completed: the agent is still sitting there waiting for
                // the next prompt, and marking it finished would make the Deck claim
                // work had ended when it had not.
                out.state = Some((session.thread_id.clone(), ThreadState::Idle));
            }

            AgentEvent::Other(kind) => {
                let mut event = base(
                    EventPayload::RuntimeUnclassified {
                        source_type: kind.clone(),
                    },
                    session,
                );
                event.summary = format!("{kind} (unrecognised)");
                out.events.push(event);
            }
        }

        out
    }
}

impl Observed {
    /// Begin following a session, adopting an existing Thread when there is one.
    fn open(activity: &AgentActivity, pane_id: &PaneId, store: &Store) -> (Self, Thread) {
        let identity = identity_for(activity);
        // A restart of Tervin, or a `claude --resume`, should land back on the same
        // Thread rather than creating a second one for the same conversation.
        let existing = store
            .thread_by_resume_id(&activity.session_id)
            .ok()
            .flatten();

        let mut session = Self {
            thread_id: existing.as_ref().map(|t| t.id.clone()).unwrap_or_default(),
            pane_id: pane_id.clone(),
            identity,
            reader: None,
            prompts: 0,
        };
        // An adopted Thread already has its prompts recorded, so a later prompt is
        // not the first and must not rename it.
        if existing.is_some() {
            session.prompts = 1;
        }
        let thread = existing.unwrap_or_else(|| session.thread(activity));
        (session, thread)
    }

    /// The Thread row for this session.
    fn thread(&self, activity: &AgentActivity) -> Thread {
        let mut thread = Thread::new(
            self.identity.clone(),
            activity.cwd.clone().unwrap_or_default(),
            format!("{} in a pane", self.identity.display_name),
        );
        thread.id = self.thread_id.clone();
        thread.state = ThreadState::Idle;
        thread.project = activity.project.clone();
        // What makes it visibly a pane session rather than one Tervin launched.
        thread.pane_id = Some(self.pane_id.clone());
        thread.resume_id = Some(activity.session_id.clone());
        thread
    }
}

/// Name the agent from what it calls itself.
///
/// A known name gets its proper display name; anything else is shown as it reported
/// itself rather than as "unknown", because the agent's own name is more useful than
/// a placeholder and this list will always be incomplete.
fn identity_for(activity: &AgentActivity) -> AgentIdentity {
    let (runtime_id, display) = match activity.agent.as_str() {
        "claude" => ("claude-code", "Claude Code"),
        "codex" => ("codex", "Codex"),
        "gemini" => ("gemini", "Gemini CLI"),
        other => (other, other),
    };
    let mut identity = AgentIdentity::new(runtime_id, display, Tier::EnhancedCli);
    identity.version = activity.plugin_version.clone();
    identity
}

/// One line for a timeline row.
///
/// Never a dump of the payload — that is available on demand, and a wall of text is
/// unreadable at a glance.
fn summarise(payload: &EventPayload) -> String {
    match payload {
        EventPayload::UserPrompted { text } => first_line(text),
        EventPayload::AgentMessage {
            text,
            is_reasoning: true,
            ..
        } => format!("Thinking: {}", first_line(text)),
        EventPayload::AgentMessage { text, .. } => first_line(text),
        EventPayload::ToolRequested { input_summary, .. } => input_summary.clone(),
        EventPayload::ToolCompleted { is_error: true, .. } => "Tool reported an error".to_string(),
        EventPayload::ToolCompleted { .. } => "Tool finished".to_string(),
        EventPayload::FileChanged { change } => format!("Changed {}", change.path),
        other => other.kind().to_string(),
    }
}

/// The first line, clipped, for use as a title or summary.
fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let trimmed = line.trim();
    let clipped: String = trimmed.chars().take(120).collect();
    if clipped.chars().count() < trimmed.chars().count() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn activity(event: AgentEvent, session: &str) -> AgentActivity {
        AgentActivity {
            agent: "claude".to_string(),
            event,
            session_id: session.to_string(),
            cwd: Some("/proj".to_string()),
            project: Some("proj".to_string()),
            query: None,
            response: None,
            transcript_path: None,
            v: Some(1),
            plugin_version: Some("2.1.0".to_string()),
        }
    }

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn pane() -> PaneId {
        PaneId::from_external("pane_1")
    }

    fn kinds(obs: &Observation) -> Vec<&'static str> {
        obs.events.iter().map(|e| e.payload.kind()).collect()
    }

    #[test]
    fn a_session_start_creates_a_read_only_thread_pinned_to_its_pane() {
        let agents = PaneAgents::new();
        let obs = agents.observe(&activity(AgentEvent::SessionStart, "s1"), &pane(), &store());

        assert_eq!(kinds(&obs), vec!["thread.started"]);
        let thread = obs.thread.expect("a Thread should have been created");
        // The pane link is what makes it visibly a session in a pane rather than one
        // Tervin launched, and it is what "Reveal in pane" needs.
        assert_eq!(thread.pane_id, Some(pane()));
        // Recorded so the same conversation is adopted rather than duplicated later.
        assert_eq!(thread.resume_id.as_deref(), Some("s1"));
        // Enhanced CLI, not Structured: there is no protocol here, only observation.
        assert_eq!(thread.agent.tier, Tier::EnhancedCli);
        assert_eq!(thread.agent.display_name, "Claude Code");
        assert_eq!(agents.thread_for("s1"), Some(thread.id));
    }

    #[test]
    fn the_first_prompt_names_the_thread_and_is_searchable() {
        let agents = PaneAgents::new();
        let store = store();
        agents.observe(&activity(AgentEvent::SessionStart, "s1"), &pane(), &store);

        let mut prompt = activity(AgentEvent::PromptSubmit, "s1");
        prompt.query = Some("fix the flaky auth test\nand explain why".to_string());
        let obs = agents.observe(&prompt, &pane(), &store);

        assert_eq!(kinds(&obs), vec!["user.prompted"]);
        // The agent never sends a title, and "Claude Code in a pane" for every row
        // would make the Threads list useless.
        assert_eq!(
            obs.thread.as_ref().map(|t| t.task_title.as_str()),
            Some("fix the flaky auth test"),
            "the first prompt should name the Thread"
        );

        // The payoff: a prompt typed into a pane is in prompt history, which is the
        // one place it has never been recorded before.
        for event in &obs.events {
            store.append_event(event, None).unwrap();
        }
        let hits = store.search_prompts("flaky auth", 10).unwrap();
        assert_eq!(hits.len(), 1, "the pane prompt was not indexed for search");
        assert!(hits[0].text.contains("fix the flaky auth test"));
    }

    #[test]
    fn a_second_prompt_does_not_rename_the_thread() {
        let agents = PaneAgents::new();
        let store = store();
        agents.observe(&activity(AgentEvent::SessionStart, "s1"), &pane(), &store);

        let mut first = activity(AgentEvent::PromptSubmit, "s1");
        first.query = Some("the original task".to_string());
        agents.observe(&first, &pane(), &store);

        let mut second = activity(AgentEvent::PromptSubmit, "s1");
        second.query = Some("a follow-up question".to_string());
        let obs = agents.observe(&second, &pane(), &store);

        // A Thread that renames itself on every turn is impossible to find again.
        assert!(
            obs.thread.is_none(),
            "the Thread was rewritten by a later prompt"
        );
    }

    #[test]
    fn two_panes_running_agents_stay_separate() {
        let agents = PaneAgents::new();
        let store = store();
        let other = PaneId::from_external("pane_2");

        let a = agents.observe(&activity(AgentEvent::SessionStart, "s1"), &pane(), &store);
        let b = agents.observe(&activity(AgentEvent::SessionStart, "s2"), &other, &store);

        let (a, b) = (a.thread.unwrap(), b.thread.unwrap());
        assert_ne!(a.id, b.id, "two sessions collapsed into one Thread");
        assert_eq!(agents.pane_for(&a.id), Some(pane()));
        assert_eq!(agents.pane_for(&b.id), Some(other));
    }

    #[test]
    fn a_session_first_seen_mid_conversation_is_still_picked_up() {
        // Tervin may start after the agent, or miss the session_start notification.
        // Requiring it would mean the whole session goes unrecorded.
        let agents = PaneAgents::new();
        let obs = agents.observe(&activity(AgentEvent::Stop, "s9"), &pane(), &store());
        assert!(
            obs.thread.is_some(),
            "a session was dropped for starting late"
        );
        assert_eq!(kinds(&obs), vec!["thread.started"]);
    }

    #[test]
    fn an_existing_thread_is_adopted_rather_than_duplicated() {
        let store = store();
        // As if Tervin had been restarted: the Thread is on disk, the live mapping
        // is gone.
        let first = PaneAgents::new();
        let original = first
            .observe(&activity(AgentEvent::SessionStart, "s1"), &pane(), &store)
            .thread
            .unwrap();
        store.upsert_thread(&original).unwrap();

        let second = PaneAgents::new();
        let obs = second.observe(&activity(AgentEvent::Stop, "s1"), &pane(), &store);

        assert_eq!(
            obs.thread.map(|t| t.id),
            Some(original.id.clone()),
            "a restart started a second Thread for the same conversation"
        );
    }

    #[test]
    fn an_adopted_thread_keeps_its_title() {
        let store = store();
        let agents = PaneAgents::new();
        let mut original = agents
            .observe(&activity(AgentEvent::SessionStart, "s1"), &pane(), &store)
            .thread
            .unwrap();
        original.task_title = "the original task".to_string();
        store.upsert_thread(&original).unwrap();

        // After a restart, the next prompt must not look like the first one.
        let restarted = PaneAgents::new();
        let mut prompt = activity(AgentEvent::PromptSubmit, "s1");
        prompt.query = Some("a follow-up".to_string());
        let obs = restarted.observe(&prompt, &pane(), &store);

        assert_eq!(
            obs.thread.map(|t| t.task_title),
            Some("the original task".to_string()),
            "a resumed Thread was renamed by its next prompt"
        );
    }

    #[test]
    fn a_stop_reads_the_transcript_and_does_not_duplicate_the_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s1.jsonl");
        let store = store();
        let agents = PaneAgents::new();

        agents.observe(&activity(AgentEvent::SessionStart, "s1"), &pane(), &store);
        let mut prompt = activity(AgentEvent::PromptSubmit, "s1");
        prompt.query = Some("say ok".to_string());
        agents.observe(&prompt, &pane(), &store);

        // The transcript holds the prompt *and* the reply; the prompt already arrived
        // over OSC, so recording it again would double every row in prompt history.
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"type":"user","message":{{"role":"user","content":"say ok"}},"timestamp":"2026-08-02T15:29:00.000Z"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","message":{{"role":"assistant","model":"claude-opus-5","content":[{{"type":"text","text":"ok"}}]}},"timestamp":"2026-08-02T15:29:02.000Z"}}"#
        )
        .unwrap();
        drop(file);

        let mut stop = activity(AgentEvent::Stop, "s1");
        stop.transcript_path = Some(path.display().to_string());
        let obs = agents.observe(&stop, &pane(), &store);

        assert_eq!(
            kinds(&obs),
            vec!["agent.message"],
            "the prompt was recorded twice, or the reply was missed"
        );
        // Idle rather than Completed: the agent is still sitting there waiting.
        assert_eq!(obs.state.map(|(_, s)| s), Some(ThreadState::Idle));
    }

    #[test]
    fn a_transcript_that_cannot_be_read_does_not_fail_the_thread() {
        let store = store();
        let agents = PaneAgents::new();
        let mut stop = activity(AgentEvent::Stop, "s1");
        // The path comes from the agent and may be gone by the time we look.
        stop.transcript_path = Some("/nonexistent/never/s1.jsonl".to_string());

        let obs = agents.observe(&stop, &pane(), &store);
        assert_eq!(kinds(&obs), vec!["thread.started"]);
        assert_eq!(obs.state.map(|(_, s)| s), Some(ThreadState::Idle));
    }

    #[test]
    fn an_unmodelled_event_is_kept_as_unclassified() {
        let agents = PaneAgents::new();
        let store = store();
        agents.observe(&activity(AgentEvent::SessionStart, "s1"), &pane(), &store);

        let obs = agents.observe(
            &activity(AgentEvent::Other("tool_use_start".to_string()), "s1"),
            &pane(),
            &store,
        );
        assert_eq!(kinds(&obs), vec!["runtime.unclassified"]);
        assert!(obs.events[0].summary.contains("tool_use_start"));
    }

    #[test]
    fn an_agent_that_is_not_recognised_is_named_as_it_reported_itself() {
        let agents = PaneAgents::new();
        let mut a = activity(AgentEvent::SessionStart, "s1");
        a.agent = "some-new-agent".to_string();

        let thread = agents.observe(&a, &pane(), &store()).thread.unwrap();
        // Better than "Unknown agent": the name it gave is the useful information,
        // and this list will always be incomplete.
        assert_eq!(thread.agent.display_name, "some-new-agent");
    }

    #[test]
    fn closing_a_pane_stops_following_its_sessions_but_keeps_the_threads() {
        let agents = PaneAgents::new();
        let store = store();
        let thread = agents
            .observe(&activity(AgentEvent::SessionStart, "s1"), &pane(), &store)
            .thread
            .unwrap();
        store.upsert_thread(&thread).unwrap();

        let dropped = agents.forget_pane(&pane());
        assert_eq!(dropped, vec![thread.id.clone()]);
        assert_eq!(agents.thread_for("s1"), None);
        // The conversation happened, and it is exactly what prompt history is for.
        assert!(store.get_thread(&thread.id).unwrap().is_some());
    }

    #[test]
    fn a_prompt_submit_with_no_text_records_nothing() {
        let agents = PaneAgents::new();
        let store = store();
        agents.observe(&activity(AgentEvent::SessionStart, "s1"), &pane(), &store);

        // The parser drops empty strings, so `query` is absent rather than "". A
        // blank row in prompt history is what makes a search feel broken.
        let obs = agents.observe(&activity(AgentEvent::PromptSubmit, "s1"), &pane(), &store);
        assert!(obs.events.is_empty());
    }

    #[test]
    fn every_event_links_back_to_the_pane_it_came_from() {
        let agents = PaneAgents::new();
        let store = store();
        let obs = agents.observe(&activity(AgentEvent::SessionStart, "s1"), &pane(), &store);
        // `Link` has no `PartialEq`, so match rather than compare.
        assert!(matches!(
            obs.events[0].links.as_slice(),
            [Link::Pane { pane_id }] if pane_id == &pane()
        ));
    }
}
