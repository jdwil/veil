//! Integration tests demonstrating the full ACR end-to-end flow.

use std::sync::Arc;

use acr_core::executor::Executor;
use acr_core::ir::{Algorithm, BinOp, Expr, LiteralValue, Param, Statement, TypeHint};
use acr_core::trace::ExecutionResult;
use acr_core::value::Value;
use acr_eval::builtin_tasks::all_tasks;
use acr_eval::EvalHarness;
use acr_library::store::{AlgorithmStore, FsStore};
use acr_library::{PromotionStatus, ScoreRecord};
use acr_runtime::runtime::Runtime;
use acr_runtime::selector::Goal;
use chrono::Utc;
use tempfile::TempDir;

// ──────────────────────────────────────────────────────────────────────────────
// Helper functions for building Algorithm IR
// ──────────────────────────────────────────────────────────────────────────────

fn lit_int(n: i64) -> Expr {
    Expr::Literal(LiteralValue::Int(n))
}

fn var(name: &str) -> Expr {
    Expr::Var(name.to_string())
}

fn binop(op: BinOp, left: Expr, right: Expr) -> Expr {
    Expr::BinOp {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn call(function: &str, args: Vec<Expr>) -> Expr {
    Expr::Call {
        function: function.to_string(),
        args,
    }
}

fn index(target: Expr, idx: Expr) -> Expr {
    Expr::Index {
        target: Box::new(target),
        index: Box::new(idx),
    }
}

fn let_stmt(name: &str, value: Expr) -> Statement {
    Statement::Let {
        name: name.to_string(),
        value,
    }
}

fn assign_stmt(name: &str, value: Expr) -> Statement {
    Statement::Assign {
        name: name.to_string(),
        value,
    }
}

fn return_stmt(expr: Expr) -> Statement {
    Statement::Return(expr)
}

fn while_stmt(condition: Expr, body: Vec<Statement>) -> Statement {
    Statement::While { condition, body }
}

fn for_stmt(var_name: &str, iter: Expr, body: Vec<Statement>) -> Statement {
    Statement::For {
        var: var_name.to_string(),
        iter,
        body,
    }
}

fn if_stmt(condition: Expr, then_body: Vec<Statement>, else_body: Vec<Statement>) -> Statement {
    Statement::If {
        condition,
        then_body,
        else_body,
    }
}

fn list_literal(items: Vec<Expr>) -> Expr {
    Expr::ListLiteral(items)
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 1: Basic executor - add two numbers
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn test_executor_basic() {
    // Algorithm: fn add(a: Int, b: Int) -> return a + b
    let algorithm = Algorithm::new(
        "add",
        "Add two integers",
        "math",
        vec![
            Param {
                name: "a".to_string(),
                type_hint: TypeHint::Int,
            },
            Param {
                name: "b".to_string(),
                type_hint: TypeHint::Int,
            },
        ],
        vec![return_stmt(binop(BinOp::Add, var("a"), var("b")))],
    );

    let trace = Executor::execute(&algorithm, vec![Value::Int(3), Value::Int(7)]).unwrap();

    match &trace.output {
        ExecutionResult::Success(val) => {
            assert_eq!(*val, Value::Int(10));
        }
        other => panic!("Expected success, got: {:?}", other),
    }

    assert!(trace.steps_executed > 0);
    assert_eq!(trace.algorithm_id, algorithm.id);
    assert_eq!(trace.algorithm_version, 1);
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 2: Eval harness - list reverse with while loop
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn test_eval_harness() {
    // Algorithm: reverse a list using a while loop
    //   let result = []
    //   let i = len(items) - 1
    //   while i >= 0:
    //     result = push(result, items[i])
    //     i = i - 1
    //   return result
    let algorithm = Algorithm::new(
        "list_reverse",
        "Reverse a list using a while loop",
        "list-manipulation",
        vec![Param {
            name: "items".to_string(),
            type_hint: TypeHint::List(Box::new(TypeHint::Any)),
        }],
        vec![
            let_stmt("result", list_literal(vec![])),
            let_stmt(
                "i",
                binop(BinOp::Sub, call("len", vec![var("items")]), lit_int(1)),
            ),
            while_stmt(
                binop(BinOp::Ge, var("i"), lit_int(0)),
                vec![
                    assign_stmt("result", call("push", vec![var("result"), index(var("items"), var("i"))])),
                    assign_stmt("i", binop(BinOp::Sub, var("i"), lit_int(1))),
                ],
            ),
            return_stmt(var("result")),
        ],
    );

    // Find the list-reverse task from built-in tasks
    let tasks = all_tasks();
    let reverse_task = tasks.iter().find(|t| t.id == "list-reverse").unwrap();

    let harness = EvalHarness::new();
    let result = harness.evaluate(&algorithm, reverse_task).unwrap();

    assert_eq!(result.score, 1.0, "Expected perfect score, got {}", result.score);
    assert_eq!(result.passed, result.total_cases);
    assert_eq!(result.failed, 0);

    // Verify individual test results
    for test_result in &result.test_results {
        assert!(
            test_result.passed,
            "Test case '{}' failed: {:?}",
            test_result.case_name, test_result.error
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 3: Library lifecycle - store, retrieve, update, promote
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_library_lifecycle() {
    let tmp_dir = TempDir::new().unwrap();
    let store = FsStore::new(tmp_dir.path().to_path_buf()).await.unwrap();

    // Create algorithm
    let algorithm = Algorithm::new(
        "sorter",
        "Sort a list of integers",
        "list-manipulation",
        vec![Param {
            name: "items".to_string(),
            type_hint: TypeHint::List(Box::new(TypeHint::Int)),
        }],
        vec![return_stmt(call("sort", vec![var("items")]))],
    );
    let algo_id = algorithm.id;

    // Store it
    let entry = store.create(&algorithm).await.unwrap();
    assert_eq!(entry.name, "sorter");
    assert_eq!(entry.status, PromotionStatus::Candidate);
    assert_eq!(entry.current_version, 1);

    // Retrieve it
    let retrieved = store.get(algo_id).await.unwrap();
    assert_eq!(retrieved.name, "sorter");
    assert_eq!(retrieved.version, 1);

    // Update with new version
    let mut updated_algo = retrieved.clone();
    updated_algo.version = 2;
    updated_algo.description = "Improved sort algorithm".to_string();
    let updated_entry = store.update(&updated_algo).await.unwrap();
    assert_eq!(updated_entry.current_version, 2);
    assert_eq!(updated_entry.versions.len(), 2);

    // Retrieve specific version
    let v1 = store.get_version(algo_id, 1).await.unwrap();
    assert_eq!(v1.version, 1);
    let v2 = store.get_version(algo_id, 2).await.unwrap();
    assert_eq!(v2.version, 2);
    assert_eq!(v2.description, "Improved sort algorithm");

    // Promote
    let promoted_entry = store.promote(algo_id).await.unwrap();
    assert_eq!(promoted_entry.status, PromotionStatus::Promoted);

    // Verify via get_entry
    let final_entry = store.get_entry(algo_id).await.unwrap();
    assert_eq!(final_entry.status, PromotionStatus::Promoted);
    assert_eq!(final_entry.current_version, 2);

    // Verify status
    let status = store.status().await.unwrap();
    assert_eq!(status.total_algorithms, 1);
    assert_eq!(status.promoted, 1);
    assert_eq!(status.candidates, 0);
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 4: Full learning loop - create, evaluate, improve, promote, execute
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_full_learning_loop() {
    let tmp_dir = TempDir::new().unwrap();
    let store = Arc::new(FsStore::new(tmp_dir.path().to_path_buf()).await.unwrap());

    // Step a: Create a candidate algorithm for filter-even task
    // Algorithm: iterate list, check mod 2 == 0, collect results
    //   let result = []
    //   for n in numbers:
    //     if n % 2 == 0:
    //       result = push(result, n)
    //   return result
    let filter_even_algo = Algorithm::new(
        "filter_even",
        "Filter even numbers from a list",
        "list-manipulation",
        vec![Param {
            name: "numbers".to_string(),
            type_hint: TypeHint::List(Box::new(TypeHint::Int)),
        }],
        vec![
            let_stmt("result", list_literal(vec![])),
            for_stmt(
                "n",
                var("numbers"),
                vec![if_stmt(
                    binop(BinOp::Eq, binop(BinOp::Mod, var("n"), lit_int(2)), lit_int(0)),
                    vec![assign_stmt(
                        "result",
                        call("push", vec![var("result"), var("n")]),
                    )],
                    vec![],
                )],
            ),
            return_stmt(var("result")),
        ],
    );
    let algo_id = filter_even_algo.id;

    // Step b: Store it in the library
    let entry = store.create(&filter_even_algo).await.unwrap();
    assert_eq!(entry.status, PromotionStatus::Candidate);

    // Step c: Evaluate against the filter-even task
    let tasks = all_tasks();
    let filter_task = tasks.iter().find(|t| t.id == "filter-even").unwrap();

    let harness = EvalHarness::new();
    let eval_result = harness.evaluate(&filter_even_algo, filter_task).unwrap();

    // Record the score
    store
        .record_score(
            algo_id,
            ScoreRecord {
                version: filter_even_algo.version,
                score: eval_result.score,
                task_id: "filter-even".to_string(),
                evaluated_at: Utc::now(),
            },
        )
        .await
        .unwrap();

    // Step d: If score < 1.0, create improved version
    // (Our algorithm should already pass all cases, but we test the flow)
    let final_score = if eval_result.score < 1.0 {
        // Create an improved version (same logic here since original should work)
        let mut improved = filter_even_algo.clone();
        improved.version = 2;
        improved.description = "Improved filter-even implementation".to_string();
        store.update(&improved).await.unwrap();

        let improved_result = harness.evaluate(&improved, filter_task).unwrap();
        store
            .record_score(
                algo_id,
                ScoreRecord {
                    version: improved.version,
                    score: improved_result.score,
                    task_id: "filter-even".to_string(),
                    evaluated_at: Utc::now(),
                },
            )
            .await
            .unwrap();
        improved_result.score
    } else {
        eval_result.score
    };

    assert_eq!(final_score, 1.0, "Expected perfect score after improvement");

    // Step e: Promote the successful version
    let promoted = store.promote(algo_id).await.unwrap();
    assert_eq!(promoted.status, PromotionStatus::Promoted);

    // Step f: Use Runtime to execute the promoted algorithm with a new input
    let mut runtime = Runtime::new(store.clone());

    let goal = Goal {
        description: "Filter even numbers".to_string(),
        domain: "list-manipulation".to_string(),
        tags: vec![],
        input: vec![Value::List(vec![
            Value::Int(10),
            Value::Int(15),
            Value::Int(20),
            Value::Int(25),
            Value::Int(30),
        ])],
    };

    let result = runtime.execute_goal(goal).await.unwrap();
    assert_eq!(
        result,
        Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)])
    );

    // Verify runtime state was updated
    let state = runtime.state();
    assert_eq!(state.history.len(), 1);
    assert!(state.history[0].success);
    assert_eq!(
        state.memory.get("last_result"),
        Some(&Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]))
    );
}
