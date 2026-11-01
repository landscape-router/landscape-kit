//! Pure OpenRC renderers, also consumed by the Alpine VM fixture generator.

use std::path::Path;

pub(crate) const ROUTER_SCRIPT_NAME: &str = "landscape-router.service";
pub(crate) const LKIT_DAEMON_SCRIPT_NAME: &str = "lkit.service";

pub(crate) fn render_router_script(canonical_root: &Path) -> String {
    format!(
        r#"#!/sbin/openrc-run

name="Landscape Router"
description="Landscape webserver reverse proxy"

depend() {{
    use {LKIT_DAEMON_SCRIPT_NAME}
    after {LKIT_DAEMON_SCRIPT_NAME}
}}

command="{0}/current/landscape-webserver"
command_args="--config-dir {0}/data --web {0}/current/static"
command_user="root"
pidfile="/run/landscape-router.pid"

start() {{
    ebegin "Starting Landscape"
    start-stop-daemon --start --make-pidfile --pidfile /run/landscape-router.pid --background --exec ${{command}} -- ${{command_args}}
    eend $?
}}

stop() {{
    ebegin "Stopping Landscape"
    start-stop-daemon --stop --pidfile /run/landscape-router.pid
    eend $?
}}
"#,
        shell_quote(&canonical_root.display().to_string())
    )
}

pub(crate) fn render_lkit_script(binary: &Path) -> String {
    format!(
        r#"#!/sbin/openrc-run

name="lkit"
description="lkit resident daemon"

command="{}"
command_args="daemon"
command_user="root"
pidfile="/run/lkit.pid"

start() {{
    ebegin "Starting lkit daemon"
    (umask 077; touch /var/log/lkit.log) || return 1
    chmod 0600 /var/log/lkit.log || return 1
    start-stop-daemon --start --make-pidfile --pidfile /run/lkit.pid --background --stdout /var/log/lkit.log --stderr /var/log/lkit.log --exec ${{command}} -- ${{command_args}}
    eend $?
}}

stop() {{
    ebegin "Stopping lkit daemon"
    start-stop-daemon --stop --pidfile /run/lkit.pid --retry TERM/10/KILL/5
    eend $?
}}
"#,
        shell_quote(&binary.display().to_string())
    )
}

pub(crate) fn shell_quote(value: &str) -> String {
    if value.contains([' ', '"', '\'']) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}
