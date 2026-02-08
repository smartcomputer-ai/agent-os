use std::cmp::Ordering;

use aos_air_types::{
    Expr, ExprConst, ExprList, ExprMap, ExprOp, ExprOpCode, ExprRecord, ExprSet, ExprVariant,
    HashRef,
};
use aos_cbor::Hash as CborHash;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::value::{Value, ValueKey, ValueMap, ValueSet};

/// Evaluation environment wires plan input, bound vars, and prior step outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Env {
    pub plan_input: Value,
    pub vars: IndexMap<String, Value>,
    pub steps: IndexMap<String, Value>,
    #[serde(default)]
    pub current_event: Option<Value>,
}

impl Env {
    pub fn new(plan_input: Value) -> Self {
        Self {
            plan_input,
            vars: IndexMap::new(),
            steps: IndexMap::new(),
            current_event: None,
        }
    }

    pub fn insert_var(&mut self, name: impl Into<String>, value: Value) -> Option<Value> {
        self.vars.insert(name.into(), value)
    }

    pub fn insert_step(&mut self, id: impl Into<String>, value: Value) -> Option<Value> {
        self.steps.insert(id.into(), value)
    }

    /// Temporarily bind an event value for `@event` references.
    pub fn push_event(&mut self, value: Value) -> Option<Value> {
        self.current_event.replace(value)
    }

    /// Restores the previous event binding (if any).
    pub fn restore_event(&mut self, prev: Option<Value>) {
        self.current_event = prev;
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new(Value::Unit)
    }
}

pub type EvalResult<T = Value> = Result<T, EvalError>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EvalError {
    #[error("missing ref {0}")]
    MissingRef(String),
    #[error("missing field '{field}' on {context}")]
    MissingField {
        field: String,
        context: &'static str,
    },
    #[error("type error: expected {expected}, got {actual}")]
    TypeError {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("invalid argument count for {op:?}: {message}")]
    InvalidArity { op: ExprOpCode, message: String },
    #[error("invalid bytes literal: {0}")]
    InvalidBytes(String),
    #[error("numeric overflow in {0:?}")]
    NumericOverflow(ExprOpCode),
    #[error("division by zero in {0:?}")]
    DivideByZero(ExprOpCode),
    #[error("op {op:?} error: {message}")]
    OpError { op: ExprOpCode, message: String },
    #[error("op {0:?} not implemented")]
    UnsupportedOp(ExprOpCode),
}

/// Evaluate an AIR expression under the provided environment.
pub fn eval_expr(expr: &Expr, env: &Env) -> EvalResult {
    match expr {
        Expr::Ref(eref) => resolve_ref(&eref.reference, env),
        Expr::Const(lit) => eval_const(lit),
        Expr::Op(op) => eval_op(op, env),
        Expr::Record(record) => eval_record(record, env),
        Expr::List(list) => eval_list(list, env),
        Expr::Set(set) => eval_set(set, env),
        Expr::Map(map) => eval_map(map, env),
        Expr::Variant(variant) => eval_variant(variant, env),
    }
}

fn eval_const(constant: &ExprConst) -> EvalResult {
    Ok(match constant {
        ExprConst::Null { .. } => Value::Null,
        ExprConst::Bool { bool } => Value::Bool(*bool),
        ExprConst::Int { int } => Value::Int(*int),
        ExprConst::Nat { nat } => Value::Nat(*nat),
        ExprConst::Dec128 { dec128 } => Value::Dec128(dec128.clone()),
        ExprConst::Text { text } => Value::Text(text.clone()),
        ExprConst::Bytes { bytes_b64 } => {
            let bytes = BASE64
                .decode(bytes_b64)
                .map_err(|err| EvalError::InvalidBytes(err.to_string()))?;
            Value::Bytes(bytes)
        }
        ExprConst::Time { time_ns } => Value::TimeNs(*time_ns),
        ExprConst::Duration { duration_ns } => Value::DurationNs(*duration_ns),
        ExprConst::Hash { hash } => Value::Hash(hash.clone()),
        ExprConst::Uuid { uuid } => Value::Uuid(uuid.clone()),
    })
}

fn eval_record(record: &ExprRecord, env: &Env) -> EvalResult {
    let mut out = IndexMap::with_capacity(record.record.len());
    for (key, value_expr) in &record.record {
        out.insert(key.clone(), eval_expr(value_expr, env)?);
    }
    Ok(Value::Record(out))
}

fn eval_list(list: &ExprList, env: &Env) -> EvalResult {
    let mut out = Vec::with_capacity(list.list.len());
    for item in &list.list {
        out.push(eval_expr(item, env)?);
    }
    Ok(Value::List(out))
}

fn eval_set(set: &ExprSet, env: &Env) -> EvalResult {
    let mut out = ValueSet::new();
    for item in &set.set {
        let value = eval_expr(item, env)?;
        out.insert(value_to_key(&value)?);
    }
    Ok(Value::Set(out))
}

fn eval_map(map: &ExprMap, env: &Env) -> EvalResult {
    let mut out = ValueMap::new();
    for entry in &map.map {
        let key_value = eval_expr(&entry.key, env)?;
        let key = value_to_key(&key_value)?;
        let value = eval_expr(&entry.value, env)?;
        out.insert(key, value);
    }
    Ok(Value::Map(out))
}

fn eval_variant(variant: &ExprVariant, env: &Env) -> EvalResult {
    let mut record = IndexMap::with_capacity(2);
    record.insert("$tag".into(), Value::Text(variant.variant.tag.clone()));
    let value = match &variant.variant.value {
        Some(inner) => eval_expr(inner, env)?,
        None => Value::Unit,
    };
    record.insert("$value".into(), value);
    Ok(Value::Record(record))
}

fn eval_op(op: &ExprOp, env: &Env) -> EvalResult {
    let values: Vec<Value> = op
        .args
        .iter()
        .map(|arg| eval_expr(arg, env))
        .collect::<EvalResult<_>>()?;
    apply_op(op.op, &values)
}

fn apply_op(op: ExprOpCode, args: &[Value]) -> EvalResult {
    use ExprOpCode::*;
    match op {
        Len => {
            require_args_exact(op, args, 1)?;
            Ok(Value::Nat(len_of(&args[0])?))
        }
        Get => {
            require_args_exact(op, args, 2)?;
            get_value(op, &args[0], &args[1])
        }
        Has => {
            require_args_exact(op, args, 2)?;
            has_value(&args[0], &args[1])
        }
        Eq => {
            require_args_exact(op, args, 2)?;
            Ok(Value::Bool(args[0] == args[1]))
        }
        Ne => {
            require_args_exact(op, args, 2)?;
            Ok(Value::Bool(args[0] != args[1]))
        }
        Lt | Le | Gt | Ge => {
            require_args_exact(op, args, 2)?;
            let ordering = compare_orderable(&args[0], &args[1])?;
            let result = match op {
                Lt => ordering == Ordering::Less,
                Le => ordering != Ordering::Greater,
                Gt => ordering == Ordering::Greater,
                Ge => ordering != Ordering::Less,
                _ => unreachable!(),
            };
            Ok(Value::Bool(result))
        }
        And => {
            require_args_at_least(op, args, 2)?;
            let mut result = true;
            for value in args {
                result &= as_bool(value)?;
            }
            Ok(Value::Bool(result))
        }
        Or => {
            require_args_at_least(op, args, 2)?;
            let mut result = false;
            for value in args {
                result |= as_bool(value)?;
            }
            Ok(Value::Bool(result))
        }
        Not => {
            require_args_exact(op, args, 1)?;
            Ok(Value::Bool(!as_bool(&args[0])?))
        }
        Concat => {
            require_args_exact(op, args, 2)?;
            Ok(Value::Text(format!(
                "{}{}",
                as_text(&args[0])?,
                as_text(&args[1])?
            )))
        }
        Hash => {
            require_args_exact(op, args, 1)?;
            let hash = CborHash::of_cbor(&args[0]).map_err(|err| EvalError::OpError {
                op,
                message: err.to_string(),
            })?;
            let hash_ref = HashRef::new(hash.to_hex()).map_err(|err| EvalError::OpError {
                op,
                message: err.to_string(),
            })?;
            Ok(Value::Hash(hash_ref))
        }
        HashBytes => {
            require_args_exact(op, args, 1)?;
            let bytes = match &args[0] {
                Value::Bytes(bytes) => bytes,
                other => {
                    return Err(EvalError::OpError {
                        op,
                        message: format!("expected bytes, got {}", other.kind()),
                    });
                }
            };
            let hash = CborHash::of_bytes(bytes);
            let hash_ref = HashRef::new(hash.to_hex()).map_err(|err| EvalError::OpError {
                op,
                message: err.to_string(),
            })?;
            Ok(Value::Hash(hash_ref))
        }
        StartsWith | EndsWith | Contains => {
            require_args_exact(op, args, 2)?;
            string_op(op, &args[0], &args[1])
        }
        Add => {
            require_args_exact(op, args, 2)?;
            add_numbers(op, &args[0], &args[1])
        }
        Sub => {
            require_args_exact(op, args, 2)?;
            sub_numbers(op, &args[0], &args[1])
        }
        Mul => {
            require_args_exact(op, args, 2)?;
            mul_numbers(op, &args[0], &args[1])
        }
        Div => {
            require_args_exact(op, args, 2)?;
            div_numbers(op, &args[0], &args[1])
        }
        Mod => {
            require_args_exact(op, args, 2)?;
            mod_numbers(op, &args[0], &args[1])
        }
    }
}

fn resolve_ref(reference: &str, env: &Env) -> EvalResult {
    if let Some(rest) = reference.strip_prefix("@plan.input") {
        return access_path(&env.plan_input, rest);
    }
    if let Some(var) = reference.strip_prefix("@var:") {
        return env
            .vars
            .get(var)
            .cloned()
            .ok_or_else(|| EvalError::MissingRef(reference.to_string()));
    }
    if let Some(rest) = reference.strip_prefix("@event") {
        let event = env
            .current_event
            .as_ref()
            .ok_or_else(|| EvalError::MissingRef(reference.to_string()))?;
        return access_path(event, rest);
    }
    if let Some(step) = reference.strip_prefix("@step:") {
        let (id, tail) = split_step_ref(step);
        let value = env
            .steps
            .get(id)
            .cloned()
            .ok_or_else(|| EvalError::MissingRef(reference.to_string()))?;
        return access_path(&value, tail);
    }
    Err(EvalError::MissingRef(reference.to_string()))
}

fn split_step_ref(step: &str) -> (&str, &str) {
    match step.split_once('.') {
        Some((id, rest)) => (id, rest),
        None => (step, ""),
    }
}

fn access_path(root: &Value, path: &str) -> EvalResult {
    if path.is_empty() {
        return Ok(root.clone());
    }
    let mut current = root;
    for segment in path.trim_start_matches('.').split('.') {
        if segment.is_empty() {
            continue;
        }
        current = match current {
            Value::Record(map) => map.get(segment).ok_or_else(|| EvalError::MissingField {
                field: segment.to_string(),
                context: "record",
            })?,
            Value::Map(map) => {
                let key = ValueKey::Text(segment.to_string());
                map.get(&key).ok_or_else(|| EvalError::MissingField {
                    field: segment.to_string(),
                    context: "map",
                })?
            }
            other => {
                return Err(EvalError::TypeError {
                    expected: "record or map",
                    actual: other.kind(),
                });
            }
        };
    }
    Ok(current.clone())
}

fn len_of(value: &Value) -> EvalResult<u64> {
    let len = match value {
        Value::List(items) => items.len(),
        Value::Set(items) => items.len(),
        Value::Map(entries) => entries.len(),
        Value::Record(fields) => fields.len(),
        Value::Text(text) => text.chars().count(),
        Value::Bytes(bytes) => bytes.len(),
        _ => {
            return Err(EvalError::TypeError {
                expected: "collection or text",
                actual: value.kind(),
            });
        }
    };
    u64::try_from(len).map_err(|_| EvalError::OpError {
        op: ExprOpCode::Len,
        message: "length exceeds u64".into(),
    })
}

fn get_value(op: ExprOpCode, target: &Value, key: &Value) -> EvalResult {
    match target {
        Value::Record(map) => {
            let field = as_text(key)?.to_string();
            map.get(&field)
                .cloned()
                .ok_or_else(|| EvalError::MissingField {
                    field,
                    context: "record",
                })
        }
        Value::Map(map) => {
            let map_key = value_to_key(key)?;
            map.get(&map_key)
                .cloned()
                .ok_or_else(|| EvalError::OpError {
                    op,
                    message: "map key not found".into(),
                })
        }
        Value::List(list) => {
            let index = as_index(key, op)?;
            list.get(index).cloned().ok_or_else(|| EvalError::OpError {
                op,
                message: format!("index {index} out of range"),
            })
        }
        other => Err(EvalError::TypeError {
            expected: "list|record|map",
            actual: other.kind(),
        }),
    }
}

fn has_value(container: &Value, needle: &Value) -> EvalResult {
    let result = match container {
        Value::Record(map) => map.contains_key(as_text(needle)?),
        Value::Map(map) => {
            let key = value_to_key(needle)?;
            map.contains_key(&key)
        }
        Value::Set(set) => {
            let key = value_to_key(needle)?;
            set.contains(&key)
        }
        Value::List(list) => list.iter().any(|item| item == needle),
        other => {
            return Err(EvalError::TypeError {
                expected: "list|record|map|set",
                actual: other.kind(),
            });
        }
    };
    Ok(Value::Bool(result))
}

fn string_op(op: ExprOpCode, haystack: &Value, needle: &Value) -> EvalResult {
    match (haystack, needle) {
        (Value::Text(h), Value::Text(n)) => {
            let result = match op {
                ExprOpCode::StartsWith => h.starts_with(n),
                ExprOpCode::EndsWith => h.ends_with(n),
                ExprOpCode::Contains => h.contains(n),
                _ => unreachable!(),
            };
            Ok(Value::Bool(result))
        }
        (Value::Bytes(h), Value::Bytes(n)) if op == ExprOpCode::Contains => {
            let result = if n.is_empty() {
                true
            } else {
                h.windows(n.len()).any(|window| window == n)
            };
            Ok(Value::Bool(result))
        }
        (Value::List(list), needle) if op == ExprOpCode::Contains => {
            Ok(Value::Bool(list.iter().any(|item| item == needle)))
        }
        (Value::Set(set), needle) if op == ExprOpCode::Contains => {
            let key = value_to_key(needle)?;
            Ok(Value::Bool(set.contains(&key)))
        }
        (Value::Map(map), needle) if op == ExprOpCode::Contains => {
            let key = value_to_key(needle)?;
            Ok(Value::Bool(map.contains_key(&key)))
        }
        (other, _) => Err(EvalError::TypeError {
            expected: match op {
                ExprOpCode::StartsWith | ExprOpCode::EndsWith => "text",
                ExprOpCode::Contains => "text|bytes|list|set|map",
                _ => unreachable!(),
            },
            actual: other.kind(),
        }),
    }
}

fn compare_orderable(a: &Value, b: &Value) -> EvalResult<Ordering> {
    match (to_orderable(a)?, to_orderable(b)?) {
        (Orderable::Number(lhs), Orderable::Number(rhs)) => Ok(lhs.cmp(&rhs)),
        (Orderable::Text(lhs), Orderable::Text(rhs)) => Ok(lhs.cmp(rhs)),
        (lhs, rhs) => Err(EvalError::TypeError {
            expected: "comparable pair",
            actual: match (lhs, rhs) {
                (Orderable::Number(_), Orderable::Text(_)) => "number vs text",
                (Orderable::Text(_), Orderable::Number(_)) => "text vs number",
                _ => "unknown",
            },
        }),
    }
}

enum Orderable<'a> {
    Number(i128),
    Text(&'a str),
}

fn to_orderable(value: &Value) -> EvalResult<Orderable<'_>> {
    match value {
        Value::Int(i) => Ok(Orderable::Number(*i as i128)),
        Value::Nat(n) => Ok(Orderable::Number(*n as i128)),
        Value::TimeNs(n) => Ok(Orderable::Number(*n as i128)),
        Value::DurationNs(d) => Ok(Orderable::Number(*d as i128)),
        Value::Text(s) => Ok(Orderable::Text(s)),
        other => Err(EvalError::TypeError {
            expected: "number or text",
            actual: other.kind(),
        }),
    }
}

fn add_numbers(op: ExprOpCode, lhs: &Value, rhs: &Value) -> EvalResult {
    if is_nat(lhs) && is_nat(rhs) {
        let sum = as_u128(lhs)? + as_u128(rhs)?;
        let nat = fits_u64(sum).map_err(|_| EvalError::NumericOverflow(op))?;
        return Ok(Value::Nat(nat));
    }
    let sum = as_i128(lhs)? + as_i128(rhs)?;
    let int = fits_i64(sum).map_err(|_| EvalError::NumericOverflow(op))?;
    Ok(Value::Int(int))
}

fn sub_numbers(op: ExprOpCode, lhs: &Value, rhs: &Value) -> EvalResult {
    if is_nat(lhs) && is_nat(rhs) {
        let left = as_u128(lhs)?;
        let right = as_u128(rhs)?;
        if left >= right {
            let diff = left - right;
            let nat = fits_u64(diff).map_err(|_| EvalError::NumericOverflow(op))?;
            return Ok(Value::Nat(nat));
        }
    }
    let diff = as_i128(lhs)? - as_i128(rhs)?;
    let int = fits_i64(diff).map_err(|_| EvalError::NumericOverflow(op))?;
    Ok(Value::Int(int))
}

fn mul_numbers(op: ExprOpCode, lhs: &Value, rhs: &Value) -> EvalResult {
    if is_nat(lhs) && is_nat(rhs) {
        let product = as_u128(lhs)? * as_u128(rhs)?;
        let nat = fits_u64(product).map_err(|_| EvalError::NumericOverflow(op))?;
        return Ok(Value::Nat(nat));
    }
    let product = as_i128(lhs)? * as_i128(rhs)?;
    let int = fits_i64(product).map_err(|_| EvalError::NumericOverflow(op))?;
    Ok(Value::Int(int))
}

fn div_numbers(op: ExprOpCode, lhs: &Value, rhs: &Value) -> EvalResult {
    if is_zero(rhs) {
        return Err(EvalError::DivideByZero(op));
    }
    if is_nat(lhs) && is_nat(rhs) {
        let numerator = as_u128(lhs)?;
        let denominator = as_u128(rhs)?;
        let result = (numerator / denominator) as u64;
        return Ok(Value::Nat(result));
    }
    let quotient = as_i128(lhs)? / as_i128(rhs)?;
    let int = fits_i64(quotient).map_err(|_| EvalError::NumericOverflow(op))?;
    Ok(Value::Int(int))
}

fn mod_numbers(op: ExprOpCode, lhs: &Value, rhs: &Value) -> EvalResult {
    if is_zero(rhs) {
        return Err(EvalError::DivideByZero(op));
    }
    if is_nat(lhs) && is_nat(rhs) {
        let numerator = as_u128(lhs)?;
        let denominator = as_u128(rhs)?;
        let result = (numerator % denominator) as u64;
        return Ok(Value::Nat(result));
    }
    let remainder = as_i128(lhs)? % as_i128(rhs)?;
    let int = fits_i64(remainder).map_err(|_| EvalError::NumericOverflow(op))?;
    Ok(Value::Int(int))
}

fn as_i128(value: &Value) -> EvalResult<i128> {
    match value {
        Value::Int(i) => Ok(*i as i128),
        Value::Nat(n) => Ok(*n as i128),
        other => Err(EvalError::TypeError {
            expected: "int or nat",
            actual: other.kind(),
        }),
    }
}

fn as_u128(value: &Value) -> EvalResult<u128> {
    match value {
        Value::Nat(n) => Ok(*n as u128),
        other => Err(EvalError::TypeError {
            expected: "nat",
            actual: other.kind(),
        }),
    }
}

fn fits_i64(value: i128) -> Result<i64, ()> {
    if value < i64::MIN as i128 || value > i64::MAX as i128 {
        Err(())
    } else {
        Ok(value as i64)
    }
}

fn fits_u64(value: u128) -> Result<u64, ()> {
    if value > u64::MAX as u128 {
        Err(())
    } else {
        Ok(value as u64)
    }
}

fn is_nat(value: &Value) -> bool {
    matches!(value, Value::Nat(_))
}

fn is_zero(value: &Value) -> bool {
    matches!(value, Value::Int(0) | Value::Nat(0))
}

fn as_bool(value: &Value) -> EvalResult<bool> {
    match value {
        Value::Bool(b) => Ok(*b),
        other => Err(EvalError::TypeError {
            expected: "bool",
            actual: other.kind(),
        }),
    }
}

fn as_text<'a>(value: &'a Value) -> EvalResult<&'a str> {
    match value {
        Value::Text(s) => Ok(s),
        other => Err(EvalError::TypeError {
            expected: "text",
            actual: other.kind(),
        }),
    }
}

fn as_index(value: &Value, op: ExprOpCode) -> EvalResult<usize> {
    let idx = match value {
        Value::Nat(n) => *n as i128,
        Value::Int(i) if *i >= 0 => *i as i128,
        _ => {
            return Err(EvalError::OpError {
                op,
                message: "index must be non-negative int or nat".into(),
            });
        }
    };
    usize::try_from(idx).map_err(|_| EvalError::OpError {
        op,
        message: "index out of range for platform".into(),
    })
}

fn value_to_key(value: &Value) -> EvalResult<ValueKey> {
    match value {
        Value::Int(i) => Ok(ValueKey::Int(*i)),
        Value::Nat(n) => Ok(ValueKey::Nat(*n)),
        Value::Text(s) => Ok(ValueKey::Text(s.clone())),
        Value::Hash(h) => Ok(ValueKey::Hash(h.as_str().to_owned())),
        Value::Uuid(u) => Ok(ValueKey::Uuid(u.clone())),
        other => Err(EvalError::TypeError {
            expected: "comparable key",
            actual: other.kind(),
        }),
    }
}

fn require_args_exact(op: ExprOpCode, args: &[Value], expected: usize) -> Result<(), EvalError> {
    if args.len() != expected {
        return Err(EvalError::InvalidArity {
            op,
            message: format!("expected {expected}, got {}", args.len()),
        });
    }
    Ok(())
}

fn require_args_at_least(op: ExprOpCode, args: &[Value], min: usize) -> Result<(), EvalError> {
    if args.len() < min {
        return Err(EvalError::InvalidArity {
            op,
            message: format!("expected >= {min}, got {}", args.len()),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{ExprMapEntry, ExprRef, VariantExpr};
    use indexmap::IndexMap;

    fn sample_env() -> Env {
        let mut input = IndexMap::new();
        input.insert("customer".into(), Value::Text("alice".into()));
        input.insert("amount".into(), Value::Nat(1200));
        Env {
            plan_input: Value::Record(input),
            vars: IndexMap::new(),
            steps: IndexMap::new(),
            current_event: None,
        }
    }

    #[test]
    fn evaluates_record_literal() {
        let record = ExprRecord {
            record: IndexMap::from([
                (
                    "greeting".into(),
                    Expr::Const(ExprConst::Text { text: "hi".into() }),
                ),
                ("count".into(), Expr::Const(ExprConst::Nat { nat: 2 })),
            ]),
        };
        let value = eval_record(&record, &sample_env()).unwrap();
        match value {
            Value::Record(map) => {
                assert_eq!(map.get("greeting"), Some(&Value::Text("hi".into())));
                assert_eq!(map.get("count"), Some(&Value::Nat(2)));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn plan_input_reference_with_path() {
        let env = sample_env();
        let expr = Expr::Ref(ExprRef {
            reference: "@plan.input.customer".into(),
        });
        let value = eval_expr(&expr, &env).unwrap();
        assert_eq!(value, Value::Text("alice".into()));
    }

    #[test]
    fn len_over_list() {
        let list = ExprList {
            list: vec![
                Expr::Const(ExprConst::Int { int: 1 }),
                Expr::Const(ExprConst::Int { int: 2 }),
            ],
        };
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Len,
            args: vec![Expr::List(list)],
        });
        let len = eval_expr(&expr, &sample_env()).unwrap();
        assert_eq!(len, Value::Nat(2));
    }

    #[test]
    fn string_contains() {
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Contains,
            args: vec![
                Expr::Const(ExprConst::Text {
                    text: "hello world".into(),
                }),
                Expr::Const(ExprConst::Text {
                    text: "world".into(),
                }),
            ],
        });
        let value = eval_expr(&expr, &sample_env()).unwrap();
        assert_eq!(value, Value::Bool(true));
    }

    #[test]
    fn hash_op_produces_hash_value() {
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Hash,
            args: vec![Expr::Const(ExprConst::Text {
                text: "alpha".into(),
            })],
        });
        let value = eval_expr(&expr, &sample_env()).unwrap();
        let expected = CborHash::of_cbor(&Value::Text("alpha".into()))
            .unwrap()
            .to_hex();
        match value {
            Value::Hash(hash) => assert_eq!(hash.as_str(), expected),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn hash_bytes_op_produces_hash_value() {
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::HashBytes,
            args: vec![Expr::Const(ExprConst::Bytes {
                bytes_b64: BASE64.encode([0x01_u8, 0x02, 0x03]),
            })],
        });
        let value = eval_expr(&expr, &sample_env()).unwrap();
        let expected = CborHash::of_bytes(&[0x01_u8, 0x02, 0x03]).to_hex();
        match value {
            Value::Hash(hash) => assert_eq!(hash.as_str(), expected),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn arithmetic_add_nat_and_int() {
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Add,
            args: vec![
                Expr::Const(ExprConst::Nat { nat: 5 }),
                Expr::Const(ExprConst::Int { int: -2 }),
            ],
        });
        let value = eval_expr(&expr, &sample_env()).unwrap();
        assert_eq!(value, Value::Int(3));
    }

    #[test]
    fn get_from_record() {
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Get,
            args: vec![
                Expr::Ref(ExprRef {
                    reference: "@plan.input".into(),
                }),
                Expr::Const(ExprConst::Text {
                    text: "amount".into(),
                }),
            ],
        });
        let value = eval_expr(&expr, &sample_env()).unwrap();
        assert_eq!(value, Value::Nat(1200));
    }

    #[test]
    fn missing_variable_errors() {
        let expr = Expr::Ref(ExprRef {
            reference: "@var:missing".into(),
        });
        let err = eval_expr(&expr, &sample_env()).unwrap_err();
        assert!(matches!(err, EvalError::MissingRef(name) if name == "@var:missing"));
    }

    #[test]
    fn has_on_set() {
        let set_expr = ExprSet {
            set: vec![
                Expr::Const(ExprConst::Int { int: 1 }),
                Expr::Const(ExprConst::Int { int: 2 }),
            ],
        };
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Has,
            args: vec![Expr::Set(set_expr), Expr::Const(ExprConst::Int { int: 2 })],
        });
        let value = eval_expr(&expr, &sample_env()).unwrap();
        assert_eq!(value, Value::Bool(true));
    }

    #[test]
    fn less_than_over_ints() {
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Lt,
            args: vec![
                Expr::Const(ExprConst::Int { int: 4 }),
                Expr::Const(ExprConst::Int { int: 9 }),
            ],
        });
        let value = eval_expr(&expr, &sample_env()).unwrap();
        assert_eq!(value, Value::Bool(true));
    }

    #[test]
    fn step_reference_reads_prior_output() {
        let mut env = sample_env();
        let mut record = IndexMap::new();
        record.insert("status".into(), Value::Text("ok".into()));
        env.insert_step("charge", Value::Record(record));
        let expr = Expr::Ref(ExprRef {
            reference: "@step:charge.status".into(),
        });
        let value = eval_expr(&expr, &env).unwrap();
        assert_eq!(value, Value::Text("ok".into()));
    }

    #[test]
    fn missing_step_reference_errors() {
        let expr = Expr::Ref(ExprRef {
            reference: "@step:unknown".into(),
        });
        let err = eval_expr(&expr, &sample_env()).unwrap_err();
        assert!(matches!(err, EvalError::MissingRef(name) if name == "@step:unknown"));
    }

    #[test]
    fn event_reference_available_when_bound() {
        let mut env = sample_env();
        let mut payload = IndexMap::new();
        payload.insert("correlation_id".into(), Value::Text("abc".into()));
        env.push_event(Value::Record(payload));
        let expr = Expr::Ref(ExprRef {
            reference: "@event.correlation_id".into(),
        });
        let value = eval_expr(&expr, &env).unwrap();
        assert_eq!(value, Value::Text("abc".into()));
    }

    #[test]
    fn missing_event_reference_errors() {
        let expr = Expr::Ref(ExprRef {
            reference: "@event.foo".into(),
        });
        let err = eval_expr(&expr, &sample_env()).unwrap_err();
        assert!(matches!(err, EvalError::MissingRef(name) if name == "@event.foo"));
    }

    #[test]
    fn divide_by_zero_errors() {
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Div,
            args: vec![
                Expr::Const(ExprConst::Nat { nat: 7 }),
                Expr::Const(ExprConst::Nat { nat: 0 }),
            ],
        });
        let err = eval_expr(&expr, &sample_env()).unwrap_err();
        assert!(matches!(err, EvalError::DivideByZero(ExprOpCode::Div)));
    }

    #[test]
    fn variant_literal_wraps_tag_and_value() {
        let expr = Expr::Variant(ExprVariant {
            variant: VariantExpr {
                tag: "Ok".into(),
                value: Some(Box::new(Expr::Const(ExprConst::Text {
                    text: "done".into(),
                }))),
            },
        });
        let value = eval_expr(&expr, &sample_env()).unwrap();
        match value {
            Value::Record(fields) => {
                assert_eq!(fields.get("$tag"), Some(&Value::Text("Ok".into())));
                assert_eq!(fields.get("$value"), Some(&Value::Text("done".into())));
            }
            other => panic!("unexpected variant representation {other:?}"),
        }
    }

    #[test]
    fn bytes_contains_subsequence() {
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Contains,
            args: vec![
                Expr::Const(ExprConst::Bytes {
                    bytes_b64: "YWJjZDEyMw==".into(), // "abcd123"
                }),
                Expr::Const(ExprConst::Bytes {
                    bytes_b64: "Y2Qx".into(), // "cd1"
                }),
            ],
        });
        let value = eval_expr(&expr, &sample_env()).unwrap();
        assert_eq!(value, Value::Bool(true));
    }

    #[test]
    fn missing_field_in_path_errors() {
        let expr = Expr::Ref(ExprRef {
            reference: "@plan.input.order_id".into(),
        });
        let err = eval_expr(&expr, &sample_env()).unwrap_err();
        assert!(matches!(err, EvalError::MissingField { field, .. } if field == "order_id"));
    }

    #[test]
    fn map_contains_key() {
        let map_expr = ExprMap {
            map: vec![ExprMapEntry {
                key: Expr::Const(ExprConst::Text { text: "foo".into() }),
                value: Expr::Const(ExprConst::Int { int: 9 }),
            }],
        };
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Contains,
            args: vec![
                Expr::Map(map_expr),
                Expr::Const(ExprConst::Text { text: "foo".into() }),
            ],
        });
        let value = eval_expr(&expr, &sample_env()).unwrap();
        assert_eq!(value, Value::Bool(true));
    }

    #[test]
    fn boolean_ops_require_bool_args() {
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::And,
            args: vec![
                Expr::Const(ExprConst::Bool { bool: true }),
                Expr::Const(ExprConst::Int { int: 1 }),
            ],
        });
        let err = eval_expr(&expr, &sample_env()).unwrap_err();
        assert!(matches!(err, EvalError::TypeError { expected, .. } if expected == "bool"));
    }

    #[test]
    fn list_index_out_of_bounds_errors() {
        let list = ExprList {
            list: vec![
                Expr::Const(ExprConst::Int { int: 1 }),
                Expr::Const(ExprConst::Int { int: 2 }),
            ],
        };
        let expr = Expr::Op(ExprOp {
            op: ExprOpCode::Get,
            args: vec![Expr::List(list), Expr::Const(ExprConst::Nat { nat: 5 })],
        });
        let err = eval_expr(&expr, &sample_env()).unwrap_err();
        assert!(matches!(
            err,
            EvalError::OpError {
                op: ExprOpCode::Get,
                ..
            }
        ));
    }
}
