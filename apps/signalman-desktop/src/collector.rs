//! One finite, exclusive, read-only observation session selected by the owner.

use crate::network::LayoutWake;
use signalman::observation::{Admission, ObservationBundle, collect};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tulle::observation_serial::ObservationClient;

const MAX_PAGES: usize = 4096;
const MAX_BYTES: usize = 2 * 1024 * 1024;

pub enum CollectorEvent {
    Finished {
        source: String,
        bundle: ObservationBundle,
        captured_unix_ms: u64,
    },
    Stopped,
    Failed(String),
}

pub struct CollectorWorker {
    events: Receiver<CollectorEvent>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl CollectorWorker {
    pub fn spawn(port: String, association: String, wake: LayoutWake) -> Self {
        let (events_tx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let actor_stop = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("signalman-observation".into())
            .spawn(move || {
                let result = run(&port, &association, &actor_stop);
                let event = match result {
                    Ok((bundle, captured_unix_ms)) => CollectorEvent::Finished {
                        source: format!("{port} (unauthenticated local USB)"),
                        bundle,
                        captured_unix_ms,
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                        CollectorEvent::Stopped
                    }
                    Err(error) => CollectorEvent::Failed(format!(
                        "Observation collection on {port} failed: {error}"
                    )),
                };
                let _ = events_tx.send(event);
                wake();
            })
            .expect("spawn Signalman observation collector");
        Self {
            events,
            stop,
            join: Some(join),
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
    pub fn drain(&self) -> Vec<CollectorEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for CollectorWorker {
    fn drop(&mut self) {
        self.stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run(
    port: &str,
    association: &str,
    stop: &AtomicBool,
) -> std::io::Result<(ObservationBundle, u64)> {
    if stop.load(Ordering::Relaxed) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "observation capture stopped",
        ));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut client = ObservationClient::open(port, 115_200, true, Duration::from_secs(2))?;
        let result = collect::capture_while(
            &mut client,
            association.as_bytes(),
            port,
            Admission {
                max_frames: MAX_PAGES,
                max_bytes: MAX_BYTES,
            },
            MAX_PAGES,
            unix_time_ms,
            || !stop.load(Ordering::Relaxed),
        )
        .await;
        let serial = client.into_inner();
        let lowered = serial.set_dtr(false);
        drop(serial);
        lowered?;
        let capture = result?;
        if !capture.reached_target {
            return Err(std::io::Error::other(
                "observation page bound was reached before the initial snapshot was complete",
            ));
        }
        Ok((capture.bundle, unix_time_ms()?))
    })
}

fn unix_time_ms() -> std::io::Result<u64> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(std::io::Error::other)?;
    u64::try_from(elapsed.as_millis()).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reaches_the_actor_and_terminal_event_is_retained_until_drain() {
        let (events_tx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let actor_stop = Arc::clone(&stop);
        let join = thread::spawn(move || {
            while !actor_stop.load(Ordering::Relaxed) {
                thread::yield_now();
            }
            events_tx.send(CollectorEvent::Stopped).unwrap();
        });
        let mut worker = CollectorWorker {
            events,
            stop,
            join: Some(join),
        };
        worker.stop();
        worker.join.take().unwrap().join().unwrap();
        assert!(matches!(
            worker.drain().as_slice(),
            [CollectorEvent::Stopped]
        ));
        assert!(worker.drain().is_empty());
    }
}
