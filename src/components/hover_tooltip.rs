use crate::lsp::{Hover, MarkupContent};
use leptos::*;

#[component]
pub fn HoverTooltip(
    #[prop(into)] hover: Signal<Option<Hover>>,
    #[prop(into)] position: (f64, f64),
) -> impl IntoView {
    let (x, y) = position;

    view! {
        <div
            class="hover-tooltip"
            style=format!("left: {}px; top: {}px;", x, y)
        >
            {move || {
                hover.get().map(|h| {
                    let content = match &h.contents {
                        MarkupContent { kind, value } => {
                            if kind == "markdown" {
                                // For now, display as plain text
                                // Could be enhanced with a markdown renderer
                                view! {
                                    <div class="hover-content markdown">
                                        <pre>{value.clone()}</pre>
                                    </div>
                                }
                                .into_view()
                            } else {
                                view! {
                                    <div class="hover-content">
                                        <pre>{value.clone()}</pre>
                                    </div>
                                }
                                .into_view()
                            }
                        }
                    };

                    view! { <div>{content}</div> }
                })
            }}
        </div>
    }
}
