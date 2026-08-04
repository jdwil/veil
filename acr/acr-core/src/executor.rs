//! Safe tree-walking interpreter for Algorithm IR.
//!
//! Provides bounded execution with step limits, stack depth tracking,
//! and full trace recording.

use std::collections::HashMap;

use chrono::Utc;

use crate::error::AcrError;
use crate::ir::{Algorithm, BinOp, Expr, LiteralValue, Statement, UnaryOp};
use crate::trace::{ExecutionResult, ExecutionTrace, TraceEvent, TraceEventKind};
use crate::value::Value;

/// Scope is a single frame in the variable scope stack.
type Scope = HashMap<String, Value>;

/// Configuration for the executor's resource limits.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    pub max_steps: usize,
    pub max_stack_depth: usize,
    pub max_list_size: usize,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_steps: 10_000,
            max_stack_depth: 100,
            max_list_size: 10_000,
        }
    }
}

/// Control flow signal used internally to handle return statements.
enum ControlFlow {
    Continue,
    Return(Value),
}

/// The tree-walking interpreter for Algorithm IR.
pub struct Executor {
    config: ExecutorConfig,
    step_counter: usize,
    max_stack_depth_seen: usize,
    events: Vec<TraceEvent>,
    scopes: Vec<Scope>,
}

impl Executor {
    /// Create a new executor with the given configuration.
    pub fn new(config: ExecutorConfig) -> Self {
        Self {
            config,
            step_counter: 0,
            max_stack_depth_seen: 0,
            events: Vec::new(),
            scopes: Vec::new(),
        }
    }

    /// Execute an algorithm with the given arguments.
    pub fn execute(
        algorithm: &Algorithm,
        args: Vec<Value>,
    ) -> Result<ExecutionTrace, AcrError> {
        Self::execute_with_config(algorithm, args, ExecutorConfig::default())
    }

    /// Execute an algorithm with a custom config.
    pub fn execute_with_config(
        algorithm: &Algorithm,
        args: Vec<Value>,
        config: ExecutorConfig,
    ) -> Result<ExecutionTrace, AcrError> {
        let mut executor = Executor::new(config);
        let started_at = Utc::now();

        // Validate argument count
        if args.len() != algorithm.params.len() {
            return Err(AcrError::InvalidArgCount {
                expected: algorithm.params.len(),
                got: args.len(),
            });
        }

        // Build input record
        let input: Vec<(String, Value)> = algorithm
            .params
            .iter()
            .zip(args.iter())
            .map(|(p, v)| (p.name.clone(), v.clone()))
            .collect();

        // Create initial scope and bind parameters
        let mut initial_scope = Scope::new();
        for (param, arg) in algorithm.params.iter().zip(args.into_iter()) {
            initial_scope.insert(param.name.clone(), arg);
        }
        executor.scopes.push(initial_scope);

        // Execute body
        let result = executor.execute_body(&algorithm.body);

        let completed_at = Utc::now();

        let output = match result {
            Ok(ControlFlow::Return(val)) => ExecutionResult::Success(val),
            Ok(ControlFlow::Continue) => ExecutionResult::Success(Value::Null),
            Err(AcrError::StepLimitExceeded { .. }) => ExecutionResult::StepLimitExceeded,
            Err(AcrError::ExecutionTimeout) => ExecutionResult::Timeout,
            Err(e) => ExecutionResult::Error(e.to_string()),
        };

        let trace = ExecutionTrace {
            algorithm_id: algorithm.id,
            algorithm_version: algorithm.version,
            started_at,
            completed_at,
            input,
            output,
            steps_executed: executor.step_counter,
            max_stack_depth: executor.max_stack_depth_seen,
            events: executor.events,
        };

        // If original result was an error (not step limit / timeout which are captured), propagate
        match &trace.output {
            ExecutionResult::Success(_) => Ok(trace),
            ExecutionResult::StepLimitExceeded => Ok(trace),
            ExecutionResult::Timeout => Ok(trace),
            ExecutionResult::Error(_) => Ok(trace),
        }
    }

    fn tick(&mut self) -> Result<(), AcrError> {
        self.step_counter += 1;
        if self.step_counter > self.config.max_steps {
            return Err(AcrError::StepLimitExceeded {
                limit: self.config.max_steps,
            });
        }
        Ok(())
    }

    fn record_event(&mut self, kind: TraceEventKind) {
        self.events.push(TraceEvent {
            step: self.step_counter,
            kind,
        });
    }

    fn push_scope(&mut self) -> Result<(), AcrError> {
        if self.scopes.len() >= self.config.max_stack_depth {
            return Err(AcrError::StackOverflow {
                depth: self.scopes.len(),
            });
        }
        self.scopes.push(Scope::new());
        if self.scopes.len() > self.max_stack_depth_seen {
            self.max_stack_depth_seen = self.scopes.len();
        }
        Ok(())
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn get_var(&self, name: &str) -> Result<Value, AcrError> {
        for scope in self.scopes.iter().rev() {
            if let Some(val) = scope.get(name) {
                return Ok(val.clone());
            }
        }
        Err(AcrError::UndefinedVariable {
            name: name.to_string(),
        })
    }

    fn set_var_current(&mut self, name: String, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, value);
        }
    }

    fn assign_var(&mut self, name: &str, value: Value) -> Result<(), AcrError> {
        // Search up the stack for an existing binding
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
                return Ok(());
            }
        }
        Err(AcrError::UndefinedVariable {
            name: name.to_string(),
        })
    }

    fn execute_body(&mut self, stmts: &[Statement]) -> Result<ControlFlow, AcrError> {
        for stmt in stmts {
            let flow = self.execute_stmt(stmt)?;
            if let ControlFlow::Return(_) = &flow {
                return Ok(flow);
            }
        }
        Ok(ControlFlow::Continue)
    }

    fn execute_stmt(&mut self, stmt: &Statement) -> Result<ControlFlow, AcrError> {
        self.tick()?;
        match stmt {
            Statement::Let { name, value } => {
                let val = self.eval_expr(value)?;
                self.record_event(TraceEventKind::VarAssigned {
                    name: name.clone(),
                    value: val.clone(),
                });
                self.set_var_current(name.clone(), val);
                Ok(ControlFlow::Continue)
            }
            Statement::Assign { name, value } => {
                let val = self.eval_expr(value)?;
                self.record_event(TraceEventKind::VarAssigned {
                    name: name.clone(),
                    value: val.clone(),
                });
                self.assign_var(name, val)?;
                Ok(ControlFlow::Continue)
            }
            Statement::If {
                condition,
                then_body,
                else_body,
            } => {
                let cond_val = self.eval_expr(condition)?;
                let taken = cond_val.is_truthy();
                self.record_event(TraceEventKind::BranchTaken {
                    condition_value: taken,
                });
                if taken {
                    self.push_scope()?;
                    let flow = self.execute_body(then_body)?;
                    self.pop_scope();
                    Ok(flow)
                } else {
                    self.push_scope()?;
                    let flow = self.execute_body(else_body)?;
                    self.pop_scope();
                    Ok(flow)
                }
            }
            Statement::While { condition, body } => {
                let mut iteration = 0usize;
                loop {
                    self.tick()?;
                    let cond_val = self.eval_expr(condition)?;
                    if !cond_val.is_truthy() {
                        break;
                    }
                    self.record_event(TraceEventKind::LoopIteration { iteration });
                    self.push_scope()?;
                    let flow = self.execute_body(body)?;
                    self.pop_scope();
                    if let ControlFlow::Return(_) = &flow {
                        return Ok(flow);
                    }
                    iteration += 1;
                }
                Ok(ControlFlow::Continue)
            }
            Statement::For { var, iter, body } => {
                let iter_val = self.eval_expr(iter)?;
                let items = match iter_val {
                    Value::List(items) => items,
                    other => {
                        return Err(AcrError::TypeError {
                            expected: "list".to_string(),
                            got: other.type_name().to_string(),
                        });
                    }
                };
                for (iteration, item) in items.into_iter().enumerate() {
                    self.tick()?;
                    self.record_event(TraceEventKind::LoopIteration { iteration });
                    self.push_scope()?;
                    self.set_var_current(var.clone(), item);
                    let flow = self.execute_body(body)?;
                    self.pop_scope();
                    if let ControlFlow::Return(_) = &flow {
                        return Ok(flow);
                    }
                }
                Ok(ControlFlow::Continue)
            }
            Statement::Return(expr) => {
                let val = self.eval_expr(expr)?;
                self.record_event(TraceEventKind::Returned(val.clone()));
                Ok(ControlFlow::Return(val))
            }
            Statement::Expr(expr) => {
                self.eval_expr(expr)?;
                Ok(ControlFlow::Continue)
            }
            Statement::Assert { condition, message } => {
                let val = self.eval_expr(condition)?;
                if val.is_truthy() {
                    self.record_event(TraceEventKind::AssertPassed {
                        message: message.clone(),
                    });
                    Ok(ControlFlow::Continue)
                } else {
                    self.record_event(TraceEventKind::AssertFailed {
                        message: message.clone(),
                    });
                    Err(AcrError::AssertionFailed {
                        message: message.clone(),
                    })
                }
            }
        }
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, AcrError> {
        match expr {
            Expr::Literal(lit) => Ok(Self::literal_to_value(lit)),
            Expr::Var(name) => self.get_var(name),
            Expr::BinOp { op, left, right } => self.eval_binop(*op, left, right),
            Expr::UnaryOp { op, operand } => self.eval_unaryop(*op, operand),
            Expr::Call { function, args } => self.eval_call(function, args),
            Expr::Index { target, index } => {
                let target_val = self.eval_expr(target)?;
                let index_val = self.eval_expr(index)?;
                self.eval_index(target_val, index_val)
            }
            Expr::FieldAccess { target, field } => {
                let target_val = self.eval_expr(target)?;
                self.eval_field_access(target_val, field)
            }
            Expr::ListLiteral(exprs) => {
                let mut items = Vec::with_capacity(exprs.len());
                for e in exprs {
                    items.push(self.eval_expr(e)?);
                }
                if items.len() > self.config.max_list_size {
                    return Err(AcrError::Custom {
                        message: format!(
                            "list size {} exceeds maximum {}",
                            items.len(),
                            self.config.max_list_size
                        ),
                    });
                }
                Ok(Value::List(items))
            }
            Expr::MapLiteral(entries) => {
                let mut map = Vec::with_capacity(entries.len());
                for (key, val_expr) in entries {
                    let val = self.eval_expr(val_expr)?;
                    map.push((key.clone(), val));
                }
                Ok(Value::Map(map))
            }
            Expr::Lambda { .. } => Err(AcrError::Custom {
                message: "lambda expressions are not supported in v0".to_string(),
            }),
        }
    }

    fn literal_to_value(lit: &LiteralValue) -> Value {
        match lit {
            LiteralValue::Null => Value::Null,
            LiteralValue::Bool(b) => Value::Bool(*b),
            LiteralValue::Int(n) => Value::Int(*n),
            LiteralValue::Float(n) => Value::Float(*n),
            LiteralValue::Str(s) => Value::Str(s.clone()),
        }
    }

    fn eval_binop(&mut self, op: BinOp, left: &Expr, right: &Expr) -> Result<Value, AcrError> {
        // Short-circuit for And/Or
        if op == BinOp::And {
            let l = self.eval_expr(left)?;
            if !l.is_truthy() {
                return Ok(Value::Bool(false));
            }
            let r = self.eval_expr(right)?;
            return Ok(Value::Bool(r.is_truthy()));
        }
        if op == BinOp::Or {
            let l = self.eval_expr(left)?;
            if l.is_truthy() {
                return Ok(Value::Bool(true));
            }
            let r = self.eval_expr(right)?;
            return Ok(Value::Bool(r.is_truthy()));
        }

        let l = self.eval_expr(left)?;
        let r = self.eval_expr(right)?;

        match op {
            BinOp::Add => self.eval_add(l, r),
            BinOp::Sub => self.eval_numeric_op(l, r, "Sub", |a, b| a - b, |a, b| a - b),
            BinOp::Mul => self.eval_numeric_op(l, r, "Mul", |a, b| a * b, |a, b| a * b),
            BinOp::Div => self.eval_div(l, r),
            BinOp::Mod => self.eval_mod(l, r),
            BinOp::Eq => Ok(Value::Bool(l == r)),
            BinOp::Ne => Ok(Value::Bool(l != r)),
            BinOp::Lt => self.eval_comparison(l, r, |ord| ord == std::cmp::Ordering::Less),
            BinOp::Le => self.eval_comparison(l, r, |ord| {
                ord == std::cmp::Ordering::Less || ord == std::cmp::Ordering::Equal
            }),
            BinOp::Gt => self.eval_comparison(l, r, |ord| ord == std::cmp::Ordering::Greater),
            BinOp::Ge => self.eval_comparison(l, r, |ord| {
                ord == std::cmp::Ordering::Greater || ord == std::cmp::Ordering::Equal
            }),
            BinOp::And | BinOp::Or => unreachable!(),
        }
    }

    fn eval_add(&self, l: Value, r: Value) -> Result<Value, AcrError> {
        match (&l, &r) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + *b as f64)),
            (Value::Str(a), Value::Str(b)) => Ok(Value::Str(format!("{}{}", a, b))),
            (Value::List(a), Value::List(b)) => {
                let mut result = a.clone();
                result.extend(b.clone());
                if result.len() > self.config.max_list_size {
                    return Err(AcrError::Custom {
                        message: format!(
                            "list size {} exceeds maximum {}",
                            result.len(),
                            self.config.max_list_size
                        ),
                    });
                }
                Ok(Value::List(result))
            }
            _ => Err(AcrError::TypeError {
                expected: "compatible types for Add".to_string(),
                got: format!("{} + {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_numeric_op(
        &self,
        l: Value,
        r: Value,
        op_name: &str,
        int_op: impl Fn(i64, i64) -> i64,
        float_op: impl Fn(f64, f64) -> f64,
    ) -> Result<Value, AcrError> {
        match (&l, &r) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(int_op(*a, *b))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float_op(*a, *b))),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(float_op(*a as f64, *b))),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(float_op(*a, *b as f64))),
            _ => Err(AcrError::TypeError {
                expected: "numeric types".to_string(),
                got: format!("{} {} {}", l.type_name(), op_name, r.type_name()),
            }),
        }
    }

    fn eval_div(&self, l: Value, r: Value) -> Result<Value, AcrError> {
        match (&l, &r) {
            (Value::Int(_), Value::Int(0)) => Err(AcrError::DivisionByZero),
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a / b)),
            (Value::Float(a), Value::Float(b)) => {
                if *b == 0.0 {
                    return Err(AcrError::DivisionByZero);
                }
                Ok(Value::Float(a / b))
            }
            (Value::Int(a), Value::Float(b)) => {
                if *b == 0.0 {
                    return Err(AcrError::DivisionByZero);
                }
                Ok(Value::Float(*a as f64 / b))
            }
            (Value::Float(a), Value::Int(b)) => {
                if *b == 0 {
                    return Err(AcrError::DivisionByZero);
                }
                Ok(Value::Float(a / *b as f64))
            }
            _ => Err(AcrError::TypeError {
                expected: "numeric types".to_string(),
                got: format!("{} / {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_mod(&self, l: Value, r: Value) -> Result<Value, AcrError> {
        match (&l, &r) {
            (Value::Int(_), Value::Int(0)) => Err(AcrError::DivisionByZero),
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a % b)),
            (Value::Float(a), Value::Float(b)) => {
                if *b == 0.0 {
                    return Err(AcrError::DivisionByZero);
                }
                Ok(Value::Float(a % b))
            }
            (Value::Int(a), Value::Float(b)) => {
                if *b == 0.0 {
                    return Err(AcrError::DivisionByZero);
                }
                Ok(Value::Float(*a as f64 % b))
            }
            (Value::Float(a), Value::Int(b)) => {
                if *b == 0 {
                    return Err(AcrError::DivisionByZero);
                }
                Ok(Value::Float(a % *b as f64))
            }
            _ => Err(AcrError::TypeError {
                expected: "numeric types".to_string(),
                got: format!("{} % {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_comparison(
        &self,
        l: Value,
        r: Value,
        pred: impl Fn(std::cmp::Ordering) -> bool,
    ) -> Result<Value, AcrError> {
        let ord = match (&l, &r) {
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal),
            (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal),
            (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)).unwrap_or(std::cmp::Ordering::Equal),
            (Value::Str(a), Value::Str(b)) => a.cmp(b),
            _ => {
                return Err(AcrError::TypeError {
                    expected: "comparable types (int, float, str)".to_string(),
                    got: format!("{} vs {}", l.type_name(), r.type_name()),
                });
            }
        };
        Ok(Value::Bool(pred(ord)))
    }

    fn eval_unaryop(&mut self, op: UnaryOp, operand: &Expr) -> Result<Value, AcrError> {
        let val = self.eval_expr(operand)?;
        match op {
            UnaryOp::Neg => match val {
                Value::Int(n) => Ok(Value::Int(-n)),
                Value::Float(n) => Ok(Value::Float(-n)),
                other => Err(AcrError::TypeError {
                    expected: "numeric".to_string(),
                    got: other.type_name().to_string(),
                }),
            },
            UnaryOp::Not => Ok(Value::Bool(!val.is_truthy())),
        }
    }

    fn eval_index(&self, target: Value, index: Value) -> Result<Value, AcrError> {
        match (target, &index) {
            (Value::List(items), Value::Int(i)) => {
                let idx = if *i < 0 {
                    items.len() as i64 + *i
                } else {
                    *i
                };
                if idx < 0 || idx as usize >= items.len() {
                    Err(AcrError::IndexOutOfBounds {
                        index: *i,
                        len: items.len(),
                    })
                } else {
                    Ok(items[idx as usize].clone())
                }
            }
            (Value::Str(s), Value::Int(i)) => {
                let chars: Vec<char> = s.chars().collect();
                let idx = if *i < 0 {
                    chars.len() as i64 + *i
                } else {
                    *i
                };
                if idx < 0 || idx as usize >= chars.len() {
                    Err(AcrError::IndexOutOfBounds {
                        index: *i,
                        len: chars.len(),
                    })
                } else {
                    Ok(Value::Str(chars[idx as usize].to_string()))
                }
            }
            (Value::Map(entries), Value::Str(key)) => {
                for (k, v) in &entries {
                    if k == key {
                        return Ok(v.clone());
                    }
                }
                Ok(Value::Null)
            }
            (target_val, _) => Err(AcrError::TypeError {
                expected: "indexable type (list, str, map)".to_string(),
                got: format!("{}[{}]", target_val.type_name(), index.type_name()),
            }),
        }
    }

    fn eval_field_access(&self, target: Value, field: &str) -> Result<Value, AcrError> {
        match target {
            Value::Map(entries) => {
                for (k, v) in &entries {
                    if k == field {
                        return Ok(v.clone());
                    }
                }
                Ok(Value::Null)
            }
            other => Err(AcrError::TypeError {
                expected: "map".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn eval_call(&mut self, function: &str, arg_exprs: &[Expr]) -> Result<Value, AcrError> {
        let mut args = Vec::with_capacity(arg_exprs.len());
        for expr in arg_exprs {
            args.push(self.eval_expr(expr)?);
        }

        self.record_event(TraceEventKind::FunctionCalled {
            name: function.to_string(),
            args: args.clone(),
        });

        match function {
            "len" => self.builtin_len(args),
            "push" => self.builtin_push(args),
            "pop" => self.builtin_pop(args),
            "head" => self.builtin_head(args),
            "tail" => self.builtin_tail(args),
            "contains" => self.builtin_contains(args),
            "concat" => self.builtin_concat(args),
            "slice" => self.builtin_slice(args),
            "sort" => self.builtin_sort(args),
            "reverse" => self.builtin_reverse(args),
            "map_get" => self.builtin_map_get(args),
            "map_set" => self.builtin_map_set(args),
            "map_keys" => self.builtin_map_keys(args),
            "to_str" => self.builtin_to_str(args),
            "to_int" => self.builtin_to_int(args),
            "split" => self.builtin_split(args),
            "join" => self.builtin_join(args),
            "type_of" => self.builtin_type_of(args),
            "print" => self.builtin_print(args),
            _ => Err(AcrError::UndefinedFunction {
                name: function.to_string(),
            }),
        }
    }

    fn expect_args(args: &[Value], expected: usize, _name: &str) -> Result<(), AcrError> {
        if args.len() != expected {
            Err(AcrError::InvalidArgCount {
                expected,
                got: args.len(),
            })
        } else {
            Ok(())
        }
    }

    fn builtin_len(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 1, "len")?;
        match &args[0] {
            Value::List(items) => Ok(Value::Int(items.len() as i64)),
            Value::Str(s) => Ok(Value::Int(s.len() as i64)),
            Value::Map(entries) => Ok(Value::Int(entries.len() as i64)),
            other => Err(AcrError::TypeError {
                expected: "list, str, or map".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_push(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 2, "push")?;
        let mut args = args;
        let value = args.pop().unwrap();
        let list = args.pop().unwrap();
        match list {
            Value::List(mut items) => {
                items.push(value);
                if items.len() > self.config.max_list_size {
                    return Err(AcrError::Custom {
                        message: format!(
                            "list size {} exceeds maximum {}",
                            items.len(),
                            self.config.max_list_size
                        ),
                    });
                }
                Ok(Value::List(items))
            }
            other => Err(AcrError::TypeError {
                expected: "list".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_pop(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 1, "pop")?;
        match &args[0] {
            Value::List(items) => {
                if items.is_empty() {
                    return Err(AcrError::Custom {
                        message: "cannot pop from empty list".to_string(),
                    });
                }
                let mut new_items = items.clone();
                new_items.pop();
                Ok(Value::List(new_items))
            }
            other => Err(AcrError::TypeError {
                expected: "list".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_head(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 1, "head")?;
        match &args[0] {
            Value::List(items) => {
                if items.is_empty() {
                    return Err(AcrError::Custom {
                        message: "cannot get head of empty list".to_string(),
                    });
                }
                Ok(items[0].clone())
            }
            other => Err(AcrError::TypeError {
                expected: "list".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_tail(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 1, "tail")?;
        match &args[0] {
            Value::List(items) => {
                if items.is_empty() {
                    return Err(AcrError::Custom {
                        message: "cannot get tail of empty list".to_string(),
                    });
                }
                Ok(Value::List(items[1..].to_vec()))
            }
            other => Err(AcrError::TypeError {
                expected: "list".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_contains(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 2, "contains")?;
        match &args[0] {
            Value::List(items) => Ok(Value::Bool(items.contains(&args[1]))),
            Value::Str(s) => match &args[1] {
                Value::Str(needle) => Ok(Value::Bool(s.contains(needle.as_str()))),
                other => Err(AcrError::TypeError {
                    expected: "str".to_string(),
                    got: other.type_name().to_string(),
                }),
            },
            other => Err(AcrError::TypeError {
                expected: "list or str".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_concat(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 2, "concat")?;
        match (&args[0], &args[1]) {
            (Value::Str(a), Value::Str(b)) => Ok(Value::Str(format!("{}{}", a, b))),
            (Value::List(a), Value::List(b)) => {
                let mut result = a.clone();
                result.extend(b.clone());
                if result.len() > self.config.max_list_size {
                    return Err(AcrError::Custom {
                        message: format!(
                            "list size {} exceeds maximum {}",
                            result.len(),
                            self.config.max_list_size
                        ),
                    });
                }
                Ok(Value::List(result))
            }
            _ => Err(AcrError::TypeError {
                expected: "two strings or two lists".to_string(),
                got: format!("{} and {}", args[0].type_name(), args[1].type_name()),
            }),
        }
    }

    fn builtin_slice(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 3, "slice")?;
        let start = match &args[1] {
            Value::Int(n) => *n as usize,
            other => {
                return Err(AcrError::TypeError {
                    expected: "int".to_string(),
                    got: other.type_name().to_string(),
                });
            }
        };
        let end = match &args[2] {
            Value::Int(n) => *n as usize,
            other => {
                return Err(AcrError::TypeError {
                    expected: "int".to_string(),
                    got: other.type_name().to_string(),
                });
            }
        };
        match &args[0] {
            Value::List(items) => {
                let end = end.min(items.len());
                let start = start.min(end);
                Ok(Value::List(items[start..end].to_vec()))
            }
            Value::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                let end = end.min(chars.len());
                let start = start.min(end);
                Ok(Value::Str(chars[start..end].iter().collect()))
            }
            other => Err(AcrError::TypeError {
                expected: "list or str".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_sort(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 1, "sort")?;
        match &args[0] {
            Value::List(items) => {
                let mut sorted = items.clone();
                sorted.sort_by(|a, b| self.compare_values(a, b));
                Ok(Value::List(sorted))
            }
            other => Err(AcrError::TypeError {
                expected: "list".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn compare_values(&self, a: &Value, b: &Value) -> std::cmp::Ordering {
        match (a, b) {
            (Value::Int(x), Value::Int(y)) => x.cmp(y),
            (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
            (Value::Str(x), Value::Str(y)) => x.cmp(y),
            (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
            (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(std::cmp::Ordering::Equal),
            _ => std::cmp::Ordering::Equal,
        }
    }

    fn builtin_reverse(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 1, "reverse")?;
        match &args[0] {
            Value::List(items) => {
                let mut reversed = items.clone();
                reversed.reverse();
                Ok(Value::List(reversed))
            }
            Value::Str(s) => Ok(Value::Str(s.chars().rev().collect())),
            other => Err(AcrError::TypeError {
                expected: "list or str".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_map_get(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 2, "map_get")?;
        let key = match &args[1] {
            Value::Str(k) => k,
            other => {
                return Err(AcrError::TypeError {
                    expected: "str".to_string(),
                    got: other.type_name().to_string(),
                });
            }
        };
        match &args[0] {
            Value::Map(entries) => {
                for (k, v) in entries {
                    if k == key {
                        return Ok(v.clone());
                    }
                }
                Ok(Value::Null)
            }
            other => Err(AcrError::TypeError {
                expected: "map".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_map_set(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 3, "map_set")?;
        let key = match &args[1] {
            Value::Str(k) => k.clone(),
            other => {
                return Err(AcrError::TypeError {
                    expected: "str".to_string(),
                    got: other.type_name().to_string(),
                });
            }
        };
        match &args[0] {
            Value::Map(entries) => {
                let mut new_entries = entries.clone();
                let mut found = false;
                for entry in &mut new_entries {
                    if entry.0 == key {
                        entry.1 = args[2].clone();
                        found = true;
                        break;
                    }
                }
                if !found {
                    new_entries.push((key, args[2].clone()));
                }
                Ok(Value::Map(new_entries))
            }
            other => Err(AcrError::TypeError {
                expected: "map".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_map_keys(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 1, "map_keys")?;
        match &args[0] {
            Value::Map(entries) => {
                let keys: Vec<Value> = entries.iter().map(|(k, _)| Value::Str(k.clone())).collect();
                Ok(Value::List(keys))
            }
            other => Err(AcrError::TypeError {
                expected: "map".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_to_str(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 1, "to_str")?;
        let s = match &args[0] {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(n) => n.to_string(),
            Value::Str(s) => s.clone(),
            other => format!("{}", other),
        };
        Ok(Value::Str(s))
    }

    fn builtin_to_int(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 1, "to_int")?;
        match &args[0] {
            Value::Int(n) => Ok(Value::Int(*n)),
            Value::Float(n) => Ok(Value::Int(*n as i64)),
            Value::Bool(b) => Ok(Value::Int(if *b { 1 } else { 0 })),
            Value::Str(s) => match s.parse::<i64>() {
                Ok(n) => Ok(Value::Int(n)),
                Err(_) => Err(AcrError::Custom {
                    message: format!("cannot convert string '{}' to int", s),
                }),
            },
            other => Err(AcrError::TypeError {
                expected: "int, float, bool, or str".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    fn builtin_split(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 2, "split")?;
        let s = match &args[0] {
            Value::Str(s) => s,
            other => {
                return Err(AcrError::TypeError {
                    expected: "str".to_string(),
                    got: other.type_name().to_string(),
                });
            }
        };
        let delimiter = match &args[1] {
            Value::Str(d) => d,
            other => {
                return Err(AcrError::TypeError {
                    expected: "str".to_string(),
                    got: other.type_name().to_string(),
                });
            }
        };
        let parts: Vec<Value> = s.split(delimiter.as_str()).map(|p| Value::Str(p.to_string())).collect();
        Ok(Value::List(parts))
    }

    fn builtin_join(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 2, "join")?;
        let items = match &args[0] {
            Value::List(items) => items,
            other => {
                return Err(AcrError::TypeError {
                    expected: "list".to_string(),
                    got: other.type_name().to_string(),
                });
            }
        };
        let delimiter = match &args[1] {
            Value::Str(d) => d,
            other => {
                return Err(AcrError::TypeError {
                    expected: "str".to_string(),
                    got: other.type_name().to_string(),
                });
            }
        };
        let strings: Result<Vec<String>, AcrError> = items
            .iter()
            .map(|v| match v {
                Value::Str(s) => Ok(s.clone()),
                other => Err(AcrError::TypeError {
                    expected: "str".to_string(),
                    got: other.type_name().to_string(),
                }),
            })
            .collect();
        Ok(Value::Str(strings?.join(delimiter.as_str())))
    }

    fn builtin_type_of(&self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 1, "type_of")?;
        Ok(Value::Str(args[0].type_name().to_string()))
    }

    fn builtin_print(&mut self, args: Vec<Value>) -> Result<Value, AcrError> {
        Self::expect_args(&args, 1, "print")?;
        // print is a no-op aside from trace recording (already recorded in eval_call)
        Ok(Value::Null)
    }
}
