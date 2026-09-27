//! 布局不变量:几何坐标断言 + 宽度/高度扫描。
//! 快照(snapshots.rs)固定“具体长什么样”,这里固定“任何允许的终端尺寸下
//! 都必须成立的结构性质”:
//! - 72×18 最小尺寸边界两侧,too-small 提示恰好出现/消失;
//! - 底栏提示按预折行逐行完整可见、长通知的结尾词仍在屏上——预留高度不足
//!   导致底行被截断的 bug 类(048fa3b、c786c61)在任意宽度下都会被抓到;
//! - header 底边、侧栏/面板分栏、底栏分隔线、语言指示右对齐的几何坐标。

use super::super::widgets::wrap_to_width;
use super::super::*;
use super::support::*;
use crate::i18n::Language;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

#[test]
fn too_small_notice_appears_exactly_below_72x18() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("boundary", false);
    // 极窄终端上提示行本身会被截断(见优化点清单),只能锚定稳定前缀。
    let notice = "Terminal too small";
    for (width, height, expect_small) in [
        (40, 10, true),
        (71, 28, true),
        (72, 17, true),
        (72, 18, false),
        (73, 18, false),
        (120, 40, false),
    ] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut app = ConsoleApp::new();
        app.snapshot = Snapshot::NotInstalled;
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let content = terminal_content(&terminal);
        assert_eq!(
            content.contains(notice),
            expect_small,
            "too-small notice at {width}x{height}"
        );
        if !expect_small {
            assert!(
                content.contains("Navigation"),
                "the menu must render at {width}x{height}"
            );
        }
    }
}

/// 宽度 40..=120 × 高度 {18, 24, 40} 全扫描:每个代表性状态下,底栏提示的
/// 每一行预折行结果都必须完整出现在屏上,长通知的结尾词也必须可见。
#[test]
fn footer_content_survives_every_width_and_height() {
    type ScreenState = (&'static str, Language, Box<dyn Fn(&mut ConsoleApp)>);
    let states: [ScreenState; 5] = [
        (
            "overview-en",
            Language::En,
            Box::new(|app| app.snapshot = Snapshot::NotInstalled),
        ),
        (
            "overview-zh",
            Language::Zh,
            Box::new(|app| app.snapshot = Snapshot::NotInstalled),
        ),
        (
            "backup-en",
            Language::En,
            Box::new(|app| {
                app.snapshot = installed_snapshot();
                app.menu_index = 2;
                app.focus = Focus::Panel;
                app.backup.state = BackupListState::Complete(vec![sample_backup_entry()]);
            }),
        ),
        (
            "install-en",
            Language::En,
            Box::new(|app| {
                app.snapshot = Snapshot::NotInstalled;
                app.menu_index = 1;
                app.focus = Focus::Panel;
            }),
        ),
        (
            "long-notice-en",
            Language::En,
            Box::new(|app| {
                app.snapshot = Snapshot::NotInstalled;
                app.notice = Notice::Error(
                    "the deployment repository could not be reached; verify the mirror endpoint \
                     and retry once connectivity is restored."
                        .into(),
                );
            }),
        ),
    ];
    let _territory = DaemonTerritory::new("sweep", false);
    for (name, language, prepare) in states {
        let _language = LanguageGuard::set(language);
        for height in [18u16, 24, 40] {
            for width in 40u16..=120 {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                let mut app = ConsoleApp::new();
                prepare(&mut app);
                terminal.draw(|frame| render(frame, &mut app)).unwrap();
                let content = terminal_content(&terminal);
                // 两种语言的 too-small 提示前缀(极窄时提示行会被截断,只能认前缀)。
                let too_small =
                    content.contains("Terminal too small") || content.contains("终端尺寸过小");
                if too_small {
                    continue;
                }
                // 提示按整屏内容宽度预折行(render_status 与此处共用 wrap_to_width),
                // 每行都必须完整落在屏上。
                for line in wrap_to_width(width, &app.hints()) {
                    assert!(
                        content.contains(&line),
                        "{name}: hints line {line:?} missing at {width}x{height}"
                    );
                }
                // 通知结尾词若被预留高度截掉即失败(词级折行不会拆开单词)。
                if let Some(tail) = app.notice.text().split_whitespace().next_back() {
                    assert!(
                        content.contains(tail),
                        "{name}: notice tail {tail:?} missing at {width}x{height}"
                    );
                }
            }
        }
    }
}

/// 100×28 规范尺寸下的结构坐标:header 占前两行、侧栏 24 列、面板从第 25 列
/// 起、底栏分隔线在倒数第三行、语言指示右对齐贴住最后一列。
#[test]
fn layout_geometry_is_stable_at_canonical_size() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("geometry", false);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.snapshot = Snapshot::NotInstalled;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let buffer = terminal.backend().buffer();

    // header 底边横线横贯整行(行 1)。
    for x in [0u16, 50, 99] {
        assert_eq!(
            buffer[(x, 1)].symbol(),
            "─",
            "header bottom border at x={x}"
        );
    }
    // 侧栏块(24 列宽)与面板块的左上角。
    assert_eq!(buffer[(0, 2)].symbol(), "┌", "navigation corner");
    assert_eq!(buffer[(23, 2)].symbol(), "┐", "navigation right border");
    assert_eq!(buffer[(24, 2)].symbol(), "┌", "panel starts at x=24");

    // 默认底栏(空通知 + 单行提示)共 3 行:分隔线在第 25 行,提示在第 27 行。
    assert_eq!(buffer[(50, 25)].symbol(), "─", "status separator row");
    let hints = wrap_to_width(100, &app.hints()).join(" ");
    assert!(
        terminal_content(&terminal)
            .lines()
            .nth(27)
            .is_some_and(|row| row.contains(hints.trim())),
        "hints must sit on the last row"
    );

    // 语言指示右对齐:行 26 的最后 4 列是 "(zh)"(英文界面显示目标语言 zh)。
    let tail: String = [96u16, 97, 98, 99]
        .iter()
        .map(|x| buffer[(*x, 26)].symbol())
        .collect();
    assert_eq!(tail, "(zh)", "language indicator right-aligned at row 26");
}
