// 翻译 catalog key 常量。与 locales/en/、locales/zh/ 下的域文件一一对应。
// 通过 crate::tr!(crate::keys::X) / crate::tr_static!(crate::keys::X) 使用。
pub(crate) const MAIN_NON_INTERACTIVE_HELP: &str = "main.non_interactive_help";
pub(crate) const MAIN_LANG_HELP: &str = "main.lang_help";
pub(crate) const MAIN_SUBCOMMAND_REQUIRED_NON_INTERACTIVE: &str =
    "main.subcommand_required_non_interactive";
pub(crate) const MAIN_UNABLE_INSTALL_CTRL_C_HANDLER: &str = "main.unable_install_ctrl_c_handler";
pub(crate) const MAIN_UNABLE_START_INTERACTIVE_CONSOLE: &str =
    "main.unable_start_interactive_console";
pub(crate) const MAIN_CHECK_ABOUT: &str = "main.check_about";
pub(crate) const MAIN_VERBOSE_HELP: &str = "main.verbose_help";
pub(crate) const MAIN_COLOR_HELP: &str = "main.color_help";
pub(crate) const MAIN_INSTALL_ABOUT: &str = "main.install_about";
pub(crate) const MAIN_VERSION_HELP: &str = "main.version_help";
pub(crate) const MAIN_REPOSITORY_HELP: &str = "main.repository_help";
pub(crate) const MAIN_INSTALL_DIR_HELP: &str = "main.install_dir_help";
pub(crate) const MAIN_ADMIN_USER_HELP: &str = "main.admin_user_help";
pub(crate) const MAIN_PASSWORD_FILE_HELP: &str = "main.password_file_help";
pub(crate) const MAIN_SERVICE_MANAGER_HELP: &str = "main.service_manager_help";
pub(crate) const MAIN_FORCE_HELP: &str = "main.force_help";
pub(crate) const MAIN_TAKEOVER_NETWORK_HELP: &str = "main.takeover_network_help";
pub(crate) const MAIN_NETWORK_ABOUT: &str = "main.network_about";
pub(crate) const MAIN_NETWORK_STATUS_ABOUT: &str = "main.network_status_about";
pub(crate) const MAIN_NETWORK_CONFIRM_ABOUT: &str = "main.network_confirm_about";
pub(crate) const MAIN_NETWORK_ROLLBACK_ABOUT: &str = "main.network_rollback_about";
pub(crate) const MAIN_SWITCH_ABOUT: &str = "main.switch_about";
pub(crate) const MAIN_SWITCH_VERSION_HELP: &str = "main.switch_version_help";
pub(crate) const MAIN_UPDATE_ABOUT: &str = "main.update_about";
pub(crate) const MAIN_UPDATE_VERSION_HELP: &str = "main.update_version_help";
pub(crate) const MAIN_REPOSITORY_OVERRIDE_HELP: &str = "main.repository_override_help";
pub(crate) const MAIN_ACCEPT_SERVICE_CHANGE_HELP: &str = "main.accept_service_change_help";
pub(crate) const MAIN_ALLOW_NO_BACKUP_HELP: &str = "main.allow_no_backup_help";
pub(crate) const MAIN_REPAIR_ABOUT: &str = "main.repair_about";
pub(crate) const MAIN_REPAIR_TARGET_HELP: &str = "main.repair_target_help";
pub(crate) const MAIN_BACKUP_ABOUT: &str = "main.backup_about";
pub(crate) const MAIN_BACKUP_CREATE_ABOUT: &str = "main.backup_create_about";
pub(crate) const MAIN_BACKUP_REMARK_HELP: &str = "main.backup_remark_help";
pub(crate) const MAIN_BACKUP_OUTPUT_HELP: &str = "main.backup_output_help";
pub(crate) const MAIN_BACKUP_LIST_ABOUT: &str = "main.backup_list_about";
pub(crate) const MAIN_BACKUP_SHOW_ABOUT: &str = "main.backup_show_about";
pub(crate) const MAIN_BACKUP_VERIFY_ABOUT: &str = "main.backup_verify_about";
pub(crate) const MAIN_BACKUP_DELETE_ABOUT: &str = "main.backup_delete_about";
pub(crate) const MAIN_BACKUP_DELETE_YES_HELP: &str = "main.backup_delete_yes_help";
pub(crate) const MAIN_BACKUP_ID_HELP: &str = "main.backup_id_help";
pub(crate) const MAIN_BACKUP_FILE_HELP: &str = "main.backup_file_help";
pub(crate) const MAIN_RESTORE_ABOUT: &str = "main.restore_about";
pub(crate) const MAIN_RESTORE_YES_HELP: &str = "main.restore_yes_help";
pub(crate) const MAIN_RESTORE_ALLOW_NO_BACKUP_HELP: &str = "main.restore_allow_no_backup_help";
pub(crate) const BACKUP_REQUIRES_EXISTING_INSTALLATION: &str =
    "backup.requires_existing_installation";
pub(crate) const BACKUP_CREATED: &str = "backup.created";
pub(crate) const BACKUP_NONE_FOUND: &str = "backup.none_found";
pub(crate) const BACKUP_LIST_INVALID: &str = "backup.list_invalid";
pub(crate) const BACKUP_VERIFIED: &str = "backup.verified";
pub(crate) const BACKUP_REMARK_PROMPT: &str = "backup.remark_prompt";
pub(crate) const BACKUP_AUTO_REMARK_SWITCH: &str = "backup.auto_remark_switch";
pub(crate) const BACKUP_AUTO_REMARK_REPAIR: &str = "backup.auto_remark_repair";
pub(crate) const BACKUP_AUTO_REMARK_RESTORE: &str = "backup.auto_remark_restore";
pub(crate) const BACKUP_DELETED: &str = "backup.deleted";
pub(crate) const BACKUP_DELETE_CONFIRM: &str = "backup.delete_confirm";
pub(crate) const BACKUP_DELETE_REFUSED: &str = "backup.delete_refused";
pub(crate) const BACKUP_DELETE_REQUIRES_YES: &str = "backup.delete_requires_yes";
pub(crate) const BACKUP_DELETE_INVALID_ID: &str = "backup.delete_invalid_id";
pub(crate) const RESTORE_REQUIRES_EXISTING_INSTALLATION: &str =
    "restore.requires_existing_installation";
pub(crate) const RESTORE_CONFIRM_PLAN: &str = "restore.confirm_plan";
pub(crate) const RESTORE_CONFIRM_MINIMAL_SCOPE: &str = "restore.confirm_minimal_scope";
pub(crate) const RESTORE_CONFIRM_STOP_WITH_OWN_MANAGER: &str =
    "restore.confirm_stop_with_own_manager";
pub(crate) const RESTORE_WARNING_NO_PROTECTION_BACKUP: &str =
    "restore.warning_no_protection_backup";
pub(crate) const RESTORE_ROLLBACK_FAILED: &str = "restore.rollback_failed";
pub(crate) const RESTORE_COMMITTED: &str = "restore.committed";
pub(crate) const RESTORE_NONE_REFERENCE_COMMAND: &str = "restore.none_reference_command";
pub(crate) const RESTORE_FAILED_ROLLED_BACK: &str = "restore.failed_rolled_back";
pub(crate) const RESTORE_FAILED_ROLLBACK_FAILED: &str = "restore.failed_rollback_failed";
pub(crate) const MAIN_RECONCILE_ABOUT: &str = "main.reconcile_about";
pub(crate) const MAIN_SERVICE_MANAGER_ABOUT: &str = "main.service_manager_about";
pub(crate) const MAIN_SERVICE_MANAGER_TARGET_HELP: &str = "main.service_manager_target_help";
pub(crate) const MAIN_UNABLE_DELEGATE_SYSTEMD: &str = "main.unable_delegate_systemd";
pub(crate) const CHECK_RUNTIME_IDENTITY_AND_PLATFORM: &str = "check.runtime_identity_and_platform";
pub(crate) const CHECK_KERNEL_VERSION_AND_CAPABILITIES: &str =
    "check.kernel_version_and_capabilities";
pub(crate) const CHECK_RESOURCE_LIMITS: &str = "check.resource_limits";
pub(crate) const CHECK_REQUIRED_COMMANDS_AND_RUNTIME_DEPENDENCIES: &str =
    "check.required_commands_and_runtime_dependencies";
pub(crate) const CHECK_PORT_CONFLICTS: &str = "check.port_conflicts";
pub(crate) const CHECK_SYSTEM_SERVICES_AND_SECURITY_POLICY: &str =
    "check.system_services_and_security_policy";
pub(crate) const CHECK_DNS_CONFIGURATION_RISKS: &str = "check.dns_configuration_risks";
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
pub(crate) const SERVICE_QUERY_FAILED: &str = "service.query_failed";
pub(crate) const SERVICE_SYSTEMCTL_UNAVAILABLE_OR_QUERY_FAILED: &str =
    "service.systemctl_unavailable_or_query_failed";
pub(crate) const SERVICE_RUNNING: &str = "service.running";
pub(crate) const SERVICE_NETWORK_MANAGER_RUNNING_TAKEOVER: &str =
    "service.network_manager_running_takeover";
pub(crate) const SERVICE_STOP_AND_DISABLE_NETWORK_MANAGER: &str =
    "service.stop_and_disable_network_manager";
pub(crate) const SERVICE_ENABLED_NOT_RUNNING: &str = "service.enabled_not_running";
pub(crate) const SERVICE_NETWORK_MANAGER_ENABLED_TAKEOVER: &str =
    "service.network_manager_enabled_takeover";
pub(crate) const SERVICE_DISABLE_NETWORK_MANAGER_NOW: &str = "service.disable_network_manager_now";
pub(crate) const SERVICE_INSTALLED_NOT_RUNNING: &str = "service.installed_not_running";
pub(crate) const SERVICE_NETWORK_MANAGER_INSTALLED_NOT_RUNNING: &str =
    "service.network_manager_installed_not_running";
pub(crate) const SERVICE_NOT_INSTALLED: &str = "service.not_installed";
pub(crate) const SERVICE_NETWORK_MANAGER_NOT_INSTALLED: &str =
    "service.network_manager_not_installed";
pub(crate) const SERVICE_RUNNING_OR_ENABLED: &str = "service.running_or_enabled";
pub(crate) const SERVICE_SYSTEMD_RESOLVED_MAY_OCCUPY_DNS: &str =
    "service.systemd_resolved_may_occupy_dns";
pub(crate) const SERVICE_RELEASE_DNS_PORT_53: &str = "service.release_dns_port_53";
pub(crate) const SERVICE_NOT_INSTALLED_OR_ENABLED: &str = "service.not_installed_or_enabled";
pub(crate) const SERVICE_SYSTEMD_RESOLVED_NOT_RUNNING_OR_ENABLED: &str =
    "service.systemd_resolved_not_running_or_enabled";
pub(crate) const SERVICE_FIREWALLD_MAY_BLOCK_RULES: &str = "service.firewalld_may_block_rules";
pub(crate) const SERVICE_CONFIRM_LANDSCAPE_PORTS_ALLOWED: &str =
    "service.confirm_landscape_ports_allowed";
pub(crate) const SERVICE_FIREWALLD_ENABLED_MAY_BLOCK_AFTER_RESTART: &str =
    "service.firewalld_enabled_may_block_after_restart";
pub(crate) const SERVICE_FIREWALLD_NOT_RUNNING_OR_ENABLED: &str =
    "service.firewalld_not_running_or_enabled";
pub(crate) const SERVICE_SELINUX_ENFORCING_MODE: &str = "service.selinux_enforcing_mode";
pub(crate) const SERVICE_REQUIRE_SELINUX_PERMISSIONS: &str = "service.require_selinux_permissions";
pub(crate) const SERVICE_SELINUX_NOT_ENFORCING: &str = "service.selinux_not_enforcing";
pub(crate) const SERVICE_UNAVAILABLE: &str = "service.unavailable";
pub(crate) const SERVICE_UNABLE_READ_SELINUX_STATUS: &str = "service.unable_read_selinux_status";
pub(crate) const SERVICE_DISABLED: &str = "service.disabled";
pub(crate) const SERVICE_SELINUX_DISABLED: &str = "service.selinux_disabled";
pub(crate) const DEPENDENCY_IP_COMMAND: &str = "dependency.ip_command";
pub(crate) const DEPENDENCY_IP_COMMAND_EXISTS_EXECUTABLE: &str =
    "dependency.ip_command_exists_executable";
pub(crate) const DEPENDENCY_NOT_FOUND: &str = "dependency.not_found";
pub(crate) const DEPENDENCY_IP_COMMAND_NOT_FOUND: &str = "dependency.ip_command_not_found";
pub(crate) const DEPENDENCY_TC_COMMAND_AND_BPF: &str = "dependency.tc_command_and_bpf";
pub(crate) const DEPENDENCY_TC_EXISTS_SUPPORTS_BPF: &str = "dependency.tc_exists_supports_bpf";
pub(crate) const DEPENDENCY_TC_FILTER_HELP_FAILED: &str = "dependency.tc_filter_help_failed";
pub(crate) const DEPENDENCY_TC_HELP_MENTIONS_BPF: &str = "dependency.tc_help_mentions_bpf";
pub(crate) const DEPENDENCY_UPGRADE_IPROUTE2: &str = "dependency.upgrade_iproute2";
pub(crate) const DEPENDENCY_UNABLE_RUN_TC_FILTER_HELP: &str =
    "dependency.unable_run_tc_filter_help";
pub(crate) const DEPENDENCY_PPPD_COMMAND: &str = "dependency.pppd_command";
pub(crate) const DEPENDENCY_PPPD_COMMAND_EXISTS: &str = "dependency.pppd_command_exists";
pub(crate) const DEPENDENCY_PPPD_NOT_FOUND: &str = "dependency.pppd_not_found";
pub(crate) const DEPENDENCY_CONTAINER_RUNTIME: &str = "dependency.container_runtime";
pub(crate) const DEPENDENCY_DOCKER_OR_PODMAN_AVAILABLE: &str =
    "dependency.docker_or_podman_available";
pub(crate) const DEPENDENCY_NO_CONTAINER_RUNTIME: &str = "dependency.no_container_runtime";
pub(crate) const DEPENDENCY_CONTAINER_RUNTIME_NOT_REQUIRED: &str =
    "dependency.container_runtime_not_required";
pub(crate) const DEPENDENCY_INSTALL_PROVIDING_IP_AND_TC: &str =
    "dependency.install_providing_ip_and_tc";
pub(crate) const DEPENDENCY_INSTALL_PPP_PACKAGE: &str = "dependency.install_ppp_package";
pub(crate) const DEPENDENCY_PACKAGE_NAMED_PPP: &str = "dependency.package_named_ppp";
pub(crate) const DEPENDENCY_RUN_INSTALL_COMMAND: &str = "dependency.run_install_command";
pub(crate) const PLATFORM_RUNTIME_IDENTITY: &str = "platform.runtime_identity";
pub(crate) const PLATFORM_RUNNING_AS_ROOT: &str = "platform.running_as_root";
pub(crate) const PLATFORM_MUST_RUN_AS_ROOT: &str = "platform.must_run_as_root";
pub(crate) const PLATFORM_USE_SUDO_OR_ROOT: &str = "platform.use_sudo_or_root";
pub(crate) const PLATFORM_OPERATING_SYSTEM: &str = "platform.operating_system";
pub(crate) const PLATFORM_OS_IS_LINUX: &str = "platform.os_is_linux";
pub(crate) const PLATFORM_ONLY_LINUX_HOSTS_SUPPORTED: &str = "platform.only_linux_hosts_supported";
pub(crate) const PLATFORM_RUN_ON_GLIBC_LINUX: &str = "platform.run_on_glibc_linux";
pub(crate) const PLATFORM_DISTRIBUTION: &str = "platform.distribution";
pub(crate) const PLATFORM_DISTRIBUTION_IDENTIFIED: &str = "platform.distribution_identified";
pub(crate) const PLATFORM_UNAVAILABLE: &str = "platform.unavailable";
pub(crate) const PLATFORM_UNABLE_READ_DISTRIBUTION_ID: &str =
    "platform.unable_read_distribution_id";
pub(crate) const PLATFORM_CONFIRM_GLIBC_AND_INSTALL_PACKAGES: &str =
    "platform.confirm_glibc_and_install_packages";
pub(crate) const PLATFORM_CPU_ARCHITECTURE: &str = "platform.cpu_architecture";
pub(crate) const PLATFORM_ARCHITECTURE_SUPPORTED_BY_ARTIFACTS: &str =
    "platform.architecture_supported_by_artifacts";
pub(crate) const PLATFORM_ARCHITECTURE_NOT_PRIMARY_TARGET: &str =
    "platform.architecture_not_primary_target";
pub(crate) const PLATFORM_CONFIRM_ARTIFACTS_EXIST: &str = "platform.confirm_artifacts_exist";
pub(crate) const PORTS_DNS_PORT: &str = "ports.dns_port";
pub(crate) const PORTS_HTTP_MANAGEMENT_PORT: &str = "ports.http_management_port";
pub(crate) const PORTS_HTTPS_MANAGEMENT_PORT: &str = "ports.https_management_port";
pub(crate) const PORTS_UNABLE_READ_LISTENER_INFORMATION: &str =
    "ports.unable_read_listener_information";
pub(crate) const PORTS_PORT_UNKNOWN: &str = "ports.port_unknown";
pub(crate) const PORTS_UNABLE_READ_ALL_LISTENER_TABLES: &str =
    "ports.unable_read_all_listener_tables";
pub(crate) const PORTS_RUN_AS_ROOT_FOR_PROC_NET: &str = "ports.run_as_root_for_proc_net";
pub(crate) const PORTS_PORT_NOT_LISTENING: &str = "ports.port_not_listening";
pub(crate) const PORTS_PORT_FREE: &str = "ports.port_free";
pub(crate) const PORTS_PORT_OCCUPIED: &str = "ports.port_occupied";
pub(crate) const PORTS_ANOTHER_SERVICE_LISTENING: &str = "ports.another_service_listening";
pub(crate) const PORTS_STOP_SERVICE_OR_MOVE_PORT: &str = "ports.stop_service_or_move_port";
pub(crate) const PORTS_LISTENER_USED_BY: &str = "ports.listener_used_by";
pub(crate) const PORTS_LISTENER_OWNER_UNREADABLE: &str = "ports.listener_owner_unreadable";
pub(crate) const DNS_RISK_NOTE: &str = "dns.risk_note";
pub(crate) const DNS_UNABLE_READ_SYMLINK_TARGET: &str = "dns.unable_read_symlink_target";
pub(crate) const DNS_RESOLV_CONF_SYMLINK: &str = "dns.resolv_conf_symlink";
pub(crate) const DNS_NO_NAMESERVER_ENTRIES: &str = "dns.no_nameserver_entries";
pub(crate) const DNS_NO_USABLE_NAMESERVER: &str = "dns.no_usable_nameserver";
pub(crate) const DNS_SYMLINK_RECOVERY_RISK: &str = "dns.symlink_recovery_risk";
pub(crate) const DNS_CONFIGURATION_VALID: &str = "dns.configuration_valid";
pub(crate) const DNS_UNREADABLE: &str = "dns.unreadable";
pub(crate) const DNS_RESOLV_CONF_EXISTS_CANNOT_READ: &str = "dns.resolv_conf_exists_cannot_read";
pub(crate) const DNS_MISSING: &str = "dns.missing";
pub(crate) const DNS_RESOLV_CONF_DOES_NOT_EXIST: &str = "dns.resolv_conf_does_not_exist";
pub(crate) const RESOURCE_HOST_MEMORY: &str = "resource.host_memory";
pub(crate) const RESOURCE_UNAVAILABLE: &str = "resource.unavailable";
pub(crate) const RESOURCE_UNABLE_READ_MEMINFO: &str = "resource.unable_read_meminfo";
pub(crate) const RESOURCE_TOTAL_AVAILABLE_MEMORY: &str = "resource.total_available_memory";
pub(crate) const RESOURCE_TOTAL_MEMORY_AVAILABLE_UNKNOWN: &str =
    "resource.total_memory_available_unknown";
pub(crate) const RESOURCE_MEMORY_INFORMATION_UNAVAILABLE: &str =
    "resource.memory_information_unavailable";
pub(crate) const RESOURCE_MEMORY_MEETS_2GIB_MINIMUM: &str = "resource.memory_meets_2gib_minimum";
pub(crate) const RESOURCE_TOTAL_MEMORY_BELOW_2GIB: &str = "resource.total_memory_below_2gib";
pub(crate) const RESOURCE_INCREASE_MEMORY_TO_2GIB: &str = "resource.increase_memory_to_2gib";
pub(crate) const RESOURCE_UNABLE_READ_MEMTOTAL: &str = "resource.unable_read_memtotal";
pub(crate) const PRESENTATION_WARNING: &str = "presentation.warning";
pub(crate) const PRESENTATION_SUGGESTION_PREFIX: &str = "presentation.suggestion_prefix";
pub(crate) const PRESENTATION_INSTALLATION_COMPLETE: &str = "presentation.installation_complete";
pub(crate) const PRESENTATION_INSTALLATION_FAILED: &str = "presentation.installation_failed";
pub(crate) const PRESENTATION_INSTALLATION_CANCELLED: &str = "presentation.installation_cancelled";
pub(crate) const PRESENTATION_DOWNLOAD: &str = "presentation.download";
pub(crate) const PRESENTATION_INSTALLATION_FINISHED_SUCCESSFULLY: &str =
    "presentation.installation_finished_successfully";
pub(crate) const PRESENTATION_INSTALLATION_REPORTED_FAILURE: &str =
    "presentation.installation_reported_failure";
pub(crate) const PRESENTATION_INSTALLATION_STOPPED_DURING_DOWNLOAD: &str =
    "presentation.installation_stopped_during_download";
pub(crate) const PRESENTATION_PREPARING_INSTALLATION: &str = "presentation.preparing_installation";
pub(crate) const PRESENTATION_WAITING_FOR_DOWNLOAD_PROGRESS: &str =
    "presentation.waiting_for_download_progress";
pub(crate) const PRESENTATION_APPLYING_CONFIGURATION: &str = "presentation.applying_configuration";
pub(crate) const PRESENTATION_STATUS: &str = "presentation.status";
pub(crate) const PRESENTATION_OUTPUT: &str = "presentation.output";
pub(crate) const PRESENTATION_CTRL_C_CLOSE: &str = "presentation.ctrl_c_close";
pub(crate) const PRESENTATION_ENTER_STOP_ESC_CANCEL: &str = "presentation.enter_stop_esc_cancel";
pub(crate) const PRESENTATION_CTRL_C_STOP_ESC_OPTIONS: &str =
    "presentation.ctrl_c_stop_esc_options";
pub(crate) const PRESENTATION_INSTALLATION_IN_PROGRESS_STOP_IGNORED: &str =
    "presentation.installation_in_progress_stop_ignored";
pub(crate) const PRESENTATION_STOP_DOWNLOAD_CONFIRM: &str = "presentation.stop_download_confirm";
pub(crate) const PRESENTATION_CONFIRM_STOP: &str = "presentation.confirm_stop";
pub(crate) const PRESENTATION_INSTALLATION_IS_APPLYING: &str =
    "presentation.installation_is_applying";
pub(crate) const PRESENTATION_OPERATION_IS_APPLYING: &str = "presentation.operation_is_applying";
pub(crate) const PRESENTATION_OPERATION_IN_PROGRESS_STOP_IGNORED: &str =
    "presentation.operation_in_progress_stop_ignored";
pub(crate) const PRESENTATION_PREPARING: &str = "presentation.preparing";
pub(crate) const PRESENTATION_STOPPING: &str = "presentation.stopping";
pub(crate) const PRESENTATION_ACTIVATING: &str = "presentation.activating";
pub(crate) const PRESENTATION_VERIFYING: &str = "presentation.verifying";
pub(crate) const PRESENTATION_OPERATION_INSTALL: &str = "presentation.operation_install";
pub(crate) const PRESENTATION_OPERATION_SWITCH: &str = "presentation.operation_switch";
pub(crate) const PRESENTATION_OPERATION_UPDATE: &str = "presentation.operation_update";
pub(crate) const PRESENTATION_OPERATION_REPAIR: &str = "presentation.operation_repair";
pub(crate) const PRESENTATION_OPERATION_RESTORE: &str = "presentation.operation_restore";
pub(crate) const PRESENTATION_OPERATION_SERVICE_MIGRATION: &str =
    "presentation.operation_service_migration";
pub(crate) const PRESENTATION_SWITCH_COMPLETE: &str = "presentation.switch_complete";
pub(crate) const PRESENTATION_SWITCH_FAILED: &str = "presentation.switch_failed";
pub(crate) const PRESENTATION_SWITCH_CANCELLED: &str = "presentation.switch_cancelled";
pub(crate) const PRESENTATION_UPDATE_COMPLETE: &str = "presentation.update_complete";
pub(crate) const PRESENTATION_UPDATE_FAILED: &str = "presentation.update_failed";
pub(crate) const PRESENTATION_UPDATE_CANCELLED: &str = "presentation.update_cancelled";
pub(crate) const PRESENTATION_REPAIR_COMPLETE: &str = "presentation.repair_complete";
pub(crate) const PRESENTATION_REPAIR_FAILED: &str = "presentation.repair_failed";
pub(crate) const PRESENTATION_REPAIR_CANCELLED: &str = "presentation.repair_cancelled";
pub(crate) const PRESENTATION_RESTORE_COMPLETE: &str = "presentation.restore_complete";
pub(crate) const PRESENTATION_RESTORE_FAILED: &str = "presentation.restore_failed";
pub(crate) const PRESENTATION_RESTORE_CANCELLED: &str = "presentation.restore_cancelled";
pub(crate) const PRESENTATION_SERVICE_MIGRATION_COMPLETE: &str =
    "presentation.service_migration_complete";
pub(crate) const PRESENTATION_SERVICE_MIGRATION_FAILED: &str =
    "presentation.service_migration_failed";
pub(crate) const PRESENTATION_SERVICE_MIGRATION_CANCELLED: &str =
    "presentation.service_migration_cancelled";
pub(crate) const PRESENTATION_OPERATION_FINISHED_SUCCESSFULLY: &str =
    "presentation.operation_finished_successfully";
pub(crate) const PRESENTATION_OPERATION_REPORTED_FAILURE: &str =
    "presentation.operation_reported_failure";
pub(crate) const PRESENTATION_OPERATION_STOPPED_DURING_DOWNLOAD: &str =
    "presentation.operation_stopped_during_download";
pub(crate) const PRESENTATION_BACKUP_CREATING: &str = "presentation.backup_creating";
pub(crate) const PRESENTATION_BACKUP_PROGRESS_EXPORTING: &str =
    "presentation.backup_progress_exporting";
pub(crate) const PRESENTATION_BACKUP_ARCHIVING: &str = "presentation.backup_archiving";
pub(crate) const PRESENTATION_BACKUP_PROGRESS_FINALIZING: &str =
    "presentation.backup_progress_finalizing";
pub(crate) const PRESENTATION_BACKUP_PROGRESS_DONE: &str = "presentation.backup_progress_done";
pub(crate) const PRESENTATION_BACKUP_PROGRESS_FAILED: &str = "presentation.backup_progress_failed";
pub(crate) const INTERACTIVE_SELECT_ONE_OPTION: &str = "interactive.select_one_option";
pub(crate) const INTERACTIVE_SELECT_ONE_OPTION_DEFAULT: &str =
    "interactive.select_one_option_default";
pub(crate) const INTERACTIVE_SELECTION_MUST_BE_NUMBER: &str =
    "interactive.selection_must_be_number";
pub(crate) const INTERACTIVE_SELECTION_OUT_OF_RANGE: &str = "interactive.selection_out_of_range";
pub(crate) const INTERACTIVE_SELECT_LAN_INTERFACES: &str = "interactive.select_lan_interfaces";
pub(crate) const INTERACTIVE_PASSWORD_AGAIN: &str = "interactive.password_again";
pub(crate) const REPORT_STATUS_HEADER: &str = "report.status_header";
pub(crate) const REPORT_CHECK_HEADER: &str = "report.check_header";
pub(crate) const REPORT_RESULT_HEADER: &str = "report.result_header";
pub(crate) const REPORT_TITLE_DETAIL: &str = "report.title_detail";
pub(crate) const REPORT_SUMMARY_LINE: &str = "report.summary_line";
pub(crate) const REPORT_CONCLUSION_BLOCKERS: &str = "report.conclusion_blockers";
pub(crate) const REPORT_CONCLUSION_UNKNOWN: &str = "report.conclusion_unknown";
pub(crate) const REPORT_CONCLUSION_WARNING: &str = "report.conclusion_warning";
pub(crate) const REPORT_CONCLUSION_PASS: &str = "report.conclusion_pass";
pub(crate) const REPORT_CONCLUSION_LINE: &str = "report.conclusion_line";
pub(crate) const SYSTEMD_WORKER_STOP_FAILED_WARNING: &str = "systemd_worker.stop_failed_warning";
pub(crate) const PREFLIGHT_SUGGESTION_PREFIX: &str = "preflight.suggestion_prefix";
pub(crate) const MANAGE_INSTALLATION_ALREADY_EXISTS: &str = "manage.installation_already_exists";
pub(crate) const MANAGE_COMMAND_REQUIRES_EXISTING_INSTALLATION: &str =
    "manage.command_requires_existing_installation";
pub(crate) const MANAGE_FORCE_CANNOT_BE_COMBINED: &str = "manage.force_cannot_be_combined";
pub(crate) const MANAGE_INSTALL_ROOT_IS: &str = "manage.install_root_is";
pub(crate) const MANAGE_FORCE_DOES_NOT_MODIFY: &str = "manage.force_does_not_modify";
pub(crate) const MANAGE_INSTALL_ROOT_MAY_CONTAIN: &str = "manage.install_root_may_contain";
pub(crate) const MANAGE_MANUALLY_DELETE_INSTALL_ROOT: &str = "manage.manually_delete_install_root";
pub(crate) const MANAGE_TAKEOVER_REQUIRES_INTERACTIVE_TERMINAL: &str =
    "manage.takeover_requires_interactive_terminal";
pub(crate) const MANAGE_ACTIVATED_AWAITING_NETWORK_CONFIRMATION: &str =
    "manage.activated_awaiting_network_confirmation";
pub(crate) const MANAGE_COMMITTED_FIRST_INSTALL: &str = "manage.committed_first_install";
pub(crate) const MANAGE_SYSTEMD_UNIT_REGISTERED: &str = "manage.systemd_unit_registered";
pub(crate) const MANAGE_TAKEOVER_AWAITING_CONFIRMATION: &str =
    "manage.takeover_awaiting_confirmation";
pub(crate) const MANAGE_RECONNECT_AND_RUN_CONFIRM: &str = "manage.reconnect_and_run_confirm";
pub(crate) const MANAGE_TAKEOVER_AWAITING_CONFIRMATION_DHCP: &str =
    "manage.takeover_awaiting_confirmation_dhcp";
pub(crate) const MANAGE_MANAGEMENT_INTERFACE: &str = "manage.management_interface";
pub(crate) const MANAGE_INITIALIZATION_PENDING: &str = "manage.initialization_pending";
pub(crate) const MANAGE_CONFIRM_BEFORE_ROLLBACK: &str = "manage.confirm_before_rollback";
pub(crate) const MANAGE_ENTER_ADMIN_PASSWORD: &str = "manage.enter_admin_password";
pub(crate) const MANAGE_OLD_MANUAL_DEPLOYMENT_WARNING: &str =
    "manage.old_manual_deployment_warning";
pub(crate) const MANAGE_MUST_RUN_AS_ROOT: &str = "manage.must_run_as_root";
pub(crate) const EXISTING_SERVICE_MANAGER_ALREADY: &str = "existing.service_manager_already";
pub(crate) const EXISTING_STATIC_PAGES_RESTORED: &str = "existing.static_pages_restored";
pub(crate) const EXISTING_BACKEND_RESTORED_AND_VERIFIED: &str =
    "existing.backend_restored_and_verified";
pub(crate) const EXISTING_REPAIR_FAILED_ROLLED_BACK: &str = "existing.repair_failed_rolled_back";
pub(crate) const EXISTING_REPAIR_FAILED_ROLLBACK_FAILED: &str =
    "existing.repair_failed_rollback_failed";
pub(crate) const EXISTING_SWITCHED_TO_VERSION: &str = "existing.switched_to_version";
pub(crate) const EXISTING_BACKUP_PRESERVED: &str = "existing.backup_preserved";
pub(crate) const EXISTING_NO_BACKUP_CREATED: &str = "existing.no_backup_created";
pub(crate) const EXISTING_ROLLED_BACK_USING_BACKUP: &str = "existing.rolled_back_using_backup";
pub(crate) const EXISTING_SWITCH_FAILED_ROLLED_BACK: &str = "existing.switch_failed_rolled_back";
pub(crate) const EXISTING_SWITCH_FAILED_ROLLBACK_FAILED: &str =
    "existing.switch_failed_rollback_failed";
pub(crate) const EXISTING_OBSERVED_INITIALIZATION_COMPLETION: &str =
    "existing.observed_initialization_completion";
pub(crate) const EXISTING_VERSION_INSTALLED_AND_VERIFIED: &str =
    "existing.version_installed_and_verified";
pub(crate) const EXISTING_KEEP_MODIFIED_UNIT: &str = "existing.keep_modified_unit";
pub(crate) const EXISTING_ACCEPT_SERVICE_CHANGE_IGNORED_UNIT: &str =
    "existing.accept_service_change_ignored_unit";
pub(crate) const EXISTING_ACCEPT_SERVICE_CHANGE_IGNORED_NO_UNIT: &str =
    "existing.accept_service_change_ignored_no_unit";
pub(crate) const DISCOVERY_SELECT_WAN_INTERFACE: &str = "discovery.select_wan_interface";
pub(crate) const DISCOVERY_SINGLE_ARM_CONFIRM: &str = "discovery.single_arm_confirm";
pub(crate) const DISCOVERY_SELECT_LAN_INTERFACES: &str = "discovery.select_lan_interfaces";
pub(crate) const DISCOVERY_MANAGEMENT_IPV4_ADDRESS: &str = "discovery.management_ipv4_address";
pub(crate) const DISCOVERY_LAN_DHCP_RANGE_START: &str = "discovery.lan_dhcp_range_start";
pub(crate) const DISCOVERY_LAN_DHCP_RANGE_END: &str = "discovery.lan_dhcp_range_end";
pub(crate) const TAKEOVER_NETWORK_COMMANDS_REQUIRE_ROOT: &str =
    "takeover.network_commands_require_root";
pub(crate) const TAKEOVER_TRANSACTION: &str = "takeover.transaction";
pub(crate) const TAKEOVER_PHASE: &str = "takeover.phase";
pub(crate) const TAKEOVER_MANAGEMENT_ADDRESS: &str = "takeover.management_address";
pub(crate) const TAKEOVER_DHCP_LEASE: &str = "takeover.dhcp_lease";
pub(crate) const TAKEOVER_CONFIRMATION_DEADLINE: &str = "takeover.confirmation_deadline";
pub(crate) const TAKEOVER_NO_TAKEOVER_AWAITING_CONFIRMATION: &str =
    "takeover.no_takeover_awaiting_confirmation";
pub(crate) const TAKEOVER_CONFIRMED_LANDSCAPE_TAKEOVER: &str =
    "takeover.confirmed_landscape_takeover";
pub(crate) const TAKEOVER_RESTORED_HOST_NETWORK_SERVICES: &str =
    "takeover.restored_host_network_services";
pub(crate) const SWITCH_DOWNGRADE_NOT_SUPPORTED: &str = "switch.downgrade_not_supported";
pub(crate) const SWITCH_TARGET_VERSION_ALREADY_ACTIVE: &str =
    "switch.target_version_already_active";
pub(crate) const SWITCH_WARNING_SERVICE_STOPPED_NO_BACKUP: &str =
    "switch.warning_service_stopped_no_backup";
pub(crate) const SWITCH_WARNING_ALLOW_NO_BACKUP_IGNORED: &str =
    "switch.warning_allow_no_backup_ignored";
pub(crate) const SWITCH_CONFIRM_STOP_WITH_OWN_MANAGER: &str =
    "switch.confirm_stop_with_own_manager";
pub(crate) const SWITCH_ROLLBACK_FAILED: &str = "switch.rollback_failed";
pub(crate) const UPDATE_REQUIRES_INTERACTIVE_TERMINAL: &str =
    "update.requires_interactive_terminal";
pub(crate) const UPDATE_SELECT_REPOSITORY: &str = "update.select_repository";
pub(crate) const UPDATE_REPOSITORY_CURRENT: &str = "update.repository_current";
pub(crate) const UPDATE_REPOSITORY_GITHUB: &str = "update.repository_github";
pub(crate) const UPDATE_REPOSITORY_MIRROR: &str = "update.repository_mirror";
pub(crate) const UPDATE_REPOSITORY_CUSTOM: &str = "update.repository_custom";
pub(crate) const UPDATE_REPOSITORY_URL: &str = "update.repository_url";
pub(crate) const UPDATE_ALREADY_UP_TO_DATE: &str = "update.already_up_to_date";
pub(crate) const UPDATE_CONFIRM_UPDATE: &str = "update.confirm_update";
pub(crate) const UPDATE_CANCELLED: &str = "update.cancelled";
pub(crate) const INSTALL_CLEANUP_FAILED_NETWORK: &str = "install.cleanup_failed_network";
pub(crate) const INSTALL_CLEANUP_FAILED_FIRST_INSTALL: &str =
    "install.cleanup_failed_first_install";
pub(crate) const REPAIR_ROLLBACK_FAILED: &str = "repair.rollback_failed";
pub(crate) const INSTALL_ONLY_X86_64_AND_AARCH64_SUPPORTED: &str =
    "install.only_x86_64_and_aarch64_supported";
pub(crate) const SERVICE_MANAGER_MIGRATED_TO_NONE: &str = "service_manager.migrated_to_none";
pub(crate) const SERVICE_MANAGER_START_MANUALLY_WITH: &str = "service_manager.start_manually_with";
pub(crate) const SERVICE_MANAGER_MIGRATED_TO_SYSTEMD: &str = "service_manager.migrated_to_systemd";
pub(crate) const PLAN_INVALID_VERSION: &str = "plan.invalid_version";
pub(crate) const PLAN_INSTALL_DIR_NOT_ABSOLUTE: &str = "plan.install_dir_not_absolute";
pub(crate) const PLAN_INVALID_ADMIN_USER: &str = "plan.invalid_admin_user";
pub(crate) const PLAN_PARAMETER_USAGE_ERROR: &str = "plan.parameter_usage_error";
pub(crate) const PLAN_REFUSED: &str = "plan.refused";
pub(crate) const PLAN_PREFLIGHT_CHECK_FAILED: &str = "plan.preflight_check_failed";
pub(crate) const PLAN_INSTALL_STATE_CORRUPTED: &str = "plan.install_state_corrupted";
pub(crate) const PLAN_TRANSACTION_CORRUPTED: &str = "plan.transaction_corrupted";
pub(crate) const PLAN_BLOCKED_BY_UNFINISHED_TRANSACTION: &str =
    "plan.blocked_by_unfinished_transaction";
pub(crate) const PLAN_ACTIVATION_DRIFT: &str = "plan.activation_drift";
pub(crate) const PLAN_DANGEROUS_DIRECTORY: &str = "plan.dangerous_directory";
pub(crate) const PLAN_LOCK_BUSY: &str = "plan.lock_busy";
pub(crate) const PLAN_UNSUPPORTED_PLATFORM: &str = "plan.unsupported_platform";
pub(crate) const PLAN_NO_STABLE_VERSION: &str = "plan.no_stable_version";
pub(crate) const PLAN_RELEASE_EXISTS: &str = "plan.release_exists";
pub(crate) const PLAN_INVALID_PASSWORD: &str = "plan.invalid_password";
pub(crate) const PLAN_INVALID_PASSWORD_FILE: &str = "plan.invalid_password_file";
pub(crate) const PLAN_INVALID_BACKUP: &str = "plan.invalid_backup";
pub(crate) const PLAN_EXPORT_FAILED: &str = "plan.export_failed";
pub(crate) const PLAN_SERVICE_NOT_RUNNING: &str = "plan.service_not_running";
pub(crate) const PLAN_NON_INTERACTIVE_ENVIRONMENT: &str = "plan.non_interactive_environment";
pub(crate) const PLAN_SYSTEMD_OPERATION_FAILED: &str = "plan.systemd_operation_failed";
pub(crate) const PLAN_HEALTH_CHECK_FAILED: &str = "plan.health_check_failed";
pub(crate) const PLAN_CONFLICTING_PROCESS: &str = "plan.conflicting_process";
pub(crate) const PLAN_HOST_STATE_BACKUP_FAILED: &str = "plan.host_state_backup_failed";
pub(crate) const PLAN_FAILED_TO_WRITE_INSTALL_STATE: &str = "plan.failed_to_write_install_state";
pub(crate) const PLAN_REPOSITORY_SELECTION_FAILED: &str = "plan.repository_selection_failed";
pub(crate) const PLAN_IO_ERROR: &str = "plan.io_error";
pub(crate) const CONSOLE_TERMINAL_REQUIRED: &str = "console.terminal_required";
pub(crate) const CONSOLE_OVERVIEW: &str = "console.overview";
pub(crate) const CONSOLE_INSTALL_MENU: &str = "console.install_menu";
pub(crate) const CONSOLE_CHECK_WORKER_STOPPED: &str = "console.check_worker_stopped";
pub(crate) const CONSOLE_CONFIGURE_NETWORK_TAKEOVER: &str = "console.configure_network_takeover";
pub(crate) const CONSOLE_ENVIRONMENT_CHECKS_NOT_COMPLETED: &str =
    "console.environment_checks_not_completed";
pub(crate) const CONSOLE_HINT_CTRL_C_EXIT_ENTER_CONFIRM_ESC_CANCEL: &str =
    "console.hint_ctrl_c_exit_enter_confirm_esc_cancel";
pub(crate) const CONSOLE_HINT_CTRL_C_EXIT_ESC_AGAIN: &str = "console.hint_ctrl_c_exit_esc_again";
pub(crate) const CONSOLE_HINT_CTRL_C_EXIT_SCROLL: &str = "console.hint_ctrl_c_exit_scroll";
pub(crate) const CONSOLE_HINT_ENTER_DETAILS_ESC_CLOSE_R: &str =
    "console.hint_enter_details_esc_close_r";
pub(crate) const CONSOLE_HINT_CTRL_C_EXIT_EDIT: &str = "console.hint_ctrl_c_exit_edit";
pub(crate) const CONSOLE_HINT_NAVIGATION: &str = "console.hint_navigation";
pub(crate) const CONSOLE_HINT_CHECKS_SELECTED: &str = "console.hint_checks_selected";
pub(crate) const CONSOLE_HINT_INSTALL_PANEL: &str = "console.hint_install_panel";
pub(crate) const CONSOLE_HINT_PANEL: &str = "console.hint_panel";
pub(crate) const CONSOLE_REPOSITORY_DEFAULT: &str = "console.repository_default";
pub(crate) const CONSOLE_REPOSITORY_MIRROR: &str = "console.repository_mirror";
pub(crate) const CONSOLE_REPOSITORY_CUSTOM: &str = "console.repository_custom";
pub(crate) const CONSOLE_MANAGER_AUTO: &str = "console.manager_auto";
pub(crate) const CONSOLE_VERSION_HELP: &str = "console.version_help";
pub(crate) const CONSOLE_REPOSITORY_HELP: &str = "console.repository_help";
pub(crate) const CONSOLE_REPOSITORY_URL_HELP: &str = "console.repository_url_help";
pub(crate) const CONSOLE_INSTALL_ROOT_HELP: &str = "console.install_root_help";
pub(crate) const CONSOLE_ADMIN_USER_HELP: &str = "console.admin_user_help";
pub(crate) const CONSOLE_PASSWORD_HELP: &str = "console.password_help";
pub(crate) const CONSOLE_CONFIRM_PASSWORD_HELP: &str = "console.confirm_password_help";
pub(crate) const CONSOLE_SERVICE_MANAGER_HELP: &str = "console.service_manager_help";
pub(crate) const CONSOLE_NETWORK_TAKEOVER_HELP: &str = "console.network_takeover_help";
pub(crate) const CONSOLE_START_INSTALLATION_HELP: &str = "console.start_installation_help";
pub(crate) const CONSOLE_INSTALL_HELP_FALLBACK_DESC: &str = "console.install_help_fallback_desc";
pub(crate) const CONSOLE_PASSWORD_CONFIRMATION_MISMATCH: &str =
    "console.password_confirmation_mismatch";
pub(crate) const CONSOLE_INVALID_WAN_GATEWAY: &str = "console.invalid_wan_gateway";
pub(crate) const CONSOLE_INVALID_LAN_DHCP_RANGE_START: &str =
    "console.invalid_lan_dhcp_range_start";
pub(crate) const CONSOLE_INVALID_LAN_DHCP_RANGE_END: &str = "console.invalid_lan_dhcp_range_end";
pub(crate) const CONSOLE_ROOT_REQUIRED_BADGE: &str = "console.root_required_badge";
pub(crate) const CONSOLE_NOT_INSTALLED_BADGE: &str = "console.not_installed_badge";
pub(crate) const CONSOLE_INSTALLED_BADGE: &str = "console.installed_badge";
pub(crate) const CONSOLE_ATTENTION_REQUIRED_BADGE: &str = "console.attention_required_badge";
pub(crate) const CONSOLE_TERMINAL_TOO_SMALL: &str = "console.terminal_too_small";
pub(crate) const CONSOLE_ENVIRONMENT_CHECKS_COULD_NOT_COMPLETE: &str =
    "console.environment_checks_could_not_complete";
pub(crate) const CONSOLE_ENVIRONMENT_CHECKS_BLOCK_INSTALLATION: &str =
    "console.environment_checks_block_installation";
pub(crate) const CONSOLE_CHECKS_DID_NOT_PASS: &str = "console.checks_did_not_pass";
pub(crate) const CONSOLE_DIALOG_ENTER_DETAILS_ESC_CLOSE_R: &str =
    "console.dialog_enter_details_esc_close_r";
pub(crate) const CONSOLE_INSTALL_BLOCKED: &str = "console.install_blocked";
pub(crate) const CONSOLE_LANDSCAPE_NETWORK_TAKEOVER: &str = "console.landscape_network_takeover";
pub(crate) const CONSOLE_SELECT_WAN_INTERFACE: &str = "console.select_wan_interface";
pub(crate) const CONSOLE_NO_IPV4: &str = "console.no_ipv4";
pub(crate) const CONSOLE_GATEWAY_NOT_FOUND: &str = "console.gateway_not_found";
pub(crate) const CONSOLE_WAN_IPV4_MODE: &str = "console.wan_ipv4_mode";
pub(crate) const CONSOLE_WAN_STATIC_IPV4_CONFIGURATION: &str =
    "console.wan_static_ipv4_configuration";
pub(crate) const CONSOLE_IPV4_ADDRESS_CIDR: &str = "console.ipv4_address_cidr";
pub(crate) const CONSOLE_DEFAULT_GATEWAY: &str = "console.default_gateway";
pub(crate) const CONSOLE_SELECT_LAN_INTERFACES: &str = "console.select_lan_interfaces";
pub(crate) const CONSOLE_NO_OTHER_INTERFACES: &str = "console.no_other_interfaces";
pub(crate) const CONSOLE_LINK_UP: &str = "console.link_up";
pub(crate) const CONSOLE_LINK_DOWN: &str = "console.link_down";
pub(crate) const CONSOLE_LAN_MANAGEMENT_IPV4_ADDRESS: &str = "console.lan_management_ipv4_address";
pub(crate) const CONSOLE_LAN_DHCP_RANGE_START: &str = "console.lan_dhcp_range_start";
pub(crate) const CONSOLE_LAN_DHCP_RANGE_END: &str = "console.lan_dhcp_range_end";
pub(crate) const CONSOLE_VALUE_PREFIX: &str = "console.value_prefix";
pub(crate) const CONSOLE_CONFIRM_NETWORK_TAKEOVER_PLAN: &str =
    "console.confirm_network_takeover_plan";
pub(crate) const CONSOLE_CONFIRM_WAN_INTERFACE: &str = "console.confirm_wan_interface";
pub(crate) const CONSOLE_CONFIRM_WAN_MODE_STATIC: &str = "console.confirm_wan_mode_static";
pub(crate) const CONSOLE_CONFIRM_WAN_MODE_DHCP: &str = "console.confirm_wan_mode_dhcp";
pub(crate) const CONSOLE_CONFIRM_LAN_MODE_WAN_ONLY: &str = "console.confirm_lan_mode_wan_only";
pub(crate) const CONSOLE_CONFIRM_LAN_INTERFACES: &str = "console.confirm_lan_interfaces";
pub(crate) const CONSOLE_CONFIRM_MANAGEMENT: &str = "console.confirm_management";
pub(crate) const CONSOLE_CONFIRM_DHCP_RANGE: &str = "console.confirm_dhcp_range";
pub(crate) const CONSOLE_CONFIRM_LAN_FLUSH_NOTE: &str = "console.confirm_lan_flush_note";
pub(crate) const CONSOLE_PRESS_ENTER_TO_START_INSTALLATION: &str =
    "console.press_enter_to_start_installation";
pub(crate) const CONSOLE_NETWORK_PANEL_TITLE: &str = "console.network_panel_title";
pub(crate) const CONSOLE_WIZARD_HINT_CANCEL: &str = "console.wizard_hint_cancel";
pub(crate) const CONSOLE_WIZARD_HINT_WAN: &str = "console.wizard_hint_wan";
pub(crate) const CONSOLE_WIZARD_HINT_MODE: &str = "console.wizard_hint_mode";
pub(crate) const CONSOLE_WIZARD_HINT_STATIC: &str = "console.wizard_hint_static";
pub(crate) const CONSOLE_WIZARD_HINT_LAN: &str = "console.wizard_hint_lan";
pub(crate) const CONSOLE_WIZARD_HINT_EDIT: &str = "console.wizard_hint_edit";
pub(crate) const CONSOLE_WIZARD_HINT_CONFIRM: &str = "console.wizard_hint_confirm";
pub(crate) const CONSOLE_CANCEL_NETWORK_WIZARD_QUESTION: &str =
    "console.cancel_network_wizard_question";
pub(crate) const CONSOLE_CANCEL_NETWORK_WIZARD_PRESS_ENTER: &str =
    "console.cancel_network_wizard_press_enter";
pub(crate) const CONSOLE_CANCEL_NETWORK_WIZARD_PRESS_ESC: &str =
    "console.cancel_network_wizard_press_esc";
pub(crate) const CONSOLE_CANCEL_WIZARD: &str = "console.cancel_wizard";
pub(crate) const CONSOLE_READY: &str = "console.ready";
pub(crate) const CONSOLE_EXIT_LANDSCAPE_KIT_QUESTION: &str = "console.exit_landscape_kit_question";
pub(crate) const CONSOLE_PRESS_ENTER_TO_EXIT: &str = "console.press_enter_to_exit";
pub(crate) const CONSOLE_PRESS_ESC_TO_CANCEL: &str = "console.press_esc_to_cancel";
pub(crate) const CONSOLE_CONFIRM_EXIT: &str = "console.confirm_exit";
pub(crate) const CONSOLE_NAVIGATION: &str = "console.navigation";
pub(crate) const CONSOLE_INSTALL_UNAVAILABLE: &str = "console.install_unavailable";
pub(crate) const CONSOLE_ROOT_PRIVILEGES_REQUIRED: &str = "console.root_privileges_required";
pub(crate) const CONSOLE_OVERVIEW_INSTALL_ROOT: &str = "console.overview_install_root";
pub(crate) const CONSOLE_LANDSCAPE_NOT_INSTALLED: &str = "console.landscape_not_installed";
pub(crate) const CONSOLE_LANDSCAPE_IS_INSTALLED: &str = "console.landscape_is_installed";
pub(crate) const CONSOLE_OVERVIEW_VERSION: &str = "console.overview_version";
pub(crate) const CONSOLE_OVERVIEW_SERVICE: &str = "console.overview_service";
pub(crate) const CONSOLE_OVERVIEW_INITIALIZATION_COMPLETE: &str =
    "console.overview_initialization_complete";
pub(crate) const CONSOLE_OVERVIEW_INITIALIZATION_PENDING: &str =
    "console.overview_initialization_pending";
pub(crate) const CONSOLE_INSTALLATION_STATE_NEEDS_ATTENTION: &str =
    "console.installation_state_needs_attention";
pub(crate) const CONSOLE_ENVIRONMENT_CHECKS: &str = "console.environment_checks";
pub(crate) const CONSOLE_NOT_RUN: &str = "console.not_run";
pub(crate) const CONSOLE_WAITING_TO_CHECK_HOST: &str = "console.waiting_to_check_host";
pub(crate) const CONSOLE_RUNNING: &str = "console.running";
pub(crate) const CONSOLE_CHECKING_THIS_HOST: &str = "console.checking_this_host";
pub(crate) const CONSOLE_FAILED: &str = "console.failed";
pub(crate) const CONSOLE_CHECKS_HAVE_NOT_RUN: &str = "console.checks_have_not_run";
pub(crate) const CONSOLE_PREFLIGHT_COUNTS: &str = "console.preflight_counts";
pub(crate) const CONSOLE_START_INSTALLATION_BUTTON: &str = "console.start_installation_button";
pub(crate) const CONSOLE_ABOUT_PREFIX: &str = "console.about_prefix";
pub(crate) const CONSOLE_ENVIRONMENT_CHECKS_HELP: &str = "console.environment_checks_help";
pub(crate) const CONSOLE_VERSION_LABEL: &str = "console.version_label";
pub(crate) const CONSOLE_REPOSITORY_LABEL: &str = "console.repository_label";
pub(crate) const CONSOLE_REPOSITORY_URL_LABEL: &str = "console.repository_url_label";
pub(crate) const CONSOLE_INSTALL_ROOT_LABEL: &str = "console.install_root_label";
pub(crate) const CONSOLE_ADMIN_USER_LABEL: &str = "console.admin_user_label";
pub(crate) const CONSOLE_PASSWORD_LABEL: &str = "console.password_label";
pub(crate) const CONSOLE_CONFIRM_PASSWORD_LABEL: &str = "console.confirm_password_label";
pub(crate) const CONSOLE_SERVICE_MANAGER_LABEL: &str = "console.service_manager_label";
pub(crate) const CONSOLE_NETWORK_TAKEOVER_LABEL: &str = "console.network_takeover_label";
pub(crate) const CONSOLE_START_INSTALLATION_LABEL: &str = "console.start_installation_label";
pub(crate) const CONSOLE_BACKUP_MENU: &str = "console.backup_menu";
pub(crate) const CONSOLE_BACKUP_CREATE: &str = "console.backup_create";
pub(crate) const CONSOLE_BACKUP_LOADING: &str = "console.backup_loading";
pub(crate) const CONSOLE_BACKUP_NONE_FOUND: &str = "console.backup_none_found";
pub(crate) const CONSOLE_BACKUP_INVALID_BADGE: &str = "console.backup_invalid_badge";
pub(crate) const CONSOLE_BACKUP_INVALID: &str = "console.backup_invalid";
pub(crate) const CONSOLE_BACKUP_SELECT_TO_RESTORE: &str = "console.backup_select_to_restore";
pub(crate) const CONSOLE_BACKUP_REQUIRES_INSTALL: &str = "console.backup_requires_install";
pub(crate) const CONSOLE_BACKUP_DETAILS_TITLE: &str = "console.backup_details_title";
pub(crate) const CONSOLE_BACKUP_ID_LABEL: &str = "console.backup_id_label";
pub(crate) const CONSOLE_BACKUP_CREATED_LABEL: &str = "console.backup_created_label";
pub(crate) const CONSOLE_BACKUP_VERSION_LABEL: &str = "console.backup_version_label";
pub(crate) const CONSOLE_BACKUP_LKIT_LABEL: &str = "console.backup_lkit_label";
pub(crate) const CONSOLE_BACKUP_ARCH_LABEL: &str = "console.backup_arch_label";
pub(crate) const CONSOLE_BACKUP_HOSTNAME_LABEL: &str = "console.backup_hostname_label";
pub(crate) const CONSOLE_BACKUP_REMARK_LABEL: &str = "console.backup_remark_label";
pub(crate) const CONSOLE_BACKUP_AUTO_LABEL: &str = "console.backup_auto_label";
pub(crate) const CONSOLE_BACKUP_SCOPE_LABEL: &str = "console.backup_scope_label";
pub(crate) const CONSOLE_BACKUP_CONTENTS_LABEL: &str = "console.backup_contents_label";
pub(crate) const CONSOLE_BACKUP_VERIFY_RUNNING: &str = "console.backup_verify_running";
pub(crate) const CONSOLE_BACKUP_VERIFIED: &str = "console.backup_verified";
pub(crate) const CONSOLE_BACKUP_VERIFY_WORKER_STOPPED: &str =
    "console.backup_verify_worker_stopped";
pub(crate) const CONSOLE_BACKUP_RESTORE_TITLE: &str = "console.backup_restore_title";
pub(crate) const CONSOLE_BACKUP_RESTORE_QUESTION: &str = "console.backup_restore_question";
pub(crate) const CONSOLE_BACKUP_RESTORE_PLAN: &str = "console.backup_restore_plan";
pub(crate) const CONSOLE_BACKUP_RESTORE_PRESS_ENTER: &str = "console.backup_restore_press_enter";
pub(crate) const CONSOLE_BACKUP_RESTORE_MINIMAL_SCOPE: &str =
    "console.backup_restore_minimal_scope";
pub(crate) const CONSOLE_BACKUP_CREATE_TITLE: &str = "console.backup_create_title";
pub(crate) const CONSOLE_BACKUP_CREATE_SCOPE: &str = "console.backup_create_scope";
pub(crate) const CONSOLE_BACKUP_CREATE_HINT: &str = "console.backup_create_hint";
pub(crate) const CONSOLE_BACKUP_DETAILS_RESTORE_HINT: &str = "console.backup_details_restore_hint";
pub(crate) const CONSOLE_BACKUP_HINT_LIST: &str = "console.backup_hint_list";
pub(crate) const CONSOLE_BACKUP_HINT_DETAILS: &str = "console.backup_hint_details";
pub(crate) const CONSOLE_BACKUP_HINT_RESTORE_CONFIRM: &str = "console.backup_hint_restore_confirm";
pub(crate) const CONSOLE_BACKUP_HINT_CREATE: &str = "console.backup_hint_create";
pub(crate) const CONSOLE_BACKUP_CREATED: &str = "console.backup_created";
pub(crate) const CONSOLE_BACKUP_CREATE_RUNNING: &str = "console.backup_create_running";
pub(crate) const CONSOLE_BACKUP_CREATE_PROGRESS_EXPORT: &str =
    "console.backup_create_progress_export";
pub(crate) const CONSOLE_BACKUP_CREATE_PROGRESS_ARCHIVE: &str =
    "console.backup_create_progress_archive";
pub(crate) const CONSOLE_BACKUP_CREATE_PROGRESS_FINALIZE: &str =
    "console.backup_create_progress_finalize";
pub(crate) const CONSOLE_BACKUP_CREATE_WORKER_STOPPED: &str =
    "console.backup_create_worker_stopped";
pub(crate) const CONSOLE_BACKUP_HINT_CREATE_RUNNING: &str = "console.backup_hint_create_running";
pub(crate) const CONSOLE_BACKUP_DELETE_TITLE: &str = "console.backup_delete_title";
pub(crate) const CONSOLE_BACKUP_DELETE_QUESTION: &str = "console.backup_delete_question";
pub(crate) const CONSOLE_BACKUP_DELETE_PLAN: &str = "console.backup_delete_plan";
pub(crate) const CONSOLE_BACKUP_DELETE_PRESS_ENTER: &str = "console.backup_delete_press_enter";
pub(crate) const CONSOLE_BACKUP_DELETED: &str = "console.backup_deleted";
pub(crate) const CONSOLE_BACKUP_HINT_DELETE_CONFIRM: &str = "console.backup_hint_delete_confirm";
pub(crate) const CONSOLE_BACKUP_SELECT_TO_DELETE: &str = "console.backup_select_to_delete";
