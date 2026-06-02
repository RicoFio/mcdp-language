//! Language server entrypoint for MCDPL editor integrations.

mod completion;
mod diagnostics;
mod line_index;
mod project_symbols;
mod semantic_tokens;

use std::collections::HashMap;

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CompletionParams, CompletionResponse, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, Location,
    MarkupContent, MarkupKind, MessageType, OneOf, Position, ReferenceParams, SemanticTokensParams,
    SemanticTokensResult, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

#[derive(Debug, PartialEq, Eq)]
enum StartupMode {
    Server,
    Version,
    Help,
    Error(String),
}

#[derive(Debug)]
struct Backend {
    client: Client,
    documents: RwLock<HashMap<Url, String>>,
    symbols: RwLock<project_symbols::ProjectSymbolIndex>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
            symbols: RwLock::new(project_symbols::ProjectSymbolIndex::default()),
        }
    }

    async fn publish_related_diagnostics(&self, changed_uri: &Url, version: Option<i32>) {
        let documents = {
            let documents = self.documents.read().await;
            documents.clone()
        };
        let index = {
            let symbols = self.symbols.read().await;
            symbols.clone()
        };
        let refresh_uris = index.diagnostic_refresh_uris(changed_uri, documents.keys());
        for uri in refresh_uris {
            let Some(text) = documents.get(&uri) else {
                continue;
            };
            let diagnostics = diagnostics::document_diagnostics(uri.as_str(), text, Some(&index));
            let diagnostic_version = (&uri == changed_uri).then_some(version).flatten();
            self.client
                .publish_diagnostics(uri, diagnostics, diagnostic_version)
                .await;
        }
    }

    async fn refresh_symbol_index(&self, uri: &Url) {
        let documents = {
            let documents = self.documents.read().await;
            documents.clone()
        };
        let index = project_symbols::ProjectSymbolIndex::for_uri(uri, &documents);
        let mut symbols = self.symbols.write().await;
        *symbols = index;
    }

    async fn offset_for_position(&self, uri: &Url, position: Position) -> Option<usize> {
        let documents = self.documents.read().await;
        documents
            .get(uri)
            .map(|source| line_index::LineIndex::new(source).offset(position))
    }

    async fn symbol_index_for(&self, uri: &Url) -> project_symbols::ProjectSymbolIndex {
        let index = {
            let symbols = self.symbols.read().await;
            symbols.clone()
        };
        if index.documents.contains_key(uri) {
            return index;
        }

        let documents = {
            let documents = self.documents.read().await;
            documents.clone()
        };
        project_symbols::ProjectSymbolIndex::for_uri(uri, &documents)
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "mcdp-lsp".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(completion::options()),
                definition_provider: Some(OneOf::Left(true)),
                document_highlight_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                references_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(semantic_tokens::server_capabilities()),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "MCDPL language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = Some(params.text_document.version);
        let text = params.text_document.text;

        {
            let mut documents = self.documents.write().await;
            documents.insert(uri.clone(), text.clone());
        }

        self.refresh_symbol_index(&uri).await;
        self.publish_related_diagnostics(&uri, version).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = Some(params.text_document.version);
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        let text = change.text;

        {
            let mut documents = self.documents.write().await;
            documents.insert(uri.clone(), text.clone());
        }

        self.refresh_symbol_index(&uri).await;
        self.publish_related_diagnostics(&uri, version).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;

        {
            let mut documents = self.documents.write().await;
            documents.remove(&uri);
        }

        self.client
            .publish_diagnostics(uri.clone(), Vec::new(), None)
            .await;
        self.refresh_symbol_index(&uri).await;
        self.publish_related_diagnostics(&uri, None).await;
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let source = {
            let documents = self.documents.read().await;
            documents.get(&uri).cloned()
        };
        let symbols = {
            let symbols = self.symbols.read().await;
            symbols.documents.get(&uri).cloned()
        };

        Ok(source.map(|source| {
            let symbols = symbols.or_else(|| {
                Some(project_symbols::DocumentSymbols::parse(
                    uri.clone(),
                    &source,
                ))
            });
            semantic_tokens::semantic_tokens(&source, symbols.as_ref()).into()
        }))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let source = {
            let documents = self.documents.read().await;
            documents.get(&uri).cloned()
        };
        let index = self.symbol_index_for(&uri).await;

        Ok(source.map(|source| {
            CompletionResponse::Array(completion::items(&uri, &source, position, Some(&index)))
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(offset) = self.offset_for_position(&uri, position).await else {
            return Ok(None);
        };
        let index = self.symbol_index_for(&uri).await;

        let Some(target) = index.definition_at(&uri, offset) else {
            return Ok(None);
        };
        let Some(location) = location_for(&index, target.uri, target.range) else {
            return Ok(None);
        };

        Ok(Some(GotoDefinitionResponse::Scalar(location)))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let Some(offset) = self.offset_for_position(&uri, position).await else {
            return Ok(None);
        };
        let index = self.symbol_index_for(&uri).await;
        let Some(occurrences) =
            index.references_at(&uri, offset, params.context.include_declaration)
        else {
            return Ok(None);
        };
        let locations = occurrences
            .into_iter()
            .filter_map(|occurrence| location_for(&index, occurrence.uri, occurrence.range))
            .collect();

        Ok(Some(locations))
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(offset) = self.offset_for_position(&uri, position).await else {
            return Ok(None);
        };
        let index = self.symbol_index_for(&uri).await;
        let Some(occurrences) = index.document_occurrences_at(&uri, offset) else {
            return Ok(None);
        };
        let highlights = occurrences
            .into_iter()
            .filter_map(|occurrence| document_highlight_for(&index, &occurrence))
            .collect();

        Ok(Some(highlights))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(offset) = self.offset_for_position(&uri, position).await else {
            return Ok(None);
        };
        let index = self.symbol_index_for(&uri).await;
        let Some(hover) = index.hover_at(&uri, offset) else {
            return Ok(None);
        };
        let Some(document) = index.documents.get(&uri) else {
            return Ok(None);
        };
        let range = line_index::LineIndex::new(&document.source).range(hover.range);

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover.contents,
            }),
            range: Some(range),
        }))
    }
}

fn startup_mode(args: &[String]) -> StartupMode {
    let Some(arg) = args.first() else {
        return StartupMode::Server;
    };
    if args.len() > 1 {
        return StartupMode::Error("error: expected at most one argument".to_owned());
    }

    match arg.as_str() {
        "--version" | "-V" => StartupMode::Version,
        "--help" | "-h" => StartupMode::Help,
        other => StartupMode::Error(format!("error: unknown argument `{other}`")),
    }
}

fn version_text() -> String {
    format!("mcdp-lsp {}", env!("CARGO_PKG_VERSION"))
}

fn help_text() -> &'static str {
    "Usage: mcdp-lsp [--version]\n\nOptions:\n  -V, --version  Print version and exit\n  -h, --help     Print help and exit\n"
}

fn location_for(
    index: &project_symbols::ProjectSymbolIndex,
    uri: Url,
    range: mcdp_language::TextRange,
) -> Option<Location> {
    let document = index.documents.get(&uri)?;
    Some(Location {
        uri,
        range: line_index::LineIndex::new(&document.source).range(range),
    })
}

fn document_highlight_for(
    index: &project_symbols::ProjectSymbolIndex,
    occurrence: &project_symbols::SymbolOccurrence,
) -> Option<DocumentHighlight> {
    let document = index.documents.get(&occurrence.uri)?;
    Some(DocumentHighlight {
        range: line_index::LineIndex::new(&document.source).range(occurrence.range),
        kind: Some(match occurrence.kind {
            project_symbols::OccurrenceKind::Text => DocumentHighlightKind::TEXT,
            project_symbols::OccurrenceKind::Read => DocumentHighlightKind::READ,
            project_symbols::OccurrenceKind::Write => DocumentHighlightKind::WRITE,
        }),
    })
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match startup_mode(&args) {
        StartupMode::Server => {}
        StartupMode::Version => {
            println!("{}", version_text());
            return;
        }
        StartupMode::Help => {
            print!("{}", help_text());
            return;
        }
        StartupMode::Error(message) => {
            eprintln!("{message}");
            eprint!("{}", help_text());
            std::process::exit(2);
        }
    }

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);

    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::{StartupMode, startup_mode, version_text};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn starts_server_without_arguments() {
        assert_eq!(startup_mode(&[]), StartupMode::Server);
    }

    #[test]
    fn recognizes_version_flags() {
        assert_eq!(startup_mode(&args(&["--version"])), StartupMode::Version);
        assert_eq!(startup_mode(&args(&["-V"])), StartupMode::Version);
        assert_eq!(
            version_text(),
            format!("mcdp-lsp {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn recognizes_help_flags() {
        assert_eq!(startup_mode(&args(&["--help"])), StartupMode::Help);
        assert_eq!(startup_mode(&args(&["-h"])), StartupMode::Help);
    }

    #[test]
    fn rejects_unknown_or_extra_arguments() {
        assert_eq!(
            startup_mode(&args(&["--bogus"])),
            StartupMode::Error("error: unknown argument `--bogus`".to_owned())
        );
        assert_eq!(
            startup_mode(&args(&["--version", "--help"])),
            StartupMode::Error("error: expected at most one argument".to_owned())
        );
    }
}
