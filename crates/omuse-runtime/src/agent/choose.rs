//! `choose_option`: a code machine's one choice out of a list a page itself shows (TIP Insure's districts, written
//! "Vadhana" for Watthana — code never matches a name to an option itself). The machine passes the question, what the
//! choice is about (the user's own words or JSON) and the options; the model picks exactly one of them
//! (`Runtime::decide`), or the default (`none`) when none fits. The result is JSON: `{"choice": "<option>"}`.

use omuse_common::util::get_str;
use serde_json::{json, Value};

use super::tools::fn_;
use super::Runtime;

/// The tool's name (code's own call, never offered to the model: `news::offered`).
pub const TOOL: &str = "choose_option";
/// The choice when no option fits.
pub const NONE: &str = "none";

/// The catalog entry (it makes the tool local; it is withheld from the model).
pub fn schema() -> Value {
    fn_(
        TOOL,
        "One option out of a page's own list (a code machine's own call).",
        json!({"question": {"type": "string"}, "about": {"type": "string"},
               "options": {"type": "array", "items": {"type": "string"}}}),
        &["question", "options"],
    )
}

/// The options of a call: the listed strings, empty ones and duplicates left out, at most 200.
pub fn options_of(a: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for o in a["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let o = o.trim().to_string();
        if !o.is_empty() && o != NONE && !out.contains(&o) {
            out.push(o);
        }
        if out.len() >= 200 {
            break;
        }
    }
    out
}

impl Runtime {
    pub(crate) async fn choose_option(&self, task_id: &str, a: &Value) -> Value {
        let options = options_of(a);
        if options.is_empty() {
            return json!({"choice": NONE});
        }
        let mut choices: Vec<&str> = options.iter().map(String::as_str).collect();
        choices.push(NONE);
        let question = format!(
            "{} Pick exactly one option as it is listed; \"{NONE}\" when none of them fits.",
            get_str(a, "question")
        );
        let picked = self
            .decide(task_id, &question, &get_str(a, "about"), &choices, NONE)
            .await;
        json!({"choice": picked})
    }
}
