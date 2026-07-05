# CORE-006: Tuples

## VEIL Syntax
```
pair: (Str, Int)
coords = (x, y)
first, second = get_pair()
```

## Generated Rust
```rust
pair: (String, i64)
let coords = (x, y);
let (first, second) = get_pair();
```

---

# CORE-007: Type Aliases

## VEIL Syntax
```
type UserId = UUID
type Coordinate = (F64, F64)
```

## Generated Rust
```rust
pub type UserId = Uuid;
pub type Coordinate = (f64, f64);
```

---

# CORE-008: Constants

## VEIL Syntax
```
const MAX_RETRIES = 3
const DEFAULT_TIMEOUT = 30000
```

## Generated Rust
```rust
pub const MAX_RETRIES: i64 = 3;
pub const DEFAULT_TIMEOUT: i64 = 30000;
```

---

# CORE-009: String Interpolation

## VEIL Syntax
```
msg = f"Hello {name}, you have {count} items"
```

## Generated Rust
```rust
let msg = format!("Hello {}, you have {} items", name, count);
```

---

# CORE-010: Array/Slice Types

## VEIL Syntax
```
scores: [Int]
matrix: [[F64]]
fixed: [Int; 3]
```

## Generated Rust
```rust
scores: Vec<i64>
matrix: Vec<Vec<f64>>
fixed: [i64; 3]
```
