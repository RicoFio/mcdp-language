//! Syntax-context completion items.

use std::fs;
use std::path::{Path, PathBuf};

use mcdp_language::{TextRange, Token, TokenKind, lex};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionTextEdit, InsertTextFormat,
    Position, Range, TextEdit, Url, WorkDoneProgressOptions,
};

use crate::line_index::LineIndex;
use crate::project_symbols::{PortDirection, PortSymbol, ProjectSymbolIndex};

pub(crate) fn options() -> CompletionOptions {
    CompletionOptions {
        resolve_provider: Some(false),
        trigger_characters: Some(
            ["`", "\"", "/", " ", "+", "-", "*", "=", "<", ">", "≤", "≥"]
                .map(str::to_owned)
                .into(),
        ),
        all_commit_characters: None,
        work_done_progress_options: WorkDoneProgressOptions::default(),
        completion_item: None,
    }
}

pub(crate) fn items(
    uri: &Url,
    source: &str,
    position: Position,
    symbols: Option<&ProjectSymbolIndex>,
) -> Vec<CompletionItem> {
    let line_index = LineIndex::new(source);
    let offset = line_index.offset(position);
    let prefix = &source[..offset];
    let suffix = &source[offset..];
    let replace_range = completion_replace_range(source, offset, &line_index);

    if is_resource_string_context(prefix) {
        return resource_path_items(uri);
    }
    if let Some(context) = instance_port_context(suffix) {
        return instance_port_items(uri, symbols, &context, replace_range);
    }
    if is_instance_name_context(prefix) {
        return instance_items(uri, symbols);
    }
    if is_model_reference_context(prefix) {
        return model_reference_items(uri, symbols);
    }
    if is_top_level_context(prefix) {
        return top_level_document_items();
    }
    if is_instance_scoped_expression_context(prefix) {
        return instance_scoped_expression_items(uri, symbols, replace_range);
    }
    if is_braced_body_context(prefix) {
        return statement_items();
    }

    Vec::new()
}

fn is_top_level_context(prefix: &str) -> bool {
    let significant_tokens: Vec<_> = lex(prefix)
        .into_iter()
        .filter(|token| !is_trivia(token.kind))
        .collect();

    significant_tokens.is_empty()
        || (significant_tokens.len() == 1
            && matches!(
                significant_tokens[0].kind,
                TokenKind::Ident | TokenKind::Keyword
            )
            && !prefix.contains('\n'))
}

fn is_braced_body_context(prefix: &str) -> bool {
    let mut depth = 0usize;
    for token in lex(prefix) {
        if matches!(token.kind, TokenKind::Comment | TokenKind::String) {
            continue;
        }
        match token.text.as_str() {
            "{" => depth += 1,
            "}" => depth = depth.saturating_sub(1),
            _ => {}
        }
    }

    depth > 0
}

fn is_model_reference_context(prefix: &str) -> bool {
    let segment = prefix
        .rsplit(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '(' | '[' | ':' | '='))
        .next()
        .unwrap_or_default();
    segment.starts_with('`')
}

fn is_instance_scoped_expression_context(prefix: &str) -> bool {
    if !is_braced_body_context(prefix) {
        return false;
    }

    let line_prefix = prefix.rsplit_once('\n').map_or(prefix, |(_, line)| line);
    let tokens = significant_tokens(line_prefix);
    tokens.iter().any(|token| is_relation_operator(&token.text))
        || tokens.last().is_some_and(|token| {
            matches!(
                token.text.as_str(),
                "+" | "-" | "*" | "/" | "(" | "," | "<=" | ">=" | "≤" | "≥" | "=" | "=="
            )
        })
}

fn is_instance_name_context(prefix: &str) -> bool {
    let tokens = significant_tokens(prefix);
    let length = tokens.len();
    if length >= 3
        && is_symbol_name(&tokens[length - 3])
        && is_reference_direction(tokens[length - 2].text.as_str())
        && tokens[length - 1].text.as_str() == "by"
    {
        return true;
    }

    length >= 4
        && is_symbol_name(&tokens[length - 4])
        && is_reference_direction(tokens[length - 3].text.as_str())
        && tokens[length - 2].text.as_str() == "by"
        && is_symbol_name(&tokens[length - 1])
}

fn instance_port_context(suffix: &str) -> Option<InstancePortContext> {
    let tokens = significant_tokens(suffix);
    let direction = reference_direction(tokens.first()?.text.as_str())?;
    if tokens.get(1).map(|token| token.text.as_str()) != Some("by") {
        return None;
    }
    let instance = tokens.get(2)?;
    if !is_symbol_name(instance) {
        return None;
    }

    Some(InstancePortContext {
        direction,
        instance_name: instance.text.clone(),
        insert_mode: InstancePortInsertMode::PortOnly,
    })
}

fn is_resource_string_context(prefix: &str) -> bool {
    let line_prefix = prefix.rsplit_once('\n').map_or(prefix, |(_, line)| line);
    let Some(resource_start) = line_prefix.rfind("resource(") else {
        return false;
    };
    let after_resource = &line_prefix[resource_start + "resource(".len()..];
    let Some((quote_index, quote)) = after_resource
        .char_indices()
        .find(|(_, ch)| matches!(ch, '"' | '\''))
    else {
        return false;
    };
    let after_quote = &after_resource[quote_index + quote.len_utf8()..];

    !after_quote.contains(quote)
}

fn top_level_document_items() -> Vec<CompletionItem> {
    [
        snippet_item("mcdp", "MCDP document", "mcdp {\n  $0\n}"),
        snippet_item("dp", "DP document", "dp {\n  $0\n}"),
        snippet_item("catalog", "Catalog document", "catalog {\n  $0\n}"),
        snippet_item(
            "choose",
            "Choice composition",
            "choose (${1:Label}: `${2:model})",
        ),
        snippet_item(
            "intersection",
            "Intersection composition",
            "intersection (${1:Label}: `${2:model})",
        ),
        snippet_item("interface", "Interface document", "interface {\n  $0\n}"),
        snippet_item("poset", "Poset document", "poset {\n  $0\n}"),
        snippet_item(
            "template",
            "Template document",
            "template [${1:T}: `${2:Interface}] mcdp {\n  $0\n}",
        ),
        snippet_item(
            "specialize",
            "Template specialization",
            "specialize [${1:T}: `${2:model}] `${3:Template}",
        ),
    ]
    .into()
}

fn statement_items() -> Vec<CompletionItem> {
    vec![
        snippet_item(
            "provides",
            "Functionality declaration",
            "provides ${1:name} [${2:Poset}]",
        ),
        snippet_item(
            "requires",
            "Requirement declaration",
            "requires ${1:name} [${2:Poset}]",
        ),
        snippet_item(
            "sub = instance",
            "Subproblem instance",
            "sub ${1:name} = instance `${2:model}",
        ),
        snippet_item(
            "implemented-by yaml resource",
            "YAML implementation binding",
            "implemented-by yaml resource(\"${1:path}\")",
        ),
        snippet_item(
            "implements",
            "Interface implementation",
            "implements `${1:interface}",
        ),
        snippet_item("import model", "Model import", "import model `${1:model}"),
    ]
}

fn model_reference_items(uri: &Url, symbols: Option<&ProjectSymbolIndex>) -> Vec<CompletionItem> {
    let names = symbols
        .map(ProjectSymbolIndex::model_completion_names)
        .filter(|names| !names.is_empty())
        .unwrap_or_else(|| model_reference_names(uri));

    names
        .into_iter()
        .map(|name| {
            plain_item(
                &name,
                CompletionItemKind::REFERENCE,
                "MCDPL model reference",
                &name,
            )
        })
        .collect()
}

fn instance_items(uri: &Url, symbols: Option<&ProjectSymbolIndex>) -> Vec<CompletionItem> {
    let Some(symbols) = symbols else {
        return Vec::new();
    };

    symbols
        .instance_completion_names(uri)
        .into_iter()
        .map(|name| {
            plain_item(
                &name,
                CompletionItemKind::VARIABLE,
                "Local MCDPL instance",
                &name,
            )
        })
        .collect()
}

fn instance_port_items(
    uri: &Url,
    symbols: Option<&ProjectSymbolIndex>,
    context: &InstancePortContext,
    replace_range: Range,
) -> Vec<CompletionItem> {
    let Some(symbols) = symbols else {
        return Vec::new();
    };

    symbols
        .instance_port_completions(uri, &context.instance_name, context.direction)
        .iter()
        .map(|port| {
            port_item(
                port,
                context.direction,
                &context.instance_name,
                context.insert_mode,
                replace_range,
            )
        })
        .collect()
}

fn instance_scoped_expression_items(
    uri: &Url,
    symbols: Option<&ProjectSymbolIndex>,
    replace_range: Range,
) -> Vec<CompletionItem> {
    let Some(symbols) = symbols else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for instance_name in symbols.instance_completion_names(uri) {
        for direction in [PortDirection::Required, PortDirection::Provided] {
            items.extend(
                symbols
                    .instance_port_completions(uri, &instance_name, direction)
                    .iter()
                    .map(|port| {
                        port_item(
                            port,
                            direction,
                            &instance_name,
                            InstancePortInsertMode::FullExpression,
                            replace_range,
                        )
                    }),
            );
        }
    }
    items
}

fn resource_path_items(uri: &Url) -> Vec<CompletionItem> {
    resource_paths(uri)
        .into_iter()
        .map(|path| plain_item(&path, CompletionItemKind::FILE, "Resource path", &path))
        .collect()
}

fn port_item(
    port: &PortSymbol,
    direction: PortDirection,
    instance_name: &str,
    insert_mode: InstancePortInsertMode,
    replace_range: Range,
) -> CompletionItem {
    let expression = instance_scoped_expression(&port.name, direction, instance_name);
    let new_text = match insert_mode {
        InstancePortInsertMode::PortOnly => port.name.clone(),
        InstancePortInsertMode::FullExpression => expression.clone(),
    };
    let detail = match port.unit.as_ref() {
        Some(unit) => format!(
            "{} port from `{instance_name}` [{}]",
            port_direction_label(direction),
            unit.text
        ),
        None => format!(
            "{} port from `{instance_name}`",
            port_direction_label(direction)
        ),
    };
    let mut item = plain_item(
        &expression,
        CompletionItemKind::VARIABLE,
        &detail,
        &new_text,
    );
    item.filter_text = Some(format!("{} {expression}", port.name));
    item.sort_text = Some(format!("{} {}", instance_name, port.name));
    item.text_edit = Some(CompletionTextEdit::Edit(TextEdit::new(
        replace_range,
        new_text,
    )));
    item.insert_text = None;
    item
}

fn snippet_item(label: &str, detail: &str, insert_text: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_owned(),
        kind: Some(CompletionItemKind::SNIPPET),
        detail: Some(detail.to_owned()),
        insert_text: Some(insert_text.to_owned()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..CompletionItem::default()
    }
}

fn plain_item(
    label: &str,
    kind: CompletionItemKind,
    detail: &str,
    insert_text: &str,
) -> CompletionItem {
    CompletionItem {
        label: label.to_owned(),
        kind: Some(kind),
        detail: Some(detail.to_owned()),
        insert_text: Some(insert_text.to_owned()),
        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
        ..CompletionItem::default()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InstancePortContext {
    direction: PortDirection,
    instance_name: String,
    insert_mode: InstancePortInsertMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstancePortInsertMode {
    PortOnly,
    FullExpression,
}

fn model_reference_names(uri: &Url) -> Vec<String> {
    let Some(root) = uri_directory(uri) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && is_mcdpl_model_file(path))
        .filter_map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

fn resource_paths(uri: &Url) -> Vec<String> {
    let Some(root) = uri_directory(uri) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    collect_yaml_paths(&root, &root, 1, &mut paths);
    paths.sort();
    paths.dedup();
    paths
}

fn collect_yaml_paths(root: &Path, dir: &Path, remaining_depth: usize, paths: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if remaining_depth > 0 {
                collect_yaml_paths(root, &path, remaining_depth - 1, paths);
            }
            continue;
        }

        if path.is_file()
            && is_yaml_file(&path)
            && let Ok(relative) = path.strip_prefix(root)
        {
            paths.push(path_to_completion_text(relative));
        }
    }
}

fn uri_directory(uri: &Url) -> Option<PathBuf> {
    uri.to_file_path()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
}

fn path_to_completion_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn is_mcdpl_model_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("mcdp" | "mcdp_interface" | "mcdp_template" | "mcdp_poset")
    )
}

fn is_yaml_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("yaml" | "yml")
    )
}

fn is_trivia(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment
    )
}

fn significant_tokens(source: &str) -> Vec<Token> {
    lex(source)
        .into_iter()
        .filter(|token| !is_trivia(token.kind))
        .collect()
}

fn is_symbol_name(token: &Token) -> bool {
    matches!(token.kind, TokenKind::Ident | TokenKind::Keyword)
}

fn reference_direction(text: &str) -> Option<PortDirection> {
    match text {
        "provided" => Some(PortDirection::Provided),
        "required" => Some(PortDirection::Required),
        _ => None,
    }
}

fn is_reference_direction(text: &str) -> bool {
    reference_direction(text).is_some()
}

fn is_relation_operator(text: &str) -> bool {
    matches!(text, "<=" | ">=" | "≤" | "≥" | "=" | "==")
}

fn port_direction_label(direction: PortDirection) -> &'static str {
    match direction {
        PortDirection::Provided => "Provided",
        PortDirection::Required => "Required",
    }
}

fn port_direction_keyword(direction: PortDirection) -> &'static str {
    match direction {
        PortDirection::Provided => "provided",
        PortDirection::Required => "required",
    }
}

fn instance_scoped_expression(
    port_name: &str,
    direction: PortDirection,
    instance_name: &str,
) -> String {
    format!(
        "{port_name} {} by {instance_name}",
        port_direction_keyword(direction)
    )
}

fn completion_replace_range(source: &str, offset: usize, line_index: &LineIndex<'_>) -> Range {
    let mut start = offset;
    for (candidate, ch) in source[..offset].char_indices().rev() {
        if !is_completion_name_char(ch) {
            break;
        }
        start = candidate;
    }

    line_index.range(TextRange::new(start, offset))
}

fn is_completion_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn options_advertise_basic_triggers() {
        let options = options();

        assert_eq!(options.resolve_provider, Some(false));
        assert_eq!(
            options.trigger_characters,
            Some(
                ["`", "\"", "/", " ", "+", "-", "*", "=", "<", ">", "≤", "≥"]
                    .map(str::to_owned)
                    .into()
            )
        );
    }

    #[test]
    fn completions_include_top_level_document_snippets() {
        let uri = test_file_url(Path::new("/tmp/model.mcdp"));
        let items = items(&uri, "", Position::new(0, 0), None);
        let labels = labels(&items);

        assert!(labels.contains(&"mcdp"));
        assert!(labels.contains(&"dp"));
        assert!(labels.contains(&"choose"));
        assert!(labels.contains(&"specialize"));
        assert_eq!(
            item_by_label(&items, "mcdp").insert_text_format,
            Some(InsertTextFormat::SNIPPET)
        );
    }

    #[test]
    fn completions_include_statement_snippets_inside_braced_body() {
        let source = "mcdp {\n  ";
        let uri = test_file_url(Path::new("/tmp/model.mcdp"));
        let items = items(&uri, source, Position::new(1, 2), None);
        let labels = labels(&items);

        assert!(labels.contains(&"provides"));
        assert!(labels.contains(&"requires"));
        assert!(labels.contains(&"sub = instance"));
        assert!(labels.contains(&"implemented-by yaml resource"));
        assert!(labels.contains(&"implements"));
        assert!(labels.contains(&"import model"));
    }

    #[test]
    fn model_reference_completions_scan_mcdpl_files_in_current_directory() {
        let temp_dir = TempDir::new("model-reference-completions");
        temp_dir.write("system.mcdp", "mcdp {\n  sub x = instance `\n}\n");
        temp_dir.write("battery.mcdp", "dp {}");
        temp_dir.write("robot.mcdp_interface", "interface {}");
        temp_dir.write("loop.mcdp_template", "template [T: `robot] mcdp {}");
        temp_dir.write("map.mcdp_poset", "poset {}");
        temp_dir.write("query.mcdp_query.yaml", "schema: query");

        let uri = temp_dir.url("system.mcdp");
        let source = "mcdp {\n  sub x = instance `\n}\n";
        let items = items(&uri, source, Position::new(1, 21), None);
        let labels = labels(&items);

        assert_eq!(
            item_by_label(&items, "battery").kind,
            Some(CompletionItemKind::REFERENCE)
        );
        assert!(labels.contains(&"robot"));
        assert!(labels.contains(&"loop"));
        assert!(labels.contains(&"map"));
        assert!(!labels.contains(&"query.mcdp_query"));
    }

    #[test]
    fn model_reference_completions_use_symbol_index_open_documents() {
        let temp_dir = TempDir::new("model-reference-index-completions");
        temp_dir.write("system.mcdp", "mcdp {\n  sub x = instance `\n}\n");
        temp_dir.write("battery.mcdp", "dp {}");
        let uri = temp_dir.url("system.mcdp");
        let draft_uri = temp_dir.url("draft_model.mcdp");
        let mut open_documents = HashMap::new();
        open_documents.insert(draft_uri, "dp { provides draft_port [Nat] }".to_owned());
        let index = ProjectSymbolIndex::for_uri(&uri, &open_documents);
        let source = "mcdp {\n  sub x = instance `\n}\n";

        let items = items(&uri, source, Position::new(1, 21), Some(&index));
        let labels = labels(&items);

        assert!(labels.contains(&"battery"));
        assert!(labels.contains(&"draft_model"));
    }

    #[test]
    fn instance_name_completions_use_current_document_bindings_after_by() {
        let temp_dir = TempDir::new("instance-name-completions");
        let source = concat!(
            "mcdp {\n",
            "  sub fu = instance `fuel\n",
            "  sub mt = instance `maintenance\n",
            "  required total_cost >= fuel_cost required by ",
            "\n}\n",
        );
        temp_dir.write("system.mcdp", source);
        temp_dir.write("fuel.mcdp", "dp { requires fuel_cost [USD] }");
        temp_dir.write("maintenance.mcdp", "dp { requires maintenance_cost [USD] }");
        let uri = temp_dir.url("system.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let items = items(
            &uri,
            source,
            position_after(source, "required by "),
            Some(&index),
        );
        let labels = labels(&items);

        assert_eq!(
            item_by_label(&items, "fu").kind,
            Some(CompletionItemKind::VARIABLE)
        );
        assert!(labels.contains(&"mt"));
    }

    #[test]
    fn instance_port_completions_use_selected_instance_and_direction() {
        let temp_dir = TempDir::new("instance-port-completions");
        let source = "\
mcdp {
  sub fu = instance `fuel
  required total_cost >=  required by fu
}
";
        temp_dir.write("system.mcdp", source);
        temp_dir.write(
            "fuel.mcdp",
            "\
dp {
  requires fuel_cost [USD]
  requires emissions [kg]
  provides generated_power [W]
}
",
        );
        let uri = temp_dir.url("system.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let items = items(&uri, source, position_after(source, ">= "), Some(&index));
        let labels = labels(&items);

        let item = item_by_label(&items, "fuel_cost required by fu");
        assert_eq!(
            item.detail.as_deref(),
            Some("Required port from `fu` [USD]")
        );
        assert_eq!(completion_new_text(item), Some("fuel_cost"));
        assert!(labels.contains(&"emissions required by fu"));
        assert!(!labels.contains(&"generated_power provided by fu"));
    }

    #[test]
    fn instance_port_completions_replace_partial_port_before_existing_suffix() {
        let temp_dir = TempDir::new("partial-instance-port-completions");
        let source = "\
mcdp {
  sub fu = instance `fuel
  required total_cost >= fuel required by fu
}
";
        temp_dir.write("system.mcdp", source);
        temp_dir.write("fuel.mcdp", "dp { requires fuel_cost [USD] }");
        let uri = temp_dir.url("system.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let items = items(
            &uri,
            source,
            position_after(source, ">= fuel"),
            Some(&index),
        );
        let item = item_by_label(&items, "fuel_cost required by fu");

        assert_eq!(
            apply_completion(source, item),
            "mcdp {\n  sub fu = instance `fuel\n  required total_cost >= fuel_cost required by fu\n}\n"
        );
    }

    #[test]
    fn instance_port_completions_insert_full_instance_scoped_expression() {
        let temp_dir = TempDir::new("full-instance-port-completions");
        let source = "\
mcdp {
  sub fu = instance `fuel
  required total_cost >= fuel
}
";
        temp_dir.write("system.mcdp", source);
        temp_dir.write(
            "fuel.mcdp",
            "\
dp {
  requires fuel_cost [USD]
  provides available_fuel [kg]
}
",
        );
        let uri = temp_dir.url("system.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let items = items(
            &uri,
            source,
            position_after(source, ">= fuel"),
            Some(&index),
        );
        let required = item_by_label(&items, "fuel_cost required by fu");
        let provided = item_by_label(&items, "available_fuel provided by fu");

        assert_eq!(
            completion_new_text(required),
            Some("fuel_cost required by fu")
        );
        assert_eq!(
            apply_completion(source, required),
            "mcdp {\n  sub fu = instance `fuel\n  required total_cost >= fuel_cost required by fu\n}\n"
        );
        assert_eq!(
            completion_new_text(provided),
            Some("available_fuel provided by fu")
        );
    }

    #[test]
    fn instance_port_completions_appear_after_operator_trigger_context() {
        let temp_dir = TempDir::new("operator-trigger-completions");
        let source = concat!(
            "mcdp {\n",
            "  sub fu = instance `fuel\n",
            "  required total_cost >= ",
            "\n}\n",
        );
        temp_dir.write("system.mcdp", source);
        temp_dir.write("fuel.mcdp", "dp { requires fuel_cost [USD] }");
        let uri = temp_dir.url("system.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let items = items(&uri, source, position_after(source, ">= "), Some(&index));
        let item = item_by_label(&items, "fuel_cost required by fu");

        assert_eq!(completion_new_text(item), Some("fuel_cost required by fu"));
    }

    #[test]
    fn resource_path_completions_scan_yaml_files_one_directory_deep() {
        let temp_dir = TempDir::new("resource-path-completions");
        temp_dir.write("model.mcdp", "dp {}");
        temp_dir.write("direct.yaml", "[]");
        temp_dir.write("yaml_catalogs/id.dpc.yaml", "[]");
        temp_dir.write("nested/ignored.txt", "");

        let uri = temp_dir.url("model.mcdp");
        let source = "dp {\n  implemented-by yaml resource(\"";
        let items = items(&uri, source, Position::new(1, 33), None);
        let labels = labels(&items);

        assert_eq!(
            item_by_label(&items, "direct.yaml").kind,
            Some(CompletionItemKind::FILE)
        );
        assert!(labels.contains(&"yaml_catalogs/id.dpc.yaml"));
        assert!(!labels.contains(&"nested/ignored.txt"));
    }

    fn labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.label.as_str()).collect()
    }

    fn position_after(source: &str, needle: &str) -> Position {
        let offset = match source.find(needle) {
            Some(offset) => offset + needle.len(),
            None => panic!("missing `{needle}` in test source"),
        };
        LineIndex::new(source).position(offset)
    }

    fn item_by_label<'a>(items: &'a [CompletionItem], label: &str) -> &'a CompletionItem {
        match items.iter().find(|item| item.label == label) {
            Some(item) => item,
            None => panic!("missing completion item `{label}`"),
        }
    }

    fn completion_new_text(item: &CompletionItem) -> Option<&str> {
        match item.text_edit.as_ref()? {
            CompletionTextEdit::Edit(edit) => Some(edit.new_text.as_str()),
            CompletionTextEdit::InsertAndReplace(edit) => Some(edit.new_text.as_str()),
        }
    }

    fn apply_completion(source: &str, item: &CompletionItem) -> String {
        let Some(CompletionTextEdit::Edit(edit)) = item.text_edit.as_ref() else {
            panic!("completion item should use a text edit");
        };
        let line_index = LineIndex::new(source);
        let start = line_index.offset(edit.range.start);
        let end = line_index.offset(edit.range.end);
        let mut completed = String::with_capacity(source.len() + edit.new_text.len());
        completed.push_str(&source[..start]);
        completed.push_str(&edit.new_text);
        completed.push_str(&source[end..]);
        completed
    }

    fn test_file_url(path: &Path) -> Url {
        match Url::from_file_path(path) {
            Ok(url) => url,
            Err(()) => panic!("could not convert test path to file URL"),
        }
    }

    fn must<T, E: std::fmt::Debug>(result: std::result::Result<T, E>, context: &str) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("{context}: {error:?}"),
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
                Ok(duration) => duration.as_nanos(),
                Err(error) => panic!("could not read system time: {error:?}"),
            };
            let path =
                std::env::temp_dir().join(format!("mcdp-lsp-{name}-{}-{now}", std::process::id()));
            must(fs::create_dir_all(&path), "could not create temp dir");
            Self { path }
        }

        fn write(&self, relative_path: &str, text: &str) {
            let path = self.path.join(relative_path);
            if let Some(parent) = path.parent() {
                must(
                    fs::create_dir_all(parent),
                    "could not create temp file parent",
                );
            }
            must(fs::write(path, text), "could not write temp file");
        }

        fn url(&self, relative_path: &str) -> Url {
            test_file_url(&self.path.join(relative_path))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
