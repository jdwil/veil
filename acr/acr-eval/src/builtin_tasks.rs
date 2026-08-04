use acr_core::value::Value;

use crate::task::{Difficulty, ParamHint, Task, TestCase};

/// Returns all built-in tasks for the list/string manipulation domain
pub fn all_tasks() -> Vec<Task> {
    vec![
        list_reverse_task(),
        list_filter_even_task(),
        string_palindrome_task(),
        list_flatten_task(),
        list_deduplicate_task(),
    ]
}

fn list_reverse_task() -> Task {
    Task {
        id: "list-reverse".to_string(),
        name: "Reverse a List".to_string(),
        description: "Given a list, return it in reverse order without using the built-in reverse function.".to_string(),
        domain: "list-manipulation".to_string(),
        difficulty: Difficulty::Easy,
        param_hints: vec![ParamHint {
            name: "items".to_string(),
            description: "The list to reverse".to_string(),
            type_hint: "List<Any>".to_string(),
        }],
        test_cases: vec![
            TestCase {
                name: "basic".to_string(),
                input: vec![Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3),
                ])],
                expected_output: Value::List(vec![
                    Value::Int(3),
                    Value::Int(2),
                    Value::Int(1),
                ]),
            },
            TestCase {
                name: "empty".to_string(),
                input: vec![Value::List(vec![])],
                expected_output: Value::List(vec![]),
            },
            TestCase {
                name: "single".to_string(),
                input: vec![Value::List(vec![Value::Int(42)])],
                expected_output: Value::List(vec![Value::Int(42)]),
            },
            TestCase {
                name: "strings".to_string(),
                input: vec![Value::List(vec![
                    Value::Str("a".to_string()),
                    Value::Str("b".to_string()),
                    Value::Str("c".to_string()),
                ])],
                expected_output: Value::List(vec![
                    Value::Str("c".to_string()),
                    Value::Str("b".to_string()),
                    Value::Str("a".to_string()),
                ]),
            },
        ],
    }
}

fn list_filter_even_task() -> Task {
    Task {
        id: "filter-even".to_string(),
        name: "Filter Even Numbers".to_string(),
        description: "Given a list of integers, return only the even numbers.".to_string(),
        domain: "list-manipulation".to_string(),
        difficulty: Difficulty::Easy,
        param_hints: vec![ParamHint {
            name: "numbers".to_string(),
            description: "List of integers".to_string(),
            type_hint: "List<Int>".to_string(),
        }],
        test_cases: vec![
            TestCase {
                name: "mixed".to_string(),
                input: vec![Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3),
                    Value::Int(4),
                    Value::Int(5),
                    Value::Int(6),
                ])],
                expected_output: Value::List(vec![
                    Value::Int(2),
                    Value::Int(4),
                    Value::Int(6),
                ]),
            },
            TestCase {
                name: "all_even".to_string(),
                input: vec![Value::List(vec![Value::Int(2), Value::Int(4)])],
                expected_output: Value::List(vec![Value::Int(2), Value::Int(4)]),
            },
            TestCase {
                name: "none_even".to_string(),
                input: vec![Value::List(vec![
                    Value::Int(1),
                    Value::Int(3),
                    Value::Int(5),
                ])],
                expected_output: Value::List(vec![]),
            },
            TestCase {
                name: "empty".to_string(),
                input: vec![Value::List(vec![])],
                expected_output: Value::List(vec![]),
            },
            TestCase {
                name: "negatives".to_string(),
                input: vec![Value::List(vec![
                    Value::Int(-2),
                    Value::Int(-1),
                    Value::Int(0),
                    Value::Int(1),
                    Value::Int(2),
                ])],
                expected_output: Value::List(vec![
                    Value::Int(-2),
                    Value::Int(0),
                    Value::Int(2),
                ]),
            },
        ],
    }
}

fn string_palindrome_task() -> Task {
    Task {
        id: "is-palindrome".to_string(),
        name: "Check Palindrome".to_string(),
        description: "Given a string, return true if it is a palindrome (same forwards and backwards), false otherwise. Case-sensitive.".to_string(),
        domain: "string-manipulation".to_string(),
        difficulty: Difficulty::Easy,
        param_hints: vec![ParamHint {
            name: "text".to_string(),
            description: "The string to check".to_string(),
            type_hint: "Str".to_string(),
        }],
        test_cases: vec![
            TestCase {
                name: "yes_racecar".to_string(),
                input: vec![Value::Str("racecar".to_string())],
                expected_output: Value::Bool(true),
            },
            TestCase {
                name: "no_hello".to_string(),
                input: vec![Value::Str("hello".to_string())],
                expected_output: Value::Bool(false),
            },
            TestCase {
                name: "yes_aba".to_string(),
                input: vec![Value::Str("aba".to_string())],
                expected_output: Value::Bool(true),
            },
            TestCase {
                name: "empty".to_string(),
                input: vec![Value::Str("".to_string())],
                expected_output: Value::Bool(true),
            },
            TestCase {
                name: "single_char".to_string(),
                input: vec![Value::Str("x".to_string())],
                expected_output: Value::Bool(true),
            },
        ],
    }
}

fn list_flatten_task() -> Task {
    Task {
        id: "list-flatten".to_string(),
        name: "Flatten Nested List".to_string(),
        description: "Given a list that may contain nested lists, flatten it into a single-level list.".to_string(),
        domain: "list-manipulation".to_string(),
        difficulty: Difficulty::Medium,
        param_hints: vec![ParamHint {
            name: "nested".to_string(),
            description: "A potentially nested list".to_string(),
            type_hint: "List<Any>".to_string(),
        }],
        test_cases: vec![
            TestCase {
                name: "one_level".to_string(),
                input: vec![Value::List(vec![
                    Value::Int(1),
                    Value::List(vec![Value::Int(2), Value::Int(3)]),
                    Value::Int(4),
                ])],
                expected_output: Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3),
                    Value::Int(4),
                ]),
            },
            TestCase {
                name: "already_flat".to_string(),
                input: vec![Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3),
                ])],
                expected_output: Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3),
                ]),
            },
            TestCase {
                name: "empty".to_string(),
                input: vec![Value::List(vec![])],
                expected_output: Value::List(vec![]),
            },
            TestCase {
                name: "deep".to_string(),
                input: vec![Value::List(vec![Value::List(vec![Value::List(vec![
                    Value::Int(1),
                ])])])],
                expected_output: Value::List(vec![Value::Int(1)]),
            },
        ],
    }
}

fn list_deduplicate_task() -> Task {
    Task {
        id: "list-dedup".to_string(),
        name: "Deduplicate List".to_string(),
        description: "Given a list, return a new list with duplicates removed, preserving first-occurrence order.".to_string(),
        domain: "list-manipulation".to_string(),
        difficulty: Difficulty::Medium,
        param_hints: vec![ParamHint {
            name: "items".to_string(),
            description: "List with potential duplicates".to_string(),
            type_hint: "List<Any>".to_string(),
        }],
        test_cases: vec![
            TestCase {
                name: "integers".to_string(),
                input: vec![Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(2),
                    Value::Int(3),
                    Value::Int(1),
                ])],
                expected_output: Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3),
                ]),
            },
            TestCase {
                name: "strings".to_string(),
                input: vec![Value::List(vec![
                    Value::Str("a".to_string()),
                    Value::Str("b".to_string()),
                    Value::Str("a".to_string()),
                ])],
                expected_output: Value::List(vec![
                    Value::Str("a".to_string()),
                    Value::Str("b".to_string()),
                ]),
            },
            TestCase {
                name: "no_dupes".to_string(),
                input: vec![Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3),
                ])],
                expected_output: Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3),
                ]),
            },
            TestCase {
                name: "all_same".to_string(),
                input: vec![Value::List(vec![
                    Value::Int(5),
                    Value::Int(5),
                    Value::Int(5),
                ])],
                expected_output: Value::List(vec![Value::Int(5)]),
            },
            TestCase {
                name: "empty".to_string(),
                input: vec![Value::List(vec![])],
                expected_output: Value::List(vec![]),
            },
        ],
    }
}
