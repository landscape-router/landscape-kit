use crate::check::model::{CheckReport, Status};
use unicode_width::UnicodeWidthStr;

const STATUS_WIDTH: usize = 7;
const STATUS_COLORS: [(Status, &str); 4] = [
    (Status::Pass, "32"),
    (Status::Warning, "33"),
    (Status::Error, "31"),
    (Status::Unknown, "35"),
];

pub fn render(report: &CheckReport, verbose: bool, color: bool) -> String {
    let mut out = String::new();
    let max_id_width = report
        .groups
        .iter()
        .flat_map(|group| group.results.iter())
        .map(|result| UnicodeWidthStr::width(result.id))
        .max()
        .unwrap_or(0);
    let id_width = max_id_width + 2;

    for (index, group) in report.groups.iter().enumerate() {
        let title = paint(group.title, "1", color);
        out.push_str(&format!("[{}] {title}\n", index + 1));
        out.push_str(&format!(
            "{}│ {}│ 结果\n",
            pad("状态", STATUS_WIDTH),
            pad("检查项", id_width)
        ));
        out.push_str(&format!(
            "{}┼ {}┼{}\n",
            "─".repeat(STATUS_WIDTH),
            "─".repeat(id_width),
            "─".repeat(12)
        ));
        for result in &group.results {
            let label = paint(result.status.label(), status_color(result.status), color);
            out.push_str(&format!(
                "{}│ {}│ {}\n",
                pad(&label, STATUS_WIDTH),
                pad(result.id, id_width),
                result.value
            ));
            if !result.reason.is_empty() {
                out.push_str(&continuation(id_width, &result.reason));
            }
            if !result.suggestion.is_empty() {
                out.push_str(&continuation(id_width, &result.suggestion));
            }
            if verbose {
                out.push_str(&continuation(id_width, &format!("标题：{}", result.title)));
                for detail in &result.details {
                    out.push_str(&continuation(id_width, detail));
                }
            }
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "汇总：{} 通过 / {} 警告 / {} 错误 / {} 未知\n",
        report.counts.pass, report.counts.warning, report.counts.error, report.counts.unknown
    ));
    let conclusion = match report.summary {
        Status::Error => "存在部署阻断项，不建议继续部署。",
        Status::Unknown => "存在无法完成的检查，不能确认环境满足要求。",
        Status::Warning => "未发现硬性错误，存在需人工确认的风险。",
        Status::Pass => "环境满足部署条件，可以继续部署。",
    };
    let conclusion = paint(conclusion, status_color(report.summary), color);
    out.push_str(&format!("结论：{conclusion}\n"));
    out
}

fn continuation(id_width: usize, text: &str) -> String {
    format!(
        "{}│ {}│ {text}\n",
        " ".repeat(STATUS_WIDTH),
        " ".repeat(id_width)
    )
}

fn pad(text: &str, width: usize) -> String {
    let len = UnicodeWidthStr::width(text);
    let mut out = String::with_capacity(width);
    out.push_str(text);
    out.push_str(&" ".repeat(width.saturating_sub(len)));
    out
}

fn status_color(status: Status) -> &'static str {
    STATUS_COLORS
        .iter()
        .find_map(|(s, code)| (*s == status).then_some(*code))
        .unwrap_or("0")
}

fn paint(text: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::model::{CheckGroup, CheckResult, StatusCounts};

    fn sample_report() -> CheckReport {
        let results = vec![
            CheckResult::new("platform.linux", "操作系统").set(
                Status::Pass,
                "linux",
                "系统为 Linux",
            ),
            CheckResult::new("runtime.root", "运行身份").set(
                Status::Error,
                "uid=1000",
                "必须以 root 身份运行",
            ),
        ];
        CheckReport {
            groups: vec![CheckGroup {
                title: "运行身份与平台",
                results,
            }],
            summary: Status::Error,
            counts: StatusCounts {
                pass: 1,
                warning: 0,
                error: 1,
                unknown: 0,
            },
        }
    }

    #[test]
    fn renders_plain_table() {
        let text = render(&sample_report(), false, false);
        assert!(text.contains("[1] 运行身份与平台"));
        assert!(text.contains("│"));
        assert!(text.contains("ERROR"));
        assert!(text.contains("必须以 root 身份运行"));
        assert!(text.contains("结论：存在部署阻断项，不建议继续部署。"));
        assert!(!text.contains("\x1b["));
    }

    #[test]
    fn aligns_columns_by_display_width() {
        let text = render(&sample_report(), false, false);
        let header = text.lines().find(|line| line.contains("状态")).unwrap();
        let row = text.lines().find(|line| line.contains("linux")).unwrap();
        let header_cells: Vec<&str> = header.split('│').collect();
        let row_cells: Vec<&str> = row.split('│').collect();
        assert_eq!(UnicodeWidthStr::width(header_cells[0]), STATUS_WIDTH);
        assert_eq!(UnicodeWidthStr::width(row_cells[0]), STATUS_WIDTH);
        assert_eq!(
            UnicodeWidthStr::width(header_cells[1]),
            UnicodeWidthStr::width(row_cells[1])
        );
    }

    #[test]
    fn colors_when_enabled() {
        let text = render(&sample_report(), false, true);
        assert!(text.contains("\x1b[31m"));
        assert!(text.contains("\x1b[0m"));
    }

    #[test]
    fn verbose_adds_details() {
        let mut report = sample_report();
        report.groups[0].results[1] = report.groups[0].results[1]
            .clone()
            .detail("使用 sudo 后重试");
        let text = render(&report, true, false);
        assert!(text.contains("标题：运行身份"));
        assert!(text.contains("使用 sudo 后重试"));
    }
}
