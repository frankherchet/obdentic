use crate::{
    audit::AuditState,
    ble::{start_session, SessionClient},
    prepare_read,
    telemetry::TelemetryState,
    ReadRequest,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::oneshot,
    task::JoinHandle,
    time::{sleep_until, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subscription {
    request: ReadRequest,
    interval: Duration,
}

impl Subscription {
    pub fn new(semantic: &str, interval: Duration) -> Result<Self, String> {
        if interval.is_zero() {
            return Err("subscription interval must be greater than zero".into());
        }
        Ok(Self {
            request: prepare_read(semantic)?,
            interval,
        })
    }

    pub fn semantic(self) -> &'static str {
        self.request.metadata().semantic
    }
}

pub struct TelemetryScheduler {
    cancel: oneshot::Sender<()>,
    task: JoinHandle<Result<(), String>>,
}

impl TelemetryScheduler {
    pub async fn start(
        adapter_id: &str,
        subscriptions: Vec<Subscription>,
        telemetry: Arc<Mutex<TelemetryState>>,
        audit: Arc<Mutex<AuditState>>,
    ) -> Result<Self, String> {
        if subscriptions.is_empty() {
            return Err("telemetry scheduler needs at least one subscription".into());
        }
        let session = start_session(adapter_id).await?;
        let (cancel, cancelled) = oneshot::channel();
        let task = tokio::spawn(run(session, subscriptions, telemetry, audit, cancelled));
        Ok(Self { cancel, task })
    }

    pub async fn stop(self) -> Result<(), String> {
        let _ = self.cancel.send(());
        self.task
            .await
            .map_err(|error| format!("telemetry scheduler stopped unexpectedly: {error}"))?
    }
}

async fn run(
    session: SessionClient,
    subscriptions: Vec<Subscription>,
    telemetry: Arc<Mutex<TelemetryState>>,
    audit: Arc<Mutex<AuditState>>,
    mut cancelled: oneshot::Receiver<()>,
) -> Result<(), String> {
    let mut schedule = subscriptions
        .into_iter()
        .map(|subscription| (subscription, Instant::now()))
        .collect::<Vec<_>>();
    let result = 'scheduler: loop {
        let next = schedule.iter().map(|(_, due)| *due).min().unwrap();
        tokio::select! {
            _ = &mut cancelled => break Ok(()),
            _ = sleep_until(next) => {
                let now = Instant::now();
                for (subscription, due) in &mut schedule {
                    if *due > now {
                        continue;
                    }
                    let transaction = match session.read(subscription.request).await {
                        Ok(transaction) => transaction,
                        Err(error) => break 'scheduler Err(error),
                    };
                    telemetry.lock().map_err(|_| "telemetry state lock poisoned")?.ingest(&transaction);
                    audit.lock().map_err(|_| "audit state lock poisoned")?.ingest(&transaction);
                    while *due <= now {
                        *due += subscription.interval;
                    }
                }
            }
        }
    };
    let result = match (result, session.shutdown().await) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
    };
    if let Err(error) = &result {
        audit
            .lock()
            .map_err(|_| "audit state lock poisoned")?
            .record_error(error);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscriptions_only_accept_known_read_only_signals_and_positive_intervals() {
        assert_eq!(
            Subscription::new("engine.rpm", Duration::from_millis(200))
                .unwrap()
                .semantic(),
            "engine.rpm"
        );
        assert!(Subscription::new("dtc.clear", Duration::from_secs(1)).is_err());
        assert!(Subscription::new("engine.rpm", Duration::ZERO).is_err());
    }
}
