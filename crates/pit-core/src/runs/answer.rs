//! Reading what a one-shot `claude -p --output-format json` printed: the analysis, the judge, a
//! card's description, a report's digest. One reader, so a change in Claude Code's output is
//! followed in one place.

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// A one-shot session's structured answer, with what it cost.
pub struct Answer<T> {
    pub output: T,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
}

/// The session's result message: the last line that is one (stream-json prints a line per
/// message), else the whole output as one JSON value, else the last line that is JSON at all.
pub fn result_of(stdout: &str) -> Option<Value> {
    let lines = || stdout.lines().rev().filter_map(|l| serde_json::from_str::<Value>(l).ok());
    lines()
        .find(|v| v.get("type").and_then(Value::as_str) == Some("result"))
        .or_else(|| serde_json::from_str(stdout.trim()).ok())
        .or_else(|| lines().next())
}

/// Reads the structured output of `what` (a word for the messages: "judge", "analysis"). Fails,
/// saying which, when there is no result, when the session ended in an error, when it returned
/// nothing, and when what it returned does not match the schema.
pub fn structured<T: DeserializeOwned>(stdout: &str, what: &str) -> Result<Answer<T>> {
    let v = result_of(stdout).with_context(|| format!("the {what} gave no result"))?;
    if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
        bail!("the {what} stopped with an error: {}", v.get("result").map(|r| r.to_string()).unwrap_or_default());
    }
    let out = v.get("structured_output").filter(|o| !o.is_null()).cloned().with_context(|| format!("the {what} returned nothing"))?;
    let output = serde_json::from_value(out).with_context(|| format!("the {what} does not match the schema"))?;
    Ok(Answer { output, cost_usd: v.get("total_cost_usd").and_then(Value::as_f64), duration_ms: v.get("duration_ms").and_then(Value::as_u64) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Out {
        n: u32,
    }

    #[test]
    fn the_result_is_found_among_other_lines_and_alone() {
        let stream = "{\"type\":\"system\"}\n{\"type\":\"result\",\"is_error\":false,\"total_cost_usd\":0.5,\"duration_ms\":7,\"structured_output\":{\"n\":3}}\n{\"type\":\"other\"}\n";
        let a: Answer<Out> = structured(stream, "judge").unwrap();
        assert_eq!((a.output.n, a.cost_usd, a.duration_ms), (3, Some(0.5), Some(7)));
        let pretty = "{\n  \"is_error\": false,\n  \"structured_output\": { \"n\": 4 }\n}";
        assert_eq!(structured::<Out>(pretty, "judge").unwrap().output.n, 4);
        let noisy = "warning: something\n{\"is_error\":false,\"structured_output\":{\"n\":5}}";
        assert_eq!(structured::<Out>(noisy, "judge").unwrap().output.n, 5);
    }

    #[test]
    fn each_way_of_failing_says_which() {
        let err = |s: &str| format!("{:#}", structured::<Out>(s, "judge").err().unwrap());
        assert!(err("not json").contains("gave no result"));
        assert!(err(r#"{"type":"result","is_error":true,"result":"boom"}"#).contains("stopped with an error: \"boom\""));
        assert!(err(r#"{"type":"result","is_error":false}"#).contains("returned nothing"));
        assert!(err(r#"{"type":"result","structured_output":null}"#).contains("returned nothing"));
        assert!(err(r#"{"type":"result","structured_output":{"n":"x"}}"#).contains("does not match the schema"));
    }
}
