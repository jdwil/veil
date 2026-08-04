use acr_core::value::Value;
use crate::task::TestResult;

/// Compare actual vs expected output. Returns true if they match.
pub fn values_match(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => (a - b).abs() < 1e-9,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| values_match(x, y))
        }
        (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|((ka, va), (kb, vb))| ka == kb && values_match(va, vb))
        }
        // Allow Int/Float cross-comparison
        (Value::Int(a), Value::Float(b)) => (*a as f64 - b).abs() < 1e-9,
        (Value::Float(a), Value::Int(b)) => (a - *b as f64).abs() < 1e-9,
        _ => false,
    }
}

/// Calculate score from test results (0.0 to 1.0)
pub fn calculate_score(results: &[TestResult]) -> f64 {
    if results.is_empty() {
        return 0.0;
    }
    let passed = results.iter().filter(|r| r.passed).count();
    passed as f64 / results.len() as f64
}
