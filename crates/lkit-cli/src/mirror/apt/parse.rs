//! apt 软件源文件的条目级解析与重写。
//!
//! 支持两种格式：
//! - one-line（`sources.list` 传统格式）：`deb [options] uri suite components...`
//! - deb822（`*.sources`）：`Types:`/`URIs:`/`Suites:`/`Components:` 字段的 stanza。
//!
//! 条目保留原始文本；重写只替换 URI 片段（span 级拼接），其余字节逐字不动，
//! 因此不识别/不命中的行与注释保持零 diff。

use super::super::common;
use super::super::{Family, MirrorName, RECOGNIZED_MIRROR_HOSTS, mirror_host};

/// 一个解析出的 apt 源条目。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AptEntry {
    /// 条目的原始文本（含行终止符；deb822 为整个 stanza）。
    pub(crate) raw: String,
    /// 条目是否启用（未被 `#` 注释）。
    pub(crate) enabled: bool,
    /// `deb` / `deb-src`（one-line 单个；deb822 的 Types 可多个）。
    pub(crate) deb_types: Vec<String>,
    /// URI 列表；`span` 是 URI 在条目原始文本中的字节区间。
    pub(crate) uris: Vec<Uri>,
    /// suites（one-line 为 URI 后第一个 token；deb822 为 Suites 字段）。
    pub(crate) suites: Vec<String>,
    /// components（one-line 为 URI 后其余 token；deb822 为 Components 字段）。
    pub(crate) components: Vec<String>,
}

/// 条目中的单个 URI 与其在原始文本中的字节区间。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Uri {
    pub(crate) value: String,
    pub(crate) span: (usize, usize),
}

impl AptEntry {
    pub(crate) fn is_cdrom(&self) -> bool {
        self.uris.iter().any(|uri| uri.value.starts_with("cdrom:"))
    }

    /// 任一 URI（去掉 scheme 后）是否以 `host` 开头且后接 URL 边界。
    /// 匹配前会去掉 URI 的 userinfo 与显式端口（IPv6 字面量除外）。
    pub(crate) fn contains_host(&self, host: &str) -> bool {
        self.uris.iter().any(|uri| {
            uri.value.split_once("://").is_some_and(|(_, rest)| {
                let (uri_host, path) = split_authority(rest);
                let candidate = format!("{uri_host}{path}");
                host.len() <= candidate.len() && candidate.starts_with(host) && {
                    let after = &candidate[host.len()..];
                    after.is_empty() || common::is_boundary(after.chars().next().unwrap())
                }
            })
        })
    }
}

/// 解析一个 apt 源文件为条目列表。空文件/纯注释文件返回空列表（不报错）。
/// 按首个非注释行判定格式：`Types:`/`URIs:` 开头视为 deb822，否则按 one-line。
pub(crate) fn parse_sources(text: &str) -> Vec<AptEntry> {
    parse_sources_with_diagnostics(text).0
}

/// 格式检查发现的异常行。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ParseIssueKind {
    /// one-line：类型不是 deb/deb-src（可能是混入的 deb822 段落或其他内容）。
    NotADebLine,
    /// one-line：是 deb/deb-src 但解析不出 URI（缺失，或 `[options]` 括号不配对）。
    MissingUri,
    /// deb822：行不是 `字段: 值`（可能是混入的 one-line 行）。
    NotAField,
    /// deb822：stanza 有字段但缺 URIs。
    StanzaWithoutUris,
}

/// 一条格式诊断：1-based 行号 + 异常类型。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct ParseIssue {
    pub(crate) line: usize,
    pub(crate) kind: ParseIssueKind,
}

/// 解析源文件并返回条目与格式诊断。诊断只报告不修改：异常行原样保留，
/// 换源时也不改写它们。
pub(crate) fn parse_sources_with_diagnostics(text: &str) -> (Vec<AptEntry>, Vec<ParseIssue>) {
    let body = text
        .lines()
        .map(str::trim_start)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("");
    if body.starts_with("Types:") || body.starts_with("URIs:") {
        parse_deb822_with_diagnostics(text)
    } else {
        parse_one_line_with_diagnostics(text)
    }
}

// ---------------------------------------------------------------------------
// one-line 格式
// ---------------------------------------------------------------------------

fn parse_one_line_with_diagnostics(text: &str) -> (Vec<AptEntry>, Vec<ParseIssue>) {
    let mut entries = Vec::new();
    let mut issues = Vec::new();
    let mut offset = 0usize;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let (mut entry, issue) = parse_one_line_entry(line);
        for uri in &mut entry.uris {
            uri.span = (uri.span.0 + offset, uri.span.1 + offset);
        }
        if let Some(kind) = issue {
            issues.push(ParseIssue {
                line: index + 1,
                kind,
            });
        }
        entries.push(entry);
        offset += line.len();
    }
    (entries, issues)
}

fn parse_one_line_entry(line: &str) -> (AptEntry, Option<ParseIssueKind>) {
    let mut enabled = true;
    let mut pos = skip_ws(line, 0);
    while pos < line.len() && line.as_bytes()[pos] == b'#' {
        enabled = false;
        pos += 1;
        pos = skip_ws(line, pos);
    }
    // 类型 token
    let type_start = pos;
    while pos < line.len() && !line.as_bytes()[pos].is_ascii_whitespace() {
        pos += 1;
    }
    let deb_type = &line[type_start..pos];
    let mut entry = AptEntry {
        raw: line.to_string(),
        enabled,
        deb_types: Vec::new(),
        uris: Vec::new(),
        suites: Vec::new(),
        components: Vec::new(),
    };
    if deb_type != "deb" && deb_type != "deb-src" {
        // 只有未注释的内容行才报 NotADebLine；`# ...` 纯注释不算问题。
        let issue = (enabled && !deb_type.is_empty()).then_some(ParseIssueKind::NotADebLine);
        return (entry, issue);
    }
    entry.deb_types.push(deb_type.to_string());
    pos = skip_ws(line, pos);
    // options：`[ ... ]`（可嵌套，如 signed-by 路径）
    if pos < line.len() && line.as_bytes()[pos] == b'[' {
        let mut depth = 0usize;
        while pos < line.len() {
            match line.as_bytes()[pos] {
                b'[' => {
                    depth += 1;
                    pos += 1;
                }
                b']' => {
                    depth -= 1;
                    pos += 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => pos += 1,
            }
        }
        pos = skip_ws(line, pos);
    }
    // URI（cdrom 特殊：`cdrom:[label]/`，label 可含空格与括号）
    let Some((uri_start, uri_end)) = take_uri(line, pos) else {
        return (entry, Some(ParseIssueKind::MissingUri));
    };
    entry.uris.push(Uri {
        value: line[uri_start..uri_end].to_string(),
        span: (uri_start, uri_end),
    });
    // suites/components（信息用途，重写不依赖它们）。不符合规范的行（如重复粘贴
    // 的第二个 URL）中，URL 形状的 token 仍按 URI 解析并参与重写，避免换源后
    // 出现"一半镜像一半官方"的坏状态。
    let mut token = skip_ws(line, uri_end);
    if let Some((start, end)) = take_token(line, token) {
        let value = &line[start..end];
        if value.contains("://") {
            entry.uris.push(Uri {
                value: value.to_string(),
                span: (start, end),
            });
        } else {
            entry.suites.push(value.to_string());
        }
        token = end;
    }
    loop {
        token = skip_ws(line, token);
        let Some((start, end)) = take_token(line, token) else {
            break;
        };
        let value = &line[start..end];
        if value.contains("://") {
            entry.uris.push(Uri {
                value: value.to_string(),
                span: (start, end),
            });
        } else {
            entry.components.push(value.to_string());
        }
        token = end;
    }
    (entry, None)
}

// ---------------------------------------------------------------------------
// deb822 格式
// ---------------------------------------------------------------------------

fn parse_deb822_with_diagnostics(text: &str) -> (Vec<AptEntry>, Vec<ParseIssue>) {
    let mut entries = Vec::new();
    let mut issues = Vec::new();
    let mut start = 0usize;
    let mut cur = 0usize;
    let mut stanza_start_line = 1usize;
    for (line_index, line) in text.split_inclusive('\n').enumerate() {
        if line.trim().is_empty() {
            if start < cur {
                let (entry, kinds) = parse_deb822_stanza(&text[start..cur], start);
                entries.push(entry);
                issues.extend(kinds.into_iter().map(|(offset, kind)| ParseIssue {
                    line: stanza_start_line + offset,
                    kind,
                }));
            }
            start = cur + line.len();
            stanza_start_line = line_index + 2;
        }
        cur += line.len();
    }
    if start < text.len() {
        let (entry, kinds) = parse_deb822_stanza(&text[start..], start);
        entries.push(entry);
        issues.extend(kinds.into_iter().map(|(offset, kind)| ParseIssue {
            line: stanza_start_line + offset,
            kind,
        }));
    }
    (entries, issues)
}

/// 解析一个 stanza。`base` 是 stanza 在文件中的字节偏移；问题返回 (stanza 内
/// 0-based 行号, 类型)，由调用方换算成文件行号。
fn parse_deb822_stanza(stanza: &str, base: usize) -> (AptEntry, Vec<(usize, ParseIssueKind)>) {
    let mut entry = AptEntry {
        raw: stanza.to_string(),
        enabled: true,
        deb_types: Vec::new(),
        uris: Vec::new(),
        suites: Vec::new(),
        components: Vec::new(),
    };
    let mut issues = Vec::new();
    let mut has_field = false;
    let mut has_uris = false;
    let mut first_field_line = None;
    let mut line_offset = 0usize;
    let mut line_no = 0usize;
    for line in stanza.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            line_offset += line.len();
            line_no += 1;
            continue;
        }
        if let Some(colon_rel) = line.find(':') {
            let key = line[..colon_rel].trim();
            // 字段名必须是单个词；URL 冒号（如 one-line 行 `deb http://...`）
            // 会让 key 带空白，判定为非字段行。
            if key.is_empty() || key.contains(char::is_whitespace) {
                issues.push((line_no, ParseIssueKind::NotAField));
            } else {
                has_field = true;
                first_field_line.get_or_insert(line_no);
                match key {
                    "Types" => {
                        entry.deb_types = collect_tokens(line, colon_rel + 1);
                    }
                    "URIs" => {
                        has_uris = true;
                        let mut token = colon_rel + 1;
                        loop {
                            token = skip_ws(line, token);
                            let Some((start, end)) = take_token(line, token) else {
                                break;
                            };
                            entry.uris.push(Uri {
                                value: line[start..end].to_string(),
                                span: (base + line_offset + start, base + line_offset + end),
                            });
                            token = end;
                        }
                    }
                    "Suites" => {
                        entry.suites = collect_tokens(line, colon_rel + 1);
                    }
                    "Components" => {
                        entry.components = collect_tokens(line, colon_rel + 1);
                    }
                    _ => {}
                }
            }
        } else {
            issues.push((line_no, ParseIssueKind::NotAField));
        }
        line_offset += line.len();
        line_no += 1;
    }
    if has_field && !has_uris {
        issues.push((
            first_field_line.unwrap_or(0),
            ParseIssueKind::StanzaWithoutUris,
        ));
    }
    (entry, issues)
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 跳过 `line[pos..]` 开头的 ASCII 空白。
fn skip_ws(line: &str, pos: usize) -> usize {
    let mut pos = pos;
    while pos < line.len() && line.as_bytes()[pos].is_ascii_whitespace() {
        pos += 1;
    }
    pos
}

/// 取 `line[pos..]` 起的一个空白分隔 token（遇到 `#` 视为行内注释开头）。
fn take_token(line: &str, pos: usize) -> Option<(usize, usize)> {
    if pos >= line.len() || line.as_bytes()[pos].is_ascii_whitespace() {
        return None;
    }
    let start = pos;
    let mut end = pos;
    while end < line.len()
        && !line.as_bytes()[end].is_ascii_whitespace()
        && line.as_bytes()[end] != b'#'
    {
        end += 1;
    }
    // 行内注释开头的 `#` 不算 token（返回 None），避免产生空 token/空 URI。
    (end > start).then_some((start, end))
}

/// 取 `line[pos..]` 起的 URI：cdrom 特殊处理，普通 URL 按空白分隔。
fn take_uri(line: &str, pos: usize) -> Option<(usize, usize)> {
    let rest = &line[pos..];
    if rest.starts_with("cdrom:") {
        // cdrom:[label]/ —— label 可含空格与方括号，匹配到 `]`（可含 `/`）。
        let mut depth = 0usize;
        let mut scan = 6usize;
        while scan < rest.len() {
            match rest.as_bytes()[scan] {
                b'[' => depth += 1,
                b']' if depth > 0 => {
                    depth -= 1;
                    if depth == 0 {
                        let end = if rest.as_bytes().get(scan + 1) == Some(&b'/') {
                            scan + 2
                        } else {
                            scan + 1
                        };
                        return Some((pos, pos + end));
                    }
                }
                _ => {}
            }
            scan += 1;
        }
        return None;
    }
    take_token(line, pos)
}

/// 取 `line[from..]` 的全部空白分隔 token。
fn collect_tokens(line: &str, from: usize) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut pos = from;
    loop {
        pos = skip_ws(line, pos);
        let Some((start, end)) = take_token(line, pos) else {
            break;
        };
        tokens.push(line[start..end].to_string());
        pos = end;
    }
    tokens
}

/// 把不重叠的编辑（升序）拼回原文本。
fn splice(text: &str, edits: &[(usize, usize, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pos = 0usize;
    for (start, end, replacement) in edits {
        out.push_str(&text[pos..*start]);
        out.push_str(replacement);
        pos = *end;
    }
    out.push_str(&text[pos..]);
    out
}

// ---------------------------------------------------------------------------
// URL 映射与重写
// ---------------------------------------------------------------------------

/// 运行时机器架构（Linux `uname` 的 machine），失败时回退到编译期架构。
pub(crate) fn runtime_arch() -> String {
    let mut uts: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut uts) } == 0 {
        let bytes: Vec<u8> = uts
            .machine
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        std::env::consts::ARCH.to_string()
    }
}

/// 是否属于 Ubuntu 官方 archive（`archive.ubuntu.com`）之外的架构。
/// arm64/armhf、riscv64、ppc64el、s390x 的内容在 `ports.ubuntu.com`（路径
/// `/ubuntu-ports`），合成/转换条目时必须选对仓库，否则 apt 找不到包。
fn is_ports_arch(arch: &str) -> bool {
    arch.starts_with("arm")
        || arch.starts_with("aarch64")
        || arch.starts_with("riscv")
        || arch.starts_with("ppc")
        || arch.starts_with("s390")
}

/// 各家族主仓库官方主机+路径到镜像路径的映射。
///
/// 官方与镜像主机可能互为前缀（如 `.../ubuntu` 是 `.../ubuntu-ports` 的前缀），
/// 因此列表顺序即替换顺序：具体路径（`-ports`）必须排在通用路径之前。
/// Ubuntu 的 security 内容与主仓库合并镜像，`security.ubuntu.com/ubuntu` 归入
/// 主仓库，始终替换。
fn apt_paths(family: Family) -> Vec<(&'static str, &'static str)> {
    match family {
        Family::Debian => vec![
            ("deb.debian.org/debian-backports", "/debian-backports"),
            ("deb.debian.org/debian-ports", "/debian-ports"),
            ("deb.debian.org/debian", "/debian"),
        ],
        Family::Ubuntu => vec![
            ("ports.ubuntu.com/ubuntu-ports", "/ubuntu-ports"),
            ("archive.ubuntu.com/ubuntu-ports", "/ubuntu-ports"),
            ("archive.ubuntu.com/ubuntu", "/ubuntu"),
            ("security.ubuntu.com/ubuntu", "/ubuntu"),
        ],
        _ => Vec::new(),
    }
}

/// Debian 独立的 security 仓库。默认不替换（安全补丁时效性、部分镜像站不镜像
/// security），仅在显式要求时替换。Ubuntu 没有独立 security 路径。
fn apt_security_paths(family: Family) -> Vec<(&'static str, &'static str)> {
    match family {
        Family::Debian => vec![
            ("deb.debian.org/debian-security", "/debian-security"),
            ("security.debian.org/debian-security", "/debian-security"),
        ],
        _ => Vec::new(),
    }
}

/// 生成替换对：
///
/// - 官方主机 URL → 目标镜像（`deb.debian.org/debian` → `mirrors.tuna.../debian`）；
/// - 其他已识别镜像（`RECOGNIZED_MIRROR_HOSTS`）的同路径 URL → 目标镜像，
///   实现 TUNA/阿里云/USTC 之间互转；
/// - `replace_security` 时额外包含 Debian security 仓库（官方与已识别镜像的
///   `debian-security` 路径）；
/// - `Official` 走 [`official_pairs`]，把所有已识别镜像（含 security）映射回官方主机。
fn replacement_pairs(
    family: Family,
    mirror: MirrorName,
    replace_security: bool,
) -> Vec<(String, String)> {
    let Some(target) = mirror_host(mirror) else {
        return official_pairs(family);
    };
    let security = if replace_security {
        apt_security_paths(family)
    } else {
        Vec::new()
    };
    // security 先于主仓库（`mirrors.x/debian` 是 `mirrors.x/debian-security` 的前缀）。
    let paths: Vec<(&str, &str)> = security
        .iter()
        .chain(apt_paths(family).iter())
        .map(|(from, path)| (*from, *path))
        .collect();
    let mut pairs: Vec<(String, String)> = paths
        .iter()
        .map(|(from, path)| ((*from).to_string(), format!("{target}{path}")))
        .collect();
    for other in RECOGNIZED_MIRROR_HOSTS {
        if other == target {
            continue;
        }
        pairs.extend(
            paths
                .iter()
                .map(|(_, path)| (format!("{other}{path}"), format!("{target}{path}"))),
        );
    }
    pairs
}

/// 官方源恢复：把所有已识别的镜像主机路径（含 security）映射回官方主机。
fn official_pairs(family: Family) -> Vec<(String, String)> {
    let paths = apt_security_paths(family)
        .into_iter()
        .chain(apt_paths(family))
        .collect::<Vec<_>>();
    let mut pairs = Vec::new();
    for mirror in RECOGNIZED_MIRROR_HOSTS {
        for (official, path) in &paths {
            pairs.push((format!("{mirror}{path}"), (*official).to_string()));
        }
    }
    pairs
}

/// 拆分 URI（去掉 scheme 后）的主机与路径，并对主机归一化：去掉 userinfo
/// （`user:pass@`）与显式端口（IPv6 字面量 `[::1]` 除外）。`path` 含前导 `/`
/// （无路径时为空串）。
fn split_authority(rest: &str) -> (&str, &str) {
    let (authority, path) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, ""),
    };
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if hostport.starts_with('[') {
        hostport
    } else {
        hostport.split_once(':').map_or(hostport, |(h, _)| h)
    };
    (host, path)
}

/// 用替换对改写单个 URI；未命中返回 `None`。
fn rewrite_uri(uri: &str, pairs: &[(String, String)]) -> Option<String> {
    let (scheme, rest) = uri.split_once("://")?;
    let (authority, path) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, ""),
    };
    let (userinfo, hostport) = authority
        .rsplit_once('@')
        .map_or((None, authority), |(u, h)| (Some(u), h));
    let host = if hostport.starts_with('[') {
        hostport
    } else {
        hostport.split_once(':').map_or(hostport, |(h, _)| h)
    };
    let candidate = format!("{host}{path}");
    pairs.iter().find_map(|(from, to)| {
        (from.len() <= candidate.len() && candidate.starts_with(from) && {
            let after = &candidate[from.len()..];
            after.is_empty() || common::is_boundary(after.chars().next().unwrap())
        })
        .then(|| {
            // 只替换主机部分：保留 userinfo 与路径其余部分，端口随主机一起丢弃。
            let prefix = userinfo.map_or(String::new(), |u| format!("{u}@"));
            let suffix = &path[from.len() - host.len()..];
            format!("{scheme}://{prefix}{to}{suffix}")
        })
    })
}

/// 对整个源文件做条目级 URL 重写。没有可替换内容时返回 `None`（文件不写）。
pub(crate) fn rewrite(
    content: &str,
    family: Family,
    mirror: MirrorName,
    replace_security: bool,
) -> Option<String> {
    let pairs = replacement_pairs(family, mirror, replace_security);
    let entries = parse_sources(content);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for entry in &entries {
        for uri in &entry.uris {
            if let Some(new_value) = rewrite_uri(&uri.value, &pairs) {
                edits.push((uri.span.0, uri.span.1, new_value));
            }
        }
    }
    if edits.is_empty() {
        return None;
    }
    Some(splice(content, &edits))
}

/// 内容是否已处于目标状态：目标镜像主机（或官方主机）的 URL 已存在。
pub(crate) fn already_target(content: &str, family: Family, mirror: MirrorName) -> bool {
    let Some(target) = mirror_host(mirror) else {
        // Official：内容中已有官方主机路径即视为已处于官方源。
        return apt_paths(family)
            .iter()
            .chain(apt_security_paths(family).iter())
            .any(|(from, _)| entry_contains_host(content, from));
    };
    apt_paths(family)
        .iter()
        .chain(apt_security_paths(family).iter())
        .any(|(_, path)| entry_contains_host(content, &format!("{target}{path}")))
}

fn entry_contains_host(content: &str, host: &str) -> bool {
    parse_sources(content)
        .iter()
        .any(|entry| entry.contains_host(host))
}

/// 把第一个启用的 cdrom 条目转换为目标镜像（保留其 suites/components）。
/// 返回重写后的整个文件文本；没有可转换条目时返回 `None`。
/// Ubuntu 在 arm64 等 ports 架构上转 `/ubuntu-ports`，否则转 `/ubuntu`。
pub(crate) fn convert_cdrom(content: &str, family: Family, mirror: MirrorName) -> Option<String> {
    convert_cdrom_with_arch(content, family, mirror, &runtime_arch())
}

/// 同 [`convert_cdrom`]，机器架构由调用方指定（测试注入）。
pub(crate) fn convert_cdrom_with_arch(
    content: &str,
    family: Family,
    mirror: MirrorName,
    arch: &str,
) -> Option<String> {
    let (host, path) = match family {
        Family::Debian => (mirror_host(mirror).unwrap_or("deb.debian.org"), "/debian"),
        Family::Ubuntu => {
            let ports = is_ports_arch(arch);
            (
                mirror_host(mirror).unwrap_or(if ports {
                    "ports.ubuntu.com"
                } else {
                    "archive.ubuntu.com"
                }),
                if ports { "/ubuntu-ports" } else { "/ubuntu" },
            )
        }
        _ => return None,
    };
    let mut edits = Vec::new();
    for entry in parse_sources(content) {
        if !entry.enabled || !entry.is_cdrom() {
            continue;
        }
        if let Some(uri) = entry.uris.first() {
            edits.push((uri.span.0, uri.span.1, format!("https://{host}{path}")));
        }
        break;
    }
    if edits.is_empty() {
        return None;
    }
    Some(splice(content, &edits))
}

/// 生成新源条目（兜底：没有 cdrom 条目可转换时追加）。
/// Ubuntu 在 arm64 等 ports 架构上使用 `/ubuntu-ports` 与 `ports.ubuntu.com`。
pub(crate) fn synth_lines(
    family: Family,
    mirror: MirrorName,
    replace_security: bool,
    codename: &str,
) -> String {
    synth_lines_with_arch(family, mirror, replace_security, codename, &runtime_arch())
}

/// 同 [`synth_lines`]，机器架构由调用方指定（测试注入）。
pub(crate) fn synth_lines_with_arch(
    family: Family,
    mirror: MirrorName,
    replace_security: bool,
    codename: &str,
    arch: &str,
) -> String {
    let target = mirror_host(mirror);
    let (main_host, main_path, components) = match family {
        Family::Debian => (
            target.unwrap_or("deb.debian.org"),
            "/debian",
            "main contrib non-free",
        ),
        Family::Ubuntu => {
            let ports = is_ports_arch(arch);
            (
                target.unwrap_or(if ports {
                    "ports.ubuntu.com"
                } else {
                    "archive.ubuntu.com"
                }),
                if ports { "/ubuntu-ports" } else { "/ubuntu" },
                "main universe restricted multiverse",
            )
        }
        _ => unreachable!("synth_lines 只用于 apt 家族"),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "# Added by lkit set-mirror\n\
         deb https://{main_host}{main_path} {codename} {components}\n"
    ));
    match family {
        Family::Debian => {
            let security_host = if replace_security {
                target.unwrap_or("deb.debian.org")
            } else {
                "deb.debian.org"
            };
            out.push_str(&format!(
                "deb https://{security_host}/debian-security {codename}-security main\n"
            ));
        }
        Family::Ubuntu => {
            let security_host = target.unwrap_or("security.ubuntu.com");
            out.push_str(&format!(
                "deb https://{security_host}{main_path} {codename}-security {components}\n"
            ));
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_line_entries_with_fields() {
        let content = concat!(
            "deb http://deb.debian.org/debian bookworm main contrib non-free\n",
            "deb-src [arch=amd64] http://deb.debian.org/debian bookworm main\n",
            "# deb http://security.debian.org/debian-security bookworm-security main\n",
            "\n",
        );
        let entries = parse_sources(content);
        assert_eq!(entries.len(), 4);
        let main = &entries[0];
        assert!(main.enabled);
        assert_eq!(main.deb_types, vec!["deb"]);
        assert_eq!(main.uris[0].value, "http://deb.debian.org/debian");
        assert_eq!(main.suites, vec!["bookworm"]);
        assert_eq!(main.components, vec!["main", "contrib", "non-free"]);
        let src = &entries[1];
        assert_eq!(src.deb_types, vec!["deb-src"]);
        assert!(src.enabled);
        assert_eq!(src.uris[0].value, "http://deb.debian.org/debian");
        let disabled = &entries[2];
        assert!(!disabled.enabled, "commented line must be disabled");
        assert_eq!(
            disabled.uris[0].value,
            "http://security.debian.org/debian-security"
        );
    }

    #[test]
    fn parses_cdrom_uri_with_spaces_in_label() {
        let content = "deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_ - Official amd64 DVD Binary-1 20240210-10:16]/ bookworm contrib main\n";
        let entries = parse_sources(content);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert!(entry.is_cdrom());
        assert_eq!(
            entry.uris[0].value,
            "cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_ - Official amd64 DVD Binary-1 20240210-10:16]/"
        );
        assert_eq!(entry.suites, vec!["bookworm"]);
        assert_eq!(entry.components, vec!["contrib", "main"]);
    }

    #[test]
    fn parses_deb822_stanzas() {
        let content = concat!(
            "# comment\n",
            "Types: deb deb-src\n",
            "URIs: http://archive.ubuntu.com/ubuntu http://security.ubuntu.com/ubuntu\n",
            "Suites: noble noble-updates\n",
            "Components: main universe\n",
            "\n",
            "Types: deb\n",
            "URIs: http://ports.ubuntu.com/ubuntu-ports\n",
            "Suites: noble\n",
            "Components: main\n",
        );
        let entries = parse_sources(content);
        assert_eq!(entries.len(), 2);
        let first = &entries[0];
        assert_eq!(first.deb_types, vec!["deb", "deb-src"]);
        assert_eq!(first.uris.len(), 2);
        assert_eq!(first.uris[0].value, "http://archive.ubuntu.com/ubuntu");
        assert_eq!(first.uris[1].value, "http://security.ubuntu.com/ubuntu");
        assert_eq!(first.suites, vec!["noble", "noble-updates"]);
        assert_eq!(first.components, vec!["main", "universe"]);
        let second = &entries[1];
        assert_eq!(second.uris[0].value, "http://ports.ubuntu.com/ubuntu-ports");
        assert_eq!(second.suites, vec!["noble"]);
    }

    #[test]
    fn inline_comments_produce_no_empty_tokens_in_one_line_entries() {
        let content = concat!(
            "deb http://deb.debian.org/debian bookworm main # trailing comment\n",
            "deb-src http://deb.debian.org/debian bookworm main # src note\n",
        );
        let entries = parse_sources(content);
        assert_eq!(entries.len(), 2);
        let main = &entries[0];
        assert_eq!(main.uris.len(), 1);
        assert_eq!(main.uris[0].value, "http://deb.debian.org/debian");
        assert_eq!(main.suites, vec!["bookworm"]);
        assert_eq!(main.components, vec!["main"]);
        assert!(
            !main.components.iter().any(String::is_empty),
            "no empty token may be collected: {:?}",
            main.components
        );
    }

    #[test]
    fn inline_comments_produce_no_empty_tokens_in_deb822_stanzas() {
        let content = concat!(
            "Types: deb\n",
            "URIs: http://deb.debian.org/debian # URI note\n",
            "Suites: bookworm # suite note\n",
            "Components: main # component note\n",
        );
        let entries = parse_sources(content);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.uris.len(), 1);
        assert_eq!(entry.uris[0].value, "http://deb.debian.org/debian");
        assert_eq!(entry.suites, vec!["bookworm"]);
        assert_eq!(entry.components, vec!["main"]);
    }

    #[test]
    fn deb_line_with_only_an_inline_comment_reports_missing_uri() {
        let (_, issues) = parse_sources_with_diagnostics("deb # nothing but a comment\n");
        assert_eq!(
            issues,
            vec![ParseIssue {
                line: 1,
                kind: ParseIssueKind::MissingUri,
            }],
            "a deb line whose URI slot starts with a comment has no URI"
        );
    }

    #[test]
    fn parse_roundtrip_keeps_bytes_when_nothing_matches() {
        let content = concat!(
            "# my custom sources\n",
            "deb [trusted=yes] http://repo.internal.example.com/debian bookworm main\n",
            "  indented comment\n",
            "# deb-src http://repo.internal.example.com/debian bookworm main\n",
        );
        // 无法识别的自定义主机 → rewrite 返回 None（文件不写）。
        let rewritten = rewrite(content, Family::Debian, MirrorName::Tuna, false);
        assert!(rewritten.is_none());
    }

    #[test]
    fn rewrites_debian_one_line_sources_to_tuna() {
        let content = concat!(
            "deb http://deb.debian.org/debian bookworm main contrib\n",
            "deb-src http://deb.debian.org/debian bookworm main\n",
            "deb http://security.debian.org/debian-security bookworm-security main\n",
        );
        let rewritten = rewrite(content, Family::Debian, MirrorName::Tuna, false).unwrap();
        assert!(rewritten.contains("http://mirrors.tuna.tsinghua.edu.cn/debian bookworm"));
        assert!(!rewritten.contains("deb.debian.org"));
        assert!(rewritten.contains("security.debian.org"));
        assert!(rewritten.contains("deb-src"));
        assert_eq!(rewritten.matches("http://").count(), 3);
        assert!(
            !rewritten.contains("https://"),
            "the original scheme must be preserved"
        );
    }

    #[test]
    fn debian_security_is_replaced_only_when_requested() {
        let content = concat!(
            "deb http://deb.debian.org/debian bookworm main\n",
            "deb http://deb.debian.org/debian-security bookworm-security main\n",
        );
        let kept = rewrite(content, Family::Debian, MirrorName::Aliyun, false).unwrap();
        assert!(kept.contains("http://mirrors.aliyun.com/debian bookworm"));
        assert!(
            kept.contains("http://deb.debian.org/debian-security"),
            "security must stay official by default"
        );
        let replaced = rewrite(content, Family::Debian, MirrorName::Aliyun, true).unwrap();
        assert!(replaced.contains("http://mirrors.aliyun.com/debian-security"));
        assert!(!replaced.contains("deb.debian.org"));
    }

    #[test]
    fn ubuntu_security_is_always_replaced() {
        let content = concat!(
            "deb http://archive.ubuntu.com/ubuntu noble main\n",
            "deb http://security.ubuntu.com/ubuntu noble-security main\n",
        );
        for replace_security in [false, true] {
            let rewritten =
                rewrite(content, Family::Ubuntu, MirrorName::Tuna, replace_security).unwrap();
            assert!(
                rewritten.contains("http://mirrors.tuna.tsinghua.edu.cn/ubuntu noble-security"),
                "ubuntu security merges into the main mirror path regardless of the flag"
            );
            assert!(!rewritten.contains("security.ubuntu.com"));
        }
    }

    #[test]
    fn rewrites_ubuntu_ports_before_archive() {
        let content = concat!(
            "deb http://archive.ubuntu.com/ubuntu noble main universe\n",
            "deb http://security.ubuntu.com/ubuntu noble-security main\n",
            "deb http://ports.ubuntu.com/ubuntu-ports noble main\n",
        );
        let rewritten = rewrite(content, Family::Ubuntu, MirrorName::Ustc, false).unwrap();
        assert!(rewritten.contains("http://mirrors.ustc.edu.cn/ubuntu noble"));
        assert!(rewritten.contains("http://mirrors.ustc.edu.cn/ubuntu-ports noble"));
        assert!(!rewritten.contains("archive.ubuntu.com"));
        assert!(!rewritten.contains("security.ubuntu.com"));
        assert!(!rewritten.contains("ports.ubuntu.com"));
    }

    #[test]
    fn rewrites_deb822_sources_files() {
        let content = concat!(
            "Types: deb\n",
            "URIs: http://deb.debian.org/debian\n",
            "Suites: bookworm\n",
            "Components: main\n",
            "\n",
            "Types: deb\n",
            "URIs: http://deb.debian.org/debian-security\n",
            "Suites: bookworm-security\n",
            "Components: main\n",
        );
        let rewritten = rewrite(content, Family::Debian, MirrorName::Aliyun, true).unwrap();
        assert!(rewritten.contains("URIs: http://mirrors.aliyun.com/debian\n"));
        assert!(rewritten.contains("URIs: http://mirrors.aliyun.com/debian-security\n"));
        assert!(rewritten.contains("Suites: bookworm-security\n"));
    }

    #[test]
    fn official_restores_original_hosts() {
        let content = concat!(
            "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main\n",
            "deb https://mirrors.aliyun.com/debian-security bookworm-security main\n",
            "deb https://mirrors.ustc.edu.cn/ubuntu noble main\n",
        );
        let debian = rewrite(content, Family::Debian, MirrorName::Official, true).unwrap();
        assert!(debian.contains("https://deb.debian.org/debian bookworm"));
        assert!(debian.contains("https://deb.debian.org/debian-security"));
        assert!(
            debian.contains("mirrors.ustc.edu.cn/ubuntu"),
            "ubuntu host is untouched by debian rules"
        );
        let ubuntu = rewrite(content, Family::Ubuntu, MirrorName::Official, true).unwrap();
        assert!(ubuntu.contains("https://archive.ubuntu.com/ubuntu noble"));
        assert!(
            ubuntu.contains("mirrors.tuna.tsinghua.edu.cn/debian"),
            "debian host is untouched by ubuntu rules"
        );
    }

    #[test]
    fn official_on_official_content_is_a_noop() {
        let content = "deb https://deb.debian.org/debian bookworm main\n";
        let rewritten = rewrite(content, Family::Debian, MirrorName::Official, true);
        assert!(rewritten.is_none());
    }

    #[test]
    fn mirror_on_mirror_content_is_a_noop() {
        let content = "deb http://mirrors.tuna.tsinghua.edu.cn/debian bookworm main\n";
        let rewritten = rewrite(content, Family::Debian, MirrorName::Tuna, false);
        assert!(rewritten.is_none());
    }

    #[test]
    fn switches_between_recognized_mirrors() {
        let content = concat!(
            "deb http://mirrors.ustc.edu.cn/debian bookworm main\n",
            "deb-src http://mirrors.ustc.edu.cn/debian bookworm main\n",
            "deb http://mirrors.ustc.edu.cn/debian-security bookworm-security main\n",
        );
        let rewritten = rewrite(content, Family::Debian, MirrorName::Tuna, false).unwrap();
        assert!(rewritten.contains("http://mirrors.tuna.tsinghua.edu.cn/debian bookworm"));
        assert!(
            rewritten.contains("http://mirrors.ustc.edu.cn/debian-security"),
            "security stays on the previous mirror by default"
        );
        let replaced = rewrite(content, Family::Debian, MirrorName::Tuna, true).unwrap();
        assert!(replaced.contains("http://mirrors.tuna.tsinghua.edu.cn/debian-security"));
        assert!(!replaced.contains("mirrors.ustc.edu.cn"));
    }

    #[test]
    fn switches_between_university_mirrors() {
        // 六所大学镜像都在已识别主机列表内，可互相转换。
        for (from, target) in [
            (MirrorName::Tuna, MirrorName::Nju),
            (MirrorName::Nju, MirrorName::Sjtu),
            (MirrorName::Sjtu, MirrorName::Zju),
            (MirrorName::Zju, MirrorName::Lzu),
            (MirrorName::Lzu, MirrorName::Bfsu),
            (MirrorName::Bfsu, MirrorName::Hust),
            (MirrorName::Hust, MirrorName::Tuna),
        ] {
            let from_host = mirror_host(from).unwrap();
            let target_host = mirror_host(target).unwrap();
            let content = format!("deb https://{from_host}/debian bookworm main\n");
            let rewritten = rewrite(&content, Family::Debian, target, false).unwrap();
            assert!(
                rewritten.contains(&format!("https://{target_host}/debian bookworm")),
                "{from:?} -> {target:?}: {rewritten}"
            );
            assert!(!rewritten.contains(from_host));
        }
    }

    #[test]
    fn switches_ports_before_main_path_between_mirrors() {
        let content = concat!(
            "deb http://mirrors.aliyun.com/ubuntu-ports noble main\n",
            "deb http://mirrors.aliyun.com/ubuntu noble main\n",
        );
        let rewritten = rewrite(content, Family::Ubuntu, MirrorName::Ustc, false).unwrap();
        assert!(rewritten.contains("http://mirrors.ustc.edu.cn/ubuntu-ports noble"));
        assert!(rewritten.contains("http://mirrors.ustc.edu.cn/ubuntu noble"));
        assert!(!rewritten.contains("mirrors.aliyun.com"));
    }

    #[test]
    fn keeps_custom_hosts_when_switching_between_mirrors() {
        let content = concat!(
            "deb http://mirrors.aliyun.com/debian bookworm main\n",
            "deb http://repo.internal.example.com/debian bookworm main\n",
        );
        let rewritten = rewrite(content, Family::Debian, MirrorName::Ustc, false).unwrap();
        assert!(rewritten.contains("http://mirrors.ustc.edu.cn/debian bookworm"));
        assert!(rewritten.contains("repo.internal.example.com/debian"));
    }

    #[test]
    fn does_not_replace_host_name_substrings() {
        let content = "deb https://www.deb.debian.org/debian bookworm main\n";
        let rewritten = rewrite(content, Family::Debian, MirrorName::Tuna, false);
        assert!(
            rewritten.is_none(),
            "www.deb.debian.org must not match deb.debian.org"
        );
    }

    #[test]
    fn replaces_multiple_occurrences_per_line() {
        // 不符合规范的一行多 URL：第二个 URL 落在组件位置，仍应一并重写。
        let content =
            "deb https://archive.ubuntu.com/ubuntu noble main https://archive.ubuntu.com/ubuntu\n";
        let rewritten = rewrite(content, Family::Ubuntu, MirrorName::Tuna, false).unwrap();
        assert_eq!(
            rewritten
                .matches("https://mirrors.tuna.tsinghua.edu.cn/ubuntu")
                .count(),
            2
        );
        assert_eq!(rewritten.matches("archive.ubuntu.com").count(), 0);
        assert_eq!(
            rewritten,
            "deb https://mirrors.tuna.tsinghua.edu.cn/ubuntu noble main https://mirrors.tuna.tsinghua.edu.cn/ubuntu\n",
            "只有 URL 片段变化，其余字节原样保留"
        );
    }

    #[test]
    fn replaces_multiple_uris_per_deb822_line() {
        let content = concat!(
            "Types: deb\n",
            "URIs: https://archive.ubuntu.com/ubuntu https://security.ubuntu.com/ubuntu\n",
            "Suites: noble noble-security\n",
            "Components: main\n",
        );
        let rewritten = rewrite(content, Family::Ubuntu, MirrorName::Tuna, false).unwrap();
        assert_eq!(
            rewritten
                .matches("https://mirrors.tuna.tsinghua.edu.cn/ubuntu")
                .count(),
            2
        );
        assert_eq!(rewritten.matches("archive.ubuntu.com").count(), 0);
        assert_eq!(rewritten.matches("security.ubuntu.com").count(), 0);
    }

    #[test]
    fn already_target_detects_current_state() {
        let official = "deb http://deb.debian.org/debian bookworm main\n";
        assert!(already_target(
            official,
            Family::Debian,
            MirrorName::Official
        ));
        assert!(!already_target(official, Family::Debian, MirrorName::Tuna));
        let mirror = "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main\n";
        assert!(already_target(mirror, Family::Debian, MirrorName::Tuna));
        assert!(!already_target(
            mirror,
            Family::Debian,
            MirrorName::Official
        ));
        assert!(
            !already_target(mirror, Family::Debian, MirrorName::Aliyun),
            "tuna content is not on aliyun"
        );
    }

    #[test]
    fn rewrites_uri_with_explicit_port_and_credentials() {
        // 显式端口：主机部分归一化后仍能命中官方/镜像映射。
        let ported = "deb https://deb.debian.org:443/debian bookworm main\n";
        let rewritten = rewrite(ported, Family::Debian, MirrorName::Tuna, false).unwrap();
        assert_eq!(
            rewritten, "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main\n",
            "explicit ports must not block rewriting"
        );
        let ported_mirror = "deb https://mirrors.tuna.tsinghua.edu.cn:443/debian bookworm main\n";
        assert!(
            rewrite(ported_mirror, Family::Debian, MirrorName::Tuna, false).is_none(),
            "the same mirror with a port is already the target"
        );
        // 凭证：保留 userinfo，只替换主机。
        let authed = "deb https://user:pass@deb.debian.org/debian bookworm main\n";
        let rewritten = rewrite(authed, Family::Debian, MirrorName::Aliyun, false).unwrap();
        assert_eq!(
            rewritten,
            "deb https://user:pass@mirrors.aliyun.com/debian bookworm main\n"
        );
        // 端口 + 凭证组合。
        let both = "deb https://u:p@archive.ubuntu.com:8443/ubuntu noble main\n";
        let rewritten = rewrite(both, Family::Ubuntu, MirrorName::Ustc, false).unwrap();
        assert_eq!(
            rewritten,
            "deb https://u:p@mirrors.ustc.edu.cn/ubuntu noble main\n"
        );
        // already_target 对带端口的镜像 URL 同样生效（no-op 判定）。
        let ported_target = "deb https://mirrors.tuna.tsinghua.edu.cn:443/debian bookworm main\n";
        assert!(already_target(
            ported_target,
            Family::Debian,
            MirrorName::Tuna
        ));
    }

    #[test]
    fn converts_cdrom_entry_keeping_suites_and_components() {
        let content = "deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_ - Official amd64 DVD Binary-1 20240210-10:16]/ bookworm contrib main non-free\n";
        let converted = convert_cdrom(content, Family::Debian, MirrorName::Tuna).unwrap();
        assert_eq!(
            converted,
            "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm contrib main non-free\n"
        );
        assert!(convert_cdrom(content, Family::Debian, MirrorName::Tuna).is_some());
        let disabled = "# deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_ - Official amd64 DVD Binary-1]/ bookworm main\n";
        assert!(
            convert_cdrom(disabled, Family::Debian, MirrorName::Tuna).is_none(),
            "disabled cdrom entries must not be converted"
        );
        let official = convert_cdrom(content, Family::Debian, MirrorName::Official).unwrap();
        assert!(official.starts_with("deb https://deb.debian.org/debian bookworm"));
    }

    #[test]
    fn ubuntu_cdrom_conversion_uses_ports_on_arm_architectures() {
        let content = "deb cdrom:[Ubuntu 24.04 LTS _Noble Numbat_ - Release amd64 (20240423)]/ noble main restricted\n";
        // 常规 x86_64 → /ubuntu。
        let x86 =
            convert_cdrom_with_arch(content, Family::Ubuntu, MirrorName::Tuna, "x86_64").unwrap();
        assert_eq!(
            x86,
            "deb https://mirrors.tuna.tsinghua.edu.cn/ubuntu noble main restricted\n"
        );
        // arm64 等 ports 架构 → /ubuntu-ports。
        for arch in ["aarch64", "armv7l", "riscv64", "ppc64el", "s390x"] {
            let arm =
                convert_cdrom_with_arch(content, Family::Ubuntu, MirrorName::Tuna, arch).unwrap();
            assert_eq!(
                arm,
                "deb https://mirrors.tuna.tsinghua.edu.cn/ubuntu-ports noble main restricted\n",
                "arch {arch} must map to ubuntu-ports"
            );
        }
        // 官方目标在 ports 架构上回落到 ports.ubuntu.com。
        let official_arm =
            convert_cdrom_with_arch(content, Family::Ubuntu, MirrorName::Official, "aarch64")
                .unwrap();
        assert!(official_arm.starts_with("deb https://ports.ubuntu.com/ubuntu-ports noble"));
        // Debian 不受架构影响。
        let debian = "deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_]/ bookworm main\n";
        let arm =
            convert_cdrom_with_arch(debian, Family::Debian, MirrorName::Tuna, "aarch64").unwrap();
        assert!(arm.contains("/debian bookworm main"));
    }

    #[test]
    fn synth_lines_use_ports_on_arm_architectures() {
        // arm64 的 Ubuntu：主仓库与 security 都落在 /ubuntu-ports。
        let arm =
            synth_lines_with_arch(Family::Ubuntu, MirrorName::Tuna, false, "noble", "aarch64");
        assert!(
            arm.contains("deb https://mirrors.tuna.tsinghua.edu.cn/ubuntu-ports noble main universe restricted multiverse")
        );
        assert!(
            arm.contains("deb https://mirrors.tuna.tsinghua.edu.cn/ubuntu-ports noble-security main universe restricted multiverse")
        );
        // x86_64 保持 /ubuntu。
        let x86 = synth_lines_with_arch(Family::Ubuntu, MirrorName::Ustc, false, "noble", "x86_64");
        assert!(x86.contains(
            "deb https://mirrors.ustc.edu.cn/ubuntu noble main universe restricted multiverse"
        ));
        // 官方目标：ports 架构回落到 ports.ubuntu.com + security.ubuntu.com。
        let official_arm = synth_lines_with_arch(
            Family::Ubuntu,
            MirrorName::Official,
            false,
            "noble",
            "aarch64",
        );
        assert!(official_arm.contains("deb https://ports.ubuntu.com/ubuntu-ports noble"));
        assert!(
            official_arm.contains("deb https://security.ubuntu.com/ubuntu-ports noble-security")
        );
    }

    #[test]
    fn synth_lines_generate_working_entries() {
        let debian = synth_lines(Family::Debian, MirrorName::Tuna, false, "bookworm");
        assert!(debian.contains(
            "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main contrib non-free"
        ));
        assert!(
            debian.contains("deb https://deb.debian.org/debian-security bookworm-security main")
        );
        assert!(!debian.contains("contrib non-free main"));

        let debian_sec = synth_lines(Family::Debian, MirrorName::Aliyun, true, "bookworm");
        assert!(
            debian_sec
                .contains("deb https://mirrors.aliyun.com/debian-security bookworm-security main")
        );

        let ubuntu = synth_lines(Family::Ubuntu, MirrorName::Ustc, false, "noble");
        assert!(ubuntu.contains(
            "deb https://mirrors.ustc.edu.cn/ubuntu noble main universe restricted multiverse"
        ));
        assert!(ubuntu.contains("deb https://mirrors.ustc.edu.cn/ubuntu noble-security main universe restricted multiverse"));

        let official = synth_lines(Family::Debian, MirrorName::Official, false, "bookworm");
        assert!(official.contains("deb https://deb.debian.org/debian bookworm"));
    }
}

#[test]
fn diagnostics_flag_unrecognized_one_line_entries() {
    let content = concat!(
        "deb http://deb.debian.org/debian bookworm main\n",
        "garbage line here\n",
        "Types: deb\n",
        "deb [arch=amd64\n",
        "# a comment\n",
        "deb\n",
    );
    let (entries, issues) = parse_sources_with_diagnostics(content);
    assert_eq!(entries.len(), 6);
    assert_eq!(
        issues,
        vec![
            ParseIssue {
                line: 2,
                kind: ParseIssueKind::NotADebLine
            },
            ParseIssue {
                line: 3,
                kind: ParseIssueKind::NotADebLine
            },
            ParseIssue {
                line: 4,
                kind: ParseIssueKind::MissingUri
            },
            ParseIssue {
                line: 6,
                kind: ParseIssueKind::MissingUri
            },
        ]
    );
}

#[test]
fn diagnostics_flag_deb822_issues() {
    let content = concat!(
        "Types: deb\n",
        "Suites: bookworm\n",
        "Components: main\n",
        "\n",
        "deb http://deb.debian.org/debian bookworm main\n",
        "\n",
        "Types: deb\n",
        "URIs: http://deb.debian.org/debian\n",
        "Suites: bookworm\n",
    );
    let (entries, issues) = parse_sources_with_diagnostics(content);
    assert_eq!(entries.len(), 3);
    assert!(
        issues.contains(&ParseIssue {
            line: 1,
            kind: ParseIssueKind::StanzaWithoutUris,
        }),
        "stanza without URIs must be flagged: {issues:?}"
    );
    assert!(
        issues.contains(&ParseIssue {
            line: 5,
            kind: ParseIssueKind::NotAField,
        }),
        "a one-line entry inside a deb822 file must be flagged: {issues:?}"
    );
    assert!(
        !issues.iter().any(|i| i.line == 7),
        "the valid stanza must be clean"
    );
}

#[test]
fn diagnostics_are_empty_for_clean_files() {
    let one_line = "deb http://deb.debian.org/debian bookworm main\n# comment\n";
    assert!(parse_sources_with_diagnostics(one_line).1.is_empty());
    let deb822 = concat!(
        "Types: deb\n",
        "URIs: http://deb.debian.org/debian\n",
        "Suites: bookworm\n",
        "Components: main\n",
    );
    assert!(parse_sources_with_diagnostics(deb822).1.is_empty());
    let empty = "";
    assert!(parse_sources_with_diagnostics(empty).1.is_empty());
    let comments = "# nothing but comments\n";
    assert!(parse_sources_with_diagnostics(comments).1.is_empty());
}

#[test]
fn deb822_issues_report_the_exact_line_inside_multi_line_stanzas() {
    let content = concat!(
        "# first stanza\n",
        "Types: deb\n",
        "URIs: http://deb.debian.org/debian\n",
        "Suites: bookworm\n",
        "Components: main\n",
        "\n",
        "Types: deb\n",
        "URIs: http://deb.debian.org/debian\n",
        "garbage inside a stanza\n",
        "Suites: bookworm\n",
        "\n",
        "Types: deb\n",
        "Suites: bookworm\n",
    );
    let (_, issues) = parse_sources_with_diagnostics(content);
    assert!(
        issues.contains(&ParseIssue {
            line: 9,
            kind: ParseIssueKind::NotAField,
        }),
        "the offending line must be reported, not the stanza start: {issues:?}"
    );
    assert!(
        issues.contains(&ParseIssue {
            line: 12,
            kind: ParseIssueKind::StanzaWithoutUris,
        }),
        "missing URIs must be reported at the stanza's first field line: {issues:?}"
    );
    assert!(
        !issues.iter().any(|i| i.line <= 5),
        "the clean first stanza must not be flagged: {issues:?}"
    );
}
