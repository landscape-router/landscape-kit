use std::process::{Command, Output};

fn lkit(args: &[&str], language: Option<&str>) -> Output {
    let mut command = base_command(args);
    if let Some(language) = language {
        command.env("LKIT_LANG", language);
    }
    command.output().unwrap()
}

fn lkit_with_system_locale(args: &[&str], name: &str, value: &str) -> Output {
    lkit_with_system_locales(args, &[(name, value)])
}

fn lkit_with_system_locales(args: &[&str], locales: &[(&str, &str)]) -> Output {
    let mut command = base_command(args);
    for (name, value) in locales {
        command.env(name, value);
    }
    command.output().unwrap()
}

fn base_command(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lkit"));
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
    let output = lkit(&["--help"], None);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("Usage: lkit"));
    assert!(text.contains("Check host readiness"));
    assert!(!text.contains("检查主机部署条件"));
}

#[test]
fn environment_selects_chinese_help() {
    let output = lkit(&["--help"], Some("zh"));
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("用法：lkit"));
    assert!(text.contains("检查主机部署条件"));
    assert!(text.contains("打印帮助"));
}

#[test]
fn system_locale_selects_chinese_by_primary_language() {
    for value in ["zh_CN.UTF-8", "zh-CN"] {
        let output = lkit_with_system_locale(&["--help"], "LANG", value);
        assert!(output.status.success());
        assert!(stdout(&output).contains("检查主机部署条件"));
    }
}

#[test]
fn system_locale_uses_standard_precedence() {
    let output = lkit_with_system_locales(
        &["--help"],
        &[("LANG", "en_US.UTF-8"), ("LC_MESSAGES", "zh_CN.UTF-8")],
    );
    assert!(stdout(&output).contains("检查主机部署条件"));

    let output = lkit_with_system_locales(
        &["--help"],
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
    let output = lkit_with_system_locale(&["--help"], "LANG", "fr_FR.UTF-8");
    assert!(output.status.success());
    assert!(stdout(&output).contains("Check host readiness"));
}

#[test]
fn explicit_language_overrides_environment_before_or_after_subcommand() {
    for args in [
        ["--lang", "en", "check", "--help"],
        ["check", "--lang", "en", "--help"],
    ] {
        let output = lkit(&args, Some("zh"));
        assert!(output.status.success());
        let text = stdout(&output);
        assert!(text.contains("Check host readiness"));
        assert!(!text.contains("检查主机部署条件"));
    }
}

#[test]
fn unsupported_environment_language_falls_back_to_english() {
    let output = lkit(&["--help"], Some("zh-CN"));
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("Check host readiness"));
    assert!(!text.contains("检查主机部署条件"));
}

#[test]
fn unsupported_explicit_language_overrides_system_locale_with_english() {
    let output = lkit_with_system_locale(&["--lang", "fr", "--help"], "LANG", "zh_CN.UTF-8");
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("Check host readiness"));
    assert!(!text.contains("检查主机部署条件"));
}

#[test]
fn chinese_localizes_unknown_subcommand_errors() {
    let output = lkit(&["unknown"], Some("zh"));
    assert_eq!(output.status.code(), Some(2));
    let text = stderr(&output);
    assert!(text.contains("错误：无法识别子命令 'unknown'"));
    assert!(text.contains("用法：lkit"));
    assert!(text.contains("更多信息请尝试 '--help'。"));
    assert!(!text.contains("unrecognized subcommand"));
}

#[test]
fn chinese_localizes_invalid_value_errors() {
    let output = lkit(&["check", "--color", "sometimes"], Some("zh"));
    assert_eq!(output.status.code(), Some(2));
    let text = stderr(&output);
    assert!(text.contains("参数 '--color <COLOR>' 的值 'sometimes' 无效"));
    assert!(text.contains("可选值：auto, always, never"));
    assert!(!text.contains("invalid value"));
}
