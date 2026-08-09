//! 文件清单收集:主文件 + `source`/`source-directory` 展开(递归,canonical 去重)。

use std::path::{Path, PathBuf};

use crate::error::HostNetError;
use crate::model::{FileSet, FileSources};

use super::parse::{self, SourceKind};

pub(crate) fn collect(sources: &FileSources) -> Result<FileSet, HostNetError> {
    let main = &sources.interfaces;
    if !main.is_absolute() {
        return Err(HostNetError::PathSafety {
            path: main.clone(),
            reason: "interfaces path must be absolute".into(),
        });
    }
    match std::fs::symlink_metadata(main) {
        Ok(_) => validate_regular_file(main)?,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileSet {
                interfaces: main.clone(),
                files: Vec::new(),
            });
        }
        Err(source) => {
            return Err(HostNetError::UnreadableFile {
                path: main.clone(),
                source,
            });
        }
    }
    let mut files: Vec<PathBuf> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut queue: Vec<PathBuf> = vec![main.clone()];
    while let Some(path) = queue.pop() {
        validate_regular_file(&path)?;
        let canonical = canonicalize(&path)?;
        if seen.contains(&canonical) {
            continue;
        }
        seen.push(canonical);
        files.push(path.clone());
        let content =
            std::fs::read_to_string(&path).map_err(|source| HostNetError::UnreadableFile {
                path: path.clone(),
                source,
            })?;
        let parsed = parse::parse(&path, &content)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        for directive in &parsed.sources {
            let resolved = resolve_pattern(&path, directive.line, parent, &directive.pattern)?;
            let matches = match directive.kind {
                SourceKind::File => expand_file_pattern(&path, directive.line, &resolved)?,
                SourceKind::Directory => {
                    expand_directory_pattern(&path, directive.line, &resolved)?
                }
            };
            for matched in matches {
                queue.push(matched);
            }
        }
    }
    let mut rest: Vec<PathBuf> = files.into_iter().skip(1).collect();
    rest.sort();
    let mut all = vec![main.clone()];
    all.append(&mut rest);
    Ok(FileSet {
        interfaces: main.clone(),
        files: all,
    })
}

fn canonicalize(path: &Path) -> Result<PathBuf, HostNetError> {
    path.canonicalize()
        .map_err(|source| HostNetError::UnreadableFile {
            path: path.to_path_buf(),
            source,
        })
}

fn validate_regular_file(path: &Path) -> Result<(), HostNetError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|source| HostNetError::UnreadableFile {
            path: path.to_path_buf(),
            source,
        })?;
    if metadata.file_type().is_symlink() {
        return Err(HostNetError::PathSafety {
            path: path.to_path_buf(),
            reason: "symbolic links are not supported for host network configuration".into(),
        });
    }
    if !metadata.file_type().is_file() {
        return Err(HostNetError::PathSafety {
            path: path.to_path_buf(),
            reason: "host network configuration must be a regular file".into(),
        });
    }
    Ok(())
}

fn resolve_pattern(
    source: &Path,
    line: usize,
    source_dir: &Path,
    pattern: &str,
) -> Result<PathBuf, HostNetError> {
    if pattern.contains(['$', '`', '\'', '"', '{', '}', '\\']) || pattern.starts_with('~') {
        return Err(HostNetError::UnsupportedSyntax {
            path: source.to_path_buf(),
            line,
            reason: format!("source pattern {pattern} requires unsupported shell expansion"),
        });
    }
    let path = Path::new(pattern);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        source_dir.join(path)
    })
}

fn expand_file_pattern(
    source: &Path,
    line: usize,
    pattern: &Path,
) -> Result<Vec<PathBuf>, HostNetError> {
    let mut matches = expand_pattern(source, line, pattern)?;
    for path in &matches {
        validate_regular_file(path)?;
    }
    matches.sort();
    Ok(matches)
}

fn expand_directory_pattern(
    source: &Path,
    line: usize,
    pattern: &Path,
) -> Result<Vec<PathBuf>, HostNetError> {
    let directories = expand_pattern(source, line, pattern)?;
    let mut files = Vec::new();
    for directory in directories {
        let metadata = std::fs::symlink_metadata(&directory).map_err(|source_error| {
            HostNetError::SourceExpansionFailed {
                path: source.to_path_buf(),
                pattern: pattern.display().to_string(),
                source: source_error,
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Err(HostNetError::PathSafety {
                path: directory,
                reason: "source-directory must resolve to a regular directory".into(),
            });
        }
        let entries = std::fs::read_dir(&directory).map_err(|source_error| {
            HostNetError::SourceExpansionFailed {
                path: source.to_path_buf(),
                pattern: pattern.display().to_string(),
                source: source_error,
            }
        })?;
        for entry in entries {
            let entry = entry.map_err(|source_error| HostNetError::SourceExpansionFailed {
                path: source.to_path_buf(),
                pattern: pattern.display().to_string(),
                source: source_error,
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            {
                continue;
            }
            validate_regular_file(&entry.path())?;
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

fn expand_pattern(
    source: &Path,
    line: usize,
    pattern: &Path,
) -> Result<Vec<PathBuf>, HostNetError> {
    let pattern_text = pattern
        .to_str()
        .ok_or_else(|| HostNetError::UnsupportedSyntax {
            path: source.to_path_buf(),
            line,
            reason: "source pattern is not valid UTF-8".into(),
        })?;
    let paths = glob::glob_with(
        pattern_text,
        glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        },
    )
    .map_err(|error| HostNetError::UnsupportedSyntax {
        path: source.to_path_buf(),
        line,
        reason: format!("invalid source pattern {pattern_text}: {error}"),
    })?;
    let mut matches = Vec::new();
    for entry in paths {
        matches.push(entry.map_err(|error| HostNetError::SourceExpansionFailed {
            path: source.to_path_buf(),
            pattern: pattern_text.to_string(),
            source: error.into(),
        })?);
    }
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lkit-hostnet-collect-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn collects_main_and_sourced_files() {
        let dir = temp_dir("basic");
        std::fs::write(
            dir.join("interfaces"),
            b"auto eth0\niface eth0 inet static\nsource fragments/*\n",
        )
        .unwrap();
        let frag = dir.join("fragments");
        std::fs::create_dir_all(&frag).unwrap();
        std::fs::write(frag.join("a.conf"), b"iface eth1 inet dhcp\n").unwrap();
        std::fs::write(frag.join("b.conf"), b"iface eth2 inet dhcp\n").unwrap();
        let sources = FileSources::new(dir.join("interfaces"));
        let file_set = collect(&sources).unwrap();
        assert_eq!(file_set.files.len(), 3);
        assert_eq!(file_set.files[0], dir.join("interfaces"));
        assert!(file_set.files[1..].contains(&frag.join("a.conf")));
        assert!(file_set.files[1..].contains(&frag.join("b.conf")));
    }

    #[test]
    fn missing_main_file_yields_empty_file_set() {
        let dir = temp_dir("missing");
        let file_set = collect(&FileSources::new(dir.join("interfaces"))).unwrap();
        assert!(file_set.is_empty());
    }

    #[test]
    fn missing_source_directory_matches_nothing() {
        let dir = temp_dir("missing-source-dir");
        std::fs::write(dir.join("interfaces"), b"source missing/*\n").unwrap();
        let file_set = collect(&FileSources::new(dir.join("interfaces"))).unwrap();
        assert_eq!(file_set.files, vec![dir.join("interfaces")]);
    }

    #[test]
    fn source_is_expanded_recursively() {
        let dir = temp_dir("recursive");
        let frag = dir.join("fragments");
        std::fs::create_dir_all(&frag).unwrap();
        std::fs::write(dir.join("interfaces"), b"source fragments/*\n").unwrap();
        std::fs::write(frag.join("outer"), b"source extra.conf\n").unwrap();
        std::fs::write(frag.join("extra.conf"), b"iface eth0 inet manual\n").unwrap();
        let file_set = collect(&FileSources::new(dir.join("interfaces"))).unwrap();
        assert!(file_set.files.contains(&frag.join("extra.conf")));
    }

    #[test]
    fn duplicate_matches_are_deduplicated() {
        let dir = temp_dir("dedup");
        let frag = dir.join("fragments");
        std::fs::create_dir_all(&frag).unwrap();
        std::fs::write(frag.join("a.conf"), b"iface eth0 inet manual\n").unwrap();
        std::fs::write(
            dir.join("interfaces"),
            b"source fragments/a.conf\nsource fragments/*\n",
        )
        .unwrap();
        let file_set = collect(&FileSources::new(dir.join("interfaces"))).unwrap();
        assert_eq!(file_set.files.len(), 2);
    }

    #[test]
    fn relative_source_resolves_against_the_sourcing_file() {
        let dir = temp_dir("relative");
        std::fs::write(dir.join("interfaces"), b"source fragments/*\n").unwrap();
        let frag = dir.join("fragments");
        std::fs::create_dir_all(&frag).unwrap();
        std::fs::write(frag.join("a.conf"), b"iface eth0 inet manual\n").unwrap();
        let file_set = collect(&FileSources::new(dir.join("interfaces"))).unwrap();
        assert!(file_set.files.contains(&frag.join("a.conf")));
    }

    #[test]
    fn source_directory_collects_only_ifupdown_names() {
        let dir = temp_dir("source-directory");
        let fragments = dir.join("interfaces.d");
        std::fs::create_dir_all(&fragments).unwrap();
        std::fs::write(fragments.join("eth0"), b"iface eth0 inet dhcp\n").unwrap();
        std::fs::write(fragments.join("lan_cfg"), b"iface eth1 inet manual\n").unwrap();
        std::fs::write(fragments.join("bad.conf"), b"iface bad inet dhcp\n").unwrap();
        std::fs::write(fragments.join(".hidden"), b"iface hidden inet dhcp\n").unwrap();
        std::fs::write(dir.join("interfaces"), b"source-directory interfaces.d\n").unwrap();
        let file_set = collect(&FileSources::new(dir.join("interfaces"))).unwrap();
        assert_eq!(file_set.files.len(), 3);
        assert!(file_set.files.contains(&fragments.join("eth0")));
        assert!(file_set.files.contains(&fragments.join("lan_cfg")));
    }

    #[test]
    fn wildcard_directory_and_character_class_are_expanded() {
        let dir = temp_dir("wildcard-dir");
        let fragments = dir.join("trees/one/interfaces.d");
        std::fs::create_dir_all(&fragments).unwrap();
        std::fs::write(fragments.join("a.conf"), b"iface eth0 inet dhcp\n").unwrap();
        std::fs::write(fragments.join("c.conf"), b"iface eth2 inet dhcp\n").unwrap();
        std::fs::write(
            dir.join("interfaces"),
            b"source trees/*/interfaces.d/[ab].conf\n",
        )
        .unwrap();
        let file_set = collect(&FileSources::new(dir.join("interfaces"))).unwrap();
        assert_eq!(
            file_set.files,
            vec![dir.join("interfaces"), fragments.join("a.conf")]
        );
    }

    #[test]
    fn source_glob_does_not_match_dotfiles() {
        let dir = temp_dir("dotfiles");
        let fragments = dir.join("interfaces.d");
        std::fs::create_dir_all(&fragments).unwrap();
        std::fs::write(fragments.join("a.conf"), b"iface eth0 inet dhcp\n").unwrap();
        std::fs::write(fragments.join(".hidden"), b"iface hidden inet dhcp\n").unwrap();
        std::fs::write(dir.join("interfaces"), b"source interfaces.d/*\n").unwrap();
        let file_set = collect(&FileSources::new(dir.join("interfaces"))).unwrap();
        assert_eq!(file_set.files.len(), 2);
        assert!(!file_set.files.contains(&fragments.join(".hidden")));
    }

    #[test]
    fn indented_source_is_collected_like_ifupdown() {
        let dir = temp_dir("indented-source");
        let fragments = dir.join("interfaces.d");
        std::fs::create_dir_all(&fragments).unwrap();
        std::fs::write(fragments.join("eth1"), b"iface eth1 inet dhcp\n").unwrap();
        std::fs::write(
            dir.join("interfaces"),
            b"iface eth0 inet dhcp\n  source interfaces.d/*\n",
        )
        .unwrap();
        let file_set = collect(&FileSources::new(dir.join("interfaces"))).unwrap();
        assert!(file_set.files.contains(&fragments.join("eth1")));
    }

    #[test]
    fn main_and_sourced_symlinks_are_rejected() {
        let dir = temp_dir("symlinks");
        let target = dir.join("target");
        std::fs::write(&target, b"iface eth0 inet dhcp\n").unwrap();
        let main = dir.join("interfaces");
        std::os::unix::fs::symlink(&target, &main).unwrap();
        assert!(matches!(
            collect(&FileSources::new(main)),
            Err(HostNetError::PathSafety { .. })
        ));

        std::fs::remove_file(dir.join("interfaces")).unwrap();
        std::fs::write(dir.join("interfaces"), b"source interfaces.d/*\n").unwrap();
        std::fs::create_dir_all(dir.join("interfaces.d")).unwrap();
        std::os::unix::fs::symlink(&target, dir.join("interfaces.d/eth0")).unwrap();
        assert!(matches!(
            collect(&FileSources::new(dir.join("interfaces"))),
            Err(HostNetError::PathSafety { .. })
        ));
    }

    #[test]
    fn shell_expansion_and_relative_main_path_are_rejected() {
        let dir = temp_dir("unsafe-pattern");
        std::fs::write(dir.join("interfaces"), b"source $HOME/interfaces\n").unwrap();
        assert!(matches!(
            collect(&FileSources::new(dir.join("interfaces"))),
            Err(HostNetError::UnsupportedSyntax { .. })
        ));
        assert!(matches!(
            collect(&FileSources::new(PathBuf::from("interfaces"))),
            Err(HostNetError::PathSafety { .. })
        ));
    }
}
