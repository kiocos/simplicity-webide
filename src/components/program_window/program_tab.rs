use std::sync::Arc;

use itertools::Itertools;
use leptos::{
    component, create_effect, create_memo, create_node_ref, create_rw_signal, ev,
    event_target_value, html, spawn_local, use_context, view, CollectView, IntoView, RwSignal, Signal,
    SignalGet, SignalGetUntracked, SignalSet, SignalUpdate, SignalWith, SignalWithUntracked,
};
use simplicityhl::parse::ParseFromStr;
use simplicityhl::simplicity::jet::elements::ElementsEnv;
use simplicityhl::{elements, simplicity};
use simplicityhl::{CompiledProgram, SatisfiedProgram, WitnessValues};

use crate::components::copy_to_clipboard::CopyToClipboard;
use crate::components::{CompletionDropdown, HoverTooltip};
use crate::components::program_window::syntax_highlighter::{highlight_code, add_error_indicators};
use crate::function::Runner;
use crate::lsp::{ConnectionState, LspClient, Position};
use crate::mcp::{MCPClient, ChatMessage};
use serde_json;
use wasm_bindgen::JsCast;

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

/// Format chat response content - intelligently format JSON responses
fn format_chat_response(content: &str) -> String {
    // Try to parse as JSON
    if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(content) {
        // Handle specific response formats from mcp-bridge
        if let Some(obj) = json_value.as_object() {
            // Check if it's a suggestions/help response
            if obj.contains_key("message") && obj.contains_key("suggestions") {
                let message = obj.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("I can help you with Simplicity contracts.");
                
                let suggestions = obj.get("suggestions")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                
                let mut formatted = message.to_string();
                if !suggestions.is_empty() {
                    formatted.push_str("\n\nYou can ask me to:\n");
                    for suggestion in suggestions.iter() {
                        formatted.push_str(&format!("  • {}\n", suggestion));
                    }
                }
                return formatted;
            }
            
            // For other JSON objects, try to format them nicely
            // Extract common fields and format them
            if let Some(message) = obj.get("message").and_then(|v| v.as_str()) {
                let mut formatted = message.to_string();
                
                // Add other relevant fields
                if let Some(suggestions) = obj.get("suggestions").and_then(|v| v.as_array()) {
                    if !suggestions.is_empty() {
                        formatted.push_str("\n\nSuggestions:\n");
                        for sug in suggestions.iter() {
                            if let Some(s) = sug.as_str() {
                                formatted.push_str(&format!("  • {}\n", s));
                            }
                        }
                    }
                }
                
                // Add error if present
                if let Some(error) = obj.get("error").and_then(|v| v.as_str()) {
                    formatted.push_str(&format!("\n\nError: {}", error));
                }
                
                // Add success status if present
                if let Some(success) = obj.get("success").and_then(|v| v.as_bool()) {
                    if !success {
                        formatted.push_str("\n\n❌ Operation failed");
                    }
                }
                
                return formatted;
            }
        }
        
        // For other JSON, pretty-print it
        if let Ok(formatted) = serde_json::to_string_pretty(&json_value) {
            return formatted;
        }
    }
    // If not JSON or formatting failed, return as-is
    content.to_string()
}

#[component]
pub fn ProgramTab() -> impl IntoView {
    let program = use_context::<Program>().expect("program should exist in context");
    let runtime = use_context::<Runtime>().expect("runtime should exist in context");
    let lsp_client = use_context::<LspClient>();
    let mcp_client = use_context::<MCPClient>();
    let lsp_client_for_hover = lsp_client.clone();
    let lsp_client_for_hover_tooltip = lsp_client.clone();
    let lsp_client_for_filtering = lsp_client.clone();
    let lsp_client_for_completion_request = lsp_client.clone();
    let lsp_client_for_completion_request_key = lsp_client.clone();
    let lsp_client_for_dropdown_check = lsp_client.clone();
    let textarea_ref = create_node_ref::<html::Textarea>();
    let line_numbers_ref = create_node_ref::<html::Div>();
    let highlight_overlay_ref = create_node_ref::<html::Pre>();

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
                        
                        // Update item count for arrow key navigation
                        completion_items_count.set(filtered.len());
                        filtered_completion_items.set(filtered);
                        
                        // Reset selection index when filtered items change
                        selected_completion_index.set(0);
                        return;
                    } else {
                        // Cursor is exactly at trigger position - show all completions
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
    // Signal to store error diagnostics for the current hover line
    let hover_error_diagnostics = create_rw_signal::<Option<Vec<crate::lsp::Diagnostic>>>(None);
    let hover_debounce_timer = create_rw_signal(0);
    let current_hover_word = create_rw_signal(Option::<(u32, u32)>::None); // Track (line, character) of current hover word

    // Chat state
    let chat_messages = create_rw_signal::<Vec<(String, String)>>(Vec::new()); // (role, content)
    let chat_input = create_rw_signal(String::new());
    let chat_input_ref = create_node_ref::<html::Textarea>();
    let pending_chat_request = create_rw_signal(Option::<i32>::None);
    
    // Effect to watch for chat responses and process them reactively
    if let Some(mcp) = mcp_client.as_ref() {
        let mcp_clone = mcp.clone();
        let chat_messages_for_effect = chat_messages.clone();
        let pending_chat_request_for_effect = pending_chat_request.clone();
        
        create_effect(move |_| {
            // Track chat_responses signal to trigger when responses arrive
            let responses = mcp_clone.chat_responses();
            let pending_id = pending_chat_request_for_effect.get();
            
            if let Some(request_id) = pending_id {
                responses.with(|responses_map| {
                    if let Some(response) = responses_map.get(&request_id) {
                        // Format the response content
                        let formatted_content = format_chat_response(&response.content);
                        
                        // Add tool info if available and not empty
                        let final_content = if let Some(ref tool) = response.tool_used {
                            if !tool.is_empty() {
                                format!("{}\n\n_Used tool: `{}`_", formatted_content, tool)
                            } else {
                                formatted_content
                            }
                        } else {
                            formatted_content
                        };
                        
                        chat_messages_for_effect.update(|messages| {
                            messages.push(("assistant".to_string(), final_content));
                        });
                        pending_chat_request_for_effect.set(None);
                    }
                });
            }
        });
    }

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

    // Track typing activity to prefer fast local highlighting while user is typing
    let typing_debounce_token = create_rw_signal(0u64);
    let typing_active = create_rw_signal(false);
    // Keep textarea text visible while overlay is being refreshed (prevents "invisible" gap)
    let overlay_dirty = create_rw_signal(false);
    // Prevent multiple simultaneous overlay updates
    let overlay_update_in_progress = create_rw_signal(false);

    let lsp_for_update = lsp_client.clone();
    let lsp_client_for_typing_outer = lsp_client.clone();
    let lsp_client_for_brackets_outer = lsp_client.clone();
    let lsp_client_for_memo_outer = lsp_client.clone();
    let update_program_text = move |event: ev::Event| {
        let new_text = event_target_value(&event);
        program.text.set(new_text.clone());

        // Debug: log current input/update metrics
        let lines = new_text.matches('\n').count() + 1;
        if let Some(el) = textarea_ref.get() {
            let st = el.scroll_top();
            let sl = el.scroll_left();
            let ss = el.selection_start().ok().flatten().unwrap_or_default();
        crate::logging::log_event(
            "INPUT",
            serde_json::json!({
                "len": new_text.len(),
                "lines": lines,
                "sel_start": ss,
                "scroll_top": st,
                "scroll_left": sl,
            }),
        );
        } else {
            crate::logging::log_event(
                "INPUT_NO_TA",
                serde_json::json!({"len": new_text.len(), "lines": lines}),
            );
        }

        // Update overlay while typing - use a debounced approach to batch rapid keystrokes
        let textarea_ref_clone2 = textarea_ref.clone();
        let highlight_overlay_ref_clone2 = highlight_overlay_ref.clone();
        let new_text_clone = new_text.clone();
        let overlay_update_in_progress_clone = overlay_update_in_progress.clone();
        let typing_debounce_token_clone = typing_debounce_token.clone();
        let lsp_client_for_typing = lsp_client_for_typing_outer.clone();
        let document_uri_for_typing = document_uri.to_string();
        
        // Cancel any pending overlay update and schedule a new one
        typing_debounce_token_clone.update(|t| *t += 1);
        let update_token = typing_debounce_token_clone.get_untracked();
        
        spawn_local(async move {
            // Small delay to batch rapid keystrokes (16ms = ~60fps)
            gloo_timers::future::TimeoutFuture::new(16).await;
            
            // Only proceed if this is still the latest update request
            if typing_debounce_token_clone.get_untracked() == update_token {
            if let Some(highlight) = highlight_overlay_ref_clone2.get_untracked() {
                    overlay_update_in_progress_clone.set(true);
                    
                    // Read the actual textarea value to ensure overlay matches exactly
                    // This prevents sync issues between textarea and overlay
                    let text_for_overlay = if let Some(textarea) = textarea_ref_clone2.get_untracked() {
                        textarea.value()
                    } else {
                        new_text_clone
                    };
                    
                    // Fast local highlight while typing - use exact textarea value
                    let mut html = highlight_code(&text_for_overlay);
                    
                    // Apply error indicators if diagnostics are available
                    if let Some(lsp) = lsp_client_for_typing.as_ref() {
                        let diagnostics = lsp.get_diagnostics_for_uri(&document_uri_for_typing);
                        html = add_error_indicators(&html, &diagnostics);
                    }
                    
                highlight.set_inner_html(&html);
                if let Some(textarea) = textarea_ref_clone2.get_untracked() {
                    let _ = highlight.set_scroll_top(textarea.scroll_top());
                    let _ = highlight.set_scroll_left(textarea.scroll_left());
                    }
                    overlay_update_in_progress_clone.set(false);
                }
            }
        });

        // Mark typing as active and debounce turning it off
        typing_active.set(true);
        // Note: typing_debounce_token is already incremented above for overlay updates
        let token = typing_debounce_token.get_untracked();
        let typing_active_setter = typing_active.clone();
        spawn_local(async move {
            // 200ms idle threshold to exit typing mode
            gloo_timers::future::TimeoutFuture::new(200).await;
            if typing_debounce_token.get_untracked() == token {
                typing_active_setter.set(false);
            }
        });

        // Send LSP update (debouncing handled by gloo_timers)
        if let Some(lsp) = lsp_for_update.as_ref() {
            let lsp = lsp.clone();
            let uri = document_uri.to_string();
            spawn_local(async move {
                // Wait 400ms before sending update to avoid spamming LSP server
                gloo_timers::future::TimeoutFuture::new(400).await;
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
    // Helper function to check if we're inside a string or comment
    let is_in_string_or_comment = |text: &str, pos: usize| -> bool {
        if pos >= text.len() {
            return false;
        }
        
        let before = &text[..pos];
        let mut in_string = false;
        let mut in_char = false;
        let mut in_comment = false;
        let mut in_line_comment = false;
        let mut string_delimiter = '"';
        let mut chars = before.chars().peekable();
        
        while let Some(ch) = chars.next() {
            if in_line_comment {
                if ch == '\n' {
                    in_line_comment = false;
                }
                continue;
            }
            
            if in_comment {
                if ch == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    in_comment = false;
                }
                continue;
            }
            
            if in_string || in_char {
                if (in_string && ch == string_delimiter) || (in_char && ch == '\'') {
                    // Check if escaped
                    let mut backslash_count = 0;
                    let mut check_pos = before.len().saturating_sub(2);
                    while check_pos < before.len() && check_pos > 0 {
                        if let Some(c) = before.chars().nth(check_pos) {
                            if c == '\\' {
                                backslash_count += 1;
                                check_pos = check_pos.saturating_sub(1);
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    
                    if backslash_count % 2 == 0 {
                        in_string = false;
                        in_char = false;
                    }
                }
                continue;
            }
            
            if ch == '"' || ch == '\'' {
                in_string = ch == '"';
                in_char = ch == '\'';
                string_delimiter = ch;
            } else if ch == '/' && chars.peek() == Some(&'/') {
                chars.next();
                in_line_comment = true;
            } else if ch == '/' && chars.peek() == Some(&'*') {
                chars.next();
                in_comment = true;
            }
        }
        
        in_string || in_char || in_comment || in_line_comment
    };
    
    // Helper function to insert text at cursor position
    let textarea_ref_for_insert = textarea_ref.clone();
    let insert_text_at_cursor = move |text: &str, insert: &str| -> Option<String> {
        if let Some(element) = textarea_ref_for_insert.get() {
            if let Ok(Some(start)) = element.selection_start() {
                if let Ok(Some(end)) = element.selection_end() {
                    let start_pos = start as usize;
                    let end_pos = end as usize;
                    let mut new_text = text.to_string();
                    new_text.replace_range(start_pos..end_pos, insert);
                    return Some(new_text);
                }
            }
        }
        None
    };

    // Clone lsp_client for bracket handler and memo
    let lsp_client_for_brackets = lsp_client_for_brackets_outer.clone();
    let lsp_client_for_memo = lsp_client_for_memo_outer.clone();
    let document_uri_for_brackets = document_uri.to_string();

    let handle_keydown = move |event: ev::KeyboardEvent| {
        let is_completions_shown = show_completions.get_untracked();
        let key = event.key();

        if key == "Enter" {
            // Handle auto-indentation when Enter is pressed (only if completions are not shown)
            if !is_completions_shown {
                if let Some(element) = textarea_ref.get() {
                    if let Ok(Some(cursor_pos)) = element.selection_start() {
                        let cursor_pos_usize = cursor_pos as usize;
                        let text = program.text.get_untracked();
                        
                        // Find the start of the current line
                        let line_start = text[..cursor_pos_usize]
                            .rfind('\n')
                            .map(|pos| pos + 1)
                            .unwrap_or(0);
                        
                        // Get the current line content up to the cursor
                        let current_line = &text[line_start..cursor_pos_usize];
                        
                        // Calculate indentation (count leading spaces or tabs)
                        let mut indent = String::new();
                        for ch in current_line.chars() {
                            if ch == ' ' || ch == '\t' {
                                indent.push(ch);
                            } else {
                                break;
                            }
                        }
                        
                        // If the line ends with an opening brace, add an extra level of indentation
                        let trimmed_line = current_line.trim();
                        if trimmed_line.ends_with('{') || trimmed_line.ends_with('(') || trimmed_line.ends_with('[') {
                            // Add one more level of indentation (use spaces, assuming 4 spaces per indent)
                            indent.push_str("    ");
                        }
                        
                        // Insert newline + indentation
                        event.prevent_default();
                        let newline_with_indent = format!("\n{}", indent);
                        
                        if let Some(new_text) = insert_text_at_cursor(&text, &newline_with_indent) {
                            program.text.set(new_text.clone());
                            
                            // Position cursor after the indentation
                            let new_cursor_pos = cursor_pos_usize + newline_with_indent.len();
                            let _ = element.set_selection_range(new_cursor_pos as u32, new_cursor_pos as u32);
                            
                            // Immediately update overlay with error indicators
                            overlay_update_in_progress.set(true);
                            if let Some(overlay) = highlight_overlay_ref.get_untracked() {
                                let mut html = highlight_code(&new_text);
                                // Apply error indicators if diagnostics are available
                                if let Some(lsp) = lsp_client_for_brackets.as_ref() {
                                    let diagnostics = lsp.get_diagnostics_for_uri(&document_uri_for_brackets);
                                    html = add_error_indicators(&html, &diagnostics);
                                }
                                overlay.set_inner_html(&html);
                                if let Some(ta) = textarea_ref.get_untracked() {
                                    let _ = overlay.set_scroll_top(ta.scroll_top());
                                    let _ = overlay.set_scroll_left(ta.scroll_left());
                                }
                            }
                            overlay_update_in_progress.set(false);
                            
                            // Mark typing as active
                            typing_active.set(true);
                            typing_debounce_token.update(|t| *t += 1);
                            let token = typing_debounce_token.get_untracked();
                            let typing_active_setter = typing_active.clone();
                            spawn_local(async move {
                                gloo_timers::future::TimeoutFuture::new(200).await;
                                if typing_debounce_token.get_untracked() == token {
                                    typing_active_setter.set(false);
                                }
                            });
                            
                            // Send LSP update (debounced)
                            if let Some(lsp) = lsp_client_for_brackets.as_ref() {
                                let lsp = lsp.clone();
                                let uri = document_uri_for_brackets.clone();
                                let text_for_lsp = new_text.clone();
                                spawn_local(async move {
                                    gloo_timers::future::TimeoutFuture::new(400).await;
                                    if let Err(e) = lsp.did_change(uri, text_for_lsp) {
                                        log::warn!("Failed to update document in LSP: {}", e);
                                    }
                                });
                            }
                            
                            return;
                        }
                    }
                }
            }
            
            // Log Enter key for debugging (if not handled above)
            if let Some(el) = textarea_ref.get() {
                let st = el.scroll_top();
                let ss = el.selection_start().ok().flatten().unwrap_or_default();
                log::info!("KEYDOWN: Enter sel_start={} scroll_top={}", ss, st);
            } else {
                log::info!("KEYDOWN: Enter");
            }
        }

        // Handle bracket auto-completion before other handlers
        if !is_completions_shown {
            let bracket_pairs = [
                ('(', ')'),
                ('[', ']'),
                ('{', '}'),
            ];
            
            for (open, close) in bracket_pairs.iter() {
                if key == open.to_string() {
                    if let Some(element) = textarea_ref.get() {
                        if let Ok(Some(start)) = element.selection_start() {
                            let text = program.text.get_untracked();
                            let pos = start as usize;
                            
                            // Don't auto-close if we're in a string or comment
                            if is_in_string_or_comment(&text, pos) {
                                return; // Let default behavior handle it
                            }
                            
                            // Check if there's already a closing bracket immediately after
                            if pos < text.len() {
                                let after = &text[pos..];
                                if let Some(next_char) = after.chars().next() {
                                    if next_char == *close {
                                        // There's already a closing bracket, just move cursor forward
                                        event.prevent_default();
                                        let _ = element.set_selection_range(start + 1, start + 1);
                                        return;
                                    }
                                }
                            }
                            
                            // Insert opening bracket + closing bracket, position cursor between them
                            event.prevent_default();
                            let pair = format!("{}{}", open, close);
                            if let Some(new_text) = insert_text_at_cursor(&text, &pair) {
                                program.text.set(new_text.clone());
                                // Position cursor between the brackets
                                let _ = element.set_selection_range(start + 1, start + 1);
                                
                                // Immediately update overlay since we prevented default input event
                                overlay_update_in_progress.set(true);
                                if let Some(overlay) = highlight_overlay_ref.get_untracked() {
                                    let html = highlight_code(&new_text);
                                    overlay.set_inner_html(&html);
                                    if let Some(ta) = textarea_ref.get_untracked() {
                                        let _ = overlay.set_scroll_top(ta.scroll_top());
                                        let _ = overlay.set_scroll_left(ta.scroll_left());
                                    }
                                }
                                overlay_update_in_progress.set(false);
                                
                                // Mark typing as active
                                typing_active.set(true);
                                typing_debounce_token.update(|t| *t += 1);
                                let token = typing_debounce_token.get_untracked();
                                let typing_active_setter = typing_active.clone();
                                spawn_local(async move {
                                    gloo_timers::future::TimeoutFuture::new(200).await;
                                    if typing_debounce_token.get_untracked() == token {
                                        typing_active_setter.set(false);
                                    }
                                });
                                
                                // Send LSP update (debounced)
                                if let Some(lsp) = lsp_client_for_brackets.as_ref() {
                                    let lsp = lsp.clone();
                                    let uri = document_uri_for_brackets.clone();
                                    let text_for_lsp = new_text.clone();
                                    spawn_local(async move {
                                        gloo_timers::future::TimeoutFuture::new(400).await;
                                        if let Err(e) = lsp.did_change(uri, text_for_lsp) {
                                            log::warn!("Failed to update document in LSP: {}", e);
                                        }
                                    });
                                }
                            }
                            return;
                        }
                    }
                }
                
                // Handle closing bracket - if there's already a closing bracket, skip it
                if key == close.to_string() {
                    if let Some(element) = textarea_ref.get() {
                        if let Ok(Some(start)) = element.selection_start() {
                            let text = program.text.get_untracked();
                            let pos = start as usize;
                            
                            // Don't skip if we're in a string or comment
                            if is_in_string_or_comment(&text, pos) {
                                return; // Let default behavior handle it
                            }
                            
                            // Check if there's already a closing bracket immediately after
                            if pos < text.len() {
                                let after = &text[pos..];
                                if let Some(next_char) = after.chars().next() {
                                    if next_char == *close {
                                        // Skip the existing closing bracket
                                        event.prevent_default();
                                        let _ = element.set_selection_range(start + 1, start + 1);
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

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
                        crate::logging::log_event("COMPLETION_ACCEPT", serde_json::json!({"label": label}));
                        // Close dropdown FIRST before updating text to prevent effect from interfering
                        show_completions.set(false);
                        
                        // Find the actual word boundaries to replace
                        completion_trigger_pos.set(None);
                        
                        // Insert at cursor, replacing the partial word
                        if let Some(element) = textarea_ref.get() {
                            if let Ok(Some(cursor_pos)) = element.selection_start() {
                                let cursor_pos_usize = cursor_pos as usize;
                                let text = program.text.get_untracked();
                                
                                // Find the start of the identifier being completed
                                // Go backwards from cursor until we hit a non-identifier character
                                let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';
                                let mut word_start = cursor_pos_usize;
                                
                                // Find the start of the word (identifier) - don't include ':' in identifier
                                for (i, ch) in text[..cursor_pos_usize].char_indices().rev() {
                                    if is_ident_char(ch) {
                                        word_start = i;
                                    } else {
                                        break;
                                    }
                                }
                                
                                // Use the word start as the replacement start
                                // This will replace just the partial identifier (e.g., "bi" in "jet::bi")
                                let replace_start = word_start;
                                
                                // Replace from word start to cursor position
                                    program.text.update(|text| {
                                    text.replace_range(replace_start..cursor_pos_usize, &label);
                                });
                                
                                // Calculate new cursor position
                                let new_pos = replace_start + label.len();
                                let _ = element.set_selection_range(new_pos as u32, new_pos as u32);

                                    // Immediately refresh overlay with local highlight
                                    overlay_dirty.set(true);
                                overlay_update_in_progress.set(true);
                                    if let Some(overlay) = highlight_overlay_ref.get_untracked() {
                                        let html = highlight_code(&program.text.get_untracked());
                                        overlay.set_inner_html(&html);
                                        if let Some(ta) = textarea_ref.get_untracked() {
                                            let _ = overlay.set_scroll_top(ta.scroll_top());
                                            let _ = overlay.set_scroll_left(ta.scroll_left());
                                        }
                                    }
                                overlay_update_in_progress.set(false);
                                    // Queue a next-tick refresh to catch layout
                                    {
                                        let highlight_ref = highlight_overlay_ref.clone();
                                        let textarea_ref_clone = textarea_ref.clone();
                                        let text_snapshot = program.text.get_untracked();
                                    let overlay_update_flag = overlay_update_in_progress.clone();
                                        spawn_local(async move {
                                            gloo_timers::future::TimeoutFuture::new(0).await;
                                        overlay_update_flag.set(true);
                                            if let Some(overlay) = highlight_ref.get_untracked() {
                                                let html = highlight_code(&text_snapshot);
                                                overlay.set_inner_html(&html);
                                                if let Some(ta) = textarea_ref_clone.get_untracked() {
                                                    let _ = overlay.set_scroll_top(ta.scroll_top());
                                                    let _ = overlay.set_scroll_left(ta.scroll_left());
                                                }
                                            }
                                        overlay_update_flag.set(false);
                                            // Overlay updated; clear dirty flag
                                            overlay_dirty.set(false);
                                        });
                                    }

                                    // Debounced didChange to LSP post-completion
                                    if let Some(lsp) = lsp_client_for_completion_request_key.as_ref() {
                                        let lsp = lsp.clone();
                                        let uri = document_uri.to_string();
                                        let text_for_lsp = program.text.get_untracked();
                                        spawn_local(async move {
                                            gloo_timers::future::TimeoutFuture::new(300).await;
                                            crate::logging::log_event(
                                                "LSP_DID_CHANGE_AFTER_COMPLETION",
                                                serde_json::json!({"uri": uri}),
                                            );
                                            if let Err(e) = lsp.did_change(uri, text_for_lsp) {
                                                log::warn!("Failed to update document in LSP after completion: {}", e);
                                            }
                                        });
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
            if let Some(lsp) = lsp_client_for_completion_request_key.as_ref() {
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
    
    // Track the last "idle" text state - only update when typing stops
    // This prevents the memo from recalculating on every keystroke
    // Initialize with current text to ensure overlay is rendered on load
    let idle_text = create_rw_signal(program.text.get());
    
    // Update idle_text when typing stops
    let idle_text_updater = idle_text.clone();
    let program_text_for_idle = program.text.clone();
    let typing_active_for_idle = typing_active.clone();
    create_effect(move |_| {
        let is_typing = typing_active_for_idle.get();
        if !is_typing {
            // When typing stops, update idle_text to trigger memo recalculation
            idle_text_updater.set(program_text_for_idle.get());
        }
    });
    
    // Also update idle_text when program.text changes externally (e.g., from example selection)
    // This ensures the overlay refreshes when examples are selected
    let idle_text_external = idle_text.clone();
    let program_text_external = program.text.clone();
    let typing_active_external = typing_active.clone();
    create_effect(move |_| {
        // Track program.text changes
        let current_text = program_text_external.get();
        let is_typing = typing_active_external.get_untracked();
        
        // Only update if we're not typing (to avoid conflicts with typing updates)
        if !is_typing {
            let text_for_idle = current_text.clone();
            idle_text_external.set(text_for_idle);
        }
    });
    
    // Create memoized highlighted code
    // - Only recalculates when idle_text changes (i.e., when typing stops) or LSP data changes
    // - This prevents flickering during typing
    let highlighted_code = create_memo(move |_| {
        let text = idle_text.get();
        
        // When idle, try to use LSP semantic tokens if available
        let html = if let Some(lsp) = lsp_client_for_memo.as_ref() {
            let map = lsp.semantic_html().get();
            if let Some(html) = map.get(document_uri) {
                let text_lines = text.matches('\n').count() + 1;
                let html_lines = html.matches('\n').count() + 1;
                if html_lines == text_lines {
                    html.clone()
                } else {
                    highlight_code(&text)
                }
            } else {
                highlight_code(&text)
            }
        } else {
        highlight_code(&text)
        };
        
        // Apply error indicators if diagnostics are available
        // Access diagnostics signal directly to make memo reactive to diagnostics changes
        if let Some(lsp) = lsp_client_for_memo.as_ref() {
            // Access the diagnostics signal to make the memo track it
            let diagnostics_map = lsp.diagnostics().get();
            let diagnostics = diagnostics_map.get(document_uri).cloned().unwrap_or_default();
            add_error_indicators(&html, &diagnostics)
        } else {
            html
        }
    });
    
    // Update the overlay's innerHTML when highlighted code changes (only when not typing)
    // Throttle overlay DOM updates to avoid expensive innerHTML sets
    let is_updating = create_rw_signal(false);
    let pending_html = create_rw_signal::<Option<String>>(None);
    let overlay_update_in_progress_for_effect = overlay_update_in_progress.clone();
    
    // Effect to update overlay when not typing - completely disabled during typing
    // Runs when typing stops (typing_active becomes false), when highlighted_code changes, or on initial mount
    let typing_active_for_effect = typing_active.clone();
    
    // Initialize overlay on mount - use effect to wait for element to be mounted
    let highlight_ref_init = highlight_overlay_ref.clone();
    let textarea_ref_init = textarea_ref.clone();
    let highlighted_code_init = highlighted_code.clone();
    let overlay_initialized = create_rw_signal(false);
    create_effect(move |_| {
        // Track when the element becomes available
        if let Some(element) = highlight_ref_init.get() {
            // Only initialize once when element is first mounted
            if !overlay_initialized.get_untracked() {
                overlay_initialized.set(true);
                let html = highlighted_code_init.get();
                element.set_inner_html(&html);
                if let Some(textarea) = textarea_ref_init.get() {
                    let _ = element.set_scroll_top(textarea.scroll_top());
                    let _ = element.set_scroll_left(textarea.scroll_left());
                }
            }
        }
    });
    
    // Force immediate overlay and textarea update when program.text changes externally (e.g., example selection)
    // This runs after highlighted_code memo is defined
    let highlight_overlay_ref_force = highlight_overlay_ref.clone();
    let textarea_ref_force = textarea_ref.clone();
    let program_text_force = program.text.clone();
    let typing_active_force = typing_active.clone();
    let lsp_client_for_force_spawn = lsp_client.clone();
    let document_uri_for_force_spawn = document_uri.to_string();
    let previous_text = create_rw_signal::<Option<String>>(None);
    create_effect(move |_| {
        // Track program.text changes
        let current_text = program_text_force.get();
        let is_typing = typing_active_force.get_untracked();
        let prev_text = previous_text.get_untracked();
        
        // Only force update if we're not typing and text actually changed (external changes like example selection)
        if !is_typing && prev_text.as_ref() != Some(&current_text) {
            previous_text.set(Some(current_text.clone()));
            
            // Immediately update textarea value to ensure it's in sync with program.text
            if let Some(textarea) = textarea_ref_force.get_untracked() {
                let text_for_textarea = current_text.clone();
                let _ = textarea.set_value(&text_for_textarea);
                
                // Use a microtask to ensure textarea DOM is updated, then read its value
                // This guarantees the overlay matches the exact textarea content
                let highlight_ref = highlight_overlay_ref_force.clone();
                let textarea_clone = textarea_ref_force.clone();
                let lsp_client_for_force_clone = lsp_client_for_force_spawn.clone();
                let document_uri_for_force_clone = document_uri_for_force_spawn.clone(); // String clone is needed here
                spawn_local(async move {
                    // Use requestAnimationFrame to ensure DOM is updated
                    gloo_timers::future::TimeoutFuture::new(0).await;
                    
                    if let Some(textarea) = textarea_clone.get_untracked() {
                        // Read the actual textarea value from DOM
                        let textarea_value = textarea.value();
                        
                        // Update overlay with the exact textarea content
                        if let Some(overlay) = highlight_ref.get_untracked() {
                            let mut html = highlight_code(&textarea_value);
                            
                            // Apply error indicators if diagnostics are available
                            if let Some(lsp) = lsp_client_for_force_clone.as_ref() {
                                let diagnostics = lsp.get_diagnostics_for_uri(&document_uri_for_force_clone);
                                html = add_error_indicators(&html, &diagnostics);
                            }
                            
                            overlay.set_inner_html(&html);
                            let _ = overlay.set_scroll_top(textarea.scroll_top());
                            let _ = overlay.set_scroll_left(textarea.scroll_left());
                        }
                    }
                });
            }
        } else if !is_typing {
            // Update previous_text even if we don't force update
            previous_text.set(Some(current_text));
        }
    });
    
    create_effect(move |_| {
        // Track typing_active to know when typing starts/stops
        let is_typing = typing_active_for_effect.get();
        
        // Track highlighted_code to trigger updates when it changes
        let html = highlighted_code.get();
        
        // Skip entirely while typing - input handler manages updates during typing
        if is_typing {
            return;
        }
        
        // Also skip if an update is already in progress (from input handler or completion handler)
        if overlay_update_in_progress_for_effect.get_untracked() {
            return;
        }
        
        // Update when typing has stopped or highlighted code changed
        // This will use LSP semantic tokens if available, otherwise local highlighting
        pending_html.set(Some(html));

        if !is_updating.get_untracked() {
            is_updating.set(true);
            overlay_update_in_progress_for_effect.set(true);
            let is_updating_flag = is_updating.clone();
            let overlay_update_flag = overlay_update_in_progress_for_effect.clone();
            let highlight_ref = highlight_overlay_ref.clone();
            let textarea_ref_clone = textarea_ref.clone();
            spawn_local(async move {
                // Small delay to batch updates
                gloo_timers::future::TimeoutFuture::new(16).await;
                
                // Double-check we're still not typing and no update is in progress
                if typing_active_for_effect.get_untracked() || overlay_update_flag.get_untracked() {
                    is_updating_flag.set(false);
                    overlay_update_flag.set(false);
                    return;
                }
                
                if let Some(element) = highlight_ref.get_untracked() {
                    if let Some(html) = pending_html.get_untracked() {
                        element.set_inner_html(&html);
                        if let Some(textarea) = textarea_ref_clone.get_untracked() {
                            let _ = element.set_scroll_top(textarea.scroll_top());
                            let _ = element.set_scroll_left(textarea.scroll_left());
                        }
                    }
                }
                is_updating_flag.set(false);
                overlay_update_flag.set(false);
            });
        }
    });
    
    // Separate effect to immediately update overlay when diagnostics change
    // This ensures errors are shown as soon as they're detected, even if user is typing
    let highlight_overlay_ref_diagnostics = highlight_overlay_ref.clone();
    let textarea_ref_diagnostics = textarea_ref.clone();
    let lsp_client_for_diagnostics_effect = lsp_client.clone();
    let document_uri_for_diagnostics_effect = document_uri.to_string();
    create_effect(move |_| {
        // Track diagnostics changes to trigger immediate overlay update
        if let Some(lsp) = lsp_client_for_diagnostics_effect.as_ref() {
            let diagnostics_map = lsp.diagnostics().get();
            let diagnostics = diagnostics_map.get(&document_uri_for_diagnostics_effect).cloned().unwrap_or_default();
            
            // Update overlay immediately when diagnostics change (whether errors exist or are cleared)
            if let Some(overlay) = highlight_overlay_ref_diagnostics.get_untracked() {
                if let Some(textarea) = textarea_ref_diagnostics.get_untracked() {
                    // Read actual textarea value to ensure sync
                    let text_for_overlay = textarea.value();
                    let mut html = highlight_code(&text_for_overlay);
                    // Apply error indicators (will be empty if no errors)
                    html = add_error_indicators(&html, &diagnostics);
                    overlay.set_inner_html(&html);
                    let _ = overlay.set_scroll_top(textarea.scroll_top());
                    let _ = overlay.set_scroll_left(textarea.scroll_left());
                }
            }
        }
    });

    // Helper function to extract word at or near a given character position
    // This allows hover to work when the mouse is near a word, not just directly on it
    let extract_word_at_position = |text: &str, pos: usize| -> Option<(usize, usize)> {
        if text.is_empty() {
            return None;
        }
        
        let chars: Vec<char> = text.chars().collect();
        if pos >= chars.len() {
            return None;
        }
        
        let is_word_char = |ch: char| ch.is_alphanumeric() || ch == '_';
        
        // Strategy: Find the nearest word character within a small range
        // This makes the hover area work for the whole word, not just the exact character
        
        // First, check if we're directly on a word character
        if is_word_char(chars[pos]) {
            // We're on a word character - find the word boundaries
            let mut start = pos;
            while start > 0 && is_word_char(chars[start - 1]) {
                start -= 1;
            }
            
            let mut end = pos;
            while end < chars.len() && is_word_char(chars[end]) {
                end += 1;
            }
            
            if start < end {
                return Some((start, end));
            }
        }
        
        // If not directly on a word, search nearby (within 3 characters) for a word
        // This makes the hover area larger and more forgiving
        let search_range = 3;
        let search_start = pos.saturating_sub(search_range);
        let search_end = (pos + search_range + 1).min(chars.len());
        
        // Find the closest word character to our position
        let mut best_word: Option<(usize, usize)> = None;
        let mut best_distance = usize::MAX;
        
        for i in search_start..search_end {
            if is_word_char(chars[i]) {
                // Found a word character - find the word boundaries
                let mut start = i;
                while start > 0 && is_word_char(chars[start - 1]) {
                    start -= 1;
                }
                
                let mut end = i;
                while end < chars.len() && is_word_char(chars[end]) {
                    end += 1;
                }
                
                if start < end {
                    // Calculate distance from mouse position to word
                    // Distance is 0 if mouse is within word, otherwise distance to nearest edge
                    let distance = if pos >= start && pos < end {
                        0 // Mouse is within the word
                    } else if pos < start {
                        start - pos // Distance to word start
                    } else {
                        pos - end + 1 // Distance to word end
                    };
                    
                    // Keep the closest word
                    if distance < best_distance {
                        best_distance = distance;
                        best_word = Some((start, end));
                    }
                }
            }
        }
        
        best_word
    };
    
    // Helper function to calculate character position from mouse coordinates
    // Uses a more accurate method that accounts for padding and textarea styling
    let calculate_char_position_from_mouse = |textarea: &web_sys::HtmlTextAreaElement, text: &str, offset_x: f64, offset_y: f64| -> Option<usize> {
        // Get textarea style to calculate character dimensions
        let window = web_sys::window()?;
        let computed_style = window
            .get_computed_style(textarea)
            .ok()??;
        
        // Get font metrics - use a more accurate method
        // Create a temporary span to measure character width
        let char_width = {
            let document = window.document()?;
            let test_span = document.create_element("span").ok()?;
            let test_span_elem: &web_sys::HtmlElement = test_span.dyn_ref()?;
            test_span_elem.set_text_content(Some("M"));
            test_span_elem.style().set_property("position", "absolute").ok()?;
            test_span_elem.style().set_property("visibility", "hidden").ok()?;
            test_span_elem.style().set_property("white-space", "pre").ok()?;
            
            let font_family = computed_style.get_property_value("font-family").ok()?;
            let font_size = computed_style.get_property_value("font-size").ok()?;
            test_span_elem.style().set_property("font-family", &font_family).ok()?;
            test_span_elem.style().set_property("font-size", &font_size).ok()?;
            
            textarea.parent_element()?.append_child(test_span_elem).ok()?;
            let width = test_span_elem.offset_width() as f64;
            let _ = textarea.parent_element()?.remove_child(test_span_elem);
            
            if width > 0.0 {
                width
            } else {
                7.2 // Fallback to approximate width
            }
        };
        
        // Get line height
        let line_height = {
            let line_height_str = computed_style.get_property_value("line-height").ok()?;
            if let Ok(height) = line_height_str.trim_end_matches("px").parse::<f64>() {
                height
            } else {
                18.0 // Fallback
            }
        };
        
        // Account for textarea padding (if any)
        // Get padding-left to adjust offset_x
        let padding_left = computed_style
            .get_property_value("padding-left")
            .ok()?
            .trim_end_matches("px")
            .parse::<f64>()
            .unwrap_or(0.0);
        
        let padding_top = computed_style
            .get_property_value("padding-top")
            .ok()?
            .trim_end_matches("px")
            .parse::<f64>()
            .unwrap_or(0.0);
        
        // Adjust offset to account for padding
        let adjusted_x = (offset_x - padding_left).max(0.0);
        let adjusted_y = (offset_y - padding_top).max(0.0);
        
        // Calculate which line we're on
        let scroll_top = textarea.scroll_top() as f64;
        let line_index = ((adjusted_y + scroll_top) / line_height).floor() as usize;
        
        // Calculate character position in that line
        let char_index = (adjusted_x / char_width).floor() as usize;
        
        // Convert line/char to absolute character position
        let lines: Vec<&str> = text.split('\n').collect();
        if line_index >= lines.len() {
            return None;
        }
        
        let mut char_pos = 0;
        for (i, line) in lines.iter().enumerate() {
            if i == line_index {
                // We're on this line
                let char_in_line = char_index.min(line.chars().count());
                return Some(char_pos + char_in_line);
            }
            char_pos += line.chars().count() + 1; // +1 for newline
        }
        
        None
    };

    // Effect to show/hide hover tooltip based on hover_info
    // Don't clear hover if we have error diagnostics (error tooltips take priority)
    let lsp_client_for_hover_effect = lsp_client_for_hover.clone();
    create_effect(move |_| {
        // Check if we have error diagnostics first - if so, don't interfere
        if hover_error_diagnostics.get().is_some() {
            return;
        }
        
        if let Some(lsp) = lsp_client_for_hover_effect.as_ref() {
            let hover_info = lsp.hover_info();
            let has_hover_data = hover_info.get().is_some();
            let current_word = current_hover_word.get();
            
            // Only show hover if we have data and we're still hovering over the same word
            if has_hover_data && current_word.is_some() {
                show_hover.set(true);
            } else {
                show_hover.set(false);
            }
        }
    });

    // Handle mouse move for hover with debouncing
    let handle_mouse_move = move |event: ev::MouseEvent| {
        // Clear any existing hover when completions are shown
        if show_completions.get_untracked() {
            show_hover.set(false);
            current_hover_word.set(None);
            hover_debounce_timer.update(|t| *t += 1); // Cancel pending hovers
            return;
        }

        if let Some(lsp) = lsp_client_for_hover.as_ref() {
            if let Some(element) = textarea_ref.get() {
                let text = program.text.get_untracked();
                
                // Calculate character position from mouse coordinates
                let offset_x = event.offset_x() as f64;
                let offset_y = event.offset_y() as f64;
                
                let char_pos = match calculate_char_position_from_mouse(&element, &text, offset_x, offset_y) {
                    Some(pos) => pos,
                    None => {
                        show_hover.set(false);
                        current_hover_word.set(None);
                        hover_debounce_timer.update(|t| *t += 1); // Cancel pending hovers
                        return;
                    }
                };
                
                // Extract word at this position
                let word_range = match extract_word_at_position(&text, char_pos) {
                    Some(range) => {
                        range
                    },
                    None => {
                        show_hover.set(false);
                        current_hover_word.set(None);
                        hover_debounce_timer.update(|t| *t += 1); // Cancel pending hovers
                        return;
                    }
                };
                
                // Set hover position for tooltip (position it near the mouse)
                hover_position.set((
                    event.client_x() as f64 + 10.0,
                    event.client_y() as f64 + 10.0,
                ));
                
                // Calculate line and character for LSP request (use start of word)
                // Always use the word's start position, not the mouse position
                let before_word = &text[..word_range.0];
                let line = before_word.matches('\n').count() as u32;
                let line_start = before_word.rfind('\n').map(|p| p + 1).unwrap_or(0);
                let character = (word_range.0 - line_start) as u32;
                
                // Check for errors on this line
                let lsp_for_errors = lsp_client_for_hover.clone();
                let document_uri_for_errors = document_uri.to_string();
                let hover_error_diagnostics_clone = hover_error_diagnostics.clone();
                let diagnostics_map = lsp_for_errors.as_ref().map(|l| l.diagnostics().get());
                if let Some(diag_map) = diagnostics_map {
                    let diagnostics = diag_map.get(&document_uri_for_errors).cloned().unwrap_or_default();
                    // Filter diagnostics for errors on the current line
                    let line_errors: Vec<_> = diagnostics.iter()
                        .filter(|d| {
                            matches!(d.severity, Some(crate::lsp::DiagnosticSeverity::Error))
                                && d.range.start.line <= line
                                && d.range.end.line >= line
                        })
                        .cloned()
                        .collect();
                    
                    if !line_errors.is_empty() {
                        hover_error_diagnostics_clone.set(Some(line_errors));
                        // Show error tooltip immediately
                        show_hover.set(true);
                    } else {
                        hover_error_diagnostics_clone.set(None);
                    }
                } else {
                    hover_error_diagnostics_clone.set(None);
                }
                
                // Check if we're hovering over the same word
                let new_word_pos = (line, character);
                let is_same_word = current_hover_word.get_untracked()
                    .map(|(l, c)| l == line && c == character)
                    .unwrap_or(false);
                
                if !is_same_word {
                    // New word - update tracking and hide hover until new data arrives (unless we have errors)
                    current_hover_word.set(Some(new_word_pos));
                    // Only hide hover if we don't have error diagnostics
                    if hover_error_diagnostics.get_untracked().is_none() {
                    show_hover.set(false); // Hide until new hover data arrives
                    }
                }
                
                // Only request LSP hover if we don't have error diagnostics on this line
                // Error tooltips take priority and don't need LSP hover
                if hover_error_diagnostics.get_untracked().is_none() {
                    // Debounce: increment timer
                    let timer_id = hover_debounce_timer.get_untracked() + 1;
                    hover_debounce_timer.set(timer_id);
                    
                    let lsp = lsp.clone();
                    let doc_uri = document_uri.to_string();
                    let position = Position { line, character };
                    let hover_debounce_timer_clone = hover_debounce_timer;
                    let current_hover_word_clone = current_hover_word;
                    
                    // Debounce hover requests (200ms - reduced for better responsiveness)
                    spawn_local(async move {
                        gloo_timers::future::TimeoutFuture::new(200).await;
                        
                        // Only proceed if timer hasn't been reset and we're still on the same word
                        if hover_debounce_timer_clone.get_untracked() == timer_id {
                            let still_on_same_word = current_hover_word_clone.get_untracked()
                                .map(|(l, c)| l == position.line && c == position.character)
                                .unwrap_or(false);
                            
                            if still_on_same_word {
                                // Request hover info from LSP
                                if let Err(e) = lsp.request_hover(doc_uri, position) {
                                    log::warn!("Failed to request hover: {}", e);
                                }
                            }
                        }
                    });
                }
            }
        }
    };

    let handle_mouse_leave = move |_: ev::MouseEvent| {
        // Clear error diagnostics when mouse leaves
        hover_error_diagnostics.set(None);
        show_hover.set(false);
        current_hover_word.set(None);
        hover_debounce_timer.update(|t| *t += 1); // Cancel pending hovers
    };

    // Chat handlers
    let mcp_client_for_chat = mcp_client.clone();
    let program_for_chat = program.clone();
    let chat_messages_for_send = chat_messages.clone();
    let chat_input_for_send = chat_input.clone();
    let pending_chat_request_for_send = pending_chat_request.clone();
    
    let send_chat_message = move |_| {
        let message = chat_input.get_untracked();
        if message.trim().is_empty() {
            return;
        }

        if let Some(mcp) = mcp_client_for_chat.as_ref() {
            let mcp = mcp.clone();
            let program_text = program_for_chat.text.get_untracked();
            
            // Add user message to chat
            chat_messages_for_send.update(|messages| {
                messages.push(("user".to_string(), message.clone()));
            });
            
            // Clear input
            chat_input_for_send.set(String::new());
            if let Some(input) = chat_input_ref.get() {
                let _ = input.set_value("");
            }

            // Send to MCP
            let messages: Vec<ChatMessage> = chat_messages_for_send.get_untracked()
                .iter()
                .map(|(role, content)| ChatMessage {
                    role: role.clone(),
                    content: content.clone(),
                })
                .collect();

            spawn_local(async move {
                match mcp.send_chat(messages, Some(program_text)).await {
                    Ok(request_id) => {
                        pending_chat_request_for_send.set(Some(request_id));
                        
                        // Set timeout fallback
                        let chat_messages_timeout = chat_messages_for_send.clone();
                        let pending_chat_request_timeout = pending_chat_request_for_send.clone();
                        spawn_local(async move {
                            gloo_timers::future::TimeoutFuture::new(30000).await; // 30 second timeout
                            
                            // Check if still pending
                            if pending_chat_request_timeout.get().is_some() {
                                chat_messages_timeout.update(|messages| {
                                    messages.push(("assistant".to_string(), 
                                        "⏱️ Request timed out. The server may be taking longer than expected.".to_string()));
                                });
                                pending_chat_request_timeout.set(None);
                            }
                        });
                    }
                    Err(e) => {
                        chat_messages_for_send.update(|messages| {
                            messages.push(("assistant".to_string(), 
                                format!("❌ Error sending message: {}", e)));
                        });
                        pending_chat_request_for_send.set(None);
                    }
                }
            });
        }
    };

    let send_chat_message_clone = send_chat_message.clone();
    let handle_chat_keydown = move |event: ev::KeyboardEvent| {
        if event.key() == "Enter" && !event.shift_key() {
            event.prevent_default();
            send_chat_message_clone(ev::MouseEvent::new("click").unwrap());
        }
    };

    view! {
        <div class="tab-content">
            <div class="copy-program">
                <CopyToClipboard content=program.text class="copy-button" tooltip_below=true>
                    <i class="far fa-copy"></i>
                </CopyToClipboard>
            </div>
            <div class="program-layout">
            <div class="editor-wrapper-container">
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
                    style:color="transparent"
                >
                    {program.text.get_untracked()}
                </textarea>
                </div>
            </div>

                // Diagnostics are now shown inline as error line highlights
            </div>

            // Show error tooltip when hovering over error lines
            {
                move || {
                    if show_hover.get() && !show_completions.get() {
                        let error_diagnostics = hover_error_diagnostics.get();
                        if let Some(errors) = error_diagnostics {
                            let error_messages: Vec<String> = errors.iter()
                                .map(|d| d.message.clone())
                                .collect();
                            let pos = hover_position.get();
                            Some(view! {
                                <div
                                    class="hover-tooltip error-tooltip"
                                    style=format!("left: {}px; top: {}px;", pos.0, pos.1)
                                >
                                    <div class="hover-content error-content">
                                        {error_messages.iter().map(|msg| {
                                            view! {
                                                <div class="error-message">{msg.clone()}</div>
                                            }
                                        }).collect_view()}
                                    </div>
                                </div>
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            }
            // Show LSP hover tooltip when hovering
            {
                move || {
                    // Only show if hover is enabled, completions are not shown, and we have hover data (and no error diagnostics)
                    if show_hover.get() && !show_completions.get() && hover_error_diagnostics.get().is_none() {
                        lsp_client_for_hover_tooltip.as_ref().and_then(|lsp| {
                            let hover_signal = lsp.hover_info();
                            let hover_data = hover_signal.get();

                            // Double-check we have data (the effect should handle this, but be safe)
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
                    // Clone handles inside the Fn closure so inner handlers can move them without making this FnOnce
                    let lsp_for_completion_handler = lsp_client_for_completion_request.clone();
                    let should_show = show_completions.get();
                    if should_show {
                        let filtered_items = filtered_completion_items.get();
                        
                        // Don't render if we have no items (the auto-hide effect will handle hiding)
                        if filtered_items.is_empty() {
                            // Check if we're still waiting for LSP completions
                            let has_lsp_completions = lsp_client_for_dropdown_check.as_ref()
                                .map(|lsp| !lsp.completions().get().is_empty())
                                .unwrap_or(false);
                            
                            if !has_lsp_completions {
                                // Still waiting for completions - don't render yet
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
                                            // Insert completion at cursor, replacing partial word
                                            crate::logging::log_event(
                                                "COMPLETION_ACCEPT",
                                                serde_json::json!({"label": label}),
                                            );
                                            if let Some(element) = textarea_ref.get_untracked() {
                                                if let Ok(Some(cursor_pos)) = element.selection_start() {
                                                    let cursor_pos_usize = cursor_pos as usize;
                                                    let text = program.text.get_untracked();
                                                    
                                                    // Find the start of the identifier being completed
                                                    // Go backwards from cursor until we hit a non-identifier character
                                                    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';
                                                    let mut word_start = cursor_pos_usize;
                                                    
                                                    // Find the start of the word (identifier) - don't include ':' in identifier
                                                    for (i, ch) in text[..cursor_pos_usize].char_indices().rev() {
                                                        if is_ident_char(ch) {
                                                            word_start = i;
                                                        } else {
                                                            break;
                                                        }
                                                    }
                                                    
                                                    // Replace from word start to cursor position
                                                        program.text.update(|text| {
                                                        text.replace_range(word_start..cursor_pos_usize, &label);
                                                    });
                                                    
                                                    // Calculate new cursor position
                                                    let new_pos = word_start + label.len();
                                                    let _ = element.set_selection_range(new_pos as u32, new_pos as u32);

                                                        // Immediately refresh overlay with local highlight (no extra keystroke needed)
                                                    overlay_update_in_progress.set(true);
                                                        if let Some(overlay) = highlight_overlay_ref.get_untracked() {
                                                            let html = crate::components::program_window::syntax_highlighter::highlight_code(&program.text.get_untracked());
                                                            overlay.set_inner_html(&html);
                                                            if let Some(ta) = textarea_ref.get_untracked() {
                                                                let _ = overlay.set_scroll_top(ta.scroll_top());
                                                                let _ = overlay.set_scroll_left(ta.scroll_left());
                                                            }
                                                        }
                                                    overlay_update_in_progress.set(false);
                                                        // Also queue a next-tick refresh to catch any layout changes
                                                        {
                                                            let highlight_ref = highlight_overlay_ref.clone();
                                                            let textarea_ref_clone = textarea_ref.clone();
                                                            let text_snapshot = program.text.get_untracked();
                                                        let overlay_update_flag = overlay_update_in_progress.clone();
                                                            spawn_local(async move {
                                                                gloo_timers::future::TimeoutFuture::new(0).await;
                                                            overlay_update_flag.set(true);
                                                                if let Some(overlay) = highlight_ref.get_untracked() {
                                                                    let html = crate::components::program_window::syntax_highlighter::highlight_code(&text_snapshot);
                                                                    overlay.set_inner_html(&html);
                                                                    if let Some(ta) = textarea_ref_clone.get_untracked() {
                                                                        let _ = overlay.set_scroll_top(ta.scroll_top());
                                                                        let _ = overlay.set_scroll_left(ta.scroll_left());
                                                                    }
                                                                }
                                                            overlay_update_flag.set(false);
                                                            });
                                                        }

                                                        // Debounced didChange to LSP for completions insertion
                                                        if let Some(lsp) = lsp_for_completion_handler.as_ref() {
                                                            let lsp = lsp.clone();
                                                            let uri = document_uri.to_string();
                                                            let text_for_lsp = program.text.get_untracked();
                                                            spawn_local(async move {
                                                                gloo_timers::future::TimeoutFuture::new(300).await;
                                                                crate::logging::log_event(
                                                                    "LSP_DID_CHANGE_AFTER_COMPLETION",
                                                                    serde_json::json!({"uri": uri}),
                                                                );
                                                                if let Err(e) = lsp.did_change(uri, text_for_lsp) {
                                                                    log::warn!("Failed to update document in LSP after completion: {}", e);
                                                                }
                                                            });
                                                        }
                                                    
                                            show_completions.set(false);
                                            completion_trigger_pos.set(None);
                                                }
                                            }
                                        }
                                        position=completion_position.get()
                                    />
                        })
                    } else {
                        None
                    }
                }
            }

            // Chat panel (hidden for now)
            {move || {
                const SHOW_CHAT: bool = false;
                if SHOW_CHAT {
                    Some(view! { <div></div> })
                } else {
                    None::<leptos::HtmlElement<leptos::html::Div>>
                }
            }}
                </div>
        </div>
    }
}

