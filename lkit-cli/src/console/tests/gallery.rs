//! 控制台布局 gallery:仅在 `LKIT_CONSOLE_GALLERY` 指向输出目录时运行,把
//! 快照同款的代表性屏幕渲染成带颜色/加粗/反显的 HTML 终端模拟页
//! (`gallery.html`)与逐屏纯文本(`*.txt`),供人工或 agent 审阅实际布局并
//! 给出修改建议。由 `scripts/console-layout-gallery.sh` 在 docker 内驱动;
//! 未设置环境变量时本测试直接返回,普通测试运行零文件写入。

use super::super::*;
use super::support::*;
use crate::i18n::Language;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

struct ScreenSpec {
    name: &'static str,
    language: Language,
    daemon_running: bool,
    width: u16,
    height: u16,
    prepare: fn(&mut ConsoleApp),
}

/// 与 snapshots.rs 相同的 18 屏(名称一致,便于快照 diff 与 gallery 对照)。
fn screen_specs() -> Vec<ScreenSpec> {
    fn spec(
        name: &'static str,
        language: Language,
        daemon_running: bool,
        width: u16,
        height: u16,
        prepare: fn(&mut ConsoleApp),
    ) -> ScreenSpec {
        ScreenSpec {
            name,
            language,
            daemon_running,
            width,
            height,
            prepare,
        }
    }
    vec![
        spec(
            "overview-navigation-not-installed-en",
            Language::En,
            false,
            100,
            28,
            |app| app.snapshot = Snapshot::NotInstalled,
        ),
        spec(
            "overview-panel-daemon-running-en",
            Language::En,
            true,
            100,
            28,
            |app| {
                app.snapshot = installed_snapshot();
                app.focus = Focus::Panel;
            },
        ),
        spec(
            "overview-panel-daemon-not-running-en",
            Language::En,
            false,
            100,
            28,
            |app| {
                app.snapshot = installed_snapshot();
                app.focus = Focus::Panel;
            },
        ),
        spec("install-form-en", Language::En, false, 100, 28, |app| {
            app.snapshot = Snapshot::NotInstalled;
            app.menu_index = 1;
            app.focus = Focus::Panel;
        }),
        spec("backup-list-en", Language::En, false, 100, 28, |app| {
            app.snapshot = installed_snapshot();
            app.menu_index = 2;
            app.focus = Focus::Panel;
            app.backup.state = BackupListState::Complete(backup_rows());
        }),
        spec("update-panel-en", Language::En, false, 100, 28, |app| {
            *app = update_ready_app();
        }),
        spec("mirror-panel-en", Language::En, false, 100, 28, |app| {
            app.snapshot = installed_snapshot();
            app.menu_index = 4;
            app.focus = Focus::Panel;
        }),
        spec("software-panel-en", Language::En, false, 100, 28, |app| {
            app.snapshot = installed_snapshot();
            app.menu_index = 5;
            app.focus = Focus::Panel;
        }),
        spec("reinit-panel-en", Language::En, false, 100, 28, |app| {
            app.snapshot = installed_snapshot();
            app.menu_index = 6;
            app.focus = Focus::Panel;
        }),
        spec("minimum-72x18-en", Language::En, false, 72, 18, |app| {
            app.snapshot = Snapshot::NotInstalled
        }),
        spec(
            "overview-stacked-74x18-en",
            Language::En,
            false,
            74,
            18,
            |app| {
                app.snapshot = installed_snapshot();
                app.focus = Focus::Panel;
            },
        ),
        spec("too-small-71x17-en", Language::En, false, 71, 17, |app| {
            app.snapshot = Snapshot::NotInstalled
        }),
        spec("too-small-71x17-zh", Language::Zh, false, 71, 17, |app| {
            app.snapshot = Snapshot::NotInstalled
        }),
        spec(
            "overview-navigation-zh",
            Language::Zh,
            false,
            100,
            28,
            |app| app.snapshot = Snapshot::NotInstalled,
        ),
        spec("backup-list-zh", Language::Zh, false, 100, 28, |app| {
            app.snapshot = installed_snapshot();
            app.menu_index = 2;
            app.focus = Focus::Panel;
            app.backup.state = BackupListState::Complete(backup_rows());
        }),
        spec(
            "footer-long-zh-notice-72x18",
            Language::Zh,
            false,
            72,
            18,
            |app| {
                app.snapshot = Snapshot::NotInstalled;
                app.notice = Notice::Error(
                    "网络回滚正在进行中，请勿关闭终端；所有网络配置将在回滚完成后恢复到上一个已知良好的状态。".into(),
                );
            },
        ),
        spec(
            "exit-confirmation-en",
            Language::En,
            false,
            100,
            28,
            |app| {
                app.snapshot = Snapshot::NotInstalled;
                app.exit_state = ExitState::Confirming;
            },
        ),
        spec("preflight-dialog-en", Language::En, false, 100, 28, |app| {
            app.snapshot = Snapshot::NotInstalled;
            app.menu_index = 1;
            app.focus = Focus::Panel;
            app.preflight.state = PreflightState::Complete(error_preflight_report());
            app.preflight_dialog = true;
        }),
    ]
}

#[test]
fn gallery() {
    let Some(out_dir) = std::env::var_os("LKIT_CONSOLE_GALLERY") else {
        return;
    };
    let out_dir = PathBuf::from(out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut sections = String::new();
    for spec in screen_specs() {
        let _language = LanguageGuard::set(spec.language);
        let _territory = DaemonTerritory::new(spec.name, spec.daemon_running);
        let mut terminal = Terminal::new(TestBackend::new(spec.width, spec.height)).unwrap();
        let mut app = ConsoleApp::new();
        (spec.prepare)(&mut app);
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let plain = terminal_content(&terminal);
        std::fs::write(
            out_dir.join(format!("{}.txt", spec.name)),
            format!(
                "== {name} ==\n{width}x{height}, language={language:?}, daemon_running={daemon}\n\n{plain}",
                name = spec.name,
                width = spec.width,
                height = spec.height,
                language = spec.language,
                daemon = spec.daemon_running,
            ),
        )
        .unwrap();
        sections.push_str(&format!(
            "<h2>{name} <small>{width}×{height} · {language} · daemon={daemon}</small></h2>\n<div class=\"term\">{html}</div>\n",
            name = html_escape(spec.name),
            width = spec.width,
            height = spec.height,
            language = if spec.language == Language::Zh { "zh" } else { "en" },
            daemon = spec.daemon_running,
            html = buffer_to_html(terminal.backend().buffer()),
        ));
    }
    let page = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>lkit console layout gallery</title>\
<style>\
body {{ background:#141414; color:#ddd; font-family:sans-serif; margin:24px; }}\
h2 {{ margin:28px 0 6px; font-size:15px; }} h2 small {{ color:#888; font-weight:normal; }}\
.term {{ font-family:'Cascadia Mono','Noto Sans Mono CJK SC',ui-monospace,Menlo,monospace; \
font-size:14px; line-height:17px; background:#000; color:#ccc; \
display:inline-block; padding:8px 10px; border-radius:6px; white-space:pre; }}\
</style></head><body>\
<h1>lkit console layout gallery</h1>\
<p>Rendered from the ratatui TestBackend buffer (deterministic fixtures, same screens as the insta snapshots).</p>\n\
{sections}</body></html>\n"
    );
    std::fs::write(out_dir.join("gallery.html"), page).unwrap();
}

/// 把一帧 Buffer 转成带样式的 HTML:相邻同风格 cell 合并为一个 span,
/// REVERSED 交换前景/背景,宽字符覆盖格跳过(与 ratatui 的 buffer_view 同法)。
fn buffer_to_html(buffer: &ratatui::buffer::Buffer) -> String {
    let width = buffer.area.width as usize;
    let mut html = String::new();
    for row in buffer.content.chunks(width) {
        let mut skip = 0usize;
        let mut run_style = None;
        let mut run_text = String::new();
        let flush =
            |style: &Option<(Color, Color, Modifier)>, text: &mut String, html: &mut String| {
                if text.is_empty() {
                    return;
                }
                let (fg, bg, modifiers) =
                    style.unwrap_or((Color::Reset, Color::Reset, Modifier::empty()));
                let (fg, bg) = if modifiers.contains(Modifier::REVERSED) {
                    (bg, fg)
                } else {
                    (fg, bg)
                };
                let mut css = String::new();
                if let Some(color) = color_css(fg) {
                    css.push_str(&format!("color:{color};"));
                }
                if let Some(color) = color_css(bg) {
                    css.push_str(&format!("background:{color};"));
                }
                if modifiers.contains(Modifier::BOLD) {
                    css.push_str("font-weight:bold;");
                }
                if modifiers.contains(Modifier::DIM) {
                    css.push_str("opacity:.55;");
                }
                if modifiers.contains(Modifier::UNDERLINED) {
                    css.push_str("text-decoration:underline;");
                }
                if modifiers.contains(Modifier::ITALIC) {
                    css.push_str("font-style:italic;");
                }
                html.push_str(&format!(
                    "<span style=\"{css}\">{}</span>",
                    html_escape(text)
                ));
                text.clear();
            };
        for cell in row {
            let symbol = cell.symbol();
            if skip == 0 {
                let style = (cell.fg, cell.bg, cell.modifier);
                if run_style != Some(style) {
                    flush(&run_style, &mut run_text, &mut html);
                    run_style = Some(style);
                }
                run_text.push_str(symbol);
            }
            skip = skip.max(symbol.width()).saturating_sub(1);
        }
        flush(&run_style, &mut run_text, &mut html);
        html.push('\n');
    }
    html
}

fn color_css(color: Color) -> Option<&'static str> {
    let hex = match color {
        Color::Reset => return None,
        Color::Black => "#000000",
        Color::Red => "#c0392b",
        Color::Green => "#0dbc79",
        Color::Yellow => "#e5e510",
        Color::Blue => "#2472c8",
        Color::Magenta => "#bc3fbc",
        Color::Cyan => "#29b8db",
        Color::Gray => "#e5e5e5",
        Color::DarkGray => "#6e6e6e",
        Color::LightRed => "#f14c4c",
        Color::LightGreen => "#23d18b",
        Color::LightYellow => "#f5f543",
        Color::LightBlue => "#3b8eea",
        Color::LightMagenta => "#d670d6",
        Color::LightCyan => "#29b8db",
        Color::White => "#ffffff",
        Color::Indexed(_) | Color::Rgb(_, _, _) => "#cccccc",
    };
    Some(hex)
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
