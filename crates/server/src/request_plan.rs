//! Atomic final request preparation. Metering and validation run on the exact
//! outgoing copy; failures never materialize defaults in the caller's request.
use serde_json::Value;

pub struct RequestPlan<T> {
    body: Value,
    result: T,
}

impl<T> RequestPlan<T> {
    pub fn prepare<E>(original: &Value, prepare: impl FnOnce(&mut Value) -> Result<T, E>) -> Result<Self, E> {
        let mut body = original.clone();
        let result = prepare(&mut body)?;
        Ok(Self { body, result })
    }

    pub fn into_parts(self) -> (Value, T) { (self.body, self.result) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failure_discards_all_partial_transformations() {
        let original = json!({"model":"native", "input":"original"});
        let result = RequestPlan::prepare(&original, |body| {
            body["max_output_tokens"] = json!(32);
            body["model"] = json!("changed");
            Err::<(), _>("unsupported final shape")
        });
        assert!(result.is_err());
        assert_eq!(original, json!({"model":"native", "input":"original"}));
    }

    #[test]
    fn success_returns_the_same_final_body_that_was_metered_once() {
        let original = json!({"input":"unchanged"});
        let mut calls = 0;
        let plan = RequestPlan::prepare(&original, |body| {
            calls += 1;
            body["max_output_tokens"] = json!(32);
            Ok::<_, ()>(body["max_output_tokens"].as_i64().unwrap())
        }).unwrap();
        let (body, bound) = plan.into_parts();
        assert_eq!(calls,1);
        assert_eq!(bound,32);
        assert_eq!(body,json!({"input":"unchanged","max_output_tokens":32}));
        assert_eq!(original,json!({"input":"unchanged"}));
    }
}
