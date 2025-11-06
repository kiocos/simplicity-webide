# LSP Integration Guide

This document explains how to use the LSP client in the Simplicity WebIDE.

## Overview

The LSP (Language Server Protocol) client is integrated into the application and automatically connects to a WebSocket server at `ws://127.0.0.1:9257` on startup.

## Architecture

```
Browser (WASM)                      Native Process
┌──────────────────────┐           ┌──────────────────────┐
│  Leptos App          │           │  LSP Server          │
│  ┌────────────────┐  │           │                      │
│  │  LspClient     │◄─┼───WS─────┤  simplicityhl-lsp    │
│  │  (context)     │  │           │  --websocket         │
│  └────────────────┘  │           │  127.0.0.1:9257      │
│         ▲            │           └──────────────────────┘
│         │            │
│  ┌──────┴─────────┐  │
│  │  Components    │  │
│  │  - Editor      │  │
│  │  - Diagnostics │  │
│  │  - Status      │  │
│  └────────────────┘  │
└──────────────────────┘
```

## Using the LSP Client

### 1. Access the Client from Components

The `LspClient` is provided via Leptos context:

```rust
use leptos::*;
use crate::lsp::LspClient;

#[component]
pub fn MyComponent() -> impl IntoView {
    let lsp_client = use_context::<LspClient>()
        .expect("LSP client should be in context");

    // Use the client...
}
```

### 2. Check Connection Status

```rust
let state = lsp_client.state();

view! {
    <div>
        {move || match state.get() {
            ConnectionState::Initialized => "LSP Ready",
            ConnectionState::Connecting => "Connecting...",
            ConnectionState::Disconnected => "Disconnected",
            ConnectionState::Error(e) => format!("Error: {}", e),
            _ => "Unknown"
        }}
    </div>
}
```

### 3. Open a Document

When loading a file or creating a new document:

```rust
let uri = "file:///example.simf".to_string();
let content = "fn main() {}".to_string();

lsp_client.did_open(uri, content)?;
```

### 4. Update Document on Changes

When the user edits the document:

```rust
// Debounce this! Don't send on every keystroke
let uri = "file:///example.simf".to_string();
let new_content = get_editor_content();

lsp_client.did_change(uri, new_content)?;
```

**Important**: You should debounce document changes (e.g., 300-500ms after last keystroke) to avoid overwhelming the LSP server.

### 5. Display Diagnostics

Use the provided `DiagnosticsList` component:

```rust
use crate::components::lsp_status::DiagnosticsList;

view! {
    <DiagnosticsList uri="file:///example.simf" />
}
```

Or access diagnostics directly:

```rust
let diagnostics = lsp_client.get_diagnostics_for_uri("file:///example.simf");

for diagnostic in diagnostics {
    println!("Line {}: {}",
        diagnostic.range.start.line,
        diagnostic.message
    );
}
```

### 6. Request Completions

```rust
use crate::lsp::Position;

let uri = "file:///example.simf".to_string();
let position = Position {
    line: 0,
    character: 10,
};

lsp_client.request_completion(uri, position)?;
```

**Note**: Currently, completion responses aren't handled yet. You'll need to implement a callback system or use signals to receive completion results.

### 7. Request Hover Information

```rust
let uri = "file:///example.simf".to_string();
let position = Position { line: 0, character: 5 };

lsp_client.request_hover(uri, position)?;
```

### 8. Go to Definition

```rust
let uri = "file:///example.simf".to_string();
let position = Position { line: 5, character: 10 };

lsp_client.request_definition(uri, position)?;
```

## Document URI Scheme

Since this is a web IDE without a real filesystem, you should:

1. Use virtual URIs like `file:///untitled-1.simf`, `file:///my-program.simf`
2. Keep URIs consistent across all LSP calls for the same document
3. Consider using the document's tab ID or a hash as part of the URI

Example:

```rust
fn generate_document_uri(document_id: &str) -> String {
    format!("file:///{}.simf", document_id)
}
```

## Integration Examples

### Editor Integration

```rust
#[component]
pub fn Editor() -> impl IntoView {
    let lsp_client = use_context::<LspClient>().unwrap();
    let (content, set_content) = create_signal(String::new());
    let document_uri = "file:///main.simf".to_string();

    // Open document when component mounts
    create_effect(move |_| {
        let _ = lsp_client.did_open(
            document_uri.clone(),
            content.get()
        );
    });

    // Debounced update on content change
    let debounced_update = create_memo(move |_| {
        let content_value = content.get();
        // TODO: Add actual debouncing here
        let _ = lsp_client.did_change(
            document_uri.clone(),
            content_value
        );
    });

    view! {
        <textarea
            on:input=move |ev| {
                set_content.set(event_target_value(&ev));
            }
            prop:value=content
        />
        <DiagnosticsList uri=document_uri />
    }
}
```

### Status Bar Integration

```rust
use crate::components::lsp_status::LspStatus;

view! {
    <div class="status-bar">
        <LspStatus />
    </div>
}
```

## Styling

Add CSS for diagnostic display:

```css
.diagnostic {
  padding: 8px;
  margin: 4px 0;
  border-left: 3px solid;
}

.diagnostic-error {
  border-color: #f14c4c;
  background: rgba(241, 76, 76, 0.1);
}

.diagnostic-warning {
  border-color: #cca700;
  background: rgba(204, 167, 0, 0.1);
}

.diagnostic-info {
  border-color: #75beff;
  background: rgba(117, 190, 255, 0.1);
}

.diagnostic-hint {
  border-color: #d4d4d4;
  background: rgba(212, 212, 212, 0.1);
}

.lsp-status {
  padding: 4px 8px;
  font-size: 12px;
}

.status-initialized {
  color: #4caf50;
}

.status-error {
  color: #f44336;
}

.status-connecting {
  color: #ff9800;
}
```

## Debugging

### Enable LSP Logging

In your browser console:

```javascript
localStorage.setItem("RUST_LOG", "debug");
```

Then reload the page. You'll see LSP messages in the console.

### Check Connection

```javascript
// In browser console
const ws = new WebSocket("ws://127.0.0.1:9257");
ws.onopen = () => console.log("Connected!");
ws.onerror = (e) => console.error("Error:", e);
```

### Common Issues

1. **"Failed to connect to LSP server"**

   - Make sure the LSP server is running: `cargo run --bin simplicityhl-lsp -- --websocket`
   - Check that it's listening on `127.0.0.1:9257`
   - Check browser security settings (WebSocket might be blocked)

2. **"No diagnostics showing"**

   - Check that you called `did_open` with the document
   - Verify the URI matches in `did_open` and `DiagnosticsList`
   - Check browser console for errors

3. **"Diagnostics not updating"**
   - Make sure you're calling `did_change` after document edits
   - Check that the document version is incrementing
   - Verify the LSP server is actually running

## Next Steps

### Features to Implement

1. **Completion UI**: Create a popup that displays completions from `request_completion`
2. **Hover Tooltips**: Show type information on hover
3. **Go to Definition**: Jump to symbol definitions
4. **Request Response Tracking**: Implement a callback system to handle LSP responses
5. **Debouncing**: Add proper debouncing for `did_change` events
6. **Reconnection**: Auto-reconnect if connection drops
7. **Multiple Documents**: Handle multiple open documents with different URIs

### Response Handling

Currently, LSP responses (for completions, hover, etc.) are logged but not processed. To handle them:

1. Add a response callback system to `LspClient`
2. Use signals to communicate results back to components
3. Implement UI components to display the results

Example structure:

```rust
// In LspClient
type CompletionCallback = Box<dyn Fn(Vec<CompletionItem>)>;
pending_completions: RwSignal<HashMap<i32, CompletionCallback>>,

// When sending request
let callback_id = self.next_id();
self.pending_completions.update(|pending| {
    pending.insert(callback_id, Box::new(move |items| {
        // Update UI signal with completion items
    }));
});

// When receiving response
if let Some(callback) = self.pending_completions.get().remove(&response.id) {
    let items: Vec<CompletionItem> = serde_json::from_value(response.result)?;
    callback(items);
}
```

## Resources

- [LSP Specification](https://microsoft.github.io/language-server-protocol/)
- [simplicityhl-lsp Documentation](/home/kio/simplicityhl-lsp)
- [Leptos Context Documentation](https://leptos-rs.github.io/leptos/context.html)
