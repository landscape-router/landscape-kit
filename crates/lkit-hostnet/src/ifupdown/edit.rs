//! 改写计划与应用:选中接口的 `iface` 块 method 改为 `manual`、删除其选项行,
//! 并从 `auto`/`allow-*` 中删除;未选中内容逐字节保留。改写经 tmp + rename 原子写回。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::HostNetError;
use crate::model::{EditOutcome, EditPlan, FileEdit, FileMetadata, FileSet};

use super::parse;

pub(crate) fn plan_unmanage(
    file_set: &FileSet,
    selected: &[String],
) -> Result<EditPlan, HostNetError> {
    let selected: HashSet<&str> = selected.iter().map(String::as_str).collect();
    let mut parsed_files = Vec::new();
    for path in &file_set.files {
        let content =
            std::fs::read_to_string(path).map_err(|source| HostNetError::UnreadableFile {
                path: path.clone(),
                source,
            })?;
        let parsed = parse::parse(path, &content)?;
        reject_ambiguous_mappings(path, &parsed, &selected)?;
        reject_selected_dependencies(path, &parsed, &selected)?;
        parsed_files.push((path, content, parsed));
    }

    // Selection declarations and iface stanzas commonly live in different sourced files.
    let managed: HashSet<String> = parsed_files
        .iter()
        .flat_map(|(_, _, parsed)| &parsed.blocks)
        .filter(|block| selected.contains(block.iface.as_str()))
        .map(|block| block.iface.clone())
        .collect();

    let mut edits = Vec::new();
    for (path, content, parsed) in parsed_files {
        let file_managed: HashSet<&str> = parsed
            .blocks
            .iter()
            .filter_map(|block| {
                managed
                    .contains(block.iface.as_str())
                    .then_some(block.iface.as_str())
            })
            .collect();
        let mut replacements = BTreeMap::new();
        let mut removals = BTreeSet::new();

        for block in &parsed.blocks {
            if !file_managed.contains(block.iface.as_str()) {
                continue;
            }
            if block.method == "ppp" {
                return Err(HostNetError::UnsupportedMethod {
                    path: path.clone(),
                    iface: block.iface.clone(),
                    method: block.method.clone(),
                });
            }
            if block.method == "manual" && block.inherits.is_none() && block.option_lines.is_empty()
            {
                continue;
            }
            let first = block.declaration_lines[0];
            replacements.insert(
                first,
                format!("iface {} {} manual", block.iface, block.family),
            );
            removals.extend(block.declaration_lines.iter().copied().skip(1));
            removals.extend(block.option_lines.iter().copied());
        }

        for selection in &parsed.selections {
            let mut kept = Vec::new();
            let mut changed = false;
            for interface in &selection.interfaces {
                if managed.contains(interface.as_str()) {
                    changed = true;
                    continue;
                }
                if interface.starts_with('/') {
                    for name in &managed {
                        let matches =
                            selection_pattern_matches(interface, name).map_err(|reason| {
                                HostNetError::UnsupportedSyntax {
                                    path: path.clone(),
                                    line: selection.lines[0] + 1,
                                    reason,
                                }
                            })?;
                        if matches {
                            return Err(HostNetError::UnsupportedSyntax {
                                path: path.clone(),
                                line: selection.lines[0] + 1,
                                reason: format!(
                                    "{} pattern {interface} selects a managed interface and cannot be safely narrowed",
                                    selection.keyword
                                ),
                            });
                        }
                    }
                }
                kept.push(interface.clone());
            }
            if !changed {
                continue;
            }
            removals.extend(selection.lines.iter().copied().skip(1));
            if kept.is_empty() {
                removals.insert(selection.lines[0]);
            } else {
                replacements.insert(
                    selection.lines[0],
                    format!("{} {}", selection.keyword, kept.join(" ")),
                );
            }
        }

        if !replacements.is_empty() || !removals.is_empty() {
            let new_lines: Vec<String> = parsed
                .lines
                .iter()
                .enumerate()
                .filter_map(|(index, line)| {
                    if removals.contains(&index) {
                        None
                    } else {
                        Some(
                            replacements
                                .get(&index)
                                .cloned()
                                .unwrap_or_else(|| line.clone()),
                        )
                    }
                })
                .collect();
            edits.push(FileEdit {
                path: path.clone(),
                original_content: content.as_bytes().to_vec(),
                content: render(&new_lines, parsed.ends_with_newline),
                metadata: capture_metadata(path)?,
            });
        }
    }
    Ok(EditPlan { edits })
}

pub(crate) fn apply(plan: &EditPlan) -> Result<EditOutcome, HostNetError> {
    for edit in &plan.edits {
        verify_edit(edit)?;
    }
    let mut edited = Vec::new();
    for edit in &plan.edits {
        verify_edit(edit)?;
        write_atomic_checked(
            &edit.path,
            edit.content.as_bytes(),
            Some(&edit.metadata),
            Some((&edit.original_content, &edit.metadata)),
        )?;
        edited.push(edit.path.clone());
    }
    Ok(EditOutcome { edited })
}

fn reject_ambiguous_mappings(
    path: &Path,
    parsed: &parse::ParsedFile,
    selected: &HashSet<&str>,
) -> Result<(), HostNetError> {
    for mapping in &parsed.mappings {
        for pattern in &mapping.patterns {
            let pattern_match =
                glob::Pattern::new(pattern).map_err(|error| HostNetError::UnsupportedSyntax {
                    path: path.to_path_buf(),
                    line: mapping.line,
                    reason: format!("invalid mapping pattern {pattern}: {error}"),
                })?;
            for interface in selected {
                if pattern_match.matches(interface) {
                    return Err(HostNetError::UnsupportedSyntax {
                        path: path.to_path_buf(),
                        line: mapping.line,
                        reason: format!(
                            "mapping pattern {pattern} selects {interface}; mapped interfaces cannot be safely unmanaged"
                        ),
                    });
                }
            }
        }
    }
    for rename in &parsed.renames {
        for value in &rename.mappings {
            let Some((from, to)) = value.split_once('=') else {
                return Err(HostNetError::UnsupportedSyntax {
                    path: path.to_path_buf(),
                    line: rename.line,
                    reason: format!("rename argument {value} must be CUR=NEW"),
                });
            };
            for interface in selected {
                let from_matches =
                    selection_pattern_matches(from, interface).map_err(|reason| {
                        HostNetError::UnsupportedSyntax {
                            path: path.to_path_buf(),
                            line: rename.line,
                            reason,
                        }
                    })?;
                if from_matches || to == *interface {
                    return Err(HostNetError::UnsupportedSyntax {
                        path: path.to_path_buf(),
                        line: rename.line,
                        reason: format!("rename {value} affects a selected interface"),
                    });
                }
            }
        }
    }
    Ok(())
}

fn reject_selected_dependencies(
    path: &Path,
    parsed: &parse::ParsedFile,
    selected: &HashSet<&str>,
) -> Result<(), HostNetError> {
    for block in &parsed.blocks {
        if selected.contains(block.iface.as_str()) {
            continue;
        }
        if let Some(template) = block.inherits.as_deref()
            && selected.contains(template)
        {
            return Err(HostNetError::UnsupportedSyntax {
                path: path.to_path_buf(),
                line: block.declaration_lines[0] + 1,
                reason: format!(
                    "interface {} inherits selected interface {template} and cannot be safely unmanaged",
                    block.iface
                ),
            });
        }
        for option in &block.options {
            if !matches!(
                option.key.as_str(),
                "bridge_ports" | "bridge-ports" | "bond-slaves" | "bond_slaves"
            ) {
                continue;
            }
            for value in &option.values {
                let pattern =
                    glob::Pattern::new(value).map_err(|error| HostNetError::UnsupportedSyntax {
                        path: path.to_path_buf(),
                        line: option.lines[0] + 1,
                        reason: format!("invalid {} pattern {value}: {error}", option.key),
                    })?;
                if value == "all" || selected.iter().any(|interface| pattern.matches(interface)) {
                    return Err(HostNetError::UnsupportedSyntax {
                        path: path.to_path_buf(),
                        line: option.lines[0] + 1,
                        reason: format!(
                            "interface {} depends on a selected interface through {}",
                            block.iface, option.key
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

fn selection_pattern_matches(pattern: &str, interface: &str) -> Result<bool, String> {
    let body = pattern
        .trim_start_matches('/')
        .split('=')
        .next()
        .unwrap_or_default();
    if body.contains('/') {
        return Err(format!(
            "complex interface selection pattern {pattern} cannot be safely evaluated"
        ));
    }
    glob::Pattern::new(body)
        .map(|pattern| pattern.matches(interface))
        .map_err(|error| format!("invalid interface selection pattern {pattern}: {error}"))
}

fn render(lines: &[String], ends_with_newline: bool) -> String {
    let mut out = lines.join("\n");
    if ends_with_newline {
        out.push('\n');
    }
    out
}

pub(crate) fn capture_metadata(path: &Path) -> Result<FileMetadata, HostNetError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|source| HostNetError::UnreadableFile {
            path: path.to_path_buf(),
            source,
        })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(HostNetError::PathSafety {
            path: path.to_path_buf(),
            reason: "host network configuration must be a regular non-symlink file".into(),
        });
    }
    Ok(FileMetadata {
        mode: metadata.mode() & 0o7777,
        uid: metadata.uid(),
        gid: metadata.gid(),
    })
}

pub(crate) fn verify_edit(edit: &FileEdit) -> Result<(), HostNetError> {
    verify_snapshot(&edit.path, &edit.original_content, &edit.metadata)
}

fn verify_snapshot(
    path: &Path,
    expected_content: &[u8],
    expected_metadata: &FileMetadata,
) -> Result<(), HostNetError> {
    if capture_metadata(path)? != *expected_metadata {
        return Err(HostNetError::ConcurrentModification {
            path: path.to_path_buf(),
        });
    }
    let content = std::fs::read(path).map_err(|source| HostNetError::UnreadableFile {
        path: path.to_path_buf(),
        source,
    })?;
    if content != expected_content {
        return Err(HostNetError::ConcurrentModification {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn write_atomic(
    path: &Path,
    bytes: &[u8],
    metadata: Option<&FileMetadata>,
) -> Result<(), HostNetError> {
    write_atomic_checked(path, bytes, metadata, None)
}

pub(crate) fn write_atomic_checked(
    path: &Path,
    bytes: &[u8],
    metadata: Option<&FileMetadata>,
    expected: Option<(&[u8], &FileMetadata)>,
) -> Result<(), HostNetError> {
    let dir = path.parent().ok_or_else(|| HostNetError::PathSafety {
        path: path.to_path_buf(),
        reason: "target has no parent directory".into(),
    })?;
    let file_name = path
        .file_name()
        .ok_or_else(|| HostNetError::PathSafety {
            path: path.to_path_buf(),
            reason: "target has no file name".into(),
        })?
        .to_string_lossy()
        .to_string();
    reject_symlink_target(path)?;
    let (tmp, mut file) = create_temp(dir, &file_name)?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|source| HostNetError::AtomicWriteFailed {
                path: tmp.clone(),
                source,
            })?;
        if let Some(metadata) = metadata {
            let result = unsafe { libc::fchown(file.as_raw_fd(), metadata.uid, metadata.gid) };
            if result != 0 {
                return Err(HostNetError::AtomicWriteFailed {
                    path: tmp.clone(),
                    source: std::io::Error::last_os_error(),
                });
            }
        }
        file.set_permissions(std::fs::Permissions::from_mode(
            metadata.map_or(0o600, |metadata| metadata.mode),
        ))
        .and_then(|()| file.sync_all())
        .map_err(|source| HostNetError::AtomicWriteFailed {
            path: tmp.clone(),
            source,
        })?;
        drop(file);
        if let Some((expected_content, expected_metadata)) = expected {
            verify_snapshot(path, expected_content, expected_metadata)?;
        }
        reject_symlink_target(path)?;
        std::fs::rename(&tmp, path).map_err(|source| HostNetError::AtomicWriteFailed {
            path: path.to_path_buf(),
            source,
        })?;
        std::fs::File::open(dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| HostNetError::AtomicWriteFailed {
                path: dir.to_path_buf(),
                source,
            })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn create_temp(dir: &Path, file_name: &str) -> Result<(PathBuf, std::fs::File), HostNetError> {
    for _ in 0..128 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            ".{file_name}.lkit-hostnet.{}.{sequence}.tmp",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(HostNetError::AtomicWriteFailed { path, source });
            }
        }
    }
    Err(HostNetError::AtomicWriteFailed {
        path: dir.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique temporary file",
        ),
    })
}

fn reject_symlink_target(path: &Path) -> Result<(), HostNetError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(HostNetError::PathSafety {
            path: path.to_path_buf(),
            reason: "refusing to replace a symbolic link".into(),
        }),
        Ok(metadata) if !metadata.file_type().is_file() => Err(HostNetError::PathSafety {
            path: path.to_path_buf(),
            reason: "atomic write target must be a regular file".into(),
        }),
        Ok(_) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(HostNetError::UnreadableFile {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lkit-hostnet-edit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_set_for(paths: &[PathBuf]) -> FileSet {
        FileSet {
            interfaces: paths[0].clone(),
            files: paths.to_vec(),
        }
    }

    #[test]
    fn rewrites_selected_block_to_manual_and_removes_auto() {
        let dir = temp_dir("rewrite");
        let path = dir.join("interfaces");
        let original = b"auto eth0\niface eth0 inet static\n    address 192.168.1.10\n    gateway 192.168.1.1\nauto eth1\niface eth1 inet dhcp\n";
        std::fs::write(&path, original).unwrap();
        let plan =
            plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()]).unwrap();
        assert_eq!(plan.edits.len(), 1);
        assert_eq!(
            plan.edits[0].content,
            "iface eth0 inet manual\nauto eth1\niface eth1 inet dhcp\n"
        );
        let outcome = apply(&plan).unwrap();
        assert_eq!(outcome.edited, vec![path.clone()]);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            plan.edits[0].content.as_bytes()
        );
    }

    #[test]
    fn unrelated_content_is_preserved_byte_for_byte() {
        let dir = temp_dir("preserve");
        let path = dir.join("interfaces");
        let original = "auto eth0\niface eth0 inet static\n    address 192.168.1.10\n\n# loopback\niface lo inet loopback\n    dns-nameservers 1.1.1.1\n";
        std::fs::write(&path, original).unwrap();
        let plan =
            plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()]).unwrap();
        assert_eq!(
            plan.edits[0].content,
            "iface eth0 inet manual\n\n# loopback\niface lo inet loopback\n    dns-nameservers 1.1.1.1\n"
        );
    }

    #[test]
    fn dual_family_blocks_are_both_rewritten() {
        let dir = temp_dir("dual");
        let path = dir.join("interfaces");
        std::fs::write(
            &path,
            b"iface eth0 inet static\n    address 192.168.1.10\niface eth0 inet6 static\n    address fd00::1\n",
        )
        .unwrap();
        let plan =
            plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()]).unwrap();
        assert_eq!(
            plan.edits[0].content,
            "iface eth0 inet manual\niface eth0 inet6 manual\n"
        );
    }

    #[test]
    fn selected_interface_in_two_files_is_edited_in_both() {
        let dir = temp_dir("two-files");
        let main = dir.join("interfaces");
        let fragment = dir.join("fragments.conf");
        std::fs::write(
            &main,
            b"auto eth0\niface eth0 inet static\n    address 192.168.1.10\n",
        )
        .unwrap();
        std::fs::write(&fragment, b"auto eth1\niface eth1 inet dhcp\n").unwrap();
        let plan = plan_unmanage(
            &file_set_for(&[main.clone(), fragment.clone()]),
            &["eth0".into(), "eth1".into()],
        )
        .unwrap();
        assert_eq!(plan.edits.len(), 2);
        assert!(
            plan.edits
                .iter()
                .all(|edit| edit.content.contains("inet manual"))
        );
    }

    #[test]
    fn selection_line_is_rewritten_when_iface_is_in_another_file() {
        let dir = temp_dir("split-selection");
        let main = dir.join("interfaces");
        let fragment = dir.join("fragment");
        std::fs::write(&main, b"auto eth0 eth1\nsource fragment\n").unwrap();
        std::fs::write(&fragment, b"iface eth0 inet dhcp\n").unwrap();

        let plan = plan_unmanage(
            &file_set_for(&[main.clone(), fragment.clone()]),
            &["eth0".into()],
        )
        .unwrap();

        assert_eq!(plan.edits.len(), 2);
        assert_eq!(plan.edits[0].content, "auto eth1\nsource fragment\n");
        assert_eq!(plan.edits[1].content, "iface eth0 inet manual\n");
    }

    #[test]
    fn ppp_method_is_rejected() {
        let dir = temp_dir("ppp");
        let path = dir.join("interfaces");
        std::fs::write(&path, b"auto eth0\niface eth0 inet ppp\n    provider isp\n").unwrap();
        let error = plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()])
            .unwrap_err();
        assert!(matches!(
            error,
            HostNetError::UnsupportedMethod { ref iface, .. } if iface == "eth0"
        ));
    }

    #[test]
    fn already_manual_without_options_produces_no_edit() {
        let dir = temp_dir("idempotent");
        let path = dir.join("interfaces");
        std::fs::write(&path, b"iface eth0 inet manual\n").unwrap();
        let plan =
            plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()]).unwrap();
        assert!(plan.edits.is_empty());
    }

    #[test]
    fn unselected_interface_produces_empty_plan() {
        let dir = temp_dir("unselected");
        let path = dir.join("interfaces");
        std::fs::write(&path, b"auto eth0\niface eth0 inet dhcp\n").unwrap();
        let plan =
            plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth9".into()]).unwrap();
        assert!(plan.edits.is_empty());
    }

    #[test]
    fn file_without_trailing_newline_keeps_that_shape() {
        let dir = temp_dir("no-newline");
        let path = dir.join("interfaces");
        std::fs::write(&path, b"iface eth0 inet static\n    address 192.168.1.10").unwrap();
        let plan =
            plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()]).unwrap();
        assert_eq!(plan.edits[0].content, "iface eth0 inet manual");
    }

    #[test]
    fn apply_preserves_file_mode_and_owner() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = temp_dir("mode");
        let path = dir.join("interfaces");
        std::fs::write(&path, b"iface eth0 inet static\n    address 192.168.1.10\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        let before = std::fs::metadata(&path).unwrap();
        let plan =
            plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()]).unwrap();
        apply(&plan).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o666
        );
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
    }

    #[test]
    fn atomic_write_failure_leaves_no_tmp_file() {
        let dir = temp_dir("atomic-failure");
        let target = dir.join("missing/interfaces");
        let error = write_atomic(&target, b"content", None).unwrap_err();
        assert!(matches!(error, HostNetError::AtomicWriteFailed { .. }));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn non_indented_continued_options_and_inherits_are_removed() {
        let dir = temp_dir("valid-syntax");
        let path = dir.join("interfaces");
        std::fs::write(
            &path,
            b"allow-custom eth0 eth1\niface eth0 inet static inherits ethernet\naddress 192.168.1.10/\\\n24\ngateway 192.168.1.1\n",
        )
        .unwrap();
        let plan =
            plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()]).unwrap();
        assert_eq!(
            plan.edits[0].content,
            "allow-custom eth1\niface eth0 inet manual\n"
        );
    }

    #[test]
    fn selection_patterns_mappings_renames_and_dependencies_are_rejected() {
        for (name, content) in [
            ("selection-pattern", "auto /eth*\niface eth0 inet dhcp\n"),
            (
                "mapping",
                "mapping eth*\n    script /bin/map\niface eth0 inet dhcp\n",
            ),
            ("rename", "rename eth*=lan\niface eth0 inet dhcp\n"),
            ("rename-pattern", "rename /eth*=lan\niface eth0 inet dhcp\n"),
            (
                "bridge",
                "iface br0 inet manual\n    bridge_ports eth0\niface eth0 inet dhcp\n",
            ),
            (
                "bond",
                "iface bond0 inet manual\n    bond-slaves eth0\niface eth0 inet dhcp\n",
            ),
        ] {
            let dir = temp_dir(name);
            let path = dir.join("interfaces");
            std::fs::write(&path, content).unwrap();
            assert!(matches!(
                plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()]),
                Err(HostNetError::UnsupportedSyntax { .. })
            ));
        }
    }

    #[test]
    fn inherited_selected_template_is_rejected() {
        let dir = temp_dir("inherits-dependency");
        let path = dir.join("interfaces");
        std::fs::write(
            &path,
            b"iface eth0 inet static\n    mtu 1400\niface eth1 inet static inherits eth0\n    address 192.0.2.2/24\n",
        )
        .unwrap();
        assert!(matches!(
            plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()]),
            Err(HostNetError::UnsupportedSyntax { .. })
        ));
    }

    #[test]
    fn apply_rejects_content_and_metadata_drift_before_writing() {
        let dir = temp_dir("drift");
        let path = dir.join("interfaces");
        let original = b"iface eth0 inet static\n    address 192.168.1.10\n";
        std::fs::write(&path, original).unwrap();
        let plan =
            plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()]).unwrap();
        std::fs::write(&path, b"iface eth0 inet dhcp\n").unwrap();
        assert!(matches!(
            apply(&plan),
            Err(HostNetError::ConcurrentModification { .. })
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"iface eth0 inet dhcp\n");

        std::fs::write(&path, original).unwrap();
        let plan =
            plan_unmanage(&file_set_for(std::slice::from_ref(&path)), &["eth0".into()]).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            apply(&plan),
            Err(HostNetError::ConcurrentModification { .. })
        ));
    }

    #[test]
    fn legacy_temporary_symlink_is_never_followed() {
        let dir = temp_dir("temporary-symlink");
        let target = dir.join("interfaces");
        let victim = dir.join("victim");
        std::fs::write(&victim, b"keep\n").unwrap();
        std::os::unix::fs::symlink(&victim, dir.join(".interfaces.lkit-hostnet.tmp")).unwrap();
        write_atomic(&target, b"new\n", None).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new\n");
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep\n");
    }
}
