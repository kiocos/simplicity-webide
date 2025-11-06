use std::cell::RefCell;

use js_sys::Date;
use serde_json::json;

thread_local! {
    static LOGS: RefCell<Vec<String>> = RefCell::new(Vec::new());
}

pub fn log_event(event: &str, data: serde_json::Value) {
    let ts = Date::now();
    let entry = json!({
        "ts": ts,
        "event": event,
        "data": data,
    });
    let line = entry.to_string();
    LOGS.with(|buf| buf.borrow_mut().push(line.clone()));
    log::info!("{}", line);
}

pub fn logs_as_text() -> String {
    LOGS.with(|buf| buf.borrow().join("\n"))
}

pub fn clear() {
    LOGS.with(|buf| buf.borrow_mut().clear());
}

pub fn len() -> usize {
    LOGS.with(|buf| buf.borrow().len())
}

