use std::sync::Arc;

use itertools::Itertools;
use leptos::{
    component, create_effect, create_memo, create_node_ref, create_rw_signal, ev,
    event_target_value, html, spawn_local, use_context, view, IntoView, RwSignal, Signal,
    SignalGet, SignalGetUntracked, SignalSet, SignalUpdate, SignalWith, SignalWithUntracked,
};
use simplicityhl::parse::ParseFromStr;
use simplicityhl::simplicity::jet::elements::ElementsEnv;
use simplicityhl::{elements, simplicity};
use simplicityhl::{CompiledProgram, SatisfiedProgram, WitnessValues};

use crate::components::copy_to_clipboard::CopyToClipboard;
use crate::components::lsp_status::DiagnosticsList;
use crate::components::{CompletionDropdown, HoverTooltip};
use crate::components::program_window::syntax_highlighter::highlight_code;
use crate::function::Runner;
use crate::lsp::{ConnectionState, LspClient, Position};

#[derive(Copy, Clone, Debug)]
pub struct Program {
    pub text: RwSignal<String>,
    cached_text: RwSignal<String>,
    pub lazy_cmr: RwSignal<Result<simplicity::Cmr, String>>,
    lazy_satisfied: RwSignal<Result<SatisfiedProgram, String>>,
}

impl Default for Program {
    fn default() -> Self {
        Self::new(String::default())
    }
}

impl Program {
    pub fn new(text: String) -> Self {
        let program = Self {
            text: create_rw_signal(text),
            cached_text: create_rw_signal("".to_string()),
            lazy_cmr: create_rw_signal(Err("".to_string())),
            lazy_satisfied: create_rw_signal(Err("".to_string())),
        };
        program.update_on_read();
        program
    }

    pub fn is_empty(&self) -> bool {
        self.text.with_untracked(String::is_empty)
    }

    pub fn cmr(self) -> Result<simplicity::Cmr, String> {
        self.update_on_read();
        self.lazy_cmr.get_untracked()
    }

    pub fn satisfied(self) -> Result<SatisfiedProgram, String> {
        self.update_on_read();
        self.lazy_satisfied.get_untracked()
    }

    pub fn update_on_read(self) {
        let needs_update = self.text.with_untracked(|text| {
            self.cached_text
                .with_untracked(|cached_text| text != cached_text)
        });
        if !needs_update {
            return;
        }
        self.text.with_untracked(|text| {
            self.cached_text.set(text.clone());
            let compiled = simplicityhl::Arguments::parse_from_str(text)
                .map_err(|error| error.to_string())
                .and_then(|args| CompiledProgram::new(text.as_str(), args));
            let cmr = compiled
                .as_ref()
                .map(|x| x.commit().cmr())
                .map_err(Clone::clone);
            self.lazy_cmr.set(cmr);
            let satisfied = compiled.and_then(|x| {
                let witness = WitnessValues::parse_from_str(text)?;
                x.satisfy(witness)
            });
            self.lazy_satisfied.set(satisfied);
        });
    }

    pub fn add_default_modules(self) {
        let (contains_witness, contains_param) = self
            .text
            .with_untracked(|text| (text.contains("mod witness"), text.contains("mod param")));
        if !contains_param {
            self.text
                .update(|text| text.insert_str(0, "mod param {}\n\n"));
        }
        if !contains_witness {
            self.text
                .update(|text| text.insert_str(0, "mod witness {}\n\n"));
        }
    }
}

#[derive(Copy, Clone)]
pub struct Runtime {
    program: Program,
    env: Signal<ElementsEnv<Arc<elements::Transaction>>>,
    pub run_succeeded: RwSignal<Option<bool>>,
    pub debug_output: RwSignal<String>,
    pub error_output: RwSignal<String>,
}

impl Runtime {
    pub fn new(program: Program, env: Signal<ElementsEnv<Arc<elements::Transaction>>>) -> Self {
        Self {
            program,
            env,
            run_succeeded: Default::default(),
            debug_output: Default::default(),
            error_output: Default::default(),
        }
    }

    fn set_success(self, success: bool) {
        spawn_local(async move {
            self.run_succeeded.set(Some(success));
            gloo_timers::future::TimeoutFuture::new(500).await;
            self.run_succeeded.set(None);
        });
        web_sys::window()
            .as_ref()
            .map(web_sys::Window::navigator)
            .map(|navigator| match success {
                true => navigator.vibrate_with_duration(200),
                false => navigator.vibrate_with_duration(500),
            });
    }

    pub fn run(self) {
        self.debug_output.update(String::clear);
        let satisfied_program = match self.program.satisfied() {
            Ok(x) => x,
            Err(error) => {
                self.error_output.set(error);
                self.set_success(false);
                return;
            }
        };
        let mut runner = Runner::for_program(satisfied_program);
        let success = self.env.with(|env| match runner.run(env) {
            Ok(..) => {
                self.error_output.update(String::clear);
                true
            }
            Err(error) => {
                self.error_output.set(error.to_string());
                false
            }
        });
        self.debug_output
            .set(runner.debug_output().into_iter().join("\n"));
        self.set_success(success);
    }
}

const TAB_KEY: u32 = 9;
const ENTER_KEY: u32 = 13;
const SPACE_KEY: u32 = 32;

#[component]
pub fn ProgramTab() -> impl IntoView {
    let program = use_context::<Program>().expect("program should exist in context");
    let runtime = use_context::<Runtime>().expect("runtime should exist in context");
    let lsp_client = use_context::<LspClient>();
    let lsp_client_for_diagnostics = lsp_client.clone();
    let lsp_client_for_hover = lsp_client.clone();
    let lsp_client_for_hover_tooltip = lsp_client.clone();
    let lsp_client_for_filtering = lsp_client.clone();
    let lsp_client_for_completion_request = lsp_client.clone();
    let lsp_client_for_dropdown_check = lsp_client.clone();
    let textarea_ref = create_node_ref::<html::Textarea>();

    // Completion state
    let show_completions = create_rw_signal(false);
    let selected_completion_index = create_rw_signal(0usize);
    let completion_position = create_rw_signal((0.0, 0.0));
    let completion_items_count = create_rw_signal(0usize);
    let completion_trigger_pos = create_rw_signal(Option::<usize>::None); // Track cursor position when completion was triggered
    let filtered_completion_items = create_rw_signal(Vec::<crate::lsp::CompletionItem>::new()); // Filtered completion items

    // Filter completion items based on partial text typed
    create_effect(move |_| {
        use crate::lsp::CompletionItem;
        
        // Track dependencies - IMPORTANT: track LSP completions signal so effect re-runs when completions arrive
        let _ = show_completions.get();
        let _ = program.text.get();
        let _ = completion_trigger_pos.get();
        
        // Track LSP completions signal to trigger re-filtering when completions arrive
        let lsp_completions = lsp_client_for_filtering.as_ref()
            .map(|lsp| {
                let completions_signal = lsp.completions();
                let completions = completions_signal.get(); // Track the signal and get value
                completions
            })
            .unwrap_or_default();
        
        // If completions are not shown, set empty
        if !show_completions.get() {
            filtered_completion_items.set(Vec::new());
            return;
        }

        // Get trigger position
        let trigger_pos = match completion_trigger_pos.get() {
            Some(pos) => pos,
            None => {
                // No trigger pos, return all
                filtered_completion_items.set(lsp_completions.clone());
                completion_items_count.set(lsp_completions.len());
                return;
            }
        };

        // Get current cursor position and text
        if let Some(element) = textarea_ref.get() {
            if let Ok(Some(cursor_pos)) = element.selection_start() {
                let cursor_pos_usize = cursor_pos as usize;
                let text = program.text.get();
                
                // Safety check: ensure cursor position is valid
                if cursor_pos_usize > text.len() {
                    // Cursor is beyond text length - show all completions
                    completion_items_count.set(lsp_completions.len());
                    filtered_completion_items.set(lsp_completions.clone());
                    selected_completion_index.set(0);
                    return;
                }
                
                // If cursor is at or after trigger position, filter or show all
                if cursor_pos_usize >= trigger_pos {
                    // If cursor is after trigger position, filter by partial text
                    if cursor_pos_usize > trigger_pos {
                        // Extract partial text typed after trigger
                        let partial_text = &text[trigger_pos..cursor_pos_usize];
                        
                        // Filter completions that start with the partial text
                        let filtered: Vec<CompletionItem> = lsp_completions
                            .iter()
                            .filter(|item| item.label.starts_with(partial_text))
                            .cloned()
                            .collect();
                        
                        log::debug!("Filtered {} completions with prefix '{}' from {} total", filtered.len(), partial_text, lsp_completions.len());
                        
                        // Update item count for arrow key navigation
                        completion_items_count.set(filtered.len());
                        filtered_completion_items.set(filtered);
                        
                        // Reset selection index when filtered items change
                        selected_completion_index.set(0);
                        return;
                    } else {
                        // Cursor is exactly at trigger position - show all completions
                        log::debug!("Cursor at trigger position, showing all {} completions", lsp_completions.len());
                        completion_items_count.set(lsp_completions.len());
                        filtered_completion_items.set(lsp_completions.clone());
                        selected_completion_index.set(0);
                        return;
                    }
                }
                // If cursor is before trigger position, something went wrong - show all as fallback
                log::warn!("Cursor position {} is before trigger position {}, showing all completions", cursor_pos_usize, trigger_pos);
            } else {
                log::warn!("Could not get cursor position, showing all completions");
            }
        } else {
            log::warn!("Textarea element not available, showing all completions");
        }
        
        // Fallback: return all completions
        completion_items_count.set(lsp_completions.len());
        filtered_completion_items.set(lsp_completions.clone());
        selected_completion_index.set(0);
    });

    // Auto-hide dropdown when filtered items become empty and cursor has moved past trigger
    let lsp_client_for_autohide = lsp_client.clone();
    create_effect(move |_| {
        let show = show_completions.get();
        let filtered_items = filtered_completion_items.get();
        let trigger_pos = completion_trigger_pos.get();
        
        // Only auto-hide if dropdown is shown, items are empty, and we have a trigger position
        if show && filtered_items.is_empty() && trigger_pos.is_some() {
            // Check if LSP has completions (if not, we're still waiting)
            let has_lsp_completions = lsp_client_for_autohide.as_ref()
                .map(|lsp| !lsp.completions().get_untracked().is_empty())
                .unwrap_or(false);
            
            if has_lsp_completions {
                // Check if cursor is past trigger position
                let trigger_pos_val = trigger_pos.unwrap();
                let cursor_at_trigger = textarea_ref.get()
                    .and_then(|el| el.selection_start().ok().flatten())
                    .map(|pos| pos as usize == trigger_pos_val)
                    .unwrap_or(false);
                
                if !cursor_at_trigger {
                    // Cursor has moved past trigger and no items match - hide dropdown
                    log::debug!("Auto-hiding dropdown: no items match after cursor moved past trigger");
                    completion_trigger_pos.set(None);
                    show_completions.set(false);
                    completion_items_count.set(0);
                }
            }
        }
    });

    // Hover state
    let show_hover = create_rw_signal(false);
    let hover_position = create_rw_signal((0.0, 0.0));
    let hover_debounce_timer = create_rw_signal(0);

    // Document URI for LSP
    let document_uri = "file:///main.simf";

    // Open document in LSP when it becomes initialized
    if let Some(lsp) = lsp_client.as_ref() {
        log::info!("LSP client is available, waiting for initialization");
        let lsp = lsp.clone();
        let uri = document_uri.to_string();
        let lsp_state = lsp.state();

        // Create effect that watches LSP state and opens document when initialized
        create_effect(move |prev_opened: Option<bool>| {
            use leptos::SignalGet;
            let state = lsp_state.get();
            let already_opened = prev_opened.unwrap_or(false);

            // Only open once when state becomes Initialized
            if !already_opened && state == ConnectionState::Initialized {
                let initial_text = program.text.get_untracked();
                let lsp = lsp.clone();
                let uri = uri.clone();

                spawn_local(async move {
                    log::info!("LSP initialized, opening document: {}", uri);
                    if let Err(e) = lsp.did_open(uri.clone(), initial_text) {
                        log::warn!("Failed to open document in LSP: {}", e);
                    } else {
                        log::info!("Successfully opened document: {}", uri);
                    }
                });

                true // Mark as opened
            } else {
                already_opened
            }
        });
    } else {
        log::warn!("LSP client is NOT available");
    }

    let lsp_for_update = lsp_client.clone();
    let update_program_text = move |event: ev::Event| {
        let new_text = event_target_value(&event);
        program.text.set(new_text.clone());

        // Send LSP update (debouncing handled by gloo_timers)
        if let Some(lsp) = lsp_for_update.as_ref() {
            let lsp = lsp.clone();
            let uri = document_uri.to_string();
            spawn_local(async move {
                // Wait 300ms before sending update to avoid spamming LSP server
                gloo_timers::future::TimeoutFuture::new(300).await;
                log::info!("Sending did_change to LSP for: {}", uri);
                if let Err(e) = lsp.did_change(uri.clone(), new_text) {
                    log::warn!("Failed to update document in LSP: {}", e);
                } else {
                    log::info!("Successfully sent did_change for: {}", uri);
                }
            });
        } else {
            log::warn!("No LSP client available for update");
        }
    };
    let insert_4_spaces = move || {
        let element = textarea_ref.get().expect("<textarea> should be mounted");
        if let Ok(Some(start)) = element.selection_start() {
            let start_ = start as usize; // safety: 32-bit machine of higher
            program.text.update(|s| s.insert_str(start_, "    "));
            let _result = element.set_selection_range(start + 4, start + 4);
        }
    };
    let delete_4_spaces = move || {
        let element = textarea_ref.get().expect("<textarea> should be mounted");
        if let Ok(Some(start)) = element.selection_start() {
            let start_ = start as usize; // safety: 32-bit machine of higher
            if start < 4 || program.text.with(|s| &s[start_ - 4..start_] != "    ") {
                return;
            }
            program
                .text
                .update(|s| s.replace_range(start_ - 4..start_, ""));
            let _result = element.set_selection_range(start - 4, start - 4);
        }
    };
    let handle_keydown = move |event: ev::KeyboardEvent| {
        let is_completions_shown = show_completions.get_untracked();

        // Handle arrow keys and Enter when completions are shown
        if is_completions_shown {
            if event.key_code() == 40 {
                // Arrow Down
                event.prevent_default();
                let items_count = completion_items_count.get_untracked();
                if items_count > 0 {
                    let current = selected_completion_index.get_untracked();
                    selected_completion_index.set((current + 1) % items_count);
                }
                return;
            } else if event.key_code() == 38 {
                // Arrow Up
                event.prevent_default();
                let items_count = completion_items_count.get_untracked();
                if items_count > 0 {
                    let current = selected_completion_index.get_untracked();
                    let new_idx = if current == 0 {
                        items_count - 1
                    } else {
                        current - 1
                    };
                    selected_completion_index.set(new_idx);
                }
                return;
            } else if event.key_code() == ENTER_KEY {
                event.prevent_default();
                // Insert the selected completion
                // Get filtered items
                let filtered_items = filtered_completion_items.get_untracked();
                    let selected_idx = selected_completion_index.get_untracked();
                if let Some(item) = filtered_items.get(selected_idx) {
                        let label = item.label.clone();
                    // Close dropdown FIRST before updating text to prevent effect from interfering
                    show_completions.set(false);
                    completion_trigger_pos.set(None);
                        // Insert at cursor
                        if let Some(element) = textarea_ref.get() {
                            if let Ok(Some(start)) = element.selection_start() {
                                if let Ok(Some(end)) = element.selection_end() {
                                    let start_pos = start as usize;
                                    let end_pos = end as usize;
                                    program.text.update(|text| {
                                        text.replace_range(start_pos..end_pos, &label);
                                    });
                                    let new_pos = start + label.len() as u32;
                                    let _ = element.set_selection_range(new_pos, new_pos);
                                }
                            }
                        }
                } else {
                    // No item selected, just close
                    show_completions.set(false);
                    completion_trigger_pos.set(None);
                }
                return;
            } else if event.key_code() == 27 {
                // Escape
                event.prevent_default();
                show_completions.set(false);
                completion_trigger_pos.set(None);
                return;
            }
        }

        // Normal key handlers
        if event.ctrl_key() && event.key_code() == ENTER_KEY {
            runtime.run();
        } else if event.ctrl_key() && event.key_code() == SPACE_KEY {
            event.prevent_default();
            // Trigger completion
            if let Some(lsp) = lsp_client_for_completion_request.as_ref() {
                if let Some(element) = textarea_ref.get() {
                    if let Ok(Some(cursor_pos)) = element.selection_start() {
                        let cursor_pos_usize = cursor_pos as usize;
                        let text = program.text.get_untracked();
                        // Calculate line and character from cursor position
                        let before_cursor = &text[..cursor_pos_usize];
                        let line = before_cursor.matches('\n').count() as u32;
                        let line_start = before_cursor.rfind('\n').map(|p| p + 1).unwrap_or(0);
                        let character = (cursor_pos_usize - line_start) as u32;

                        let position = Position { line, character };
                        log::info!("Requesting completion at line {}, char {}", line, character);

                        // Store the trigger position for filtering
                        completion_trigger_pos.set(Some(cursor_pos_usize));

                        // Calculate pixel position for dropdown (approximate)
                        // This uses fixed positioning based on the textarea's typical location
                        let x = 150.0 + (character as f64 * 7.2); // Approximate character width
                        let y = 200.0 + (line as f64 * 18.0); // Approximate line height
                        completion_position.set((x, y));

                        selected_completion_index.set(0);

                        if let Err(e) = lsp.request_completion(document_uri.to_string(), position) {
                            log::warn!("Failed to request completion: {}", e);
                        } else {
                            log::info!("Completion request sent, setting show_completions to true");
                            // Cache the items count for efficient arrow key navigation
                            let items_count = lsp.completions().get_untracked().len();
                            completion_items_count.set(items_count);
                            show_completions.set(true);
                            log::debug!(
                                "show_completions is now: {}",
                                show_completions.get_untracked()
                            );
                        }
                    }
                }
            }
        } else if event.key_code() == TAB_KEY {
            event.prevent_default();
            match event.shift_key() {
                false => insert_4_spaces(),
                true => delete_4_spaces(),
            }
        }
    };

    // Calculate line numbers based on content
    let line_numbers = create_memo(move |_| {
        use leptos::SignalGet;
        let text = program.text.get();
        // Count newlines + 1 to get total lines (handles empty lines correctly)
        let line_count = if text.is_empty() {
            1
        } else {
            text.matches('\n').count() + 1
        };
        (1..=line_count)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    });

    // Sync line numbers and highlight overlay scroll with textarea scroll
    let line_numbers_ref = create_node_ref::<html::Div>();
    let highlight_overlay_ref = create_node_ref::<html::Pre>();
    let handle_scroll = move |_| {
        if let Some(textarea) = textarea_ref.get() {
            let scroll_top = textarea.scroll_top();
            let scroll_left = textarea.scroll_left();
            if let Some(line_nums) = line_numbers_ref.get() {
            let _ = line_nums.set_scroll_top(scroll_top);
            }
            if let Some(highlight) = highlight_overlay_ref.get() {
                let _ = highlight.set_scroll_top(scroll_top);
                let _ = highlight.set_scroll_left(scroll_left);
            }
        }
    };
    
    // Create memoized highlighted code
    let highlighted_code = create_memo(move |_| {
        let text = program.text.get();
        highlight_code(&text)
    });
    
    // Update the overlay's innerHTML when highlighted code changes
    create_effect(move |_| {
        let html = highlighted_code.get();
        
        // Debug: Log a sample of the HTML to see if spans are being created
        let sample = if html.len() > 200 {
            format!("{}...", &html[..200])
        } else {
            html.clone()
        };
        log::info!("Highlighted HTML sample (first 200 chars): {}", sample);
        log::info!("Full HTML length: {}, contains 'hl-keyword': {}", html.len(), html.contains("hl-keyword"));
        
        // Try to set immediately, and also schedule a check
        if let Some(element) = highlight_overlay_ref.get() {
            log::info!("Setting highlight overlay HTML immediately, length: {}", html.len());
            element.set_inner_html(&html);
            log::info!("HTML set successfully");
        } else {
            log::warn!("Highlight overlay element not found, will retry");
            // Element not ready yet, try again after a tick
            let highlight_ref = highlight_overlay_ref.clone();
            let html_clone = html.clone(); // Clone the HTML string for the async block
            spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(10).await;
                if let Some(element) = highlight_ref.get() {
                    log::info!("Setting highlight overlay HTML (delayed), length: {}", html_clone.len());
                    element.set_inner_html(&html_clone);
                    log::info!("HTML set successfully (delayed)");
                } else {
                    log::warn!("Highlight overlay element still not found after delay");
                }
            });
        }
    });

    // Handle mouse move for hover with debouncing
    let handle_mouse_move = move |event: ev::MouseEvent| {
        // Clear any existing hover when completions are shown
        if show_completions.get_untracked() {
            show_hover.set(false);
            return;
        }

        if let Some(lsp) = lsp_client_for_hover.as_ref() {
            if let Some(_element) = textarea_ref.get() {
                // Calculate approximate cursor position from mouse coordinates
                // This is a simplified approach based on fixed character/line dimensions
                let char_width = 7.2;
                let line_height = 18.0;

                let offset_x = event.offset_x() as f64;
                let offset_y = event.offset_y() as f64;

                let line = (offset_y / line_height).floor() as u32;
                let character = (offset_x / char_width).floor() as u32;

                // Set hover position for tooltip
                hover_position.set((
                    event.client_x() as f64 + 10.0,
                    event.client_y() as f64 + 10.0,
                ));

                // Debounce: increment timer
                let timer_id = hover_debounce_timer.get_untracked() + 1;
                hover_debounce_timer.set(timer_id);

                let lsp = lsp.clone();
                let doc_uri = document_uri.to_string();

                // Debounce hover requests (500ms)
                spawn_local(async move {
                    gloo_timers::future::TimeoutFuture::new(500).await;

                    // Only proceed if timer hasn't been reset
                    if hover_debounce_timer.get_untracked() == timer_id {
                        let position = Position { line, character };
                        if let Err(e) = lsp.request_hover(doc_uri, position) {
                            log::debug!("Failed to request hover: {}", e);
                        } else {
                            show_hover.set(true);
                        }
                    }
                });
            }
        }
    };

    let handle_mouse_leave = move |_: ev::MouseEvent| {
        show_hover.set(false);
        hover_debounce_timer.update(|t| *t += 1); // Cancel pending hovers
    };

    view! {
        <div class="tab-content">
            <div class="copy-program">
                <CopyToClipboard content=program.text class="copy-button" tooltip_below=true>
                    <i class="far fa-copy"></i>
                </CopyToClipboard>
            </div>
            <div class="editor-container">
                <div class="line-numbers" node_ref=line_numbers_ref>
                    {move || line_numbers.get()}
                </div>
                <div class="editor-wrapper">
                    <pre
                        class="syntax-highlight-overlay"
                        node_ref=highlight_overlay_ref
                    />
                <textarea
                    class="program-input-field"
                    placeholder="Enter your program here"
                    rows="25"
                    cols="80"
                    spellcheck="false"
                    wrap="off"
                    prop:value=program.text
                    on:input=update_program_text
                    on:keydown=handle_keydown
                    on:scroll=handle_scroll
                    on:mousemove=handle_mouse_move
                    on:mouseleave=handle_mouse_leave
                    node_ref=textarea_ref
                    name="program-input"
                >
                    {program.text.get_untracked()}
                </textarea>
                </div>
            </div>

            // Show hover tooltip when hovering
            {
                move || {
                    if show_hover.get() && !show_completions.get() {
                        lsp_client_for_hover_tooltip.as_ref().and_then(|lsp| {
                            let hover_signal = lsp.hover_info();
                            let hover_data = hover_signal.get();

                            if hover_data.is_some() {
                                let hover_sig: Signal<_> = hover_signal.into();
                                Some(view! {
                                    <HoverTooltip
                                        hover=hover_sig
                                        position=hover_position.get()
                                    />
                                })
                            } else {
                                None
                            }
                        })
                    } else {
                        None
                    }
                }
            }

            // Show completion dropdown when triggered
            {
                move || {
                    let should_show = show_completions.get();
                    log::debug!("Completion dropdown check: show={}", should_show);
                    if should_show {
                        let filtered_items = filtered_completion_items.get();
                        log::debug!("Filtered completion items count: {}", filtered_items.len());
                        
                        // Don't render if we have no items (the auto-hide effect will handle hiding)
                        if filtered_items.is_empty() {
                            // Check if we're still waiting for LSP completions
                            let has_lsp_completions = lsp_client_for_dropdown_check.as_ref()
                                .map(|lsp| !lsp.completions().get().is_empty())
                                .unwrap_or(false);
                            
                            if !has_lsp_completions {
                                // Still waiting for completions - don't render yet
                                log::debug!("Waiting for LSP completions to arrive...");
                                return None;
                            }
                            // Otherwise, we have LSP completions but filtered items are empty
                            // The auto-hide effect will handle hiding the dropdown
                            return None;
                        }
                        
                        // Show dropdown if we have items
                        log::info!("Showing completion dropdown with {} filtered items", filtered_items.len());
                        // Use the filtered items signal directly
                        let items_sig: Signal<_> = filtered_completion_items.read_only().into();
                                Some(view! {
                                    <CompletionDropdown
                                        items=items_sig
                                        selected_index=selected_completion_index
                                        on_select=move |label: String| {
                                            // Insert completion at cursor
                                            if let Some(element) = textarea_ref.get() {
                                                if let Ok(Some(start)) = element.selection_start() {
                                                    if let Ok(Some(end)) = element.selection_end() {
                                                        let start_pos = start as usize;
                                                        let end_pos = end as usize;
                                                        program.text.update(|text| {
                                                            text.replace_range(start_pos..end_pos, &label);
                                                        });
                                                        let new_pos = start + label.len() as u32;
                                                        let _ = element.set_selection_range(new_pos, new_pos);
                                                    }
                                                }
                                            }
                                            show_completions.set(false);
                                    completion_trigger_pos.set(None);
                                        }
                                        position=completion_position.get()
                                    />
                        })
                    } else {
                        None
                    }
                }
            }

            // Display LSP diagnostics below the editor
            {lsp_client_for_diagnostics.as_ref().map(|_| view! {
                <div class="lsp-diagnostics">
                    <DiagnosticsList uri=document_uri />
                </div>
            })}
        </div>
    }
}
