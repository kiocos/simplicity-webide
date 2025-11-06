use leptos::*;
// no extra imports needed

use crate::logging;

use crate::lsp::{ConnectionState, LspClient};

/// Debug component to show LSP connection status
#[component]
pub fn LspDebugStatus() -> impl IntoView {
    let lsp_client = use_context::<LspClient>();

    view! {
        <div style="position: fixed; top: 10px; right: 10px; background: #1a1a1a; border: 1px solid #444; padding: 10px; border-radius: 4px; z-index: 9999; font-size: 12px; min-width: 220px;">
            {move || {
                if let Some(lsp) = lsp_client.as_ref() {
                    let state = lsp.state();
                    let state_val = state.get();
                    let (color, text) = match state_val {
                        ConnectionState::Disconnected => ("#888", "⚫ Disconnected"),
                        ConnectionState::Connecting => ("#ff9800", "🟡 Connecting..."),
                        ConnectionState::Connected => ("#2196f3", "🔵 Connected"),
                        ConnectionState::Initialized => ("#4caf50", "🟢 Initialized ✓"),
                        ConnectionState::Error(_) => ("#f44336", "🔴 Error"),
                    };

                    view! {
                        <div>
                            <div style=format!("color: {}", color)>
                                <strong>"LSP Status: "</strong>{text}
                            </div>
                            <div style="margin-top: 6px; display:flex; gap:6px; align-items:center;">
                                <span style="color:#aaa;">{move || format!("logs: {}", logging::len())}</span>
                                <button
                                  style="background:#2e2e2e;color:#ddd;border:1px solid #555;border-radius:3px;padding:2px 6px;cursor:pointer;"
                                  on:click=move |_| {
                                      // Copy logs to clipboard
                                      if let Some(win) = web_sys::window() {
                                          let nav = win.navigator();
                                          let cb = nav.clipboard();
                                          let _ = cb.write_text(&logging::logs_as_text());
                                      }
                                  }
                                >
                                  "Copy Logs"
                                </button>
                                <button
                                  style="background:#2e2e2e;color:#ddd;border:1px solid #555;border-radius:3px;padding:2px 6px;cursor:pointer;"
                                  on:click=move |_| { logging::clear(); }
                                >
                                  "Clear"
                                </button>
                            </div>
                            {if let ConnectionState::Error(e) = state_val {
                                view! {
                                    <div style="color: #f88; margin-top: 4px; font-size: 10px;">
                                        {e}
                                    </div>
                                }.into_view()
                            } else {
                                view! { <></> }.into_view()
                            }}
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <div style="color: #888;">
                            "LSP: Not Available"
                        </div>
                    }.into_view()
                }
            }}
        </div>
    }
}
