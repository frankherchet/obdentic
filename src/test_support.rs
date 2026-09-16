//! Shared scripted [`AdapterLink`](crate::elm::AdapterLink) fake for tests.
//!
//! Two adapters satisfy the adapter seam: the production Carly backend
//! ([`CarlyCuaV200`](crate::adapter::CarlyCuaV200)) and this scripted fake,
//! which makes the seam real rather than hypothetical. This replaces what
//! used to be three separate, near-identical `ScriptedExchange` copies in
//! `adapter.rs`, `ble.rs` and `elm.rs`.

use crate::elm::{AdapterLink, ElmExchange, ExchangeError};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct Shared {
    responses: VecDeque<Result<String, ExchangeError>>,
    commands: Vec<String>,
    disconnects: u32,
}

pub(crate) struct ScriptedExchange {
    shared: Arc<Mutex<Shared>>,
}

/// A cheap, cloneable observer into a [`ScriptedExchange`]'s state that
/// outlives moving the exchange itself into a session.
#[derive(Clone)]
pub(crate) struct ScriptedExchangeSpy {
    shared: Arc<Mutex<Shared>>,
}

impl ScriptedExchangeSpy {
    pub(crate) fn commands(&self) -> Vec<String> {
        self.shared.lock().unwrap().commands.clone()
    }

    pub(crate) fn disconnects(&self) -> u32 {
        self.shared.lock().unwrap().disconnects
    }
}

impl ScriptedExchange {
    /// Script a sequence of successful responses.
    pub(crate) fn new(responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::from_results(responses.into_iter().map(|response| Ok(response.into())))
    }

    /// Script a sequence of results, so failures can be interleaved with
    /// successful responses.
    pub(crate) fn from_results(
        responses: impl IntoIterator<Item = Result<String, ExchangeError>>,
    ) -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                responses: responses.into_iter().collect(),
                commands: Vec::new(),
                disconnects: 0,
            })),
        }
    }

    /// An observer that keeps seeing this exchange's state after the
    /// exchange itself has been moved into a session.
    pub(crate) fn spy(&self) -> ScriptedExchangeSpy {
        ScriptedExchangeSpy {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Every command sent so far, in order.
    pub(crate) fn commands(&self) -> Vec<String> {
        self.spy().commands()
    }

    /// The exact 8 responses a real Carly adapter gives for
    /// `ATI, AT@1, ATZ, ATE0, ATL0, ATS1, ATH1, ATSP0`.
    pub(crate) fn carly_init_responses() -> Vec<String> {
        [
            "ELM327 v1.4 v100\r>",
            "carly-universal v200\r>",
            "ELM327 v1.4 v100\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }
}

impl ElmExchange for ScriptedExchange {
    async fn exchange(
        &mut self,
        command: &str,
        _command_timeout: Duration,
    ) -> Result<String, ExchangeError> {
        let mut shared = self.shared.lock().unwrap();
        shared.commands.push(command.to_owned());
        shared.responses.pop_front().unwrap_or_else(|| {
            Err(ExchangeError::Response(
                "script ended before adapter response".into(),
            ))
        })
    }
}

impl AdapterLink for ScriptedExchange {
    async fn disconnect(&mut self) -> Result<(), String> {
        self.shared.lock().unwrap().disconnects += 1;
        Ok(())
    }

    async fn disconnect_best_effort(&mut self) {
        self.shared.lock().unwrap().disconnects += 1;
    }
}
