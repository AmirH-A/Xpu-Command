//! The assistant's plan for a request with several steps: shown in the pill, step by step, so the
//! user sees where it is. Not an action: it never reaches the gate and runs nothing.
use serde::Deserialize;
use serde_json::{json, Value};

pub const TOOL: &str = "plan";
const MAX_STEPS: usize = 8;
const MAX_STEP_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub steps: Vec<String>,
    /// The step being worked on, from 1.
    pub now: usize,
}

impl Plan {
    pub fn current(&self) -> &str {
        &self.steps[self.now - 1]
    }

    /// "2/3": short enough for the pill's agent label in any language.
    pub fn progress(&self) -> String {
        format!("{}/{}", self.now, self.steps.len())
    }

    /// The whole plan on one line, for the activity log.
    pub fn summary(&self) -> String {
        self.steps.iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect::<Vec<_>>().join(" · ")
    }
}

pub fn schema() -> Value {
    json!({
        "name": TOOL,
        "description": "Show the user your plan for a request with two or more steps, as short steps (a few words each, in the user's language). Call it once before the first action; when you start a later step, call it again with the same steps and now set to that step, in the same turn as that step's first action. It runs nothing.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "steps": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": MAX_STEPS },
                "now": { "type": "integer", "minimum": 1, "maximum": MAX_STEPS, "description": "The step being started, from 1. Default 1." }
            },
            "required": ["steps"],
            "additionalProperties": false
        },
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    steps: Vec<String>,
    now: Option<usize>,
}

pub fn parse(v: &Value) -> Result<Plan, String> {
    let a: Args = serde_json::from_value(v.clone()).map_err(|e| format!("invalid arguments: {e}"))?;
    if a.steps.is_empty() || a.steps.len() > MAX_STEPS {
        return Err(format!("give 1 to {MAX_STEPS} steps"));
    }
    let steps: Vec<String> = a.steps.iter().map(|s| crate::agents::clip(s, MAX_STEP_CHARS)).collect();
    if steps.iter().any(String::is_empty) {
        return Err("a step is empty".into());
    }
    let now = a.now.unwrap_or(1);
    if !(1..=steps.len()).contains(&now) {
        return Err(format!("now must be between 1 and {}", steps.len()));
    }
    Ok(Plan { steps, now })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_is_checked_and_cleaned() {
        let p = parse(&json!({ "steps": ["Open Mail", "Find Sara's invoice", "Reply\nwith thanks"], "now": 2 })).unwrap();
        assert_eq!(p.current(), "Find Sara's invoice");
        assert_eq!(p.progress(), "2/3");
        assert_eq!(p.steps[2], "Reply with thanks", "no line breaks reach the pill");
        assert_eq!(p.summary(), "1. Open Mail · 2. Find Sara's invoice · 3. Reply with thanks");
        assert_eq!(parse(&json!({ "steps": ["Open Notes"] })).unwrap().now, 1);
        assert!(parse(&json!({ "steps": [] })).is_err());
        assert!(parse(&json!({ "steps": vec!["a"; 9] })).is_err());
        assert!(parse(&json!({ "steps": ["a", "b"], "now": 3 })).is_err());
        assert!(parse(&json!({ "steps": ["a"], "now": 0 })).is_err());
        assert!(parse(&json!({ "steps": [" "] })).is_err());
        assert!(parse(&json!({ "steps": ["a"], "extra": 1 })).is_err());
        assert_eq!(parse(&json!({ "steps": ["x".repeat(500)] })).unwrap().steps[0].chars().count(), MAX_STEP_CHARS + 1, "clipped, with an ellipsis");
    }
}
