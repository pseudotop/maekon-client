use std::sync::Arc;
use std::time::Duration;

use oneshim_core::ports::integration::IntegrationInsightProducerPort;
use tokio::sync::watch;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct IntegrationProducerRuntimeLoopProfile {
    pub produce_interval: Duration,
}

impl Default for IntegrationProducerRuntimeLoopProfile {
    fn default() -> Self {
        Self {
            produce_interval: Duration::from_secs(30),
        }
    }
}

#[derive(Clone)]
pub struct IntegrationProducerRuntimeLoop {
    producer: Arc<dyn IntegrationInsightProducerPort>,
    profile: IntegrationProducerRuntimeLoopProfile,
}

impl IntegrationProducerRuntimeLoop {
    pub fn new(
        producer: Arc<dyn IntegrationInsightProducerPort>,
        profile: IntegrationProducerRuntimeLoopProfile,
    ) -> Self {
        Self { producer, profile }
    }

    pub async fn run(&self, mut shutdown_rx: watch::Receiver<bool>) {
        let mut produce_interval = tokio::time::interval(self.profile.produce_interval);
        produce_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = produce_interval.tick() => {
                    if let Err(error) = self.producer.produce_pending().await {
                        warn!(error = %error, "integration producer cycle failed");
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::{watch, Mutex};

    use super::*;
    use oneshim_core::error::CoreError;

    struct MockProducer {
        calls: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl IntegrationInsightProducerPort for MockProducer {
        async fn produce_pending(&self) -> Result<usize, CoreError> {
            *self.calls.lock().await += 1;
            Ok(0)
        }
    }

    #[tokio::test]
    async fn producer_loop_runs_until_shutdown() {
        let producer = Arc::new(MockProducer {
            calls: Arc::new(Mutex::new(0)),
        });
        let loop_runner = IntegrationProducerRuntimeLoop::new(
            producer.clone(),
            IntegrationProducerRuntimeLoopProfile {
                produce_interval: Duration::from_millis(5),
            },
        );
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(async move {
            loop_runner.run(shutdown_rx).await;
        });

        tokio::time::sleep(Duration::from_millis(25)).await;
        shutdown_tx.send(true).unwrap();
        handle.await.unwrap();

        assert!(*producer.calls.lock().await > 0);
    }
}
