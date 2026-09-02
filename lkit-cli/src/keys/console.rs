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
pub(crate) const CONSOLE_VERSION_HELP: &str = "console.version_help";
pub(crate) const CONSOLE_REPOSITORY_HELP: &str = "console.repository_help";
pub(crate) const CONSOLE_REPOSITORY_URL_HELP: &str = "console.repository_url_help";
pub(crate) const CONSOLE_INSTALL_ROOT_HELP: &str = "console.install_root_help";
pub(crate) const CONSOLE_ADMIN_USER_HELP: &str = "console.admin_user_help";
pub(crate) const CONSOLE_PASSWORD_HELP: &str = "console.password_help";
pub(crate) const CONSOLE_CONFIRM_PASSWORD_HELP: &str = "console.confirm_password_help";
// TODO(network-takeover): 恢复网络接管开关时放开以下两个 key 及对应的 locale 条目。
// pub(crate) const CONSOLE_NETWORK_TAKEOVER_HELP: &str = "console.network_takeover_help";
pub(crate) const CONSOLE_START_INSTALLATION_HELP: &str = "console.start_installation_help";
pub(crate) const CONSOLE_FLARE_PSK_REQUIRED: &str = "console.flare_psk_required";
pub(crate) const CONSOLE_FLARE_PSK_TOO_SHORT: &str = "console.flare_psk_too_short";
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
pub(crate) const CONSOLE_DIALOG_ENTER_DEPLOY_ESC_CLOSE_R: &str =
    "console.dialog_enter_deploy_esc_close_r";
pub(crate) const CONSOLE_INSTALL_BLOCKED: &str = "console.install_blocked";
pub(crate) const CONSOLE_LANDSCAPE_NETWORK_TAKEOVER: &str = "console.landscape_network_takeover";
pub(crate) const CONSOLE_SELECT_WAN_INTERFACE: &str = "console.select_wan_interface";
pub(crate) const CONSOLE_NO_IPV4: &str = "console.no_ipv4";
pub(crate) const CONSOLE_GATEWAY_NOT_FOUND: &str = "console.gateway_not_found";
pub(crate) const CONSOLE_WAN_IPV4_MODE: &str = "console.wan_ipv4_mode";
pub(crate) const CONSOLE_TAB_STATIC: &str = "console.tab_static";
pub(crate) const CONSOLE_TAB_DHCP: &str = "console.tab_dhcp";
pub(crate) const CONSOLE_WAN_DHCP_CLIENT_HINT: &str = "console.wan_dhcp_client_hint";
pub(crate) const CONSOLE_IPV4_ADDRESS_CIDR: &str = "console.ipv4_address_cidr";
pub(crate) const CONSOLE_DEFAULT_GATEWAY: &str = "console.default_gateway";
pub(crate) const CONSOLE_SELECT_LAN_INTERFACES: &str = "console.select_lan_interfaces";
pub(crate) const CONSOLE_NO_OTHER_INTERFACES: &str = "console.no_other_interfaces";
pub(crate) const CONSOLE_LINK_UP: &str = "console.link_up";
pub(crate) const CONSOLE_LINK_DOWN: &str = "console.link_down";
pub(crate) const CONSOLE_LAN_MANAGEMENT_IPV4_ADDRESS: &str = "console.lan_management_ipv4_address";
pub(crate) const CONSOLE_LAN_DHCP_RANGE_START: &str = "console.lan_dhcp_range_start";
pub(crate) const CONSOLE_LAN_DHCP_RANGE_END: &str = "console.lan_dhcp_range_end";
pub(crate) const CONSOLE_LAN_DHCP_CONFIGURATION: &str = "console.lan_dhcp_configuration";
pub(crate) const CONSOLE_CONFIRM_AND_CONTINUE: &str = "console.confirm_and_continue";
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
pub(crate) const CONSOLE_WIZARD_HINT_CONFIG: &str = "console.wizard_hint_config";
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
pub(crate) const CONSOLE_LANGUAGE_SWITCH_HINT: &str = "console.language_switch_hint";
pub(crate) const CONSOLE_LANGUAGE_CURRENT: &str = "console.language_current";
#[cfg(not(test))]
pub(crate) const CONSOLE_LANGUAGE_SAVE_FAILED: &str = "console.language_save_failed";
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
pub(crate) const CONSOLE_DAEMON_NOT_RUNNING_NOTICE: &str = "console.daemon_not_running_notice";
pub(crate) const CONSOLE_DAEMON_SPAWN_UNAVAILABLE_NOTICE: &str =
    "console.daemon_spawn_unavailable_notice";
pub(crate) const CONSOLE_OVERVIEW_LKIT_DAEMON_RUNNING: &str =
    "console.overview_lkit_daemon_running";
pub(crate) const CONSOLE_OVERVIEW_LKIT_DAEMON_NOT_RUNNING: &str =
    "console.overview_lkit_daemon_not_running";
pub(crate) const CONSOLE_HEADER_DAEMON_RUNNING: &str = "console.header_daemon_running";
pub(crate) const CONSOLE_HEADER_DAEMON_NOT_RUNNING: &str = "console.header_daemon_not_running";
pub(crate) const CONSOLE_OVERVIEW_LKIT_SECTION: &str = "console.overview_lkit_section";
pub(crate) const CONSOLE_OVERVIEW_LKIT_VERSION: &str = "console.overview_lkit_version";
pub(crate) const CONSOLE_OVERVIEW_DEPLOY_DAEMON: &str = "console.overview_deploy_daemon";
pub(crate) const CONSOLE_OVERVIEW_SHOW_PSK: &str = "console.overview_show_psk";
pub(crate) const CONSOLE_OVERVIEW_HINT_DEPLOY: &str = "console.overview_hint_deploy";
pub(crate) const CONSOLE_DEPLOY_DAEMON_TITLE: &str = "console.deploy_daemon_title";
pub(crate) const CONSOLE_DEPLOY_DAEMON_QUESTION: &str = "console.deploy_daemon_question";
pub(crate) const CONSOLE_DEPLOY_FLARE_PURPOSE: &str = "console.deploy_flare_purpose";
pub(crate) const CONSOLE_DEPLOY_FLARE_EMPTY: &str = "console.deploy_flare_empty";
pub(crate) const CONSOLE_DEPLOY_FLARE_HINT: &str = "console.deploy_flare_hint";
pub(crate) const CONSOLE_DEPLOY_PSK_MISMATCH: &str = "console.deploy_psk_mismatch";
pub(crate) const CONSOLE_DEPLOY_DAEMON_START: &str = "console.deploy_daemon_start";
pub(crate) const CONSOLE_DEPLOY_DAEMON_PRESS_ESC: &str = "console.deploy_daemon_press_esc";
pub(crate) const CONSOLE_DEPLOY_DAEMON_RUNNING: &str = "console.deploy_daemon_running";
pub(crate) const CONSOLE_DEPLOY_DAEMON_HINT_CONFIRM: &str = "console.deploy_daemon_hint_confirm";
pub(crate) const CONSOLE_DEPLOY_DAEMON_WORKER_STOPPED: &str =
    "console.deploy_daemon_worker_stopped";
pub(crate) const CONSOLE_ABOUT_PREFIX: &str = "console.about_prefix";
pub(crate) const CONSOLE_ENVIRONMENT_CHECKS_HELP: &str = "console.environment_checks_help";
pub(crate) const CONSOLE_VERSION_LABEL: &str = "console.version_label";
pub(crate) const CONSOLE_REPOSITORY_LABEL: &str = "console.repository_label";
pub(crate) const CONSOLE_REPOSITORY_URL_LABEL: &str = "console.repository_url_label";
pub(crate) const CONSOLE_INSTALL_ROOT_LABEL: &str = "console.install_root_label";
pub(crate) const CONSOLE_ADMIN_USER_LABEL: &str = "console.admin_user_label";
pub(crate) const CONSOLE_PASSWORD_LABEL: &str = "console.password_label";
pub(crate) const CONSOLE_CONFIRM_PASSWORD_LABEL: &str = "console.confirm_password_label";
pub(crate) const CONSOLE_CONFIRM_PSK_LABEL: &str = "console.confirm_psk_label";
pub(crate) const CONSOLE_FLARE_PSK_LABEL: &str = "console.flare_psk_label";
pub(crate) const CONSOLE_SHOW_PSK_TITLE: &str = "console.show_psk_title";
pub(crate) const CONSOLE_SHOW_PSK_PURPOSE: &str = "console.show_psk_purpose";
pub(crate) const CONSOLE_SHOW_PSK_EMPTY: &str = "console.show_psk_empty";
pub(crate) const CONSOLE_SHOW_PSK_SAVE: &str = "console.show_psk_save";
pub(crate) const CONSOLE_SHOW_PSK_HINT: &str = "console.show_psk_hint";
pub(crate) const CONSOLE_FLARE_DIALOG_TITLE: &str = "console.flare_dialog_title";
pub(crate) const CONSOLE_FLARE_DIALOG_PURPOSE: &str = "console.flare_dialog_purpose";
pub(crate) const CONSOLE_FLARE_DIALOG_HINT: &str = "console.flare_dialog_hint";
pub(crate) const CONSOLE_FLARE_DEVICES_LABEL: &str = "console.flare_devices_label";
pub(crate) const CONSOLE_FLARE_ETHERTYPE_LABEL: &str = "console.flare_ethertype_label";
pub(crate) const CONSOLE_FLARE_FORWARD_PORTS_LABEL: &str = "console.flare_forward_ports_label";
pub(crate) const CONSOLE_FLARE_TOKEN_LABEL: &str = "console.flare_token_label";
pub(crate) const CONSOLE_FLARE_TOKEN_UNSET: &str = "console.flare_token_unset";
pub(crate) const CONSOLE_FLARE_SAVED: &str = "console.flare_saved";
pub(crate) const CONSOLE_FLARE_SAVE_FAILED: &str = "console.flare_save_failed";
pub(crate) const CONSOLE_OVERVIEW_HINT_FLARE: &str = "console.overview_hint_flare";
// TODO(network-takeover): 恢复网络接管开关时放开。
// pub(crate) const CONSOLE_NETWORK_TAKEOVER_LABEL: &str = "console.network_takeover_label";
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
pub(crate) const CONSOLE_BACKUP_CORRUPT_DIALOG: &str = "console.backup_corrupt_dialog";
pub(crate) const CONSOLE_BACKUP_CORRUPT_TITLE: &str = "console.backup_corrupt_title";
pub(crate) const CONSOLE_BACKUP_CORRUPT_QUESTION: &str = "console.backup_corrupt_question";
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
pub(crate) const CONSOLE_UPDATE_MENU: &str = "console.update_menu";
pub(crate) const CONSOLE_UPDATE_UNAVAILABLE: &str = "console.update_unavailable";
pub(crate) const CONSOLE_UPDATE_CURRENT_VERSION_LABEL: &str =
    "console.update_current_version_label";
pub(crate) const CONSOLE_UPDATE_BUTTON: &str = "console.update_button";
pub(crate) const CONSOLE_UPDATE_RESOLVING: &str = "console.update_resolving";
pub(crate) const CONSOLE_UPDATE_RESOLVE_WORKER_STOPPED: &str =
    "console.update_resolve_worker_stopped";
pub(crate) const CONSOLE_UPDATE_REPOSITORY_UNAVAILABLE: &str =
    "console.update_repository_unavailable";
pub(crate) const CONSOLE_UPDATE_CONFIRM_TITLE: &str = "console.update_confirm_title";
pub(crate) const CONSOLE_UPDATE_CONFIRM_QUESTION: &str = "console.update_confirm_question";
pub(crate) const CONSOLE_UPDATE_CONFIRM_PLAN: &str = "console.update_confirm_plan";
pub(crate) const CONSOLE_UPDATE_CONFIRM_NOTE: &str = "console.update_confirm_note";
pub(crate) const CONSOLE_UPDATE_CONFIRM_PRESS_ENTER: &str = "console.update_confirm_press_enter";
pub(crate) const CONSOLE_UPDATE_HINT_PANEL: &str = "console.update_hint_panel";
pub(crate) const CONSOLE_UPDATE_HINT_CONFIRM: &str = "console.update_hint_confirm";
pub(crate) const CONSOLE_UPDATE_HINT_RESOLVING: &str = "console.update_hint_resolving";
pub(crate) const CONSOLE_UNINSTALL_MENU: &str = "console.uninstall_menu";
pub(crate) const CONSOLE_UNINSTALL_UNAVAILABLE: &str = "console.uninstall_unavailable";
pub(crate) const CONSOLE_UNINSTALL_ACTION: &str = "console.uninstall_action";
pub(crate) const CONSOLE_UNINSTALL_VERSION_LABEL: &str = "console.uninstall_version_label";
pub(crate) const CONSOLE_UNINSTALL_SERVICE_LABEL: &str = "console.uninstall_service_label";
pub(crate) const CONSOLE_UNINSTALL_DATA_LOSS: &str = "console.uninstall_data_loss";
pub(crate) const CONSOLE_UNINSTALL_RETAINED: &str = "console.uninstall_retained";
pub(crate) const CONSOLE_UNINSTALL_HOST_NETWORK_WARNING: &str =
    "console.uninstall_host_network_warning";
pub(crate) const CONSOLE_UNINSTALL_CONFIRM_TITLE: &str = "console.uninstall_confirm_title";
pub(crate) const CONSOLE_UNINSTALL_CONFIRM_QUESTION: &str = "console.uninstall_confirm_question";
pub(crate) const CONSOLE_UNINSTALL_CONFIRM_PLAN: &str = "console.uninstall_confirm_plan";
pub(crate) const CONSOLE_UNINSTALL_CONFIRM_PRESS_ENTER: &str =
    "console.uninstall_confirm_press_enter";
pub(crate) const CONSOLE_UNINSTALL_HINT_PANEL: &str = "console.uninstall_hint_panel";
pub(crate) const CONSOLE_UNINSTALL_HINT_CONFIRM: &str = "console.uninstall_hint_confirm";
pub(crate) const CONSOLE_REINIT_MENU: &str = "console.reinit_menu";
pub(crate) const CONSOLE_MIRROR_MENU: &str = "console.mirror_menu";
pub(crate) const CONSOLE_MIRROR_DETECTING: &str = "console.mirror_detecting";
pub(crate) const CONSOLE_MIRROR_DETECT_FAILED: &str = "console.mirror_detect_failed";
pub(crate) const CONSOLE_MIRROR_HOST: &str = "console.mirror_host";
pub(crate) const CONSOLE_MIRROR_RESTORE_ROW: &str = "console.mirror_restore_row";
pub(crate) const CONSOLE_MIRROR_SECURITY_ROW: &str = "console.mirror_security_row";
pub(crate) const CONSOLE_MIRROR_CDROM_ROW: &str = "console.mirror_cdrom_row";
pub(crate) const CONSOLE_MIRROR_CONFIRM_APPLY_TITLE: &str = "console.mirror_confirm_apply_title";
pub(crate) const CONSOLE_MIRROR_CONFIRM_APPLY: &str = "console.mirror_confirm_apply";
pub(crate) const CONSOLE_MIRROR_CONFIRM_RESTORE_TITLE: &str =
    "console.mirror_confirm_restore_title";
pub(crate) const CONSOLE_MIRROR_CONFIRM_RESTORE: &str = "console.mirror_confirm_restore";
pub(crate) const CONSOLE_MIRROR_CONFIRM_ENTER: &str = "console.mirror_confirm_enter";
pub(crate) const CONSOLE_MIRROR_CONFIRM_ESC: &str = "console.mirror_confirm_esc";
pub(crate) const CONSOLE_MIRROR_HINT_PANEL: &str = "console.mirror_hint_panel";
pub(crate) const CONSOLE_MIRROR_HINT_CONFIRM: &str = "console.mirror_hint_confirm";
pub(crate) const CONSOLE_MIRROR_PROBING: &str = "console.mirror_probing";
pub(crate) const CONSOLE_MIRROR_CONFIRM_UNKNOWN_WARNING: &str =
    "console.mirror_confirm_unknown_warning";
pub(crate) const CONSOLE_SOFTWARE_MENU: &str = "console.software_menu";
pub(crate) const CONSOLE_SOFTWARE_DETECTING: &str = "console.software_detecting";
pub(crate) const CONSOLE_SOFTWARE_DETECT_FAILED: &str = "console.software_detect_failed";
pub(crate) const CONSOLE_SOFTWARE_HOST: &str = "console.software_host";
pub(crate) const CONSOLE_SOFTWARE_SOURCE_ROW: &str = "console.software_source_row";
pub(crate) const CONSOLE_SOFTWARE_CONFIRM_TITLE: &str = "console.software_confirm_title";
pub(crate) const CONSOLE_SOFTWARE_CONFIRM_QUESTION: &str = "console.software_confirm_question";
pub(crate) const CONSOLE_SOFTWARE_CONFIRM_ENTER: &str = "console.software_confirm_enter";
pub(crate) const CONSOLE_SOFTWARE_CONFIRM_SWITCH: &str = "console.software_confirm_switch";
pub(crate) const CONSOLE_SOFTWARE_CONFIRM_ESC: &str = "console.software_confirm_esc";
pub(crate) const CONSOLE_SOFTWARE_INSTALLING: &str = "console.software_installing";
pub(crate) const CONSOLE_SOFTWARE_PHASE_PREPARING: &str = "console.software_phase_preparing";
pub(crate) const CONSOLE_SOFTWARE_PHASE_PACKAGES: &str = "console.software_phase_packages";
pub(crate) const CONSOLE_SOFTWARE_PHASE_SERVICE: &str = "console.software_phase_service";
pub(crate) const CONSOLE_SOFTWARE_INSTALLED: &str = "console.software_installed";
pub(crate) const CONSOLE_SOFTWARE_WORKER_STOPPED: &str = "console.software_worker_stopped";
pub(crate) const CONSOLE_SOFTWARE_CANCEL_HINT: &str = "console.software_cancel_hint";
pub(crate) const CONSOLE_SOFTWARE_CANCEL_TITLE: &str = "console.software_cancel_title";
pub(crate) const CONSOLE_SOFTWARE_CANCEL_QUESTION: &str = "console.software_cancel_question";
pub(crate) const CONSOLE_SOFTWARE_CANCEL_NOTE: &str = "console.software_cancel_note";
pub(crate) const CONSOLE_SOFTWARE_CANCEL_PRESS_ENTER: &str = "console.software_cancel_press_enter";
pub(crate) const CONSOLE_SOFTWARE_HINT_PANEL: &str = "console.software_hint_panel";
pub(crate) const CONSOLE_SOFTWARE_HINT_CONFIRM: &str = "console.software_hint_confirm";
pub(crate) const CONSOLE_SOFTWARE_HINT_RUNNING: &str = "console.software_hint_running";
pub(crate) const CONSOLE_BASE_PACKAGES_ROW: &str = "console.base_packages_row";
pub(crate) const CONSOLE_BASE_PACKAGES_MISSING: &str = "console.base_packages_missing";
pub(crate) const CONSOLE_BASE_PACKAGES_DIALOG_TITLE: &str = "console.base_packages_dialog_title";
pub(crate) const CONSOLE_BASE_PACKAGES_CONFIRM: &str = "console.base_packages_confirm";
pub(crate) const CONSOLE_BASE_PACKAGES_HINTS: &str = "console.base_packages_hints";
pub(crate) const CONSOLE_BASE_PACKAGES_HINT_DIALOG: &str = "console.base_packages_hint_dialog";
pub(crate) const CONSOLE_BASE_PACKAGES_ALREADY_INSTALLED: &str =
    "console.base_packages_already_installed";
pub(crate) const CONSOLE_BASE_PACKAGES_INSTALLING: &str = "console.base_packages_installing";
pub(crate) const CONSOLE_BASE_PACKAGES_INSTALLED_OK: &str = "console.base_packages_installed_ok";
pub(crate) const CONSOLE_BASE_PACKAGES_NONE: &str = "console.base_packages_none";
pub(crate) const CONSOLE_BASE_PACKAGES_WORKER_STOPPED: &str =
    "console.base_packages_worker_stopped";
pub(crate) const CONSOLE_BASE_PACKAGES_CANCEL_HINT: &str = "console.base_packages_cancel_hint";
pub(crate) const CONSOLE_BASE_PACKAGES_CANCEL_QUESTION: &str =
    "console.base_packages_cancel_question";
pub(crate) const CONSOLE_BASE_PACKAGES_CANCEL_NOTE: &str = "console.base_packages_cancel_note";
pub(crate) const CONSOLE_BASE_PACKAGES_CANCEL_PRESS_ENTER: &str =
    "console.base_packages_cancel_press_enter";
pub(crate) const CONSOLE_BASE_PACKAGES_CANCEL_TITLE: &str = "console.base_packages_cancel_title";
pub(crate) const CONSOLE_REINIT_VERSION_LABEL: &str = "console.reinit_version_label";
pub(crate) const CONSOLE_REINIT_UNAVAILABLE: &str = "console.reinit_unavailable";
pub(crate) const CONSOLE_REINIT_UNAVAILABLE_HINT: &str = "console.reinit_unavailable_hint";
pub(crate) const CONSOLE_REINIT_SUMMARY: &str = "console.reinit_summary";
pub(crate) const CONSOLE_REINIT_WIPE_SCOPE: &str = "console.reinit_wipe_scope";
pub(crate) const CONSOLE_REINIT_BEGIN: &str = "console.reinit_begin";
pub(crate) const CONSOLE_REINIT_ENTER_CREDENTIALS: &str = "console.reinit_enter_credentials";
pub(crate) const CONSOLE_REINIT_PLAN_SUMMARY: &str = "console.reinit_plan_summary";
pub(crate) const CONSOLE_REINIT_LAN_NONE: &str = "console.reinit_lan_none";
pub(crate) const CONSOLE_REINIT_EXECUTE: &str = "console.reinit_execute";
pub(crate) const CONSOLE_REINIT_PLAN_MISSING: &str = "console.reinit_plan_missing";
pub(crate) const CONSOLE_REINIT_CONFIRM_TITLE: &str = "console.reinit_confirm_title";
pub(crate) const CONSOLE_REINIT_CONFIRM_WIPE: &str = "console.reinit_confirm_wipe";
pub(crate) const CONSOLE_REINIT_CONFIRM_BACKUP: &str = "console.reinit_confirm_backup";
pub(crate) const CONSOLE_REINIT_CONFIRM_WINDOW: &str = "console.reinit_confirm_window";
pub(crate) const CONSOLE_REINIT_CONFIRM_PROMPT: &str = "console.reinit_confirm_prompt";
pub(crate) const CONSOLE_REINIT_HINT_PANEL: &str = "console.reinit_hint_panel";
pub(crate) const CONSOLE_REINIT_HINT_CONFIRM: &str = "console.reinit_hint_confirm";
pub(crate) const CONSOLE_TAKEOVER_PENDING_BADGE: &str = "console.takeover_pending_badge";
pub(crate) const CONSOLE_TAKEOVER_PENDING_WINDOW: &str = "console.takeover_pending_window";
pub(crate) const CONSOLE_TAKEOVER_PENDING_TITLE: &str = "console.takeover_pending_title";
pub(crate) const CONSOLE_TAKEOVER_PENDING_TRANSACTION: &str =
    "console.takeover_pending_transaction";
pub(crate) const CONSOLE_TAKEOVER_PENDING_PHASE: &str = "console.takeover_pending_phase";
pub(crate) const CONSOLE_TAKEOVER_PENDING_ADDRESS: &str = "console.takeover_pending_address";
pub(crate) const CONSOLE_TAKEOVER_PENDING_DEADLINE: &str = "console.takeover_pending_deadline";
pub(crate) const CONSOLE_TAKEOVER_PENDING_NOW: &str = "console.takeover_pending_now";
pub(crate) const CONSOLE_TAKEOVER_PENDING_COUNTDOWN: &str = "console.takeover_pending_countdown";
pub(crate) const CONSOLE_TAKEOVER_PENDING_HINT: &str = "console.takeover_pending_hint";
pub(crate) const CONSOLE_TAKEOVER_PENDING_LATER: &str = "console.takeover_pending_later";
pub(crate) const CONSOLE_TAKEOVER_PENDING_CONFIRM: &str = "console.takeover_pending_confirm";
pub(crate) const CONSOLE_TAKEOVER_PENDING_ROLLING_BACK: &str =
    "console.takeover_pending_rolling_back";
pub(crate) const CONSOLE_TAKEOVER_PENDING_KEY_HINT: &str = "console.takeover_pending_key_hint";
