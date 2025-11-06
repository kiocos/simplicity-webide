use super::types::*;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::{SinkExt, StreamExt};
use gloo_net::websocket::futures::WebSocket;
use gloo_net::websocket::Message;
use leptos::*;
use std::collections::HashMap;
use wasm_bindgen_futures::spawn_local;

#[derive(Debug, Clone)]
enum PendingRequest {
    Completion,
    #[allow(dead_code)]
    Hover,
    #[allow(dead_code)]
    Definition,
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
        }
    }

    pub fn state(&self) -> ReadSignal<ConnectionState> {
        self.state.read_only()
    }

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
                                log::debug!("Parsing completion result: {:?}", result);
                                match serde_json::from_value::<Vec<CompletionItem>>(result.clone())
                                {
                                    Ok(items) => {
                                        log::info!(
                                            "✅ Successfully parsed {} completion items",
                                            items.len()
                                        );
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
                        PendingRequest::Hover => {
                            if let Some(result) = response.result {
                                log::debug!("Parsing hover result: {:?}", result);
                                match serde_json::from_value::<Hover>(result) {
                                    Ok(hover) => {
                                        log::info!("✅ Successfully parsed hover info");
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
                } else {
                    if let Some(result) = &response.result {
                        log::debug!("Response result: {:?}", result);
                    }
                }

                if let Some(error) = &response.error {
                    log::error!("LSP error response: {:?}", error);
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

                log::debug!("Updated diagnostics for: {}", params.uri);
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

        log::debug!("Sending LSP message: {}", json);

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
        log::debug!("did_open called, current state: {:?}", state);
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
                    content,
                },
            );
        });

        log::debug!("Opened document: {}", uri);

        Ok(())
    }

    pub fn did_change(&self, uri: String, content: String) -> Result<(), LspClientError> {
        let state = self.state.get_untracked();
        log::debug!("did_change called, current state: {:?}", state);
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

        log::debug!("Changed document: {} (version {})", uri, version);

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

        log::debug!("Requesting completion at position {:?}", position);
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

        log::debug!("Requesting hover at position {:?}", position);
        self.send_message(&request)?;

        Ok(())
    }

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
