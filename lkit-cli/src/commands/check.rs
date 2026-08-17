use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{Args, ValueEnum};

use crate::check;
use crate::check::model::Status;
use crate::report;

#[derive(Debug, Args)]
pub struct Check {
    /// 输出每个检查项的详细信息
    #[arg(long)]
    pub verbose: bool,

    /// 输出颜色：auto（默认）/always/never
    #[arg(long, value_enum, default_value_t = ColorArg::Auto)]
    pub color: ColorArg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorArg {
    Auto,
    Always,
    Never,
}

pub fn run(args: &Check) -> ExitCode {
    let report = check::run_all();
    let use_color = match args.color {
        ColorArg::Auto => std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        ColorArg::Always => true,
        ColorArg::Never => false,
    };
    print!("{}", report::render(&report, args.verbose, use_color));
    match report.summary {
        Status::Error | Status::Unknown => ExitCode::FAILURE,
        Status::Warning | Status::Pass => ExitCode::SUCCESS,
    }
}
