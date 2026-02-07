---
description: "Guidelines for writing compilable rustdoc examples in Rust code"
applyTo: "**/*.rs"
---

# Rustdoc Examples Guidelines

Writing high-quality documentation examples is critical for API usability and maintainability. All rustdoc examples should be compilable and demonstrate actual usage patterns.

## Core Principles

- **Examples must be compilable** unless there is a clear reason they cannot be (document the reason in a comment).
- Use the `no_run` attribute only when the example cannot actually execute at compile-time (e.g., requires runtime setup like a filesystem, network, or database).
- Always verify examples compile by running `cargo test --doc`.
- Examples should demonstrate actual usage patterns, not pseudocode.
- For complex setup requirements (like loading fonts or initializing context), show the minimal setup needed or explain why it cannot be included.

## Example Patterns

### ❌ Avoid: Using `ignore` attribute

````rust
/// ```ignore  // Don't just ignore compilation
/// let x = some_function();
/// ```
````

This hides compilation issues and makes examples unreliable.

### ✅ Good: Using `compile_fail` for intentional errors

````rust
/// ```compile_fail
/// let x: i32 = "not an integer";  // This should not compile
/// ```
````

Use this when you want to show code that intentionally fails to compile (e.g., demonstrating type safety).

### ✅ Good: Using `no_run` for I/O operations

````rust
/// ```no_run
/// use std::fs;
/// let contents = fs::read_to_string("file.txt")?;
/// ```
````

Use this for examples that compile but cannot run in `cargo test --doc` (file I/O, network calls, database queries, complex initialization).

### ✅ Best: Fully compilable and runnable examples

````rust
/// ```
/// let x = 2 + 2;
/// assert_eq!(x, 4);
/// ```
````

This is the ideal: examples that both compile and run. Use this whenever possible.

## Special Cases

### Complex Initialization (Unsafe Context)

When your example requires complex initialization (like a Context struct), document why it cannot be included:

````rust
/// ```no_run
/// use cadmus_core::view::dialog::Dialog;
/// use cadmus_core::view::{Event, ViewId};
///
/// // Note: In actual use, `context` is provided by the application.
/// // Dialog::builder requires a properly initialized Context with
/// // Display and Fonts, so we show the API pattern here.
/// # let mut context = unsafe { std::mem::zeroed() };
/// let dialog = Dialog::builder(ViewId::AboutDialog, "Are you sure?".to_string())
///     .add_button("Cancel", Event::Close(ViewId::AboutDialog))
///     .add_button("Confirm", Event::Validate)
///     .build(&mut context);
/// ```
````

### Multi-line Example Setup

Use line comments (`#` prefix) to hide boilerplate setup that distracts from the main concept:

````rust
/// ```
/// # fn my_function(x: i32) -> i32 { x * 2 }
/// let result = my_function(5);
/// assert_eq!(result, 10);
/// ```
````

Lines starting with `#` are compiled but hidden from documentation output.

## Verification Workflow

Before committing rustdoc examples:

1. **Write the example** with appropriate attributes
2. **Test it compiles**: `cargo test --doc`
3. **Verify in documentation**: `cargo doc --open`
4. **Review the output** to ensure examples are clear and properly formatted

## Common Issues and Solutions

| Issue                         | Solution                                |
| ----------------------------- | --------------------------------------- |
| Example doesn't compile       | Check imports and type signatures       |
| Boilerplate distracts         | Use `#` to hide setup code              |
| Can't run in test environment | Use `no_run` attribute and document why |
| Example is pseudocode         | Rewrite to be actual, working code      |
| Example confuses API usage    | Simplify or add explanatory comments    |

## Quality Checklist

Before committing code with rustdoc examples:

- [ ] All examples compile (`cargo test --doc`)
- [ ] Examples demonstrate actual usage patterns
- [ ] Unnecessary boilerplate is hidden with `#`
- [ ] `no_run` is justified with a comment
- [ ] Examples appear correctly in generated documentation
- [ ] Complex examples are broken into smaller, digestible pieces
