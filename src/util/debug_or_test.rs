/// True in debug builds and in test builds (including `cargo test --release`).
pub(crate) const DEBUG_OR_TEST: bool = cfg!(debug_assertions) || cfg!(test);
