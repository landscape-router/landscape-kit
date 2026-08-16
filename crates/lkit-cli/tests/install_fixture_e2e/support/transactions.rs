use std::path::{Path, PathBuf};

/// 读取 lkit 地盘 `transactions/` 下的唯一事务文件。
pub(crate) fn read_only_transaction(territory: &Path) -> serde_json::Value {
    let paths: Vec<PathBuf> = std::fs::read_dir(territory.join("transactions"))
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            (path.extension().and_then(|value| value.to_str()) == Some("json")).then_some(path)
        })
        .collect();
    assert_eq!(paths.len(), 1);
    serde_json::from_slice(&std::fs::read(&paths[0]).unwrap()).unwrap()
}

/// 在已有多个事务(如接管安装 + reinit)的地盘 transactions/ 中按 operation 查找事务。
pub(crate) fn transaction_of_operation(territory: &Path, operation: &str) -> serde_json::Value {
    let paths: Vec<PathBuf> = std::fs::read_dir(territory.join("transactions"))
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            (path.extension().and_then(|value| value.to_str()) == Some("json")).then_some(path)
        })
        .collect();
    let mut found = None;
    for path in paths {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if value["operation"] == operation {
            found = Some(value);
        }
    }
    found.unwrap_or_else(|| panic!("no {operation} transaction found"))
}

pub(crate) fn transaction_count(territory: &Path) -> usize {
    std::fs::read_dir(territory.join("transactions"))
        .unwrap()
        .filter_map(Result::ok)
        .count()
}
