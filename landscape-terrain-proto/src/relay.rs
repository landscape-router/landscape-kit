//! Event-driven relay scheduling and bounded-channel progress notifications.

use std::collections::HashSet;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;

/// Coalesces progress by connection key. Unlike the data channel, this wake
/// path cannot be blocked by the relay's pending-byte limit.
#[derive(Clone)]
pub struct RelayWake<K> {
    inner: Arc<WakeInner<K>>,
}

struct WakeInner<K> {
    keys: Mutex<HashSet<K>>,
    notify: Notify,
}

impl<K: Copy + Eq + Hash> Default for RelayWake<K> {
    fn default() -> Self {
        Self {
            inner: Arc::new(WakeInner {
                keys: Mutex::new(HashSet::new()),
                notify: Notify::new(),
            }),
        }
    }
}

impl<K: Copy + Eq + Hash> RelayWake<K> {
    fn notify(&self, key: K) {
        if self.inner.keys.lock().unwrap().insert(key) {
            self.inner.notify.notify_one();
        }
    }

    pub fn watch<T>(&self, key: K, receiver: mpsc::Receiver<T>) -> RelayReceiver<T, K> {
        RelayReceiver {
            receiver,
            key,
            wake: self.clone(),
        }
    }

    /// Cancellation safe: keys are removed only when this future returns them.
    pub async fn recv(&self) -> Vec<K> {
        loop {
            let keys: Vec<_> = self.inner.keys.lock().unwrap().drain().collect();
            if !keys.is_empty() {
                return keys;
            }
            self.inner.notify.notified().await;
        }
    }
}

/// Wakes the stack when a consumer frees channel space or exits. Notifications
/// happen before a possibly slow socket write, not only after it completes.
pub struct RelayReceiver<T, K: Copy + Eq + Hash> {
    receiver: mpsc::Receiver<T>,
    key: K,
    wake: RelayWake<K>,
}

impl<T, K: Copy + Eq + Hash> RelayReceiver<T, K> {
    pub async fn recv(&mut self) -> Option<T> {
        let message = self.receiver.recv().await;
        if message.is_some() {
            self.wake.notify(self.key);
        }
        message
    }
}

impl<T, K: Copy + Eq + Hash> Drop for RelayReceiver<T, K> {
    fn drop(&mut self) {
        // Publish the closed state before waking the stack on another worker.
        self.receiver.close();
        self.wake.notify(self.key);
    }
}

/// No protocol deadline means no timer. Immediate work yields once so a busy
/// stack cannot monopolize the executor with a sequence of ready deadlines.
pub async fn wait_for_poll(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) if deadline <= Instant::now() => tokio::task::yield_now().await,
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn idle_has_no_protocol_timer() {
        assert!(
            tokio::time::timeout(Duration::from_secs(3600), wait_for_poll(None))
                .await
                .is_err()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn waits_for_the_exact_deadline() {
        let deadline = Instant::now() + Duration::from_secs(10);
        wait_for_poll(Some(deadline)).await;
        assert_eq!(Instant::now(), deadline);
    }

    #[tokio::test]
    async fn immediate_deadline_yields_before_repolling() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        let mut wait = std::pin::pin!(wait_for_poll(Some(Instant::now())));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(wait.as_mut().poll(&mut context), Poll::Pending));
        assert!(matches!(wait.as_mut().poll(&mut context), Poll::Ready(())));
    }

    #[tokio::test]
    async fn capacity_and_receiver_drop_wake_without_data_channel_space() {
        let wake = RelayWake::default();
        let (tx, rx) = mpsc::channel(1);
        let mut rx = wake.watch((7, 1), rx);
        tx.send(42).await.unwrap();
        assert_eq!(tx.capacity(), 0);
        assert_eq!(rx.recv().await, Some(42));
        // No stack-bound data message is needed to resume the producer.
        assert_eq!(wake.recv().await, vec![(7, 1)]);
        assert_eq!(tx.capacity(), 1);
        drop(rx);
        assert!(tx.is_closed());
        assert_eq!(wake.recv().await, vec![(7, 1)]);
    }

    #[tokio::test]
    async fn wakes_coalesce_and_keep_connection_generations_distinct() {
        let wake = RelayWake::default();
        for _ in 0..1000 {
            wake.notify((7, 1));
        }
        wake.notify((7, 2));
        let keys = wake.recv().await;
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&(7, 1)));
        assert!(keys.contains(&(7, 2)));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_wait_does_not_lose_later_progress() {
        let wake = RelayWake::default();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), wake.recv())
                .await
                .is_err()
        );
        wake.notify(7);
        assert_eq!(wake.recv().await, vec![7]);
        // A leftover Notify permit must not produce an empty ready event.
        assert!(
            tokio::time::timeout(Duration::from_secs(1), wake.recv())
                .await
                .is_err()
        );
        wake.notify(8);
        assert_eq!(wake.recv().await, vec![8]);
    }
}
