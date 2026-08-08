pub(crate) const KERNEL_KERNEL_VERSION: &str = "kernel.kernel_version";
pub(crate) const KERNEL_KERNEL_MEETS_6_9_REQUIREMENT: &str = "kernel.kernel_meets_6_9_requirement";
pub(crate) const KERNEL_KERNEL_VERSION_BELOW_REQUIREMENT: &str =
    "kernel.kernel_version_below_requirement";
pub(crate) const KERNEL_UPGRADE_KERNEL_6_9_OR_LATER: &str = "kernel.upgrade_kernel_6_9_or_later";
pub(crate) const KERNEL_UNABLE_PARSE_KERNEL_VERSION: &str = "kernel.unable_parse_kernel_version";
pub(crate) const KERNEL_UNAVAILABLE: &str = "kernel.unavailable";
pub(crate) const KERNEL_UNABLE_READ_OSRELEASE: &str = "kernel.unable_read_osrelease";
pub(crate) const KERNEL_BPF_SUBSYSTEM_AND_JIT: &str = "kernel.bpf_subsystem_and_jit";
pub(crate) const KERNEL_BPF_PROG_GET_NEXT_ID_PROBE_SUCCEEDED: &str =
    "kernel.bpf_prog_get_next_id_probe_succeeded";
pub(crate) const KERNEL_BPF_PROG_GET_NEXT_ID_ENOENT: &str = "kernel.bpf_prog_get_next_id_enoent";
pub(crate) const KERNEL_BPF_SYSCALL_UNAVAILABLE: &str = "kernel.bpf_syscall_unavailable";
pub(crate) const KERNEL_KERNEL_DOES_NOT_SUPPORT_BPF_SYSCALL: &str =
    "kernel.kernel_does_not_support_bpf_syscall";
pub(crate) const KERNEL_USE_KERNEL_SUPPORTING_EBPF: &str = "kernel.use_kernel_supporting_ebpf";
pub(crate) const KERNEL_BPF_DENIED: &str = "kernel.bpf_denied";
pub(crate) const KERNEL_BPF_PERMISSION_ERROR_AS_ROOT: &str = "kernel.bpf_permission_error_as_root";
pub(crate) const KERNEL_CHECK_SECCOMP_OR_LSM_RESTRICTION: &str =
    "kernel.check_seccomp_or_lsm_restriction";
pub(crate) const KERNEL_PERMISSION_DENIED: &str = "kernel.permission_denied";
pub(crate) const KERNEL_CANNOT_PROBE_BPF_WITH_CURRENT_IDENTITY: &str =
    "kernel.cannot_probe_bpf_with_current_identity";
pub(crate) const KERNEL_RUN_LKIT_CHECK_AS_ROOT: &str = "kernel.run_lkit_check_as_root";
pub(crate) const KERNEL_BPF_PROBE_UNEXPECTED_ERROR: &str = "kernel.bpf_probe_unexpected_error";
pub(crate) const KERNEL_UNKNOWN_ERROR: &str = "kernel.unknown_error";
pub(crate) const KERNEL_BPF_PROBE_FAILED: &str = "kernel.bpf_probe_failed";
pub(crate) const KERNEL_BPF_AVAILABLE_JIT_ENABLED: &str = "kernel.bpf_available_jit_enabled";
pub(crate) const KERNEL_BPF_SYSCALL_AND_JIT_AVAILABLE: &str =
    "kernel.bpf_syscall_and_jit_available";
pub(crate) const KERNEL_JIT_DISABLED: &str = "kernel.jit_disabled";
pub(crate) const KERNEL_BPF_JIT_IS_DISABLED: &str = "kernel.bpf_jit_is_disabled";
pub(crate) const KERNEL_ENABLE_JIT_SYSCTL: &str = "kernel.enable_jit_sysctl";
pub(crate) const KERNEL_UNRECOGNIZED_BPF_JIT_STATUS: &str = "kernel.unrecognized_bpf_jit_status";
pub(crate) const KERNEL_JIT_STATUS_FILE_UNREADABLE_BUILTIN: &str =
    "kernel.jit_status_file_unreadable_builtin";
pub(crate) const KERNEL_CONFIG_BPF_JIT_Y: &str = "kernel.config_bpf_jit_y";
pub(crate) const KERNEL_JIT_IS_A_MODULE: &str = "kernel.jit_is_a_module";
pub(crate) const KERNEL_UNABLE_CONFIRM_BPF_JIT_MODULE_LOADED: &str =
    "kernel.unable_confirm_bpf_jit_module_loaded";
pub(crate) const KERNEL_BPF_JIT_DISABLED_AT_BUILD: &str = "kernel.bpf_jit_disabled_at_build";
pub(crate) const KERNEL_USE_KERNEL_WITH_CONFIG_BPF_JIT: &str =
    "kernel.use_kernel_with_config_bpf_jit";
pub(crate) const KERNEL_UNKNOWN: &str = "kernel.unknown";
pub(crate) const KERNEL_UNABLE_READ_BPF_JIT_ENABLE_OR_CONFIG: &str =
    "kernel.unable_read_bpf_jit_enable_or_config";
#[cfg(not(target_os = "linux"))]
pub(crate) const KERNEL_NOT_LINUX: &str = "kernel.not_linux";
#[cfg(not(target_os = "linux"))]
pub(crate) const KERNEL_CURRENT_PLATFORM_CANNOT_PROBE_BPF: &str =
    "kernel.current_platform_cannot_probe_bpf";
pub(crate) const KERNEL_KERNEL_BTF_INFORMATION: &str = "kernel.kernel_btf_information";
pub(crate) const KERNEL_PRESENT_AND_READABLE: &str = "kernel.present_and_readable";
pub(crate) const KERNEL_KERNEL_BTF_INFORMATION_AVAILABLE: &str =
    "kernel.kernel_btf_information_available";
pub(crate) const KERNEL_PRESENT_BUT_UNREADABLE: &str = "kernel.present_but_unreadable";
pub(crate) const KERNEL_BTF_EXISTS_BUT_CANNOT_READ: &str = "kernel.btf_exists_but_cannot_read";
pub(crate) const KERNEL_MISSING: &str = "kernel.missing";
pub(crate) const KERNEL_BTF_PATH_DOES_NOT_EXIST: &str = "kernel.btf_path_does_not_exist";
pub(crate) const KERNEL_USE_KERNEL_WITH_BTF_SUPPORT: &str = "kernel.use_kernel_with_btf_support";
pub(crate) const KERNEL_CGROUP_FILESYSTEM: &str = "kernel.cgroup_filesystem";
pub(crate) const KERNEL_MOUNTED: &str = "kernel.mounted";
pub(crate) const KERNEL_CGROUP_FILESYSTEM_AVAILABLE: &str = "kernel.cgroup_filesystem_available";
pub(crate) const KERNEL_MOUNTED_BUT_UNREADABLE: &str = "kernel.mounted_but_unreadable";
pub(crate) const KERNEL_CGROUP_FS_UNREADABLE: &str = "kernel.cgroup_fs_unreadable";
pub(crate) const KERNEL_NOT_MOUNTED: &str = "kernel.not_mounted";
pub(crate) const KERNEL_NO_CGROUP_MOUNTED_AT_SYS_FS_CGROUP: &str =
    "kernel.no_cgroup_mounted_at_sys_fs_cgroup";
pub(crate) const KERNEL_CHECK_CGROUP_SUPPORT_ENABLED: &str = "kernel.check_cgroup_support_enabled";
pub(crate) const KERNEL_UNABLE_READ_SELF_MOUNTS: &str = "kernel.unable_read_self_mounts";
pub(crate) const KERNEL_AVAILABLE: &str = "kernel.available";
pub(crate) const KERNEL_CGROUP_V2_CPU_CONTROLLER_ENABLED: &str =
    "kernel.cgroup_v2_cpu_controller_enabled";
pub(crate) const KERNEL_DISABLED: &str = "kernel.disabled";
pub(crate) const KERNEL_CGROUP_V2_CPU_CONTROLLER_NOT_AVAILABLE: &str =
    "kernel.cgroup_v2_cpu_controller_not_available";
pub(crate) const KERNEL_ADD_CPU_TO_CONTROLLERS: &str = "kernel.add_cpu_to_controllers";
pub(crate) const KERNEL_CGROUP_V1_CPU_CONTROLLER_MOUNTED: &str =
    "kernel.cgroup_v1_cpu_controller_mounted";
pub(crate) const KERNEL_UNABLE_READ_CGROUP_CONTROLLER_INFORMATION: &str =
    "kernel.unable_read_cgroup_controller_information";
pub(crate) const KERNEL_CGROUP_FILESYSTEM_UNAVAILABLE: &str =
    "kernel.cgroup_filesystem_unavailable";
pub(crate) const KERNEL_CGROUP_BPF_SUPPORT: &str = "kernel.cgroup_bpf_support";
pub(crate) const KERNEL_BPF_EVENTS_SUPPORT: &str = "kernel.bpf_events_support";
pub(crate) const KERNEL_ENABLED: &str = "kernel.enabled";
pub(crate) const KERNEL_CONFIG_NAME_BUILTIN: &str = "kernel.config_name_builtin";
pub(crate) const KERNEL_MODULE: &str = "kernel.module";
pub(crate) const KERNEL_CONFIG_NAME_MODULE: &str = "kernel.config_name_module";
pub(crate) const KERNEL_USE_KERNEL_WITH_CONFIG_OPTION: &str =
    "kernel.use_kernel_with_config_option";
pub(crate) const KERNEL_UNABLE_READ_KERNEL_CONFIG_CAPABILITY: &str =
    "kernel.unable_read_kernel_config_capability";
