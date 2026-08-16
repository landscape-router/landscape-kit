use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// 每个测试独立的临时世界:lkit 地盘(territory)与 landscape 根(landscape)。
fn empty_world(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("lkit-i18n-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let territory = root.join("territory");
    let landscape = root.join("landscape");
    std::fs::create_dir_all(&territory).unwrap();
    std::fs::create_dir_all(&landscape).unwrap();
    (territory, landscape)
}

/// 写入让 `backup list` 可到达"无备份"分支的最小有效安装状态:
/// 状态位于地盘 `state/install-state.json`,landscape 根记录在状态中。
fn write_backup_state(territory: &Path, landscape: &Path) {
    std::fs::create_dir_all(territory.join("state")).unwrap();
    std::fs::write(
        territory.join("state/install-state.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "layout_version": 2,
            "install_root": landscape.display().to_string(),
            "canonical_install_root": landscape.display().to_string(),
            "active_version": "1.2.3",
            "assets": {
                "webserver": {
                    "architecture": "x86_64",
                    "sha256": "a".repeat(64),
                    "size": 10,
                },
                "static_archive": {
                    "sha256": "b".repeat(64),
                    "size": 20,
                },
            },
            "initialization": {
                "status": "complete",
                "lock_present": true,
                "initialized_at": "2026-08-01T16:30:00Z",
            },
            "service": {
                "manager": "systemd",
                "registered": true,
                "enabled": true,
                "verified": true,
                "definition_path": "service/landscape-router.service",
                "definition_sha256": "c".repeat(64),
            },
            "last_transaction_id": null,
            "committed_at": "2026-08-01T16:30:00Z",
        }))
        .unwrap(),
    )
    .unwrap();
}

fn lkit(args: &[&str], territory: &Path, language: Option<&str>) -> Output {
    let mut command = base_command(args, territory);
    if let Some(language) = language {
        command.env("LKIT_LANG", language);
    }
    command.output().unwrap()
}

fn lkit_with_system_locale(args: &[&str], territory: &Path, name: &str, value: &str) -> Output {
    lkit_with_system_locales(args, territory, &[(name, value)])
}

fn lkit_with_system_locales(args: &[&str], territory: &Path, locales: &[(&str, &str)]) -> Output {
    let mut command = base_command(args, territory);
    for (name, value) in locales {
        command.env(name, value);
    }
    command.output().unwrap()
}

/// 所有子进程都把 lkit 地盘指到本测试的临时目录,避免读到/写到真实
/// `/root/.lkit/`(config.toml、状态等)。config.toml 现在位于地盘。
fn base_command(args: &[&str], territory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lkit"));
    command.env("LKIT_TERRITORY", territory);
    command.args(args);
    for name in ["LKIT_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
        command.env_remove(name);
    }
    command
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn defaults_to_english_help() {
    let (territory, _landscape) = empty_world("help-default");
    let output = lkit(&["--help"], &territory, None);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("Usage: lkit"));
    assert!(text.contains("Check host readiness"));
    assert!(!text.contains("检查主机部署条件"));
}

#[test]
fn environment_selects_chinese_help() {
    let (territory, _landscape) = empty_world("help-zh");
    let output = lkit(&["--help"], &territory, Some("zh"));
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("用法：lkit"));
    assert!(text.contains("检查主机部署条件"));
    assert!(text.contains("打印帮助"));
}

#[test]
fn system_locale_selects_chinese_by_primary_language() {
    let (territory, _landscape) = empty_world("help-locale");
    for value in ["zh_CN.UTF-8", "zh-CN"] {
        let output = lkit_with_system_locale(&["--help"], &territory, "LANG", value);
        assert!(output.status.success());
        assert!(stdout(&output).contains("检查主机部署条件"));
    }
}

#[test]
fn system_locale_uses_standard_precedence() {
    let (territory, _landscape) = empty_world("help-precedence");
    let output = lkit_with_system_locales(
        &["--help"],
        &territory,
        &[("LANG", "en_US.UTF-8"), ("LC_MESSAGES", "zh_CN.UTF-8")],
    );
    assert!(stdout(&output).contains("检查主机部署条件"));

    let output = lkit_with_system_locales(
        &["--help"],
        &territory,
        &[
            ("LANG", "zh_CN.UTF-8"),
            ("LC_MESSAGES", "zh_CN.UTF-8"),
            ("LC_ALL", "en_US.UTF-8"),
        ],
    );
    assert!(stdout(&output).contains("Check host readiness"));
}

#[test]
fn unsupported_system_locale_falls_back_to_english() {
    let (territory, _landscape) = empty_world("help-unsupported-locale");
    let output = lkit_with_system_locale(&["--help"], &territory, "LANG", "fr_FR.UTF-8");
    assert!(output.status.success());
    assert!(stdout(&output).contains("Check host readiness"));
}

#[test]
fn explicit_language_overrides_environment_before_or_after_subcommand() {
    let (territory, _landscape) = empty_world("help-explicit");
    for args in [
        ["--lang", "en", "check", "--help"],
        ["check", "--lang", "en", "--help"],
    ] {
        let output = lkit(&args, &territory, Some("zh"));
        assert!(output.status.success());
        let text = stdout(&output);
        assert!(text.contains("Check host readiness"));
        assert!(!text.contains("检查主机部署条件"));
    }
}

#[test]
fn unsupported_environment_language_falls_back_to_english() {
    let (territory, _landscape) = empty_world("help-env-unsupported");
    let output = lkit(&["--help"], &territory, Some("zh-CN"));
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("Check host readiness"));
    assert!(!text.contains("检查主机部署条件"));
}

#[test]
fn unsupported_explicit_language_overrides_system_locale_with_english() {
    let (territory, _landscape) = empty_world("help-explicit-unsupported");
    let output = lkit_with_system_locale(
        &["--lang", "fr", "--help"],
        &territory,
        "LANG",
        "zh_CN.UTF-8",
    );
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("Check host readiness"));
    assert!(!text.contains("检查主机部署条件"));
}

#[test]
fn chinese_localizes_unknown_subcommand_errors() {
    let (territory, _landscape) = empty_world("error-subcommand");
    let output = lkit(&["unknown"], &territory, Some("zh"));
    assert_eq!(output.status.code(), Some(2));
    let text = stderr(&output);
    assert!(text.contains("错误：无法识别子命令 'unknown'"));
    assert!(text.contains("用法：lkit"));
    assert!(text.contains("更多信息请尝试 '--help'。"));
    assert!(!text.contains("unrecognized subcommand"));
}

#[test]
fn chinese_localizes_invalid_value_errors() {
    let (territory, _landscape) = empty_world("error-invalid");
    let output = lkit(&["check", "--color", "sometimes"], &territory, Some("zh"));
    assert_eq!(output.status.code(), Some(2));
    let text = stderr(&output);
    assert!(text.contains("参数 '--color <COLOR>' 的值 'sometimes' 无效"));
    assert!(text.contains("可选值：auto, always, never"));
    assert!(!text.contains("invalid value"));
}

fn with_config_language(territory: &Path, language: &str) {
    std::fs::write(
        territory.join("config.toml"),
        format!(
            "schema_version = 1\n\n[repository]\nkind = \"github\"\nlocation = \"ThisSeanZhang/landscape\"\n\n[ui]\nlanguage = \"{language}\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn config_presets_chinese_language() {
    let (territory, landscape) = empty_world("config-zh");
    with_config_language(&territory, "zh");
    write_backup_state(&territory, &landscape);
    let output = lkit(&["backup", "list"], &territory, None);
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(text.contains("在") && text.contains("下没有找到 .lkb 备份"));
    assert!(!text.contains("no .lkb backups found"));
}

#[test]
fn explicit_or_environment_language_overrides_config_preset() {
    let (territory, landscape) = empty_world("config-override");
    with_config_language(&territory, "zh");
    write_backup_state(&territory, &landscape);
    for args in [
        vec!["--lang", "en", "backup", "list"],
        vec!["backup", "list", "--lang", "en"],
    ] {
        let output = lkit(&args, &territory, None);
        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains("no .lkb backups found"));
        assert!(!stderr(&output).contains("下没有找到 .lkb 备份"));
    }
    let output = lkit(&["backup", "list"], &territory, Some("en"));
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no .lkb backups found"));
}

#[test]
fn unsupported_or_corrupt_config_language_falls_back_to_english() {
    let (territory, landscape) = empty_world("config-unsupported");
    with_config_language(&territory, "fr");
    write_backup_state(&territory, &landscape);
    let output = lkit(&["backup", "list"], &territory, None);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no .lkb backups found"));

    let (territory, landscape) = empty_world("config-corrupt");
    write_backup_state(&territory, &landscape);
    std::fs::write(territory.join("config.toml"), b"not toml [[[").unwrap();
    let output = lkit(&["backup", "list"], &territory, None);
    assert_eq!(
        output.status.code(),
        Some(1),
        "corrupt config must not block the command"
    );
    assert!(stderr(&output).contains("no .lkb backups found"));
}
