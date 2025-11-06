use crate::lsp::{CompletionItem, CompletionItemKind};
use leptos::html::Div;
use leptos::*;
use wasm_bindgen::JsCast;

#[component]
pub fn CompletionDropdown(
    #[prop(into)] items: Signal<Vec<CompletionItem>>,
    #[prop(into)] selected_index: RwSignal<usize>,
    #[prop(into)] on_select: Callback<String>,
    #[prop(into)] position: (f64, f64),
) -> impl IntoView {
    let (x, y) = position;
    let container_ref = create_node_ref::<Div>();

    // Use create_effect to imperatively update only the selected items
    create_effect(move |prev_index: Option<usize>| {
        let current_index = selected_index.get();

        if let Some(container) = container_ref.get() {
            let element: &web_sys::Element = container.as_ref();

            // Remove 'selected' class from previous item
            if let Some(prev) = prev_index {
                if let Ok(Some(prev_item)) =
                    element.query_selector(&format!("[data-index='{}']", prev))
                {
                    let _ = prev_item.class_list().remove_1("selected");
                }
            }

            // Add 'selected' class to current item
            if let Ok(Some(current_item)) =
                element.query_selector(&format!("[data-index='{}']", current_index))
            {
                let _ = current_item.class_list().add_1("selected");
                // Scroll into view if needed
                if let Some(scroll_elem) = current_item.dyn_ref::<web_sys::HtmlElement>() {
                    scroll_elem.scroll_into_view_with_bool(false);
                }
            }
        }

        current_index
    });

    view! {
        <div
            class="completion-dropdown"
            style=format!("left: {}px; top: {}px;", x, y)
            node_ref=container_ref
        >
            <For
                each=move || items.get().into_iter().enumerate()
                key=|(i, item)| (i.clone(), item.label.clone())
                children=move |(index, item)| {
                    let label = item.label.clone();
                    let label_for_click = label.clone();
                    let label_for_display = label.clone();
                    let kind = item.kind;
                    let detail = item.detail.clone();

                    let kind_icon = match kind {
                        Some(CompletionItemKind::Function) => "ƒ",
                        Some(CompletionItemKind::Method) => "ƒ",
                        Some(CompletionItemKind::Variable) => "v",
                        Some(CompletionItemKind::Field) => "f",
                        Some(CompletionItemKind::Class) => "C",
                        Some(CompletionItemKind::Interface) => "I",
                        Some(CompletionItemKind::Module) => "M",
                        Some(CompletionItemKind::Property) => "p",
                        Some(CompletionItemKind::Keyword) => "k",
                        Some(CompletionItemKind::Enum) => "E",
                        _ => "•",
                    };

                    // Mark first item as selected initially
                    let initial_class = if index == 0 { "completion-item selected" } else { "completion-item" };

                    view! {
                        <div
                            class=initial_class
                            data-index=index
                            on:click=move |_| {
                                on_select.call(label_for_click.clone());
                            }
                        >
                            <span class="completion-icon">{kind_icon}</span>
                            <span class="completion-label">{label_for_display}</span>
                            {detail.as_ref().map(|d| view! {
                                <span class="completion-detail">{d.clone()}</span>
                            })}
                        </div>
                    }
                }
            />
        </div>
    }
}
