//! Bounded diagnostic exports for disposable fixture worlds. Never export data/config backups.
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value, json};

const MAX_BYTES: u64 = 256 * 1024;
const MAX_ENTRIES: usize = 2000;

fn read_file(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "diagnostic file exceeds limit"
    );
    Ok(bytes)
}

fn sensitive(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "password",
        "passwd",
        "token",
        "secret",
        "psk",
        "authorization",
        "cookie",
        "恢复码",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

fn summary(value: &Value) -> Value {
    let mut selected = serde_json::Map::new();
    for field in [
        "schema_version",
        "id",
        "transaction_id",
        "kind",
        "operation",
        "status",
        "stage",
        "phase",
        "revision",
        "exit_code",
        "cancellable",
        "cancel_requested",
        "cancel_cleaned",
        "sequence",
        "at",
        "event",
    ] {
        if let Some(value) = value.get(field) {
            if field == "event" {
                let mut event = serde_json::Map::new();
                for key in ["step", "stage", "status"] {
                    if let Some(text) = value.get(key).and_then(Value::as_str)
                        && text.len() <= 80
                        && text.bytes().all(|c| c.is_ascii_lowercase() || c == b'_')
                        && !sensitive(text)
                    {
                        event.insert(key.into(), text.into());
                    }
                }
                selected.insert(field.into(), Value::Object(event));
            } else if !value.is_array() && !value.is_object() {
                selected.insert(
                    field.into(),
                    if value.as_str().is_some_and(sensitive) {
                        "[redacted]".into()
                    } else {
                        value.clone()
                    },
                );
            }
        }
    }
    Value::Object(selected)
}

fn walk(root: &Path, dir: &Path, entries: &mut Vec<Value>, depth: usize) -> Result<()> {
    if depth > 10 || entries.len() >= MAX_ENTRIES {
        return Ok(());
    }
    let mut children = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let path = entry.path();
        let relative = path.strip_prefix(root)?;
        let kind = entry.file_type()?;
        let mut item = json!({"path": relative, "kind": if kind.is_symlink() {
            "symlink"
        } else if kind.is_dir() { "directory" } else { "file" }});
        // Do not follow symlinks, including links to files outside the fixture.
        if kind.is_file() {
            item["size"] = entry.metadata()?.len().into();
            let parent = relative.parent().unwrap_or_else(|| Path::new(""));
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let records = parent == Path::new("territory/run/operations")
                || parent == Path::new("territory/transactions");
            let log = matches!(
                relative.to_str(),
                Some(
                    "territory/run/daemon.log"
                        | "territory/run/switch-frontend.log"
                        | "landscape.log"
                        | "install-debug.log"
                )
            );
            let calls = matches!(
                relative.to_str(),
                Some("systemctl-calls.jsonl" | "init-calls.jsonl")
            );
            let marker = parent == Path::new("territory/run")
                && name.starts_with("test-repair-")
                && (name.ends_with(".gate")
                    || name.ends_with(".reached")
                    || name.ends_with(".failed"));
            if records
                && matches!(
                    path.extension().and_then(|x| x.to_str()),
                    Some("json" | "events")
                )
            {
                item["record"] = match read_file(&path)
                    .and_then(|bytes| Ok(serde_json::from_slice::<Value>(&bytes)?))
                {
                    Ok(Value::Array(values)) => {
                        Value::Array(values.iter().take(128).map(summary).collect())
                    }
                    Ok(value) => summary(&value),
                    Err(_) => json!({"unreadable": true}),
                };
            } else if log || calls {
                item["log"] = match read_file(&path) {
                    Ok(bytes) => Value::Array(
                        String::from_utf8_lossy(&bytes)
                            .lines()
                            .take(2000)
                            .map(|line| {
                                if sensitive(line) {
                                    Value::String("[redacted]".into())
                                } else {
                                    Value::String(line.into())
                                }
                            })
                            .collect(),
                    ),
                    Err(_) => json!(["[unreadable or exceeds size limit]"]),
                };
            } else if marker {
                item["checkpoint"] = true.into();
            }
        }
        entries.push(item);
        if kind.is_dir() {
            walk(root, &path, entries, depth + 1)?;
        }
    }
    Ok(())
}

pub fn export(root: &Path, destination: &Path) -> Result<()> {
    anyhow::ensure!(
        std::fs::symlink_metadata(root)?.is_dir(),
        "fixture root is not a real directory"
    );
    let mut entries = Vec::new();
    walk(root, root, &mut entries, 0)?;
    std::fs::create_dir_all(destination)?;
    let output = destination.join("snapshot.json");
    std::fs::write(
        &output,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1, "entry_limit": MAX_ENTRIES, "file_size_limit": MAX_BYTES,
            "entries": entries
        }))?,
    )
    .with_context(|| format!("write fixture diagnostics {}", output.display()))
}
