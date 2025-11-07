use leptos::{component, provide_context, view, IntoView, RwSignal};
use wasm_bindgen_futures::spawn_local;

use super::program_window::{select_example, Program, ProgramWindow, Runtime};
use crate::components::footer::Footer;
use crate::components::lsp_debug::LspDebugStatus;
use crate::components::navigation::Navigation;
use crate::components::run_window::{HashCount, KeyCount, RunWindow, SignedData, TxEnv};
use crate::components::state::LocalStorage;
use crate::examples;
use crate::lsp::LspClient;
use crate::mcp::MCPClient;
use crate::transaction::TxParams;
use crate::util::{HashedData, SigningKeys};

#[derive(Copy, Clone, Debug, Default)]
pub struct ActiveRunTab(pub RwSignal<&'static str>);

#[component]
pub fn App() -> impl IntoView {
    let program = Program::load_from_storage().unwrap_or_default();
    provide_context(program);
    let tx_params = TxParams::load_from_storage().unwrap_or_default();
    let tx_env = TxEnv::new(program, tx_params);
    provide_context(tx_env);
    provide_context(SigningKeys::load_from_storage().unwrap_or_default());
    provide_context(SignedData::new(tx_env.lazy_env));
    provide_context(HashedData::load_from_storage().unwrap_or_default());
    provide_context(KeyCount::load_from_storage().unwrap_or_default());
    provide_context(HashCount::load_from_storage().unwrap_or_default());
    provide_context(Runtime::new(program, tx_env.lazy_env));
    provide_context(ActiveRunTab::default());

    // Initialize LSP client
    let lsp_client = LspClient::new();
    provide_context(lsp_client.clone());

    // Connect to LSP server on startup
    spawn_local(async move {
        // Try to connect to local LSP server
        if let Err(e) = lsp_client.connect("ws://127.0.0.1:9257").await {
            log::warn!("Failed to connect to LSP server: {}", e);
            // Continue without LSP - the app should still work
        } else {
            log::info!("Successfully connected to LSP server");
        }
    });

    // Initialize MCP client
    let mcp_client = MCPClient::new();
    provide_context(mcp_client.clone());

    // Connect to MCP bridge on startup
    spawn_local(async move {
        // Try to connect to local MCP bridge
        if let Err(e) = mcp_client.connect("ws://127.0.0.1:3001").await {
            log::warn!("Failed to connect to MCP bridge: {}", e);
            // Continue without MCP - the app should still work
        } else {
            log::info!("Successfully connected to MCP bridge");
        }
    });

    if program.is_empty() {
        select_example(examples::get("✍️️ P2PK").expect("P2PK example should exist"))
    }

    view! {
        <Navigation />
        // <LspDebugStatus />
        <section class="main-content">
            <ProgramWindow />
            <RunWindow />
        </section>
        <Footer />
    }
}
