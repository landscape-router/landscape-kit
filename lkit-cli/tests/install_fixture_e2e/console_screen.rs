//! termlens 屏幕级 console e2e:在真 PTY 中运行 `lkit`,VT 解码后断言
//! "用户看到的屏幕"而非原始字节流。全角中文在 ratatui 增量 diff 字节流中
//! 不连续的问题在此消失,锚点可以直接使用本地化文本(对比 console.rs 中
//! 只能锚定 ASCII 巧合锚点的旧测试);等待一律用有界 `wait_until`,不用 sleep。

use std::time::Duration;

use super::support::*;
use termlens::Key;

const TIMEOUT: Duration = Duration::from_secs(10);

fn screen_terminal(world_name: &str, cols: u16, rows: u16) -> (termlens::Terminal, TestWorld) {
    let world = TestWorld::new(world_name);
    let terminal = termlens::Terminal::builder()
        .size(cols, rows)
        .env_clear()
        .env("LKIT_TERRITORY", world.path("territory"))
        .timeout(TIMEOUT)
        .spawn(LKIT)
        .unwrap();
    (terminal, world)
}

/// 中文会话:侧栏与底栏可以直接锚定中文文本;`l` 切到英文后底栏显示目标
/// 语言中文,`[ui] language` 写回配置;双 Esc + Enter 退出干净。
#[test]
fn zh_session_anchors_on_localized_screen_text() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let world = TestWorld::new("termlens-zh");
    let config_path = world.path("territory").join("config.toml");
    let mut terminal = termlens::Terminal::builder()
        .size(100, 28)
        .env_clear()
        .env("LKIT_TERRITORY", world.path("territory"))
        .env("LKIT_LANG", "zh")
        .timeout(TIMEOUT)
        .spawn(LKIT)
        .unwrap();

    terminal
        .wait_until(|screen| screen.contains("导航"))
        .unwrap();
    // 中文底栏显示目标语言:"[L] 切换到 English (en)"。
    terminal
        .wait_until(|screen| screen.contains("切换到 English"))
        .unwrap();
    assert!(terminal.screen().alternate_screen());

    terminal.send(Key::Char('l')).unwrap();
    // 中文会话按 `l` 切到英文:底栏变为 "[L] Switch to 中文 (zh)",写回 en。
    terminal
        .wait_until(|screen| screen.contains("Switch to 中文"))
        .unwrap();
    let config = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        config.contains("language = \"en\""),
        "config after zh->en toggle: {config}"
    );

    // 再切回中文,验证两个方向的写回都持久化。
    terminal.send(Key::Char('l')).unwrap();
    terminal
        .wait_until(|screen| screen.contains("切换到 English"))
        .unwrap();
    let config = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        config.contains("language = \"zh\""),
        "config after en->zh toggle: {config}"
    );

    // 从中文界面退出:退出锚点全部使用中文——这正是旧字节流测试做不到的
    // (全角文本在增量 diff 字节流中不连续,只能靠 ASCII 巧合锚点 + sleep)。
    terminal.send(Key::Esc).unwrap();
    terminal
        .wait_until(|screen| screen.contains("退出已预备"))
        .unwrap();
    terminal.send(Key::Esc).unwrap();
    terminal
        .wait_until(|screen| screen.contains("确认退出"))
        .unwrap();
    terminal.send(Key::Enter).unwrap();
    assert!(terminal.wait_exit().unwrap().success());
    assert!(
        !terminal.screen().alternate_screen(),
        "console must leave the alternate screen on exit"
    );
}

/// 尺寸路径:100x28 正常渲染,收到 SIGWINCH 缩到 60x14 后进入 too-small
/// 提示屏(菜单消失),再放大回 100x28 恢复完整布局,最后正常退出。
#[test]
fn resize_roundtrips_through_the_too_small_screen() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut terminal, _world) = screen_terminal("termlens-resize", 100, 28);
    terminal
        .wait_until(|screen| screen.contains("Navigation"))
        .unwrap();

    terminal.resize(60, 14).unwrap();
    terminal
        .wait_until(|screen| screen.contains("Terminal too small"))
        .unwrap();
    assert!(
        !terminal.screen().contains("Navigation"),
        "the menu must disappear on the too-small screen"
    );

    terminal.resize(100, 28).unwrap();
    terminal
        .wait_until(|screen| screen.contains("Navigation"))
        .unwrap();

    terminal.send(Key::Esc).unwrap();
    terminal
        .wait_until(|screen| screen.contains("Exit armed"))
        .unwrap();
    terminal.send(Key::Esc).unwrap();
    terminal
        .wait_until(|screen| screen.contains("Exit Landscape Kit?"))
        .unwrap();
    terminal.send(Key::Enter).unwrap();
    assert!(terminal.wait_exit().unwrap().success());
}
