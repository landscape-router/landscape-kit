//! Named application tasks and OS threads. Task names are Tokio metadata,
//! not OS thread names; a diagnostic subscriber is needed to inspect them.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// Spawn with Tokio's native task name in every build profile.
#[track_caller]
pub fn spawn_task<F>(name: &'static str, future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::task::Builder::new()
        .name(name)
        .spawn(future)
        .expect("spawn named Tokio task")
}

/// Like `std::thread::spawn`, including its panic-on-spawn-error semantics.
#[track_caller]
pub fn spawn_thread<F, T>(name: &'static str, f: F) -> thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(f)
        .expect("spawn named OS thread")
}

/// Unique short worker names. Callers keep prefixes short enough for Linux's
/// 15-byte visible thread-name limit (including the numeric suffix).
pub fn runtime_thread_name_fn(prefix: &'static str) -> impl Fn() -> String + Send + Sync + 'static {
    let sequence = AtomicUsize::new(0);
    move || format!("{prefix}-{}", sequence.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_names::tests as names;

    #[test]
    fn named_thread_preserves_name_and_result() {
        let thread = spawn_thread(names::THREAD, || {
            assert_eq!(thread::current().name(), Some(names::THREAD));
            42
        });
        assert_eq!(thread.join().unwrap(), 42);
    }

    #[test]
    fn runtime_names_are_unique() {
        let name = runtime_thread_name_fn(names::RUNTIME);
        assert_eq!(name(), "terrain-test-0");
        assert_eq!(name(), "terrain-test-1");
    }

    #[test]
    fn native_task_name_is_emitted_to_tracing() {
        use std::sync::{Arc, Mutex};
        use tracing::span::{Attributes, Id, Record};
        use tracing::{Event, Metadata, Subscriber};

        #[derive(Clone, Default)]
        struct Capture(Arc<Mutex<Vec<String>>>);

        impl tracing::field::Visit for Capture {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "task.name" {
                    self.0.lock().unwrap().push(value.to_owned());
                }
            }

            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "task.name" {
                    self.0.lock().unwrap().push(format!("{value:?}"));
                }
            }
        }

        impl Subscriber for Capture {
            fn enabled(&self, _: &Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, attributes: &Attributes<'_>) -> Id {
                attributes.record(&mut self.clone());
                static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
                Id::from_u64(NEXT_ID.fetch_add(1, Ordering::Relaxed) as u64)
            }
            fn record(&self, _: &Id, _: &Record<'_>) {}
            fn record_follows_from(&self, _: &Id, _: &Id) {}
            fn event(&self, _: &Event<'_>) {}
            fn enter(&self, _: &Id) {}
            fn exit(&self, _: &Id) {}
        }

        let capture = Capture::default();
        let _subscriber = tracing::subscriber::set_default(capture.clone());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            spawn_task(names::RESULT, async {}).await.unwrap();
        });
        let captured = capture.0.lock().unwrap();
        assert!(
            captured.iter().any(|name| name == names::RESULT),
            "captured task names: {captured:?}"
        );
    }

    #[test]
    fn named_task_preserves_join_and_abort() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert_eq!(spawn_task(names::RESULT, async { 42 }).await.unwrap(), 42);
            let task = spawn_task(names::PENDING, std::future::pending::<()>());
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        });
    }
}
