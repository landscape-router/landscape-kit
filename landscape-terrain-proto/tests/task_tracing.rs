//! A separate test binary isolates the thread-local tracing subscriber from
//! other tests concurrently initializing Tokio's process-wide callsite cache.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use landscape_terrain_proto::concurrency::spawn_task;
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
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Id::from_u64(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
    fn record(&self, _: &Id, values: &Record<'_>) {
        values.record(&mut self.clone());
    }
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, _: &Event<'_>) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[test]
fn native_task_name_is_emitted_to_tracing() {
    // This is a test-only probe, not a new application task name.
    const NAME: &str = module_path!();
    let capture = Capture::default();
    let _subscriber = tracing::subscriber::set_default(capture.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        spawn_task(NAME, async {}).await.unwrap();
    });
    let captured = capture.0.lock().unwrap();
    assert!(
        captured.iter().any(|name| name == NAME),
        "captured task names: {captured:?}"
    );
}
