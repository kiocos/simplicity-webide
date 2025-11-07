use crate::components::markdown_parser::parse_markdown;
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
                                // Parse markdown and render as HTML
                                let html = parse_markdown(value);
                                view! {
                                    <div class="hover-content markdown" inner_html=html></div>
                                }
                                .into_view()
                            } else {
                                // Plain text - escape HTML and preserve whitespace
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
