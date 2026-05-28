//! i18n message skeleton — V1 hardcoded Chinese, V2 replaces with fluent/ICU.

use std::collections::HashMap;

/// Central message formatter. V1: hardcoded Chinese with simple `{key}` substitution.
/// V2: replace internals with fluent/ICU; call sites stay unchanged.
pub struct CliMessages;

impl CliMessages {
    /// Format a user-facing message. `params` is used for `{key}` substitution.
    pub fn format(key: &str, params: &HashMap<&str, &str>) -> String {
        let template = match key {
            "menu.title" => "Landscape Kit 管理工具",
            "menu.status" => "查看状态",
            "menu.start" => "启动服务",
            "menu.stop" => "停止服务",
            "menu.restart" => "重启服务",
            "menu.logs" => "查看日志",
            "menu.diagnose" => "诊断检查",
            "menu.install" => "安装 Landscape",
            "menu.backup" => "备份",
            "menu.restore" => "恢复",
            "menu.upgrade" => "升级",
            "menu.rollback" => "回滚",
            "menu.config_export" => "导出配置",
            "menu.exit" => "退出",
            "menu.soon_suffix" => "（即将推出）",
            "service.started" => "服务已启动",
            "service.stopped" => "服务已停止",
            "service.restarted" => "服务已重启",
            "not_implemented" => "该功能将在 {milestone} 版本推出",
            "status.systemd.active" => "systemd 服务: 运行中",
            "status.systemd.inactive" => "systemd 服务: 未运行",
            "status.api.ok" => "Landscape API: 可达",
            "status.api.unreachable" => "Landscape API: 不可达",
            "status.version.unknown" => "未知版本",
            "diagnose.pass" => "✓ 通过",
            "diagnose.fail" => "✗ 失败",
            "error.not_installed" => "Landscape 未安装",
            "error.permission_denied" => "权限不足，请使用 sudo",
            "error.not_tty" => "交互式菜单需要终端，请使用子命令（如 lkit status）",
            "error.suggestion.not_installed" => "请先安装 Landscape",
            "error.suggestion.permission" => "请使用 sudo 或以 root 身份运行",
            "error.suggestion.generic" => "请检查日志或使用 -v 获取详细信息",
            "self.version" => "lkit {version}",
            _ => key,
        };
        let mut result = template.to_string();
        for (k, v) in params {
            result = result.replace(&format!("{{{k}}}"), v);
        }
        result
    }
}

/// Convenience: format a message with no parameters.
pub fn msg(key: &str) -> String {
    CliMessages::format(key, &HashMap::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_returns_static_text() {
        assert_eq!(msg("menu.title"), "Landscape Kit 管理工具");
    }

    #[test]
    fn format_substitutes_params() {
        let mut params = HashMap::new();
        params.insert("milestone", "M2");
        assert_eq!(
            CliMessages::format("not_implemented", &params),
            "该功能将在 M2 版本推出"
        );
    }

    #[test]
    fn msg_unknown_key_returns_key() {
        assert_eq!(msg("nonexistent.key"), "nonexistent.key");
    }
}
