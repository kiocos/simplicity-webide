use super::types::*;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::{SinkExt, StreamExt};
use gloo_net::websocket::futures::WebSocket;
use gloo_net::websocket::Message;
use leptos::*;
use std::collections::HashMap;
use wasm_bindgen_futures::spawn_local;

// For compatibility with different LSP encodings of semantic tokens
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SemanticTokenObj {
    delta_line: u32,
    delta_start: u32,
    length: u32,
    token_type: u32,
    token_modifiers_bitset: u32,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SemanticTokensObj {
    #[allow(dead_code)]
    result_id: Option<String>,
    data: Vec<SemanticTokenObj>,
}

#[derive(Debug, Clone)]
enum PendingRequest {
    Completion,
    #[allow(dead_code)]
    Hover,
    #[allow(dead_code)]
    Definition,
    SemanticTokens(String), // uri
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Initialized,
    Error(String),
}

#[derive(Debug, Clone)]
pub enum LspClientError {
    NotConnected,
    NotInitialized,
    WebSocketError(String),
    SerializationError(String),
    #[allow(dead_code)]
    RequestError(String),
}

impl std::fmt::Display for LspClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LspClientError::NotConnected => write!(f, "LSP client is not connected"),
            LspClientError::NotInitialized => write!(f, "LSP client is not initialized"),
            LspClientError::WebSocketError(e) => write!(f, "WebSocket error: {}", e),
            LspClientError::SerializationError(e) => write!(f, "Serialization error: {}", e),
            LspClientError::RequestError(e) => write!(f, "Request error: {}", e),
        }
    }
}

#[derive(Clone)]
pub struct LspClient {
    sender: RwSignal<Option<UnboundedSender<String>>>,
    next_id: RwSignal<i32>,
    state: RwSignal<ConnectionState>,
    #[allow(dead_code)]
    server_capabilities: RwSignal<Option<ServerCapabilities>>,
    diagnostics: RwSignal<HashMap<String, Vec<Diagnostic>>>,
    documents: RwSignal<HashMap<String, DocumentState>>,
    completions: RwSignal<Vec<CompletionItem>>,
    hover_info: RwSignal<Option<Hover>>,
    pending_requests: RwSignal<HashMap<i32, PendingRequest>>,
    semantic_html: RwSignal<HashMap<String, String>>, // uri -> html overlay
}

#[derive(Debug, Clone)]
struct DocumentState {
    version: i32,
    #[allow(dead_code)]
    content: String,
}

impl LspClient {
    pub fn new() -> Self {
        Self {
            sender: create_rw_signal(None),
            next_id: create_rw_signal(1),
            state: create_rw_signal(ConnectionState::Disconnected),
            server_capabilities: create_rw_signal(None),
            diagnostics: create_rw_signal(HashMap::new()),
            documents: create_rw_signal(HashMap::new()),
            completions: create_rw_signal(Vec::new()),
            hover_info: create_rw_signal(None),
            pending_requests: create_rw_signal(HashMap::new()),
            semantic_html: create_rw_signal(HashMap::new()),
        }
    }

    pub fn state(&self) -> ReadSignal<ConnectionState> {
        self.state.read_only()
    }

    #[allow(dead_code)]
    pub fn diagnostics(&self) -> ReadSignal<HashMap<String, Vec<Diagnostic>>> {
        self.diagnostics.read_only()
    }

    pub fn get_diagnostics_for_uri(&self, uri: &str) -> Vec<Diagnostic> {
        // This is called from reactive contexts, so .get() is correct here
        self.diagnostics.get().get(uri).cloned().unwrap_or_default()
    }

    pub fn completions(&self) -> ReadSignal<Vec<CompletionItem>> {
        self.completions.read_only()
    }

    pub fn hover_info(&self) -> ReadSignal<Option<Hover>> {
        self.hover_info.read_only()
    }

    pub fn semantic_html(&self) -> ReadSignal<HashMap<String, String>> {
        self.semantic_html.read_only()
    }

    pub async fn connect(&self, url: &str) -> Result<(), LspClientError> {
        log::info!("Connecting to LSP server at: {}", url);
        self.state.set(ConnectionState::Connecting);

        let ws =
            WebSocket::open(url).map_err(|e| LspClientError::WebSocketError(format!("{:?}", e)))?;

        let (tx, rx) = unbounded();
        self.sender.set(Some(tx));
        self.state.set(ConnectionState::Connected);
        log::info!("WebSocket connection established");

        // Spawn WebSocket I/O tasks
        let client = self.clone();
        client.run_websocket_loop(ws, rx).await?;

        // Initialize the LSP connection
        self.initialize().await?;

        Ok(())
    }

    async fn run_websocket_loop(
        &self,
        ws: WebSocket,
        rx: UnboundedReceiver<String>,
    ) -> Result<(), LspClientError> {
        use futures::stream::StreamExt as _;

        // Split the WebSocket into sink and stream
        let (mut sink, mut stream) = ws.split();

        // Spawn receiver task
        let client = self.clone();
        spawn_local(async move {
            log::info!("WebSocket receiver task started");
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Err(e) = client.handle_message(&text).await {
                            log::error!("Error handling LSP message: {:?}", e);
                        }
                    }
                    Ok(Message::Bytes(_)) => {
                        log::warn!("Received unexpected binary message from LSP server");
                    }
                    Err(e) => {
                        log::error!("WebSocket receiver error: {:?}", e);
                        client.state.set(ConnectionState::Error(format!("{:?}", e)));
                        break;
                    }
                }
            }
            log::warn!("WebSocket receiver task ended");
            client.state.set(ConnectionState::Disconnected);
            client.sender.set(None);
        });

        // Spawn sender task
        spawn_local(async move {
            log::info!("WebSocket sender task started");
            let mut rx = rx;
            while let Some(text) = rx.next().await {
                log::debug!("Sending via WebSocket: {}", text);
                if let Err(e) = sink.send(Message::Text(text)).await {
                    log::error!("Error sending via WebSocket: {:?}", e);
                    break;
                } else {
                    log::debug!("Message sent via WebSocket successfully");
                }
            }
            log::warn!("WebSocket sender task ended - channel closed");
        });

        Ok(())
    }

    async fn handle_message(&self, text: &str) -> Result<(), LspClientError> {
        log::debug!("Received LSP message: {}", text);

        let message: JsonRpcMessage = serde_json::from_str(text)
            .map_err(|e| LspClientError::SerializationError(e.to_string()))?;

        match message {
            JsonRpcMessage::Notification(notification) => {
                self.handle_notification(notification).await?;
            }
            JsonRpcMessage::Response(response) => {
                log::info!("Received LSP response for request ID {}", response.id);

                // Check what kind of request this was a response to
                let request_type = self
                    .pending_requests
                    .with_untracked(|requests| requests.get(&response.id).cloned());

                if let Some(request_type) = request_type {
                    match request_type {
                        PendingRequest::Completion => {
                            if let Some(result) = response.result {
                                match serde_json::from_value::<Vec<CompletionItem>>(result.clone())
                                {
                                    Ok(items) => {
                                        for item in items.iter().take(3) {
                                            log::debug!("  - {}", item.label);
                                        }
                                        self.completions.set(items);
                                    }
                                    Err(e) => {
                                        log::warn!("Failed to parse as array: {}", e);
                                        // Try parsing as a single completion list object
                                        if let Ok(list) =
                                            serde_json::from_value::<serde_json::Value>(result)
                                        {
                                            if let Some(items_array) = list.get("items") {
                                                if let Ok(items) =
                                                    serde_json::from_value::<Vec<CompletionItem>>(
                                                        items_array.clone(),
                                                    )
                                                {
                                                    log::info!(
                                                        "✅ Successfully parsed {} completion items from list",
                                                        items.len()
                                                    );
                                                    self.completions.set(items);
                                                } else {
                                                    log::error!("Failed to parse items array");
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                log::warn!("Completion response has no result");
                            }
                        }
                        PendingRequest::SemanticTokens(uri) => {
                            if let Some(result) = &response.result {
                                if let Ok(tokens) = serde_json::from_value::<SemanticTokens>(result.clone()) {
                                    if let Some(doc) = self.documents.get_untracked().get(&uri) {
                                        let html = build_html_from_semantic_tokens(&doc.content, &tokens);
                                        self.semantic_html.update(|m| { m.insert(uri.clone(), html); });
                                    }
                                } else if let Ok(obj) = serde_json::from_value::<SemanticTokensObj>(result.clone()) {
                                    let mut flat: Vec<u32> = Vec::with_capacity(obj.data.len() * 5);
                                    for t in obj.data {
                                        flat.push(t.delta_line);
                                        flat.push(t.delta_start);
                                        flat.push(t.length);
                                        flat.push(t.token_type);
                                        flat.push(t.token_modifiers_bitset);
                                    }
                                    let tokens = SemanticTokens { result_id: None, data: flat };
                                    if let Some(doc) = self.documents.get_untracked().get(&uri) {
                                        let html = build_html_from_semantic_tokens(&doc.content, &tokens);
                                        self.semantic_html.update(|m| { m.insert(uri.clone(), html); });
                                    }
                                } else {
                                    log::warn!("Unexpected semantic tokens result shape: {:?}", result);
                                }
                            }
                        }
                        PendingRequest::Hover => {
                            if let Some(result) = response.result {
                                match serde_json::from_value::<Hover>(result) {
                                    Ok(hover) => {
                                        self.hover_info.set(Some(hover));
                                    }
                                    Err(e) => {
                                        log::warn!("Failed to parse hover: {}", e);
                                        self.hover_info.set(None);
                                    }
                                }
                            } else {
                                log::debug!(
                                    "Hover response has no result (no hover info at this position)"
                                );
                                self.hover_info.set(None);
                            }
                        }
                        _ => {
                            log::debug!("Response result: {:?}", response.result);
                        }
                    }

                    // Remove from pending requests
                    self.pending_requests.update(|requests| {
                        requests.remove(&response.id);
                    });
                }
            }
            JsonRpcMessage::Request(_) => {
                log::warn!("Received unexpected request from server");
            }
        }

        Ok(())
    }

    async fn handle_notification(
        &self,
        notification: JsonRpcNotification,
    ) -> Result<(), LspClientError> {
        match notification.method.as_str() {
            "textDocument/publishDiagnostics" => {
                let params: PublishDiagnosticsParams = serde_json::from_value(notification.params)
                    .map_err(|e| LspClientError::SerializationError(e.to_string()))?;

                self.diagnostics.update(|diagnostics| {
                    diagnostics.insert(params.uri.clone(), params.diagnostics);
                });

            }
            _ => {
                log::debug!("Unhandled notification: {}", notification.method);
            }
        }

        Ok(())
    }

    fn send_message(&self, message: &JsonRpcMessage) -> Result<(), LspClientError> {
        let sender = self
            .sender
            .get_untracked()
            .ok_or(LspClientError::NotConnected)?;

        let json = serde_json::to_string(message)
            .map_err(|e| LspClientError::SerializationError(e.to_string()))?;

        sender
            .unbounded_send(json)
            .map_err(|e| LspClientError::WebSocketError(format!("{:?}", e)))?;

        Ok(())
    }

    fn next_id(&self) -> i32 {
        let id = self.next_id.get_untracked();
        self.next_id.set(id + 1);
        id
    }

    async fn initialize(&self) -> Result<(), LspClientError> {
        log::info!("Starting LSP initialization handshake");

        let params = InitializeParams {
            process_id: None,
            client_info: Some(ClientInfo {
                name: "Simplicity WebIDE".to_string(),
                version: "0.1.0".to_string(),
            }),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    completion: Some(CompletionCapability {
                        dynamic_registration: false,
                    }),
                    hover: Some(HoverCapability {
                        dynamic_registration: false,
                    }),
                    definition: Some(DefinitionCapability {
                        dynamic_registration: false,
                    }),
                }),
            },
            root_uri: None,
        };

        let request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "initialize".to_string(),
            params: serde_json::to_value(params)
                .map_err(|e| LspClientError::SerializationError(e.to_string()))?,
        });

        log::info!("Sending initialize request to LSP server");
        self.send_message(&request)?;

        // Wait longer for the server to respond (increase from 100ms to 500ms)
        log::info!("Waiting for server response...");
        gloo_timers::future::TimeoutFuture::new(500).await;

        // Send initialized notification
        let notification = JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "initialized".to_string(),
            params: serde_json::json!({}),
        });

        log::info!("Sending initialized notification");
        self.send_message(&notification)?;

        self.state.set(ConnectionState::Initialized);

        log::info!("✅ LSP client fully initialized and ready!");

        Ok(())
    }

    pub fn did_open(&self, uri: String, content: String) -> Result<(), LspClientError> {
        let state = self.state.get_untracked();
        if state != ConnectionState::Initialized {
            log::warn!(
                "Cannot open document - LSP state is {:?}, not Initialized",
                state
            );
            return Err(LspClientError::NotInitialized);
        }

        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "simplicityhl".to_string(),
                version: 1,
                text: content.clone(),
            },
        };

        let notification = JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "textDocument/didOpen".to_string(),
            params: serde_json::to_value(params)
                .map_err(|e| LspClientError::SerializationError(e.to_string()))?,
        });

        self.send_message(&notification)?;

        // Track document state
        self.documents.update(|docs| {
            docs.insert(
                uri.clone(),
                DocumentState {
                    version: 1,
                    content: content.clone(),
                },
            );
        });

        // Request initial semantic tokens
        let _ = self.request_semantic_tokens(uri);

        Ok(())
    }

    pub fn did_change(&self, uri: String, content: String) -> Result<(), LspClientError> {
        let state = self.state.get_untracked();
        if state != ConnectionState::Initialized {
            log::warn!(
                "Cannot update document - LSP state is {:?}, not Initialized",
                state
            );
            return Err(LspClientError::NotInitialized);
        }

        let version = self
            .documents
            .with_untracked(|docs| docs.get(&uri).map(|doc| doc.version + 1).unwrap_or(1));

        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                text: content.clone(),
            }],
        };

        let notification = JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "textDocument/didChange".to_string(),
            params: serde_json::to_value(params)
                .map_err(|e| LspClientError::SerializationError(e.to_string()))?,
        });

        self.send_message(&notification)?;

        // Update document state
        self.documents.update(|docs| {
            docs.insert(uri.clone(), DocumentState { version, content });
        });


        // Request updated semantic tokens
        let _ = self.request_semantic_tokens(uri);

        Ok(())
    }

    pub fn request_completion(
        &self,
        uri: String,
        position: Position,
    ) -> Result<(), LspClientError> {
        if self.state.get_untracked() != ConnectionState::Initialized {
            return Err(LspClientError::NotInitialized);
        }

        let params = CompletionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        };

        let request_id = self.next_id();
        let request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: request_id,
            method: "textDocument/completion".to_string(),
            params: serde_json::to_value(params)
                .map_err(|e| LspClientError::SerializationError(e.to_string()))?,
        });

        // Track this pending request
        self.pending_requests.update(|requests| {
            requests.insert(request_id, PendingRequest::Completion);
        });

        self.send_message(&request)?;

        Ok(())
    }

    pub fn request_semantic_tokens(&self, uri: String) -> Result<(), LspClientError> {
        if self.state.get_untracked() != ConnectionState::Initialized {
            return Err(LspClientError::NotInitialized);
        }

        let params = SemanticTokensParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
        };

        let request_id = self.next_id();
        let request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: request_id,
            method: "textDocument/semanticTokens/full".to_string(),
            params: serde_json::to_value(params)
                .map_err(|e| LspClientError::SerializationError(e.to_string()))?,
        });

        self.pending_requests
            .update(|r| {
                r.insert(request_id, PendingRequest::SemanticTokens(uri.clone()));
            });

        self.send_message(&request)?;
        Ok(())
    }

    pub fn request_hover(&self, uri: String, position: Position) -> Result<(), LspClientError> {
        if self.state.get_untracked() != ConnectionState::Initialized {
            return Err(LspClientError::NotInitialized);
        }

        let params = HoverParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        };

        let request_id = self.next_id();
        let request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: request_id,
            method: "textDocument/hover".to_string(),
            params: serde_json::to_value(params)
                .map_err(|e| LspClientError::SerializationError(e.to_string()))?,
        });

        // Track this pending request
        self.pending_requests.update(|requests| {
            requests.insert(request_id, PendingRequest::Hover);
        });

        self.send_message(&request)?;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn request_definition(
        &self,
        uri: String,
        position: Position,
    ) -> Result<(), LspClientError> {
        if self.state.get_untracked() != ConnectionState::Initialized {
            return Err(LspClientError::NotInitialized);
        }

        let params = DefinitionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        };

        let request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "textDocument/definition".to_string(),
            params: serde_json::to_value(params)
                .map_err(|e| LspClientError::SerializationError(e.to_string()))?,
        });

        self.send_message(&request)?;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn disconnect(&self) {
        self.sender.set(None);
        self.state.set(ConnectionState::Disconnected);
        self.documents.update(|docs| docs.clear());
        log::info!("LSP client disconnected");
    }
}

impl Default for LspClient {
    fn default() -> Self {
        Self::new()
    }
}

// ----------------------------------------------------------------------------
// Semantic tokens HTML generator (keeps in sync with server legend ordering)
// ----------------------------------------------------------------------------

// Map token type index to CSS class used by the overlay
fn token_type_to_class(idx: u32) -> &'static str {
    match idx {
        0 => "hl-keyword",
        1 => "hl-string",
        2 => "hl-comment",
        3 => "hl-number",
        4 => "hl-function",
        5 => "hl-operator",
        6 => "hl-namespace",
        _ => "",
    }
}

fn escape_html_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

fn build_html_from_semantic_tokens(text: &str, tokens: &SemanticTokens) -> String {
    // Decode delta-encoded tokens into absolute (line, start, len, type)
    let mut decoded: Vec<(u32, u32, u32, u32)> = Vec::with_capacity(tokens.data.len() / 5);
    let mut line = 0u32;
    let mut col = 0u32;
    let mut i = 0usize;
    let d = &tokens.data;
    while i + 4 < d.len() {
        let delta_line = d[i];
        let delta_start = d[i + 1];
        let length = d[i + 2];
        let token_type = d[i + 3];
        // let modifiers = d[i + 4];
        if delta_line > 0 {
            line += delta_line;
            col = 0;
        }
        col = col.saturating_add(delta_start);
        decoded.push((line, col, length, token_type));
        i += 5;
    }

    // Group tokens by line
    let mut by_line: std::collections::HashMap<u32, Vec<(u32, u32, u32)>> = std::collections::HashMap::new();
    for (l, s, len, ty) in decoded {
        by_line.entry(l).or_default().push((s, len, ty));
    }
    for vec in by_line.values_mut() {
        vec.sort_by_key(|(s, _len, _ty)| *s);
    }

    let mut result = String::with_capacity(text.len() * 2);
    crate::logging::log_event(
        "LSP_TOKENS",
        serde_json::json!({
            "text_lines": text.lines().count(),
            "token_lines": by_line.len(),
        }),
    );
    // Use lines() to avoid producing a spurious trailing empty line entry
    let lines: Vec<&str> = text.lines().collect();
    let last_idx = lines.len().saturating_sub(1);
    for (line_idx, line_text) in lines.iter().enumerate() {
        let mut cursor: usize = 0;
        let line_u32 = line_idx as u32;
        if let Some(toks) = by_line.get(&line_u32) {
            for (start, len, ty) in toks.iter().copied() {
                let start_usize = start as usize;
                let end_usize = start_usize.saturating_add(len as usize);
                if start_usize > cursor && start_usize <= line_text.len() {
                    result.push_str(&escape_html_text(&line_text[cursor..start_usize]));
                }
                if start_usize < line_text.len() {
                    let end = end_usize.min(line_text.len());
                    let class = token_type_to_class(ty);
                    if !class.is_empty() {
                        result.push_str("<span class=\"");
                        result.push_str(class);
                        result.push_str("\">");
                        result.push_str(&escape_html_text(&line_text[start_usize..end]));
                        result.push_str("</span>");
                    } else {
                        result.push_str(&escape_html_text(&line_text[start_usize..end]));
                    }
                    cursor = end;
                }
            }
        }
        if cursor < line_text.len() {
            result.push_str(&escape_html_text(&line_text[cursor..]));
        }
        if line_idx < last_idx {
            result.push('\n');
        }
    }

    // Preserve all trailing newlines exactly as in the source text
    let trailing_nl = text.chars().rev().take_while(|&c| c == '\n').count();
    for _ in 0..trailing_nl {
        result.push('\n');
    }

    result
}
