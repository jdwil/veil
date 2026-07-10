# Writing Layer Prompts

This guide defines how to write the `prompt` section of a `.layer` file. These
prompts are fed to LLMs (RAG-style) when they generate VEIL code that uses your
layer. The prompt is the layer's "instruction manual for AI."

---

## Purpose

The `prompt` section teaches an LLM agent:
1. What constructs and statements the layer provides
2. How to use them correctly (constraints)
3. Common patterns and idioms
4. What NOT to do and why

The codegen toolchain ignores this section entirely — it exists solely for LLM
context injection.

---

## Format

```
prompt
  [Content indented under the prompt keyword]
  [All lines at this indent level are accumulated as raw text]
```

The content uses RFC 2119 keywords (MUST, SHOULD, MAY, MUST NOT, SHOULD NOT)
for clarity of obligation vs recommendation.

---

## Required Sections

Every layer prompt MUST include these sections, in this order:

### 1. Role Statement
A single opening line establishing context.
```
You are writing VEIL code using the [layer_name] layer.
```

### 2. Overview
One paragraph (2-4 sentences) explaining what this layer provides and when to
use it.

### 3. Constructs Available
A brief reference of keywords and their purpose. Use a compact list format:
```
## Constructs
- `kw` (Shape) — one-line purpose
```

### 4. Constraints
Directive rules using RFC 2119 keywords. Every negative constraint MUST explain
WHY — this prevents the LLM from "creatively" violating it.
```
## Constraints
- You MUST [action] because [reason]
- You MUST NOT [action] because [consequence]
- You SHOULD [recommendation] because [benefit]
```

### 5. Patterns
Minimal, complete examples showing correct usage. Annotate with brief comments.
These should be copy-paste-ready templates the LLM can adapt.

### 6. Common Mistakes
Anticipate what LLMs will get wrong based on observed failures. Format as
"wrong → right" pairs with explanations.

---

## Style Guidelines

- **Be directive, not descriptive.** "You MUST use @dep" not "Dependencies are
  declared with @dep."
- **Every constraint has a reason.** "You MUST NOT construct adapters in handlers
  because the DI container manages lifecycle and testing requires mock injection."
- **Examples are minimal but complete.** Show the smallest correct usage, not a
  full application.
- **Avoid redundancy with the layer file itself.** Don't re-list every field of
  every construct — focus on usage patterns and constraints the LLM needs to
  understand.
- **Assume the LLM knows Rust/TypeScript.** Don't explain what async/await is.
  DO explain VEIL-specific syntax (`!` suffix, `->` closures, `Res!<T>`).
- **Address the LLM directly.** Use "you" and imperative mood.

---

## Testing Your Prompt

After writing a prompt, verify by asking yourself:
1. Could an LLM produce valid `.veil` code using only this prompt + the layer's
   construct list? If not, add what's missing.
2. Does the prompt prevent the specific errors you've seen in generated code?
3. Is every MUST NOT accompanied by a "because"?

---

## Example: Minimal Layer Prompt

```
prompt
  You are writing VEIL code using the transports layer.

  ## Overview
  This layer provides HTTP endpoint and WebSocket handler constructs for
  defining network-facing APIs. Use it alongside ddd.layer to expose your
  domain services over HTTP.

  ## Constructs
  - `endpoint` (fn) — a single HTTP endpoint with method, path, and handler body
  - `ws` (fn) — a WebSocket connection handler with on_connect/on_message steps

  ## Constraints
  - You MUST declare the HTTP method and path in the endpoint's annotations
    because the runtime uses them for route registration.
  - You MUST NOT put business logic directly in endpoints because they are
    thin wrappers that delegate to application handlers.
  - You SHOULD use typed request/response bodies because they generate
    OpenAPI documentation automatically.

  ## Patterns
  ```
  endpoint GetUser
    @method(GET)
    @path("/users/{id}")
    input
      id: Id
      @dep user_handler: HandleGetUser
    step handle
      result = user_handler.execute!(id)
      ret result
  ```

  ## Common Mistakes
  - Wrong: Putting SQL or repo calls directly in an endpoint.
    Right: Delegate to a handler in the application group.
  - Wrong: Forgetting @method and @path annotations.
    Right: Every endpoint MUST have both — the runtime cannot route without them.
```
