use super::types::*;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::SinkExt;
use gloo_net::websocket::futures::WebSocket;
use gloo_net::websocket::Message;
use leptos::*;
use serde_json::Value;
use std::collections::HashMap;
use wasm_bindgen_futures::spawn_local;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MCPConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Debug, Clone)]
pub enum MCPClientError {
    NotConnected,
    WebSocketError(String),
    SerializationError(String),
    RequestError(String),
}

impl std::fmt::Display for MCPClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MCPClientError::NotConnected => write!(f, "MCP client is not connected"),
            MCPClientError::WebSocketError(e) => write!(f, "WebSocket error: {}", e),
            MCPClientError::SerializationError(e) => write!(f, "Serialization error: {}", e),
            MCPClientError::RequestError(e) => write!(f, "Request error: {}", e),
        }
    }
}

#[derive(Clone)]
pub struct MCPClient {
    state: RwSignal<MCPConnectionState>,
    sender: RwSignal<Option<UnboundedSender<String>>>,
    next_id: RwSignal<i32>,
    pending_requests: RwSignal<HashMap<i32, PendingRequest>>,
    tools: RwSignal<Vec<MCPTool>>,
    chat_responses: RwSignal<HashMap<i32, ChatResponse>>,
}

#[derive(Debug, Clone)]
enum PendingRequest {
    ListTools,
    CallTool,
    Chat,
}

impl MCPClient {
    pub fn new() -> Self {
        Self {
            state: create_rw_signal(MCPConnectionState::Disconnected),
            sender: create_rw_signal(None),
            next_id: create_rw_signal(1),
            pending_requests: create_rw_signal(HashMap::new()),
            tools: create_rw_signal(Vec::new()),
            chat_responses: create_rw_signal(HashMap::new()),
        }
    }

    pub fn state(&self) -> ReadSignal<MCPConnectionState> {
        self.state.read_only()
    }

    pub fn tools(&self) -> ReadSignal<Vec<MCPTool>> {
        self.tools.read_only()
    }

    pub async fn connect(&self, url: &str) -> Result<(), MCPClientError> {
        log::info!("Connecting to MCP bridge at: {}", url);
        self.state.set(MCPConnectionState::Connecting);

        let ws = WebSocket::open(url)
            .map_err(|e| MCPClientError::WebSocketError(format!("{:?}", e)))?;

        let (tx, rx) = unbounded();
        self.sender.set(Some(tx));
        self.state.set(MCPConnectionState::Connected);
        log::info!("WebSocket connection established");

        let client = self.clone();
        client.run_websocket_loop(ws, rx).await?;

        // List tools on connect
        let _ = self.list_tools().await;

        Ok(())
    }

    async fn run_websocket_loop(
        &self,
        ws: WebSocket,
        rx: UnboundedReceiver<String>,
    ) -> Result<(), MCPClientError> {
        use futures::stream::StreamExt as _;

        let (mut sink, mut stream) = ws.split();

        let client = self.clone();
        spawn_local(async move {
            log::info!("WebSocket receiver task started");
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Err(e) = client.handle_message(&text).await {
                            log::error!("Error handling MCP message: {:?}", e);
                        }
                    }
                    Ok(Message::Bytes(_)) => {
                        log::warn!("Received unexpected binary message from MCP bridge");
                    }
                    Err(e) => {
                        log::error!("WebSocket receiver error: {:?}", e);
                        client.state.set(MCPConnectionState::Error(format!("{:?}", e)));
                        break;
                    }
                }
            }
            log::warn!("WebSocket receiver task ended");
            client.state.set(MCPConnectionState::Disconnected);
            client.sender.set(None);
        });

        spawn_local(async move {
            log::info!("WebSocket sender task started");
            let mut rx = rx;
            while let Some(text) = rx.next().await {
                log::debug!("Sending via WebSocket: {}", text);
                if let Err(e) = sink.send(Message::Text(text)).await {
                    log::error!("Error sending via WebSocket: {:?}", e);
                    break;
                }
            }
            log::warn!("WebSocket sender task ended");
        });

        Ok(())
    }

    async fn handle_message(&self, text: &str) -> Result<(), MCPClientError> {
        log::debug!("Received MCP message: {}", text);

        let response: MCPResponse = serde_json::from_str(text)
            .map_err(|e| MCPClientError::SerializationError(e.to_string()))?;

        let request_type = self
            .pending_requests
            .with_untracked(|requests| requests.get(&response.id).cloned());

        if let Some(request_type) = request_type {
            match request_type {
                PendingRequest::ListTools => {
                    if let Some(result) = response.result {
                        if let Ok(list_result) = serde_json::from_value::<ListToolsResult>(result) {
                            let tools_count = list_result.tools.len();
                            self.tools.set(list_result.tools);
                            log::info!("Received {} tools", tools_count);
                        }
                    }
                }
                PendingRequest::CallTool => {
                    log::info!("Tool call response received");
                }
                PendingRequest::Chat => {
                    if let Some(result) = response.result {
                        if let Ok(chat_response) = serde_json::from_value::<ChatResponse>(result) {
                            self.chat_responses.update(|responses| {
                                responses.insert(response.id, chat_response);
                            });
                            log::info!("Chat response received");
                        }
                    }
                }
            }

            self.pending_requests.update(|requests| {
                requests.remove(&response.id);
            });
        }

        Ok(())
    }

    fn send_message(&self, message: &MCPRequest) -> Result<(), MCPClientError> {
        let sender = self
            .sender
            .get()
            .ok_or(MCPClientError::NotConnected)?;

        let text = serde_json::to_string(message)
            .map_err(|e| MCPClientError::SerializationError(e.to_string()))?;

        sender
            .unbounded_send(text)
            .map_err(|_| MCPClientError::WebSocketError("Failed to send message".to_string()))?;

        Ok(())
    }

    fn next_id(&self) -> i32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    pub async fn list_tools(&self) -> Result<(), MCPClientError> {
        let request = MCPRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "tools/list".to_string(),
            params: None,
        };

        let request_id = request.id;
        self.pending_requests.update(|requests| {
            requests.insert(request_id, PendingRequest::ListTools);
        });

        self.send_message(&request)?;
        Ok(())
    }

    pub async fn call_tool(
        &self,
        name: String,
        arguments: Value,
    ) -> Result<Value, MCPClientError> {
        let request = MCPRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": name,
                "arguments": arguments,
            })),
        };

        let request_id = request.id;
        self.pending_requests.update(|requests| {
            requests.insert(request_id, PendingRequest::CallTool);
        });

        self.send_message(&request)?;

        // Wait for response (simplified - in production you'd use a proper async channel)
        gloo_timers::future::TimeoutFuture::new(5000).await;

        // For now, return empty - proper implementation would use channels
        Ok(serde_json::json!({}))
    }

    pub fn chat_responses(&self) -> ReadSignal<HashMap<i32, ChatResponse>> {
        self.chat_responses.read_only()
    }

    pub async fn send_chat(
        &self,
        messages: Vec<ChatMessage>,
        program: Option<String>,
    ) -> Result<i32, MCPClientError> {
        let request = MCPRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "chat".to_string(),
            params: Some(serde_json::json!({
                "messages": messages,
                "program": program,
            })),
        };

        let request_id = request.id;
        self.pending_requests.update(|requests| {
            requests.insert(request_id, PendingRequest::Chat);
        });

        self.send_message(&request)?;
        Ok(request_id)
    }

    pub fn get_chat_response(&self, request_id: i32) -> Option<ChatResponse> {
        self.chat_responses.with(|responses| {
            responses.get(&request_id).cloned()
        })
    }
}

