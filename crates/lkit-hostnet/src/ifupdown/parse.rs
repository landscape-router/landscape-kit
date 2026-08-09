//! ifupdown(5) 语义的保守解析器。
//!
//! 解析器保留每条逻辑语句覆盖的原始物理行。ifupdown 允许 iface/mapping 选项不缩进，
//! 因此只有已知顶层关键字会结束 stanza；其他语句在 stanza 内按选项处理。

use std::path::Path;

use crate::error::HostNetError;

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct InterfaceBlock {
    pub iface: String,
    pub family: String,
    pub method: String,
    /// `iface` 逻辑语句覆盖的物理行下标(0-based,升序)。
    pub declaration_lines: Vec<usize>,
    pub inherits: Option<String>,
    /// 该 stanza 的所有选项物理行下标(0-based,升序)。
    pub option_lines: Vec<usize>,
    pub options: Vec<InterfaceOption>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct InterfaceOption {
    pub key: String,
    pub values: Vec<String>,
    pub lines: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceKind {
    File,
    Directory,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct SourceDirective {
    pub line: usize,
    pub pattern: String,
    pub kind: SourceKind,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct SelectionLine {
    pub keyword: String,
    pub interfaces: Vec<String>,
    pub lines: Vec<usize>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct MappingLine {
    pub patterns: Vec<String>,
    pub line: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct RenameLine {
    pub mappings: Vec<String>,
    pub line: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ParsedFile {
    /// 原始行(不含换行符,保留行内内容)。
    pub lines: Vec<String>,
    /// 原始文件是否以换行符结尾。
    pub ends_with_newline: bool,
    pub blocks: Vec<InterfaceBlock>,
    pub sources: Vec<SourceDirective>,
    pub selections: Vec<SelectionLine>,
    pub mappings: Vec<MappingLine>,
    pub renames: Vec<RenameLine>,
}

pub(crate) fn parse(path: &Path, content: &str) -> Result<ParsedFile, HostNetError> {
    let ends_with_newline = content.ends_with('\n');
    let body = content.strip_suffix('\n').unwrap_or(content);
    let lines: Vec<String> = body.split('\n').map(|line| line.to_string()).collect();
    let mut blocks: Vec<InterfaceBlock> = Vec::new();
    let mut sources = Vec::new();
    let mut selections = Vec::new();
    let mut mappings = Vec::new();
    let mut renames = Vec::new();
    let mut current = Stanza::None;

    for logical in logical_lines(path, &lines)? {
        let line_number = logical.lines[0] + 1;
        let body = logical.text.trim_start_matches([' ', '\t']);
        if body.trim().is_empty() || body.starts_with('#') {
            continue;
        }

        let tokens: Vec<&str> = body.split_whitespace().collect();
        let Some(keyword) = tokens.first().copied() else {
            continue;
        };
        // ifupdown accepts leading whitespace before top-level directives. Keep
        // indentation only for distinguishing unknown options outside a stanza.
        let top_level = is_top_level(keyword);
        if !top_level {
            add_option(path, line_number, &mut current, &logical, &tokens)?;
            continue;
        }

        close_iface(&mut current, &mut blocks);
        match keyword {
            "iface" => {
                if tokens.len() != 4 && tokens.len() != 6 {
                    return Err(HostNetError::UnsupportedSyntax {
                        path: path.to_path_buf(),
                        line: line_number,
                        reason: format!("expected 'iface <iface> <family> <method>', got: {body}"),
                    });
                }
                if tokens.len() == 6 && tokens[4] != "inherits" {
                    return Err(HostNetError::UnsupportedSyntax {
                        path: path.to_path_buf(),
                        line: line_number,
                        reason: format!("unsupported iface suffix in: {body}"),
                    });
                }
                current = Stanza::Iface(InterfaceBlock {
                    iface: tokens[1].to_string(),
                    family: tokens[2].to_string(),
                    method: tokens[3].to_string(),
                    declaration_lines: logical.lines,
                    inherits: (tokens.len() == 6).then(|| tokens[5].to_string()),
                    option_lines: Vec::new(),
                    options: Vec::new(),
                });
            }
            "auto" | "no-auto-down" | "no-scripts" => {
                require_arguments(path, line_number, keyword, &tokens)?;
                selections.push(SelectionLine {
                    keyword: keyword.to_string(),
                    interfaces: tokens[1..]
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect(),
                    lines: logical.lines,
                });
                current = Stanza::None;
            }
            "mapping" => {
                require_arguments(path, line_number, keyword, &tokens)?;
                mappings.push(MappingLine {
                    patterns: tokens[1..]
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect(),
                    line: line_number,
                });
                current = Stanza::Mapping;
            }
            "rename" => {
                require_arguments(path, line_number, keyword, &tokens)?;
                renames.push(RenameLine {
                    mappings: tokens[1..]
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect(),
                    line: line_number,
                });
                current = Stanza::None;
            }
            "source" | "source-directory" => {
                if tokens.len() != 2 {
                    return Err(HostNetError::UnsupportedSyntax {
                        path: path.to_path_buf(),
                        line: line_number,
                        reason: format!("expected '{keyword} <pattern>', got: {body}"),
                    });
                }
                sources.push(SourceDirective {
                    line: line_number,
                    pattern: tokens[1].to_string(),
                    kind: if keyword == "source" {
                        SourceKind::File
                    } else {
                        SourceKind::Directory
                    },
                });
                current = Stanza::None;
            }
            keyword if keyword.starts_with("allow-") => {
                require_arguments(path, line_number, keyword, &tokens)?;
                selections.push(SelectionLine {
                    keyword: keyword.to_string(),
                    interfaces: tokens[1..]
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect(),
                    lines: logical.lines,
                });
                current = Stanza::None;
            }
            _ => unreachable!("top-level keyword was classified before dispatch"),
        }
    }
    close_iface(&mut current, &mut blocks);
    Ok(ParsedFile {
        lines,
        ends_with_newline,
        blocks,
        sources,
        selections,
        mappings,
        renames,
    })
}

struct LogicalLine {
    text: String,
    lines: Vec<usize>,
    indented: bool,
}

fn logical_lines(path: &Path, lines: &[String]) -> Result<Vec<LogicalLine>, HostNetError> {
    let mut logical = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let first = index;
        let mut text = strip_carriage_return(&lines[index]).to_string();
        let mut physical = vec![index];
        let indented = text.starts_with(' ') || text.starts_with('\t');
        while text.ends_with('\\') {
            text.pop();
            index += 1;
            if index >= lines.len() {
                return Err(HostNetError::UnsupportedSyntax {
                    path: path.to_path_buf(),
                    line: first + 1,
                    reason: "unterminated line continuation".into(),
                });
            }
            text.push_str(strip_carriage_return(&lines[index]));
            physical.push(index);
        }
        logical.push(LogicalLine {
            text,
            lines: physical,
            indented,
        });
        index += 1;
    }
    Ok(logical)
}

fn strip_carriage_return(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

fn is_top_level(keyword: &str) -> bool {
    matches!(
        keyword,
        "iface"
            | "auto"
            | "mapping"
            | "rename"
            | "source"
            | "source-directory"
            | "no-auto-down"
            | "no-scripts"
    ) || keyword.starts_with("allow-")
}

fn require_arguments(
    path: &Path,
    line: usize,
    keyword: &str,
    tokens: &[&str],
) -> Result<(), HostNetError> {
    if tokens.len() > 1 {
        return Ok(());
    }
    Err(HostNetError::UnsupportedSyntax {
        path: path.to_path_buf(),
        line,
        reason: format!("{keyword} requires at least one argument"),
    })
}

fn add_option(
    path: &Path,
    line_number: usize,
    current: &mut Stanza,
    logical: &LogicalLine,
    tokens: &[&str],
) -> Result<(), HostNetError> {
    match current {
        Stanza::Mapping => Ok(()),
        Stanza::Iface(block) => {
            block.option_lines.extend(logical.lines.iter().copied());
            block.options.push(InterfaceOption {
                key: tokens[0].to_string(),
                values: tokens[1..]
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect(),
                lines: logical.lines.clone(),
            });
            Ok(())
        }
        Stanza::None => Err(HostNetError::UnsupportedSyntax {
            path: path.to_path_buf(),
            line: line_number,
            reason: if logical.indented {
                "indented option outside of any stanza".into()
            } else {
                format!("unknown statement '{}'", tokens[0])
            },
        }),
    }
}

enum Stanza {
    None,
    Mapping,
    Iface(InterfaceBlock),
}

fn close_iface(current: &mut Stanza, blocks: &mut Vec<InterfaceBlock>) {
    if let Stanza::Iface(block) = std::mem::replace(current, Stanza::None) {
        blocks.push(block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse_ok(content: &str) -> ParsedFile {
        parse(&PathBuf::from("/etc/network/interfaces"), content).unwrap()
    }

    fn parse_err(content: &str) -> HostNetError {
        parse(&PathBuf::from("/etc/network/interfaces"), content).unwrap_err()
    }

    #[test]
    fn parses_static_block_with_options() {
        let parsed = parse_ok(
            "auto eth0\niface eth0 inet static\n    address 192.168.1.10\n    netmask 255.255.255.0\n",
        );
        assert_eq!(parsed.blocks.len(), 1);
        let block = &parsed.blocks[0];
        assert_eq!(block.iface, "eth0");
        assert_eq!(block.family, "inet");
        assert_eq!(block.method, "static");
        assert_eq!(block.declaration_lines, vec![1]);
        assert_eq!(block.option_lines, vec![2, 3]);
        assert!(parsed.sources.is_empty());
    }

    #[test]
    fn comments_and_blank_lines_do_not_end_a_stanza() {
        let parsed = parse_ok(
            "iface eth0 inet static\n    address 192.168.1.10\n\n# note\n    gateway 192.168.1.1\n",
        );
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].option_lines, vec![1, 4]);
    }

    #[test]
    fn non_indented_statements_end_the_stanza() {
        let parsed = parse_ok(
            "iface eth0 inet static\n    address 192.168.1.10\nauto eth1\niface eth1 inet dhcp\n    hostname host\n",
        );
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.blocks[0].iface, "eth0");
        assert_eq!(parsed.blocks[0].option_lines, vec![1]);
        assert_eq!(parsed.blocks[1].iface, "eth1");
        assert_eq!(parsed.blocks[1].option_lines, vec![4]);
    }

    #[test]
    fn dual_family_blocks_for_one_interface() {
        let parsed = parse_ok(
            "iface eth0 inet static\n    address 192.168.1.10\niface eth0 inet6 static\n    address fd00::1\n",
        );
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.blocks[0].family, "inet");
        assert_eq!(parsed.blocks[1].family, "inet6");
    }

    #[test]
    fn mapping_and_allow_hotplug_are_accepted() {
        let parsed = parse_ok(
            "mapping eth0\n    script /usr/local/bin/map\nallow-hotplug eth1\niface eth1 inet dhcp\nallow-auto eth2\niface eth2 inet manual\n",
        );
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.blocks[0].iface, "eth1");
        assert_eq!(parsed.blocks[1].iface, "eth2");
    }

    #[test]
    fn source_lines_are_collected_and_end_the_stanza() {
        let parsed = parse_ok(
            "iface eth0 inet static\n    address 192.168.1.10\nsource /etc/network/interfaces.d/*\n",
        );
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(
            parsed.sources,
            vec![SourceDirective {
                line: 3,
                pattern: "/etc/network/interfaces.d/*".to_string(),
                kind: SourceKind::File,
            }]
        );
    }

    #[test]
    fn trailing_newline_is_recorded() {
        assert!(parse_ok("auto eth0\n").ends_with_newline);
        assert!(!parse_ok("auto eth0").ends_with_newline);
    }

    #[test]
    fn accepts_crlf_blank_lines_and_continuations() {
        let parsed = parse_ok(
            "auto eth0 \\\r\n eth1\r\n\r\niface eth0 inet static\r\n    address 192.0.2.2/\\\r\n24\r\n",
        );
        assert_eq!(parsed.selections[0].interfaces, vec!["eth0", "eth1"]);
        assert_eq!(parsed.blocks[0].option_lines, vec![4, 5]);
        assert_eq!(parsed.blocks[0].options[0].values, vec!["192.0.2.2/24"]);
    }

    #[test]
    fn malformed_iface_line_is_rejected() {
        let error = parse_err("iface eth0 inet\n");
        assert!(matches!(
            error,
            HostNetError::UnsupportedSyntax { line: 1, .. }
        ));
        let error = parse_err("iface eth0 inet static extra\n");
        assert!(matches!(
            error,
            HostNetError::UnsupportedSyntax { line: 1, .. }
        ));
    }

    #[test]
    fn unknown_statement_is_rejected() {
        let error = parse_err("flooble eth0\n");
        assert!(matches!(
            error,
            HostNetError::UnsupportedSyntax { line: 1, .. }
        ));
    }

    #[test]
    fn source_directory_is_collected() {
        let parsed = parse_ok("source-directory /etc/network/interfaces.d\n");
        assert_eq!(parsed.sources.len(), 1);
        assert_eq!(parsed.sources[0].kind, SourceKind::Directory);
        assert_eq!(parsed.sources[0].line, 1);
    }

    #[test]
    fn option_outside_stanza_is_rejected() {
        let error = parse_err("    address 192.168.1.10\n");
        assert!(matches!(
            error,
            HostNetError::UnsupportedSyntax { line: 1, .. }
        ));
    }

    #[test]
    fn indented_comment_outside_stanza_is_allowed() {
        let parsed = parse_ok("    # leading comment\niface eth0 inet manual\n");
        assert_eq!(parsed.blocks.len(), 1);
    }

    #[test]
    fn empty_file_parses() {
        let parsed = parse_ok("");
        assert!(parsed.blocks.is_empty());
        assert!(parsed.sources.is_empty());
    }

    #[test]
    fn non_indented_options_belong_to_the_iface_stanza() {
        let parsed = parse_ok(
            "iface eth0 inet static\naddress 192.168.1.10/24\ngateway 192.168.1.1\nauto eth1\n",
        );
        assert_eq!(parsed.blocks[0].option_lines, vec![1, 2]);
        assert_eq!(parsed.blocks[0].options[0].key, "address");
        assert_eq!(parsed.selections[0].interfaces, vec!["eth1"]);
    }

    #[test]
    fn continuations_keep_their_physical_line_ranges() {
        let parsed = parse_ok(
            "auto eth0 \\\n eth1\niface eth0 inet static\n    address 192.168.1.10/\\\n24\n",
        );
        assert_eq!(parsed.selections[0].interfaces, vec!["eth0", "eth1"]);
        assert_eq!(parsed.selections[0].lines, vec![0, 1]);
        assert_eq!(parsed.blocks[0].option_lines, vec![3, 4]);
        assert_eq!(parsed.blocks[0].options[0].values, vec!["192.168.1.10/24"]);
    }

    #[test]
    fn iface_inherits_and_standard_top_level_directives_are_accepted() {
        let parsed = parse_ok(
            "rename old*=new\nno-auto-down eth9\nno-scripts eth8\nallow-custom eth7\niface eth0 inet static inherits ethernet\naddress 192.168.1.10/24\n",
        );
        assert_eq!(parsed.renames.len(), 1);
        assert_eq!(parsed.selections.len(), 3);
        assert_eq!(parsed.blocks[0].inherits.as_deref(), Some("ethernet"));
        assert_eq!(parsed.blocks[0].option_lines, vec![5]);
    }

    #[test]
    fn indented_top_level_directives_are_accepted() {
        let parsed = parse_ok(
            "  auto eth0\n  iface eth0 inet dhcp\n    address 192.0.2.2/24\n  source-directory interfaces.d\n",
        );
        assert_eq!(parsed.selections[0].interfaces, vec!["eth0"]);
        assert_eq!(parsed.blocks[0].iface, "eth0");
        assert_eq!(parsed.blocks[0].option_lines, vec![2]);
        assert_eq!(parsed.sources.len(), 1);
    }

    #[test]
    fn unterminated_continuation_is_rejected() {
        let error = parse_err("auto eth0 \\");
        assert!(matches!(
            error,
            HostNetError::UnsupportedSyntax { line: 1, .. }
        ));
    }
}
