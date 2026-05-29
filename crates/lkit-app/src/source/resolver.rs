//! Multi-source resolver — concurrent probing and optimal source selection.

use std::sync::Arc;
use std::time::Duration;

use lkit_core::{ReleaseManifest, ReleaseSource, SourceError};
use tracing::{debug, warn};

/// Result of a successful source probe.
#[derive(Debug, Clone)]
pub struct ProbeResult {
    /// Name of the selected source.
    pub source_name: String,
    /// Round-trip latency of the probe.
    pub latency: Duration,
    /// The release manifest from this source.
    pub manifest: ReleaseManifest,
    /// Actual resolved tag (may differ from input if "latest" was used).
    pub resolved_tag: String,
}

/// Multi-source resolver — probes sources concurrently and selects the fastest.
pub struct SourceResolver {
    sources: Vec<Arc<dyn ReleaseSource>>,
}

impl SourceResolver {
    /// Create a resolver from a list of sources (ordered by priority).
    pub fn new(sources: Vec<Arc<dyn ReleaseSource>>) -> Self {
        Self { sources }
    }

    /// Probe all sources concurrently and return results sorted by latency.
    ///
    /// Returns all successful probes sorted fastest-first. Returns error only
    /// if ALL sources fail. Caller uses the first result, falls back to later ones.
    /// Total timeout: 15 seconds (spec §3.7).
    pub async fn resolve(&self, tag: Option<&str>) -> Result<Vec<ProbeResult>, SourceError> {
        // Step 1: Resolve actual tag (handle "latest")
        let actual_tag = match tag {
            Some(t) => t.to_string(),
            None => self.resolve_latest().await?,
        };

        // Step 2: Concurrent probe with 15s total timeout
        let handles: Vec<_> = self
            .sources
            .iter()
            .map(|source| {
                let source = Arc::clone(source);
                let tag = actual_tag.clone();
                tokio::spawn(async move {
                    let result = source.probe(&tag).await;
                    (source.name().to_string(), result)
                })
            })
            .collect();

        let results =
            tokio::time::timeout(Duration::from_secs(15), futures::future::join_all(handles))
                .await
                .map_err(|_| SourceError::ProbeTimeout)?;

        // Step 3: Collect successes, sorted by latency
        let mut successes: Vec<(String, Duration)> = Vec::new();
        let mut errors: Vec<(String, String)> = Vec::new();

        for result in results {
            match result {
                Ok((name, Ok(latency))) => {
                    debug!("source '{}' probed in {:?}", name, latency);
                    successes.push((name, latency));
                }
                Ok((name, Err(e))) => {
                    warn!("source '{}' probe failed: {}", name, e);
                    errors.push((name, e.to_string()));
                }
                Err(e) => {
                    warn!("probe task panicked: {}", e);
                }
            }
        }

        if successes.is_empty() {
            let msg = errors
                .iter()
                .map(|(name, err)| format!("  {name}: {err}"))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(SourceError::Network(format!("所有源探测失败:\n{msg}")));
        }

        // Sort by latency (fastest first)
        successes.sort_by_key(|(_, latency)| *latency);

        // Step 4: Concurrently fetch manifests from all successful sources
        let manifest_handles: Vec<_> = successes
            .into_iter()
            .filter_map(|(source_name, latency)| {
                let source = self.sources.iter().find(|s| s.name() == source_name)?;
                let source = Arc::clone(source);
                let tag = actual_tag.clone();
                Some(tokio::spawn(async move {
                    let manifest = source.get_artifacts(&tag).await;
                    (source.name().to_string(), latency, manifest, tag)
                }))
            })
            .collect();

        let manifest_results = futures::future::join_all(manifest_handles).await;

        let mut probe_results = Vec::new();
        for result in manifest_results {
            match result {
                Ok((source_name, latency, Ok(manifest), resolved_tag)) => {
                    probe_results.push(ProbeResult {
                        source_name,
                        latency,
                        manifest,
                        resolved_tag,
                    });
                }
                Ok((source_name, _, Err(e), _)) => {
                    warn!("source '{}' get_artifacts failed: {}", source_name, e);
                }
                Err(e) => {
                    warn!("manifest fetch task panicked: {}", e);
                }
            }
        }

        if probe_results.is_empty() {
            return Err(SourceError::Network("所有源获取 manifest 失败".into()));
        }

        // Re-sort by latency after concurrent fetch (order preserved but ensure correctness)
        probe_results.sort_by_key(|r| r.latency);

        Ok(probe_results)
    }

    /// Resolve the latest tag by asking the first available source.
    async fn resolve_latest(&self) -> Result<String, SourceError> {
        for source in &self.sources {
            match source.latest_tag().await {
                Ok(tag) => return Ok(tag),
                Err(e) => {
                    debug!("source '{}' latest_tag failed: {}", source.name(), e);
                    continue;
                }
            }
        }
        Err(SourceError::Network("无法解析 latest 版本".into()))
    }

    /// Get the source by name, for downloading artifacts.
    pub fn get_source(&self, name: &str) -> Option<&Arc<dyn ReleaseSource>> {
        self.sources.iter().find(|s| s.name() == name)
    }

    /// Get all sources ordered by priority.
    pub fn sources(&self) -> &[Arc<dyn ReleaseSource>] {
        &self.sources
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Mock source for testing.
    struct MockSource {
        source_name: String,
        probe_latency: Option<Duration>,
        probe_error: Option<String>,
        latest_tag: Option<String>,
        latest_error: Option<String>,
    }

    #[async_trait::async_trait]
    impl ReleaseSource for MockSource {
        fn name(&self) -> &str {
            &self.source_name
        }
        async fn latest_tag(&self) -> Result<String, SourceError> {
            match (&self.latest_tag, &self.latest_error) {
                (Some(tag), _) => Ok(tag.clone()),
                (_, Some(msg)) => Err(SourceError::Network(msg.clone())),
                _ => Err(SourceError::Network("not configured".into())),
            }
        }
        async fn list_versions(&self) -> Result<Vec<String>, SourceError> {
            Ok(vec![])
        }
        async fn get_artifacts(&self, tag: &str) -> Result<ReleaseManifest, SourceError> {
            Ok(ReleaseManifest {
                format_version: 1,
                tag: tag.into(),
                generated_at: String::new(),
                generated_by: None,
                artifacts: vec![],
            })
        }
        fn artifact_url(&self, _tag: &str, _name: &str) -> String {
            String::new()
        }
        async fn probe(&self, _tag: &str) -> Result<Duration, SourceError> {
            match (self.probe_latency, &self.probe_error) {
                (Some(d), _) => Ok(d),
                (_, Some(msg)) => Err(SourceError::Network(msg.clone())),
                _ => Err(SourceError::Network("not configured".into())),
            }
        }
    }

    #[tokio::test]
    async fn resolve_selects_fastest_source() -> Result<(), Box<dyn std::error::Error>> {
        let sources: Vec<Arc<dyn ReleaseSource>> = vec![
            Arc::new(MockSource {
                source_name: "slow".into(),
                probe_latency: Some(Duration::from_millis(200)),
                probe_error: None,
                latest_tag: Some("v1.0".into()),
                latest_error: None,
            }),
            Arc::new(MockSource {
                source_name: "fast".into(),
                probe_latency: Some(Duration::from_millis(10)),
                probe_error: None,
                latest_tag: Some("v1.0".into()),
                latest_error: None,
            }),
        ];
        let resolver = SourceResolver::new(sources);
        let results = resolver.resolve(Some("v1.0")).await?;
        assert!(!results.is_empty());
        assert_eq!(results[0].source_name, "fast");
        Ok(())
    }

    #[tokio::test]
    async fn resolve_returns_all_successful_sorted() -> Result<(), Box<dyn std::error::Error>> {
        let sources: Vec<Arc<dyn ReleaseSource>> = vec![
            Arc::new(MockSource {
                source_name: "a".into(),
                probe_latency: Some(Duration::from_millis(50)),
                probe_error: None,
                latest_tag: Some("v1.0".into()),
                latest_error: None,
            }),
            Arc::new(MockSource {
                source_name: "b".into(),
                probe_latency: None,
                probe_error: Some("fail".into()),
                latest_tag: Some("v1.0".into()),
                latest_error: None,
            }),
            Arc::new(MockSource {
                source_name: "c".into(),
                probe_latency: Some(Duration::from_millis(10)),
                probe_error: None,
                latest_tag: Some("v1.0".into()),
                latest_error: None,
            }),
        ];
        let resolver = SourceResolver::new(sources);
        let results = resolver.resolve(Some("v1.0")).await?;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].source_name, "c");
        assert_eq!(results[1].source_name, "a");
        Ok(())
    }

    #[tokio::test]
    async fn resolve_all_fail_returns_error() -> Result<(), Box<dyn std::error::Error>> {
        let sources: Vec<Arc<dyn ReleaseSource>> = vec![
            Arc::new(MockSource {
                source_name: "bad1".into(),
                probe_latency: None,
                probe_error: Some("timeout".into()),
                latest_tag: Some("v1.0".into()),
                latest_error: None,
            }),
            Arc::new(MockSource {
                source_name: "bad2".into(),
                probe_latency: None,
                probe_error: Some("refused".into()),
                latest_tag: Some("v1.0".into()),
                latest_error: None,
            }),
        ];
        let resolver = SourceResolver::new(sources);
        let result = resolver.resolve(Some("v1.0")).await;
        assert!(result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn resolve_latest_calls_latest_tag() -> Result<(), Box<dyn std::error::Error>> {
        let sources: Vec<Arc<dyn ReleaseSource>> = vec![Arc::new(MockSource {
            source_name: "src".into(),
            probe_latency: Some(Duration::from_millis(10)),
            probe_error: None,
            latest_tag: Some("v2.0".into()),
            latest_error: None,
        })];
        let resolver = SourceResolver::new(sources);
        let results = resolver.resolve(None).await?;
        assert_eq!(results[0].resolved_tag, "v2.0");
        Ok(())
    }
}
