use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    emit_locale_watch(Path::new("locales"));
}

/// rust-i18n 的 `i18n!` 宏在编译期读取 locale 文件，但宏本身不向 cargo
/// 声明文件依赖。这里递归输出 `rerun-if-changed`，让任何 locale 文件变化
/// 都触发重编译与宏重新展开。
fn emit_locale_watch(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                emit_locale_watch(&entry.path());
            }
        }
    }
}
