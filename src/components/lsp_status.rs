use leptos::*;

use crate::lsp::{ConnectionState, LspClient};

/// A simple component to display LSP connection status
#[component]
pub fn LspStatus() -> impl IntoView {
    let lsp_client = use_context::<LspClient>().expect("LSP client should be in context");
    let state = lsp_client.state();

    view! {
        <div class="lsp-status">
            {move || {
                match state.get() {
                    ConnectionState::Disconnected => {
                        view! { <span class="status-disconnected">"LSP: Disconnected"</span> }
                    }
                    ConnectionState::Connecting => {
                        view! { <span class="status-connecting">"LSP: Connecting..."</span> }
                    }
                    ConnectionState::Connected => {
                        view! { <span class="status-connected">"LSP: Connected"</span> }
                    }
                    ConnectionState::Initialized => {
                        view! { <span class="status-initialized">"LSP: Ready"</span> }
                    }
                    ConnectionState::Error(ref e) => {
                        view! {
                            <span class="status-error">{format!("LSP Error: {}", e)}</span>
                        }
                    }
                }
            }}
        </div>
    }
}

/// A component to display diagnostics for a specific document URI
#[component]
pub fn DiagnosticsList(#[prop(into)] uri: String) -> impl IntoView {
    let lsp_client = use_context::<LspClient>().expect("LSP client should be in context");

    // Create a memo that tracks diagnostics for this URI
    let diagnostics = create_memo(move |_| lsp_client.get_diagnostics_for_uri(&uri));

    view! {
        <div class="diagnostics-list">
            <For
                each=move || diagnostics.get()
                key=|diag| format!("{:?}:{:?}:{}", diag.range.start.line, diag.range.start.character, diag.message)
                children=move |diagnostic| {
                    let severity_class = match diagnostic.severity {
                        Some(crate::lsp::DiagnosticSeverity::Error) => "diagnostic-error",
                        Some(crate::lsp::DiagnosticSeverity::Warning) => "diagnostic-warning",
                        Some(crate::lsp::DiagnosticSeverity::Information) => "diagnostic-info",
                        Some(crate::lsp::DiagnosticSeverity::Hint) => "diagnostic-hint",
                        None => "diagnostic-default",
                    };

                    // Format severity as a string for display/logging
                    let severity_text = match diagnostic.severity {
                        Some(crate::lsp::DiagnosticSeverity::Error) => "Error",
                        Some(crate::lsp::DiagnosticSeverity::Warning) => "Warning",
                        Some(crate::lsp::DiagnosticSeverity::Information) => "Info",
                        Some(crate::lsp::DiagnosticSeverity::Hint) => "Hint",
                        None => "Unknown",
                    };

                    // Log the diagnostic with severity
                    log::debug!("Diagnostic [{}]: Line {} - {}",
                        severity_text,
                        diagnostic.range.start.line + 1,
                        diagnostic.message
                    );

                    view! {
                        <div class=format!("diagnostic {}", severity_class)>
                            <span class="diagnostic-location">
                                {format!("Line {}:{} [{}]", diagnostic.range.start.line + 1, diagnostic.range.start.character + 1, severity_text)}
                            </span>
                            <span class="diagnostic-message">{diagnostic.message.clone()}</span>
                        </div>
                    }
                }
            />
        </div>
    }
}
