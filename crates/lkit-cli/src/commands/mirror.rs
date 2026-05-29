//! `lkit mirror` — mirror management subcommands.

use std::path::PathBuf;
use std::sync::Arc;

use lkit_client::{GithubSource, HttpMirrorSource, LocalSource};
use lkit_core::ReleaseSource;
use lkit_mirror::serve::{self, ServeConfig};
use lkit_mirror::sync::{self, SyncConfig, SyncScope};
use lkit_mirror::target::MirrorTarget;
use lkit_mirror::target::local::LocalTarget;

use crate::cli::{
    MirrorAction, MirrorArgs, MirrorListArgs, MirrorServeArgs, MirrorSyncArgs, MirrorTargetType,
    MirrorVerifyArgs, SyncSourceType,
};

/// Dispatch mirror subcommand.
pub(crate) async fn run(args: MirrorArgs) -> anyhow::Result<()> {
    match args.action {
        MirrorAction::Sync(args) => run_sync(args).await,
        MirrorAction::Serve(args) => run_serve(args).await,
        MirrorAction::Verify(args) => run_verify(args).await,
        MirrorAction::List(args) => run_list(args).await,
    }
}

async fn run_sync(args: MirrorSyncArgs) -> anyhow::Result<()> {
    let prefix = args
        .prefix
        .unwrap_or_else(|| extract_repo_name(&args.repo).to_string());

    let scope = if args.all {
        SyncScope::All
    } else if let Some(tag) = args.tag {
        SyncScope::Tag(tag)
    } else if let Some(n) = args.latest {
        SyncScope::LatestN(n)
    } else if let Some(since) = args.since {
        SyncScope::Since(since)
    } else {
        SyncScope::Latest
    };

    let config = SyncConfig {
        prefix,
        scope,
        force: args.force,
    };

    // DI assembly: build concrete source from args
    let http_client = reqwest::Client::builder().user_agent("lkit").build()?;
    let source = build_source(
        &args.source,
        &args.repo,
        args.source_url.as_deref(),
        args.source_path.as_deref(),
        http_client,
    )?;
    let target = build_target(
        &args.target,
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.endpoint.as_deref(),
        &args.s3_prefix,
    )?;

    let result = sync::run_sync(&config, source.as_ref(), target.as_ref()).await?;

    println!("同步完成:");
    if !result.synced.is_empty() {
        println!("  已同步: {}", result.synced.join(", "));
    }
    if !result.skipped.is_empty() {
        println!("  已跳过: {}", result.skipped.join(", "));
    }
    if !result.failed.is_empty() {
        println!("  失败:");
        for (tag, err) in &result.failed {
            println!("    {tag}: {err}");
        }
    }

    Ok(())
}

async fn run_serve(args: MirrorServeArgs) -> anyhow::Result<()> {
    let config = ServeConfig {
        path: PathBuf::from(&args.path),
        port: args.port,
        bind: args.bind,
    };
    serve::serve(config).await?;
    Ok(())
}

async fn run_verify(args: MirrorVerifyArgs) -> anyhow::Result<()> {
    let target = build_target(
        &args.target,
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.endpoint.as_deref(),
        &args.s3_prefix,
    )?;

    let results = lkit_mirror::verify::verify(target.as_ref(), &args.prefix).await?;

    if results.is_empty() {
        println!("没有找到已同步的版本");
        return Ok(());
    }

    let mut all_passed = true;
    for r in &results {
        if r.passed {
            println!("  {} ✓", r.tag);
        } else {
            all_passed = false;
            println!("  {} ✗", r.tag);
            for err in &r.errors {
                println!("    - {err}");
            }
        }
    }

    if !all_passed {
        anyhow::bail!("部分版本校验失败");
    }

    Ok(())
}

async fn run_list(args: MirrorListArgs) -> anyhow::Result<()> {
    let target = build_target(
        &args.target,
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.endpoint.as_deref(),
        &args.s3_prefix,
    )?;

    let versions = lkit_mirror::list::list_versions(target.as_ref(), &args.prefix).await?;

    if versions.is_empty() {
        println!("没有找到已同步的版本");
        return Ok(());
    }

    let latest = lkit_mirror::list::read_latest(target.as_ref(), &args.prefix).await?;

    if let Some(ref l) = latest {
        println!("Latest: {l}");
    }
    println!();

    for v in &versions {
        let marker = if latest.as_deref() == Some(&v.tag) {
            " (latest)"
        } else {
            ""
        };
        println!("  {} — {} artifacts{}", v.tag, v.artifact_count, marker);
    }

    Ok(())
}

fn build_target(
    target_type: &MirrorTargetType,
    path: Option<&str>,
    bucket: Option<&str>,
    endpoint: Option<&str>,
    s3_prefix: &str,
) -> anyhow::Result<Box<dyn MirrorTarget>> {
    match target_type {
        MirrorTargetType::Local => {
            let path =
                path.ok_or_else(|| anyhow::anyhow!("--path is required for local target"))?;
            Ok(Box::new(LocalTarget::new(path)))
        }
        MirrorTargetType::S3 => {
            let bucket =
                bucket.ok_or_else(|| anyhow::anyhow!("--bucket is required for s3 target"))?;
            let endpoint =
                endpoint.ok_or_else(|| anyhow::anyhow!("--endpoint is required for s3 target"))?;
            let access_key = std::env::var("AWS_ACCESS_KEY_ID")
                .map_err(|_| anyhow::anyhow!("AWS_ACCESS_KEY_ID env var required"))?;
            let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY")
                .map_err(|_| anyhow::anyhow!("AWS_SECRET_ACCESS_KEY env var required"))?;
            let target = lkit_mirror::target::s3::S3Target::new(
                endpoint,
                bucket,
                &access_key,
                &secret_key,
                s3_prefix,
            )?;
            Ok(Box::new(target))
        }
    }
}

fn extract_repo_name(repo: &str) -> &str {
    repo.split('/').next_back().unwrap_or(repo)
}

/// Build a release source from CLI arguments.
fn build_source(
    source_type: &SyncSourceType,
    repo: &str,
    source_url: Option<&str>,
    source_path: Option<&str>,
    http_client: reqwest::Client,
) -> anyhow::Result<Arc<dyn ReleaseSource>> {
    match source_type {
        SyncSourceType::Github => Ok(Arc::new(GithubSource::new(
            "sync-source",
            repo,
            http_client,
        )?)),
        SyncSourceType::Http => {
            let url = source_url
                .ok_or_else(|| anyhow::anyhow!("--source-url is required for http source"))?;
            Ok(Arc::new(HttpMirrorSource::new(
                "http-mirror",
                url,
                http_client,
            )))
        }
        SyncSourceType::Local => {
            let path = source_path
                .ok_or_else(|| anyhow::anyhow!("--source-path is required for local source"))?;
            Ok(Arc::new(LocalSource::new("local-source", path)))
        }
    }
}
