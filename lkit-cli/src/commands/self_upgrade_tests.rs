use clap::{Command, FromArgMatches};
use serde_json::{Value, json};

use super::*;
use crate::release::repository::test_server::{TestResponse, TestServer};

fn release(tag: &str, prerelease: bool, draft: bool) -> Value {
    json!({ "tag_name": tag, "prerelease": prerelease, "draft": draft, "assets": [] })
}

fn source(server: &TestServer) -> UpgradeSource {
    UpgradeSource {
        api_root: source_root(&server.base).unwrap(),
        download_root: source_root(&format!("{}/downloads", server.base)).unwrap(),
    }
}

fn parse(args: &[&str]) -> Result<SelfCommand, clap::Error> {
    let command = <SelfCommand as Args>::augment_args(Command::new("self"));
    let matches = command.try_get_matches_from(args)?;
    SelfCommand::from_arg_matches(&matches)
}

#[test]
fn prerelease_flag_is_opt_in_and_only_applies_to_self_upgrade() {
    let SelfAction::Upgrade(default) = parse(&["self", "upgrade"]).unwrap().action else {
        panic!("expected upgrade");
    };
    assert!(!default.include_prerelease);
    for command in ["upgrade", "update"] {
        let SelfAction::Upgrade(args) = parse(&["self", command, "--include-prerelease"])
            .unwrap()
            .action
        else {
            panic!("expected upgrade");
        };
        assert!(args.include_prerelease);
        assert!(args.version.is_none());
    }
    let SelfAction::Upgrade(args) = parse(&[
        "self",
        "upgrade",
        "--version",
        "v1.2.0-rc2",
        "--include-prerelease",
    ])
    .unwrap()
    .action
    else {
        panic!("expected upgrade");
    };
    assert_eq!(args.version.as_deref(), Some("v1.2.0-rc2"));
    for command in ["install", "remove"] {
        assert!(parse(&["self", command, "--include-prerelease"]).is_err());
    }
}

#[tokio::test]
async fn default_discovery_uses_only_latest_stable_endpoint() {
    let server = TestServer::start(|path| {
        assert_eq!(
            path,
            "/repos/landscape-router/landscape-kit/releases/latest"
        );
        TestResponse::ok(serde_json::to_vec(&release("v1.2.0", false, false)).unwrap())
    });
    let selected = fetch_release(&source(&server), None, false).await.unwrap();
    assert_eq!(selected.tag_name, "v1.2.0");
    assert_eq!(
        upgrade_release_version(&selected, None, false).unwrap(),
        Version::parse("1.2.0").unwrap()
    );
    assert_eq!(server.request_count(), 1);
}

#[test]
fn stable_selection_rejects_prerelease_tags_even_when_metadata_is_mislabelled() {
    for metadata in [
        release("v1.2.0-rc2", true, false),
        release("v1.2.0-rc2", false, false),
        release("v1.2.0", true, false),
        release("v1.2.0", false, true),
        release("not-a-version", false, false),
    ] {
        let metadata: GithubRelease = serde_json::from_value(metadata).unwrap();
        assert!(upgrade_release_version(&metadata, None, false).is_err());
    }
}

#[tokio::test]
async fn opt_in_selects_highest_version_not_first_or_most_recent_release() {
    for (tags, expected) in [
        (vec!["v1.2.0-rc2", "v1.2.0-rc10", "v1.1.0"], "v1.2.0-rc10"),
        (vec!["v1.2.0-rc10", "v1.2.0"], "v1.2.0"),
        (vec!["v1.2.0", "v1.3.0-rc1"], "v1.3.0-rc1"),
        (vec!["v1.2.0-rc10", "v1.2.0-rc.11"], "v1.2.0-rc.11"),
    ] {
        let mut releases: Vec<_> = tags
            .into_iter()
            .map(|tag| release(tag, tag.contains('-'), false))
            .collect();
        releases.extend([
            release("v99.0.0", false, true),
            release("v99.0.0-rc", true, false),
            release("not-semver", false, false),
        ]);
        let body = serde_json::to_vec(&releases).unwrap();
        let server = TestServer::start(move |path| {
            assert_eq!(
                path,
                "/repos/landscape-router/landscape-kit/releases?per_page=100&page=1"
            );
            TestResponse::ok(body.clone())
        });
        let selected = fetch_release(&source(&server), None, true).await.unwrap();
        assert_eq!(selected.tag_name, expected);
        assert!(upgrade_release_version(&selected, None, true).is_ok());
        assert_eq!(server.request_count(), 1);
    }
}

#[tokio::test]
async fn opt_in_searches_later_pages_before_selecting_a_release() {
    let page_one: Vec<_> = (0..100)
        .map(|patch| release(&format!("v0.1.{patch}"), false, false))
        .collect();
    let server = TestServer::start(move |path| {
        let body = match path {
            "/repos/landscape-router/landscape-kit/releases?per_page=100&page=1" => {
                serde_json::to_vec(&page_one).unwrap()
            }
            "/repos/landscape-router/landscape-kit/releases?per_page=100&page=2" => {
                serde_json::to_vec(&vec![release("v1.0.0-rc2", true, false)]).unwrap()
            }
            _ => panic!("unexpected request {path}"),
        };
        TestResponse::ok(body)
    });
    let selected = fetch_release(&source(&server), None, true).await.unwrap();
    assert_eq!(selected.tag_name, "v1.0.0-rc2");
    assert_eq!(server.request_count(), 2);
}

#[tokio::test]
async fn explicit_version_takes_priority_over_prerelease_discovery() {
    for include_prerelease in [false, true] {
        let server = TestServer::start(|path| {
            assert_eq!(
                path,
                "/repos/landscape-router/landscape-kit/releases/tags/v1.2.0-rc2"
            );
            TestResponse::ok(serde_json::to_vec(&release("v1.2.0-rc2", true, false)).unwrap())
        });
        let selected = fetch_release(&source(&server), Some("v1.2.0-rc2"), include_prerelease)
            .await
            .unwrap();
        assert_eq!(
            upgrade_release_version(&selected, Some("v1.2.0-rc2"), include_prerelease).unwrap(),
            Version::parse("1.2.0-rc2").unwrap()
        );
        assert_eq!(server.request_count(), 1);
    }
}

#[test]
fn explicit_versions_reject_drafts_and_mismatched_response_tags() {
    for metadata in [
        release("v1.2.0-rc2", true, true),
        release("v1.2.0-rc3", true, false),
    ] {
        let metadata = serde_json::from_value(metadata).unwrap();
        assert!(upgrade_release_version(&metadata, Some("v1.2.0-rc2"), true).is_err());
    }
}

#[tokio::test]
async fn invalid_explicit_versions_make_no_api_requests() {
    let server = TestServer::start(|_| panic!("invalid version must not make requests"));
    assert!(
        fetch_release(&source(&server), Some("invalid"), true)
            .await
            .is_err()
    );
    assert_eq!(server.request_count(), 0);
}

#[tokio::test]
async fn empty_or_ineligible_release_lists_fail_without_falling_back_or_downloading() {
    for releases in [
        vec![],
        vec![release("v9.0.0", false, true)],
        vec![release("v9.0.0-rc", true, false)],
    ] {
        let body = serde_json::to_vec(&releases).unwrap();
        let server = TestServer::start(move |_| TestResponse::ok(body.clone()));
        assert!(fetch_release(&source(&server), None, true).await.is_err());
        assert_eq!(
            server.request_paths(),
            vec!["/repos/landscape-router/landscape-kit/releases?per_page=100&page=1"]
        );
    }
}

#[tokio::test]
async fn incomplete_pagination_does_not_use_a_partial_selection() {
    let page = serde_json::to_vec(&vec![release("v1.0.0", false, false); 100]).unwrap();
    let server = TestServer::start(move |path| {
        if path.ends_with("page=1") {
            TestResponse::ok(page.clone())
        } else {
            TestResponse::status(404, "Not Found", vec![])
        }
    });
    assert!(fetch_release(&source(&server), None, true).await.is_err());
    assert_eq!(server.request_count(), 2);
}

#[tokio::test]
async fn discovery_has_a_page_limit_and_never_silently_truncates_results() {
    let page = serde_json::to_vec(&vec![release("v1.0.0", false, false); 100]).unwrap();
    let server = TestServer::start(move |_| TestResponse::ok(page.clone()));
    let Err(error) = fetch_release(&source(&server), None, true).await else {
        panic!("page limit must fail");
    };
    assert!(error.to_string().contains("exceeded 100 pages"));
    assert_eq!(server.request_count(), 100);
}

#[tokio::test]
async fn invalid_api_responses_fail_without_falling_back() {
    for malformed in [false, true] {
        let server = TestServer::start(move |_| {
            if malformed {
                TestResponse::ok(b"not JSON".to_vec())
            } else {
                TestResponse::status(404, "Not Found", vec![])
            }
        });
        assert!(fetch_release(&source(&server), None, true).await.is_err());
        assert_eq!(server.request_count(), 1);
    }
}
