use leptos::{component, use_context, spawn_local, view, IntoView, SignalGetUntracked, SignalSet, SignalUpdate};

use crate::components::app::ActiveRunTab;
use crate::components::dropdown::Dropdown;
use crate::components::program_window::Program;
use crate::components::run_window::{SignedData, TxEnv};
use crate::examples::Example;
use crate::lsp::LspClient;
use crate::util::{HashedData, SigningKeys};

pub fn select_example(example: Example) {
    let program = use_context::<Program>().expect("program should exist in context");
    let tx_env = use_context::<TxEnv>().expect("transaction environment should exist in context");
    let active_run_tab =
        use_context::<ActiveRunTab>().expect("active run tab should exist in context");
    let signing_keys = use_context::<SigningKeys>().expect("signing keys should exist in context");
    let signed_data = use_context::<SignedData>().expect("signed data should exist in context");
    let hashed_data = use_context::<HashedData>().expect("hashed data should exist in context");
    let lsp_client = use_context::<LspClient>();

    tx_env.params.set(example.params());
    let arguments = example.arguments(&signing_keys.public_keys, &hashed_data.hashes);
    let program_text = format!("{arguments}\n\n{}", example.template_text());
    program.text.set(program_text.clone());
    program.update_on_read();

    let witness = example.witness(
        &signing_keys.secret_keys,
        &hashed_data.preimages,
        signed_data.sighash_all.get_untracked(),
    );
    let satisfied_text = format!("{witness}\n\n{program_text}");
    let satisfied_text_for_lsp = satisfied_text.clone();
    program.text.set(satisfied_text.clone());
    
    // Notify LSP about the text change
    if let Some(lsp) = lsp_client {
        let lsp = lsp.clone();
        let uri = "file:///main.simf".to_string();
        spawn_local(async move {
            if let Err(e) = lsp.did_change(uri, satisfied_text_for_lsp) {
                log::warn!("Failed to update document in LSP after example selection: {}", e);
            }
        });
    }
    
    active_run_tab.0.update(|_| {}); // refresh active tab
}

#[component]
pub fn ExamplesDropdown() -> impl IntoView {
    let examples = crate::examples::keys().collect::<Vec<&'static str>>();
    let select_example = move |selected| match crate::examples::get(selected) {
        Some(example) => select_example(example),
        None => {
            // do nothing
        }
    };

    view! {
        <Dropdown name="Examples" options=examples select_option=select_example />
    }
}
