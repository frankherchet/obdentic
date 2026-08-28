//! Single-owner runtime state actor.
//!
//! The actor is deliberately transport-neutral: it accepts only typed
//! [`RuntimeEvent`] values and owns the one mutable [`RuntimeState`].

use crate::{
    runtime_reducer::{self, RuntimeEvent, TransitionError},
    runtime_state::{Activity, Phase, RuntimeContext, RuntimeState},
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

/// Maximum number of commands waiting for the state owner.
pub const CHANNEL_CAPACITY: usize = 32;

/// Local sequence assigned to each accepted reducer transition.
pub type TransitionSequence = u64;

/// An immutable value returned to consumers of the runtime actor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeSnapshot {
    state: RuntimeState,
    transition_sequence: TransitionSequence,
}

impl RuntimeSnapshot {
    const fn new(state: RuntimeState, transition_sequence: TransitionSequence) -> Self {
        Self {
            state,
            transition_sequence,
        }
    }

    /// The complete transport-neutral runtime state at this sequence.
    pub const fn state(self) -> RuntimeState {
        self.state
    }

    /// The local monotonically increasing sequence of accepted transitions.
    pub const fn transition_sequence(self) -> TransitionSequence {
        self.transition_sequence
    }

    /// Alias useful to consumers that call the value a sequence number.
    pub const fn sequence(self) -> TransitionSequence {
        self.transition_sequence
    }

    pub const fn state_version(self) -> u16 {
        self.state.state_version()
    }

    pub const fn phase(self) -> Phase {
        self.state.phase()
    }

    pub const fn activity(self) -> Activity {
        self.state.activity()
    }

    pub const fn context(self) -> RuntimeContext {
        self.state.context()
    }
}

/// The actor-authoritative result of one accepted runtime event.
///
/// `from` and `to` are captured by the state owner together with the monotone
/// sequence, so consumers never infer a prior state from local shadow state.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeTransition {
    transition_sequence: TransitionSequence,
    event: RuntimeEvent,
    from: RuntimeState,
    to: RuntimeState,
}

impl RuntimeTransition {
    const fn new(
        transition_sequence: TransitionSequence,
        event: RuntimeEvent,
        from: RuntimeState,
        to: RuntimeState,
    ) -> Self {
        Self {
            transition_sequence,
            event,
            from,
            to,
        }
    }

    pub const fn sequence(self) -> TransitionSequence {
        self.transition_sequence
    }

    pub const fn transition_sequence(self) -> TransitionSequence {
        self.transition_sequence
    }

    pub const fn event(self) -> RuntimeEvent {
        self.event
    }

    pub const fn from(self) -> RuntimeState {
        self.from
    }

    pub const fn to(self) -> RuntimeState {
        self.to
    }

    pub const fn state(self) -> RuntimeState {
        self.to
    }

    pub const fn activity(self) -> Activity {
        self.to.activity()
    }
}

/// Errors from command delivery or the pure reducer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RuntimeActorError {
    /// The reducer rejected the event; the owned state was left unchanged.
    Transition(TransitionError),
    /// The actor has reached `phase/stopped` and accepts no more events.
    Stopped,
    /// The bounded command queue is full for a non-blocking send.
    QueueFull,
    /// The actor task is no longer available.
    Closed,
    /// The local sequence cannot be advanced without wrapping.
    SequenceOverflow,
}

impl RuntimeActorError {
    pub const fn transition(self) -> Option<TransitionError> {
        match self {
            Self::Transition(error) => Some(error),
            Self::Stopped | Self::QueueFull | Self::Closed | Self::SequenceOverflow => None,
        }
    }
}

impl std::fmt::Display for RuntimeActorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transition(error) => write!(formatter, "runtime transition rejected: {error}"),
            Self::Stopped => formatter.write_str("runtime actor is stopped"),
            Self::QueueFull => formatter.write_str("runtime actor command queue is full"),
            Self::Closed => formatter.write_str("runtime actor is closed"),
            Self::SequenceOverflow => formatter.write_str("runtime transition sequence overflow"),
        }
    }
}

impl std::error::Error for RuntimeActorError {}

/// Cloneable command client.  It contains no runtime state and no transport.
#[derive(Clone)]
pub struct RuntimeClient {
    commands: mpsc::Sender<Command>,
}

impl RuntimeClient {
    /// Submit one event and await its actor-authoritative transition.
    pub async fn send(&self, event: RuntimeEvent) -> Result<RuntimeTransition, RuntimeActorError> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Event { event, reply })
            .await
            .map_err(|_| RuntimeActorError::Closed)?;
        result.await.map_err(|_| RuntimeActorError::Closed)?
    }

    /// Submit one event without waiting for room in the bounded queue.
    pub fn try_send(
        &self,
        event: RuntimeEvent,
    ) -> Result<oneshot::Receiver<Result<RuntimeTransition, RuntimeActorError>>, RuntimeActorError>
    {
        let (reply, result) = oneshot::channel();
        self.commands
            .try_send(Command::Event { event, reply })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => RuntimeActorError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => RuntimeActorError::Closed,
            })?;
        Ok(result)
    }

    /// Get a clone of the current state without exposing mutable state.
    pub async fn snapshot(&self) -> Result<RuntimeSnapshot, RuntimeActorError> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Snapshot { reply })
            .await
            .map_err(|_| RuntimeActorError::Closed)?;
        result.await.map_err(|_| RuntimeActorError::Closed)
    }

    /// Apply shutdown as one deterministic `stopping -> stopped` operation.
    pub async fn shutdown(&self) -> Result<(), RuntimeActorError> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Shutdown { reply })
            .await
            .map_err(|_| RuntimeActorError::Closed)?;
        result.await.map_err(|_| RuntimeActorError::Closed)?
    }
}

/// Compatibility alias for callers that name the client after the actor.
pub type Client = RuntimeClient;

enum Command {
    Event {
        event: RuntimeEvent,
        reply: oneshot::Sender<Result<RuntimeTransition, RuntimeActorError>>,
    },
    Snapshot {
        reply: oneshot::Sender<RuntimeSnapshot>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), RuntimeActorError>>,
    },
}

/// Start the sole state owner and return its cloneable client plus task.
pub fn start() -> (RuntimeClient, JoinHandle<()>) {
    let (commands, receiver) = mpsc::channel(CHANNEL_CAPACITY);
    let task = tokio::spawn(run(receiver));
    (RuntimeClient { commands }, task)
}

/// Explicitly named entry point for integrations that avoid a generic `start`.
pub fn start_runtime_actor() -> (RuntimeClient, JoinHandle<()>) {
    start()
}

async fn run(mut commands: mpsc::Receiver<Command>) {
    let mut state = RuntimeState::default();
    let mut transition_sequence = 0;

    while let Some(command) = commands.recv().await {
        match command {
            Command::Event { event, reply } => {
                let result = apply(&mut state, &mut transition_sequence, event);
                let _ = reply.send(result);
            }
            Command::Snapshot { reply } => {
                let _ = reply.send(RuntimeSnapshot::new(state, transition_sequence));
            }
            Command::Shutdown { reply } => {
                let result = shutdown(&mut state, &mut transition_sequence);
                let _ = reply.send(result);
            }
        }
    }
}

fn apply(
    state: &mut RuntimeState,
    transition_sequence: &mut TransitionSequence,
    event: RuntimeEvent,
) -> Result<RuntimeTransition, RuntimeActorError> {
    if state.phase() == Phase::Stopped {
        return Err(RuntimeActorError::Stopped);
    }
    let from = *state;
    let next = runtime_reducer::transition(state, event).map_err(RuntimeActorError::Transition)?;
    let next_sequence = transition_sequence
        .checked_add(1)
        .ok_or(RuntimeActorError::SequenceOverflow)?;
    *state = next;
    *transition_sequence = next_sequence;
    Ok(RuntimeTransition::new(next_sequence, event, from, next))
}

fn shutdown(
    state: &mut RuntimeState,
    transition_sequence: &mut TransitionSequence,
) -> Result<(), RuntimeActorError> {
    if state.phase() == Phase::Stopped {
        return Err(RuntimeActorError::Stopped);
    }
    if state.phase() != Phase::Stopping {
        apply(state, transition_sequence, RuntimeEvent::ShutdownRequested)?;
    }
    apply(state, transition_sequence, RuntimeEvent::ShutdownCompleted)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_reducer::{RuntimeEvent, RuntimeEventKind};

    #[tokio::test]
    async fn starts_in_init_idle_with_zero_sequence() {
        let (client, task) = start();
        let snapshot = client.snapshot().await.unwrap();
        assert_eq!(snapshot.state().identity(), (Phase::Init, Activity::Idle));
        assert_eq!(snapshot.state_version(), 1);
        assert_eq!(snapshot.transition_sequence(), 0);
        drop(client);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn valid_events_are_serialized_and_sequence_is_monotonic() {
        let (client, task) = start();
        let ready = client
            .send(RuntimeEvent::InitializationCompleted)
            .await
            .unwrap();
        let reading = client.send(RuntimeEvent::ReadStarted).await.unwrap();
        let idle = client.send(RuntimeEvent::ReadCompleted).await.unwrap();

        assert_eq!(ready.state().identity(), (Phase::Ready, Activity::Idle));
        assert_eq!(reading.state().identity(), (Phase::Ready, Activity::Read));
        assert_eq!(idle.state().identity(), (Phase::Ready, Activity::Idle));
        assert_eq!(
            [ready, reading, idle].map(RuntimeTransition::transition_sequence),
            [1, 2, 3]
        );
        drop(client);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn invalid_event_does_not_mutate_state_or_sequence() {
        let (client, task) = start();
        let before = client.snapshot().await.unwrap();
        let error = client.send(RuntimeEvent::ReadCompleted).await.unwrap_err();
        let after = client.snapshot().await.unwrap();

        assert_eq!(
            error.transition(),
            Some(TransitionError::InvalidOrder {
                phase: Phase::Init,
                activity: Activity::Idle,
                event: RuntimeEventKind::ReadCompleted,
            })
        );
        assert_eq!(before, after);
        drop(client);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_senders_are_serialized_by_the_actor() {
        let (client, task) = start();
        let ready = client
            .send(RuntimeEvent::InitializationCompleted)
            .await
            .unwrap();
        assert_eq!(ready.transition_sequence(), 1);

        let first = client.clone();
        let second = client.clone();
        let (left, right) = tokio::join!(
            first.send(RuntimeEvent::ReadStarted),
            second.send(RuntimeEvent::ReadCompleted),
        );
        let sequences = [left, right]
            .into_iter()
            .filter_map(Result::ok)
            .map(RuntimeTransition::transition_sequence)
            .collect::<Vec<_>>();
        assert!(matches!(sequences.as_slice(), [2] | [2, 3]));

        drop(client);
        drop(first);
        drop(second);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn slow_snapshot_consumer_does_not_hold_a_lock() {
        let (client, task) = start();
        let (reply, result) = oneshot::channel();
        client
            .commands
            .send(Command::Snapshot { reply })
            .await
            .unwrap();
        let ready = client
            .send(RuntimeEvent::InitializationCompleted)
            .await
            .unwrap();
        assert_eq!(ready.transition_sequence(), 1);
        drop(result);
        drop(client);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_stops_once_and_rejects_later_events() {
        let (client, task) = start();
        client.shutdown().await.unwrap();
        let stopped = client.snapshot().await.unwrap();
        assert_eq!(stopped.state().identity(), (Phase::Stopped, Activity::Idle));
        assert_eq!(stopped.transition_sequence(), 2);
        assert_eq!(client.shutdown().await, Err(RuntimeActorError::Stopped));
        assert_eq!(
            client.send(RuntimeEvent::FatalRuntimeError).await,
            Err(RuntimeActorError::Stopped)
        );
        drop(client);
        task.await.unwrap();
    }

    #[test]
    fn public_api_contains_no_transport_owner() {
        fn accepts_client(_: RuntimeClient) {}
        let (client, task) = {
            // This compile-time shape check intentionally never starts a task.
            let (commands, _) = mpsc::channel::<Command>(CHANNEL_CAPACITY);
            (RuntimeClient { commands }, None::<JoinHandle<()>>)
        };
        accepts_client(client);
        assert!(task.is_none());
    }
}
