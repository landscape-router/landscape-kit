use std::path::Path;

use crate::error::HostNetError;
use crate::model::{
    EditOutcome, EditPlan, FileSet, FileSources, Manifest, ToolPaths, UnmanageOutcome, Validation,
};

/// 宿主网络适配器统一函数面。适配器无内部状态,方法按以下顺序调用:
///
/// `execute_unmanage` 固定执行 收集 → 改写计划 → backup → apply → validate；
/// backup 成功后的失败都会调用 guarded restore,不会覆盖外部修改。
///
/// 摘除与显式恢复对称:`restore` 按 manifest 逐字覆盖并恢复 mode/uid/gid,幂等。
/// 事务失败时使用 `restore_if_unchanged`,只回滚仍处于本次编辑结果的文件。
pub trait HostNetworkAdapter {
    fn collect(&self, sources: &FileSources) -> Result<FileSet, HostNetError>;

    fn plan_unmanage(
        &self,
        file_set: &FileSet,
        selected: &[String],
    ) -> Result<EditPlan, HostNetError>;

    fn apply(&self, plan: &EditPlan) -> Result<EditOutcome, HostNetError>;

    fn backup(&self, plan: &EditPlan, dest: &Path) -> Result<Manifest, HostNetError>;

    fn restore(&self, manifest: &Manifest) -> Result<(), HostNetError>;

    /// Restore only files that are still either unchanged or exactly at this
    /// plan's edited state. External edits are preserved and reported.
    fn restore_if_unchanged(
        &self,
        manifest: &Manifest,
        plan: &EditPlan,
    ) -> Result<(), HostNetError>;

    fn validate(&self, file_set: &FileSet, tools: &ToolPaths) -> Result<Validation, HostNetError>;

    fn execute_unmanage(
        &self,
        sources: &FileSources,
        selected: &[String],
        backup_dir: &Path,
        tools: &ToolPaths,
    ) -> Result<UnmanageOutcome, HostNetError> {
        let file_set = self.collect(sources)?;
        let plan = self.plan_unmanage(&file_set, selected)?;
        if plan.edits.is_empty() {
            return Ok(UnmanageOutcome {
                manifest: None,
                edited: Vec::new(),
                validation: Validation::Clean,
            });
        }

        let manifest = self.backup(&plan, backup_dir)?;
        let edit_outcome = match self.apply(&plan) {
            Ok(outcome) => outcome,
            Err(operation) => {
                return restore_after_failure(self, &manifest, &plan, operation);
            }
        };
        let validation = match self.validate(&file_set, tools) {
            Ok(validation) => validation,
            Err(operation) => {
                return restore_after_failure(self, &manifest, &plan, operation);
            }
        };
        if let Validation::Failed { exit, stderr } = &validation {
            let operation = HostNetError::ValidationFailed {
                exit: *exit,
                stderr: stderr.clone(),
            };
            return restore_after_failure(self, &manifest, &plan, operation);
        }

        Ok(UnmanageOutcome {
            manifest: Some(manifest),
            edited: edit_outcome.edited,
            validation,
        })
    }
}

fn restore_after_failure<A: HostNetworkAdapter + ?Sized>(
    adapter: &A,
    manifest: &Manifest,
    plan: &EditPlan,
    operation: HostNetError,
) -> Result<UnmanageOutcome, HostNetError> {
    match adapter.restore_if_unchanged(manifest, plan) {
        Ok(()) => Err(operation),
        Err(recovery) => Err(HostNetError::RecoveryFailed {
            operation: Box::new(operation),
            recovery: Box::new(recovery),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::model::{FileEdit, FileMetadata};

    struct FakeAdapter {
        empty: bool,
        apply_fails: bool,
        validation: Validation,
        restore_fails: bool,
        restored: Cell<bool>,
    }

    impl FakeAdapter {
        fn error(path: &str) -> HostNetError {
            HostNetError::AtomicWriteFailed {
                path: PathBuf::from(path),
                source: std::io::Error::other("injected failure"),
            }
        }
    }

    impl HostNetworkAdapter for FakeAdapter {
        fn collect(&self, sources: &FileSources) -> Result<FileSet, HostNetError> {
            Ok(FileSet {
                interfaces: sources.interfaces.clone(),
                files: vec![sources.interfaces.clone()],
            })
        }

        fn plan_unmanage(
            &self,
            file_set: &FileSet,
            _selected: &[String],
        ) -> Result<EditPlan, HostNetError> {
            Ok(EditPlan {
                edits: (!self.empty)
                    .then(|| FileEdit {
                        path: file_set.interfaces.clone(),
                        original_content: b"original\n".to_vec(),
                        content: "changed\n".into(),
                        metadata: FileMetadata {
                            mode: 0o644,
                            uid: 0,
                            gid: 0,
                        },
                    })
                    .into_iter()
                    .collect(),
            })
        }

        fn apply(&self, plan: &EditPlan) -> Result<EditOutcome, HostNetError> {
            if self.apply_fails {
                Err(Self::error("/apply"))
            } else {
                Ok(EditOutcome {
                    edited: plan.edits.iter().map(|edit| edit.path.clone()).collect(),
                })
            }
        }

        fn backup(&self, _plan: &EditPlan, _dest: &Path) -> Result<Manifest, HostNetError> {
            Ok(Manifest {
                schema_version: crate::model::MANIFEST_SCHEMA_VERSION,
                files: Vec::new(),
            })
        }

        fn restore(&self, _manifest: &Manifest) -> Result<(), HostNetError> {
            self.restored.set(true);
            if self.restore_fails {
                Err(Self::error("/restore"))
            } else {
                Ok(())
            }
        }

        fn restore_if_unchanged(
            &self,
            manifest: &Manifest,
            _plan: &EditPlan,
        ) -> Result<(), HostNetError> {
            self.restore(manifest)
        }

        fn validate(
            &self,
            _file_set: &FileSet,
            _tools: &ToolPaths,
        ) -> Result<Validation, HostNetError> {
            Ok(self.validation.clone())
        }
    }

    fn adapter(validation: Validation) -> FakeAdapter {
        FakeAdapter {
            empty: false,
            apply_fails: false,
            validation,
            restore_fails: false,
            restored: Cell::new(false),
        }
    }

    #[test]
    fn execute_returns_success_for_clean_and_unavailable_validation() {
        for validation in [Validation::Clean, Validation::Unavailable] {
            let adapter = adapter(validation.clone());
            let result = adapter
                .execute_unmanage(
                    &FileSources::new("/interfaces".into()),
                    &["eth0".into()],
                    Path::new("/backup"),
                    &ToolPaths::default(),
                )
                .unwrap();
            assert!(result.manifest.is_some());
            assert_eq!(result.validation, validation);
            assert!(!adapter.restored.get());
        }
    }

    #[test]
    fn execute_restores_validation_failure() {
        let adapter = adapter(Validation::Failed {
            exit: Some(2),
            stderr: "bad config".into(),
        });
        let error = adapter
            .execute_unmanage(
                &FileSources::new("/interfaces".into()),
                &["eth0".into()],
                Path::new("/backup"),
                &ToolPaths::default(),
            )
            .unwrap_err();
        assert!(matches!(error, HostNetError::ValidationFailed { .. }));
        assert!(adapter.restored.get());
    }

    #[test]
    fn execute_reports_operation_and_recovery_failures() {
        let adapter = FakeAdapter {
            apply_fails: true,
            restore_fails: true,
            ..adapter(Validation::Clean)
        };
        let error = adapter
            .execute_unmanage(
                &FileSources::new("/interfaces".into()),
                &["eth0".into()],
                Path::new("/backup"),
                &ToolPaths::default(),
            )
            .unwrap_err();
        assert!(matches!(error, HostNetError::RecoveryFailed { .. }));
        assert!(adapter.restored.get());
    }

    #[test]
    fn execute_skips_backup_for_empty_plan() {
        let adapter = FakeAdapter {
            empty: true,
            ..adapter(Validation::Clean)
        };
        let result = adapter
            .execute_unmanage(
                &FileSources::new("/interfaces".into()),
                &["eth0".into()],
                Path::new("/backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        assert!(result.manifest.is_none());
        assert!(result.edited.is_empty());
    }
}
