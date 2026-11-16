- Use `cargo +nightly fmt` to format rust code
- On commit, run `cargo +nightly fmt` when the change includes Rust
  code
- Use `cargo check --all-targets` when checking compilation

# Rust code style

- Use the minimum visibility that works: private, then `pub(crate)`,
  then `pub`. Use `pub(crate)` for items shared within the crate.
  Prefer private (no `pub`) over `pub(crate)` where possible. A
  `pub(crate)` item used only in its own module (including that
  module's tests) must be private.
- Use `"{x}"` instead of `"{}"` for format strings where `x` is the
  variable name
- Do not use `map_or`, use `map(..).unwrap_or(..)` instead
- Do not use `.take()`. Use `mem::take` instead
- Use `cmp::min` instead of `.min`
- Do not use `Default::default()`
- When deriving `Display`, use qualified name
  (e.g., `derive_more::Display`)
- Do not use `super` keyword; use imports (`use crate::...`) instead
- Do not use `use ... as` except in arc-error
- Do not use the `ref` keyword. Match by value and borrow in the guard
  (e.g., `Err(err) if err.kind() == ...`)
- Prefer imports over qualified names (e.g., `use foo::Bar; Bar` not
  `foo::Bar`)
- Do not import `anyhow::Result`. Always write `anyhow::Result`
- Do not use `Self` except in return type
- In `assert_eq!`, expected value is LHS
  (e.g., `assert_eq!(expected, actual)`)
- Use `Vec::new()` not `vec![]`
- Prefer readable code over comments. Do not comment key bindings or
  other facts that belong to a different module. Keep a comment only
  for a constraint the code cannot show.
- Test functions must be named with `test_` prefix
- Do not use `#[cfg(test)]` outside of tests. Test-only functions
  belong in the test module. Items that stay outside tests but are
  used only by tests use `#[cfg_attr(not(test), expect(dead_code))]`
- Submodules are allowed only in `mod.rs`
- No code in `mod.rs`, only `mod` declarations
- No module may contain both code and submodules. A module is either
  a `mod.rs` that only declares submodules, or a single file of code
  with no `mod` declarations (inline `#[cfg(test)] mod tests` is
  allowed). Do not write `mod foo;` in a file that also has items.
