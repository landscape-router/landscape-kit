use std::process::ExitCode;

use clap::Args;

use crate::interaction::interactive::Tty;
use crate::interaction::plan;
use crate::mirror::apt::parse::ParseIssueKind;
use crate::mirror::{self, Family, Host, MirrorError, MirrorName, MirrorStatus};

#[derive(Debug, Args)]
pub struct SetMirror {
    /// Mirror to apply: official, ustc, tencent, huawei, aliyun, nju, sjtu, zju, lzu, hust, bfsu or tuna
    #[arg(value_enum, value_name = "MIRROR")]
    pub mirror: Option<MirrorName>,
    /// List available mirrors for this host
    #[arg(long, conflicts_with_all = ["mirror", "show", "restore", "check"])]
    pub list: bool,
    /// Show the current package sources
    #[arg(long, conflicts_with_all = ["mirror", "list", "restore", "check"])]
    pub show: bool,
    /// Restore the backed-up original package sources
    #[arg(long, conflicts_with_all = ["mirror", "list", "show", "check"])]
    pub restore: bool,
    /// Check the source file format (read-only; apt only)
    #[arg(long, conflicts_with_all = ["mirror", "list", "show", "restore"])]
    pub check: bool,
    /// Also replace the Debian security repository (kept official by default)
    #[arg(long)]
    pub replace_security: bool,
    /// Keep the CD-ROM source entry (commented out by default; apt only)
    #[arg(long)]
    pub keep_cdrom: bool,
    /// Skip the interactive confirmation
    #[arg(long)]
    pub yes: bool,
}

pub fn run(args: &SetMirror) -> ExitCode {
    let host = match mirror::detect_host() {
        Ok(host) => host,
        Err(error) => {
            eprintln!("set-mirror: {error}");
            return ExitCode::FAILURE;
        }
    };
    if args.check {
        return run_check(&host);
    }
    if args.list {
        return list_mirrors(&host);
    }
    if args.show {
        return show_sources(&host);
    }
    if args.restore {
        return run_restore(&host, args.yes);
    }
    match args.mirror {
        Some(mirror) => {
            let status = mirror::probe(&host, mirror);
            run_apply(
                &host,
                mirror,
                args.yes,
                args.replace_security,
                !args.keep_cdrom,
                status,
            )
        }
        None => run_interactive(&host),
    }
}

fn list_mirrors(host: &Host) -> ExitCode {
    println!(
        "set-mirror: {}",
        crate::tr!(
            crate::keys::SET_MIRROR_LIST_HEADER,
            family = host.family.label()
        )
    );
    let statuses = mirror::probe_all(host);
    for (index, mirror) in mirror::list_mirrors().into_iter().enumerate() {
        let marker = match statuses
            .get(&mirror)
            .copied()
            .unwrap_or(MirrorStatus::Unknown)
        {
            MirrorStatus::Available => crate::tr!(crate::keys::MIRROR_STATUS_AVAILABLE),
            MirrorStatus::Unavailable => crate::tr!(crate::keys::MIRROR_STATUS_UNAVAILABLE),
            MirrorStatus::Unknown => crate::tr!(crate::keys::MIRROR_STATUS_UNKNOWN),
        };
        println!(
            "  {}. {} ({}) [{}]",
            index + 1,
            mirror.label(),
            mirror.id(),
            marker
        );
    }
    ExitCode::SUCCESS
}

fn show_sources(host: &Host) -> ExitCode {
    match mirror::show_sources(host) {
        Ok(content) => {
            println!(
                "set-mirror: {}",
                crate::tr!(crate::keys::SET_MIRROR_SHOW_HEADER)
            );
            print!("{content}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("set-mirror: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_apply(
    host: &Host,
    mirror: MirrorName,
    yes: bool,
    replace_security: bool,
    disable_cdrom: bool,
    status: MirrorStatus,
) -> ExitCode {
    match status {
        MirrorStatus::Unavailable => {
            return fail(crate::tr!(
                crate::keys::SET_MIRROR_MIRROR_UNAVAILABLE,
                mirror = mirror.label(),
                family = host.family.label()
            ));
        }
        MirrorStatus::Unknown => {
            // 探测失败（离线/超时/TLS）：警告后继续，不阻断用户显式换源。
            eprintln!(
                "set-mirror: {}",
                crate::tr!(
                    crate::keys::SET_MIRROR_AVAILABILITY_UNKNOWN,
                    mirror = mirror.label(),
                    family = host.family.label()
                )
            );
        }
        MirrorStatus::Available => {}
    }
    if let Err(error) = require_root() {
        return fail(error);
    }
    if !yes && !crate::interaction::interactive::is_non_interactive() {
        let mut tty = match Tty::open() {
            Ok(tty) => tty,
            Err(error) => return fail_install(&error),
        };
        let confirmed = match tty.confirm(&crate::tr!(
            crate::keys::SET_MIRROR_CONFIRM_APPLY,
            family = host.family.label(),
            mirror = mirror.label()
        )) {
            Ok(confirmed) => confirmed,
            Err(error) => return fail_install(&error),
        };
        if !confirmed {
            println!(
                "set-mirror: {}",
                crate::tr!(crate::keys::SET_MIRROR_CANCELLED)
            );
            return ExitCode::FAILURE;
        }
    }
    match mirror::apply(host, mirror, replace_security, disable_cdrom) {
        Ok(report) if report.changed_files == 0 => {
            println!(
                "set-mirror: {}",
                crate::tr!(
                    crate::keys::SET_MIRROR_NO_CHANGE,
                    family = host.family.label(),
                    mirror = mirror.label()
                )
            );
            ExitCode::SUCCESS
        }
        Ok(report) => {
            println!(
                "set-mirror: {}",
                crate::tr!(
                    crate::keys::SET_MIRROR_APPLIED,
                    family = host.family.label(),
                    mirror = mirror.label(),
                    files = report.changed_files
                )
            );
            match report.fallback {
                Some(crate::mirror::Fallback::CdromConverted) => {
                    println!(
                        "set-mirror: {}",
                        crate::tr!(crate::keys::SET_MIRROR_CDROM_CONVERTED)
                    );
                }
                Some(crate::mirror::Fallback::CdromDisabled) => {
                    println!(
                        "set-mirror: {}",
                        crate::tr!(crate::keys::SET_MIRROR_CDROM_DISABLED)
                    );
                }
                Some(crate::mirror::Fallback::SourceAdded) => {
                    println!(
                        "set-mirror: {}",
                        crate::tr!(
                            crate::keys::SET_MIRROR_SOURCE_ADDED,
                            family = host.family.label()
                        )
                    );
                }
                None => {}
            }
            if report.cdrom_commented > 0 {
                println!(
                    "set-mirror: {}",
                    crate::tr!(
                        crate::keys::SET_MIRROR_CDROM_COMMENTED,
                        count = report.cdrom_commented
                    )
                );
            }
            if report.unrecognized_lines > 0 {
                println!(
                    "set-mirror: {}",
                    crate::tr!(
                        crate::keys::SET_MIRROR_UNRECOGNIZED_LINES,
                        count = report.unrecognized_lines
                    )
                );
            }
            if let Some(path) = &report.backup_path {
                println!(
                    "set-mirror: {}",
                    crate::tr!(crate::keys::SET_MIRROR_BACKUP_AT, path = path.display())
                );
            }
            if report.skipped_repositories > 0 {
                println!(
                    "set-mirror: {}",
                    crate::tr!(
                        crate::keys::SET_MIRROR_SKIPPED,
                        count = report.skipped_repositories
                    )
                );
            }
            refresh_after_change(host);
            ExitCode::SUCCESS
        }
        Err(error) => fail(error),
    }
}

/// 换源成功后刷新软件包索引,让新源立即生效。刷新失败只警告,
/// 不把已写入的源文件回滚(换源本身已成功)。
fn refresh_after_change(host: &Host) {
    println!(
        "set-mirror: {}",
        crate::tr!(crate::keys::SET_MIRROR_REFRESHING)
    );
    if let Err(error) = crate::mirror::refresh::refresh_index(host.family, true) {
        eprintln!(
            "set-mirror: {}",
            crate::tr!(crate::keys::SET_MIRROR_REFRESH_FAILED, error = error)
        );
    }
}

fn run_restore(host: &Host, yes: bool) -> ExitCode {
    if let Err(error) = require_root() {
        return fail(error);
    }
    if !yes && !crate::interaction::interactive::is_non_interactive() {
        let mut tty = match Tty::open() {
            Ok(tty) => tty,
            Err(error) => return fail_install(&error),
        };
        let confirmed = match tty.confirm(&crate::tr!(
            crate::keys::SET_MIRROR_CONFIRM_RESTORE,
            family = host.family.label()
        )) {
            Ok(confirmed) => confirmed,
            Err(error) => return fail_install(&error),
        };
        if !confirmed {
            println!(
                "set-mirror: {}",
                crate::tr!(crate::keys::SET_MIRROR_CANCELLED)
            );
            return ExitCode::FAILURE;
        }
    }
    match mirror::restore(host) {
        Ok(()) => {
            println!(
                "set-mirror: {}",
                crate::tr!(crate::keys::SET_MIRROR_RESTORED)
            );
            refresh_after_change(host);
            ExitCode::SUCCESS
        }
        Err(error) => fail(error),
    }
}

/// 只读格式检查：逐文件列出无法识别的行，存在问题时返回退出码 `1`。
fn run_check(host: &Host) -> ExitCode {
    if !matches!(host.family, Family::Debian | Family::Ubuntu) {
        println!(
            "set-mirror: {}",
            crate::tr!(crate::keys::SET_MIRROR_CHECK_NOT_APT)
        );
        return ExitCode::SUCCESS;
    }
    match mirror::check_format(host) {
        Ok(report) if report.is_empty() => {
            println!(
                "set-mirror: {}",
                crate::tr!(crate::keys::SET_MIRROR_CHECK_CLEAN)
            );
            ExitCode::SUCCESS
        }
        Ok(report) => {
            let mut count = 0usize;
            for (path, issues) in &report {
                for issue in issues {
                    println!(
                        "set-mirror: {}: {}",
                        path.display(),
                        crate::tr!(
                            crate::keys::SET_MIRROR_CHECK_ISSUE,
                            line = issue.line,
                            detail = issue_label(issue.kind)
                        )
                    );
                    count += 1;
                }
            }
            eprintln!(
                "set-mirror: {}",
                crate::tr!(
                    crate::keys::SET_MIRROR_CHECK_SUMMARY,
                    count = count,
                    files = report.len()
                )
            );
            ExitCode::from(1)
        }
        Err(error) => fail(error),
    }
}

fn issue_label(kind: ParseIssueKind) -> String {
    match kind {
        ParseIssueKind::NotADebLine => {
            crate::tr!(crate::keys::SET_MIRROR_ISSUE_NOT_A_DEB_LINE)
        }
        ParseIssueKind::MissingUri => crate::tr!(crate::keys::SET_MIRROR_ISSUE_MISSING_URI),
        ParseIssueKind::NotAField => crate::tr!(crate::keys::SET_MIRROR_ISSUE_NOT_A_FIELD),
        ParseIssueKind::StanzaWithoutUris => {
            crate::tr!(crate::keys::SET_MIRROR_ISSUE_STANZA_WITHOUT_URIS)
        }
    }
}

/// 无参数且非交互：需要至少一个参数。
fn run_interactive(host: &Host) -> ExitCode {
    if crate::interaction::interactive::is_non_interactive() {
        return fail_usage(crate::tr!(crate::keys::SET_MIRROR_REQUIRES_ARGS));
    }
    let statuses = mirror::probe_all(host);
    let selectable: Vec<MirrorName> = mirror::list_mirrors()
        .into_iter()
        .filter(|mirror| {
            statuses
                .get(mirror)
                .copied()
                .unwrap_or(MirrorStatus::Unknown)
                != MirrorStatus::Unavailable
        })
        .collect();
    let unavailable: Vec<String> = mirror::list_mirrors()
        .into_iter()
        .filter(|mirror| {
            statuses
                .get(mirror)
                .copied()
                .unwrap_or(MirrorStatus::Unknown)
                == MirrorStatus::Unavailable
        })
        .map(|mirror| mirror.label())
        .collect();
    if !unavailable.is_empty() {
        eprintln!(
            "set-mirror: {}",
            crate::tr!(
                crate::keys::SET_MIRROR_SKIPPED_UNAVAILABLE,
                mirrors = unavailable.join(", ")
            )
        );
    }
    let mut tty = match Tty::open() {
        Ok(tty) => tty,
        Err(error) => return fail_install(&error),
    };
    let options: Vec<String> = selectable.iter().map(|mirror| mirror.label()).collect();
    if options.is_empty() {
        return fail(crate::tr!(crate::keys::SET_MIRROR_ALL_UNAVAILABLE));
    }
    let selected = match tty.select_one(
        &crate::tr!(crate::keys::SET_MIRROR_SELECT_MIRROR),
        &options,
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => return fail_install(&error),
    };
    let mirror = selectable[selected];
    let status = statuses
        .get(&mirror)
        .copied()
        .unwrap_or(MirrorStatus::Unknown);
    // 仅 apt 家族且源文件里确实有启用的 CD 源时，询问是否注释（默认注释）。
    let disable_cdrom = if matches!(host.family, Family::Debian | Family::Ubuntu)
        && mirror::has_enabled_cdrom(host)
    {
        match select_cdrom(&mut tty) {
            Ok(disable) => disable,
            Err(code) => return code,
        }
    } else {
        true
    };
    if mirror == MirrorName::Official {
        // 恢复官方源没有 security 选择：全部恢复为官方。
        return run_apply(host, mirror, false, true, disable_cdrom, status);
    }
    let replace_security = match select_security(&mut tty, host) {
        Ok(replace) => replace,
        Err(code) => return code,
    };
    run_apply(host, mirror, false, replace_security, disable_cdrom, status)
}

/// 询问是否注释 `deb cdrom:` 条目，默认注释（第一项）。
fn select_cdrom(tty: &mut Tty) -> Result<bool, ExitCode> {
    let options = vec![
        crate::tr!(crate::keys::SET_MIRROR_CDROM_COMMENT),
        crate::tr!(crate::keys::SET_MIRROR_CDROM_KEEP),
    ];
    match tty.select_one(
        &crate::tr!(crate::keys::SET_MIRROR_CDROM_PROMPT),
        &options,
        Some(0),
    ) {
        Ok(selected) => Ok(selected == 0),
        Err(error) => Err(fail_install(&error)),
    }
}

/// 询问是否同时替换 Debian 的独立 security 仓库，默认不替换。
/// Ubuntu 的 security 与主仓库合并镜像，不询问。
fn select_security(tty: &mut Tty, host: &Host) -> Result<bool, ExitCode> {
    if host.family != Family::Debian {
        return Ok(false);
    }
    let options = vec![
        crate::tr!(crate::keys::SET_MIRROR_SECURITY_KEEP),
        crate::tr!(crate::keys::SET_MIRROR_SECURITY_REPLACE),
    ];
    match tty.select_one(
        &crate::tr!(crate::keys::SET_MIRROR_SECURITY_PROMPT),
        &options,
        Some(0),
    ) {
        Ok(selected) => Ok(selected == 1),
        Err(error) => Err(fail_install(&error)),
    }
}

fn require_root() -> Result<(), MirrorError> {
    if crate::mirror::root_allowed() {
        Ok(())
    } else {
        Err(MirrorError::Message(crate::tr!(
            crate::keys::SET_MIRROR_ROOT_REQUIRED
        )))
    }
}

/// 输出 `set-mirror: <error>` 并返回失败退出码。
fn fail(error: impl std::fmt::Display) -> ExitCode {
    eprintln!("set-mirror: {error}");
    ExitCode::FAILURE
}

/// 输出 `set-mirror: <error>` 并按错误类型映射退出码：参数类错误返回 `2`，
/// 其余普通失败返回 `1`（与其余命令的 `ParameterUsage` 约定一致）。
fn fail_install(error: &plan::InstallError) -> ExitCode {
    match error {
        plan::InstallError::ParameterUsage(_) => fail_usage(error),
        _ => fail(error),
    }
}

/// 输出 `set-mirror: <error>` 并返回参数使用错误退出码 `2`。
fn fail_usage(error: impl std::fmt::Display) -> ExitCode {
    eprintln!("set-mirror: {error}");
    ExitCode::from(2)
}
