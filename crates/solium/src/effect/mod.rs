//! Effects as user folders: the host, the sandbox, the programs, the
//! executor, the rules and the transitions. \[16\] §2; the spec §6.
//!
//! Singular, apart from the `solium_effects` crate, which holds what is data
//! and text; this module holds Lua, GL and everything per frame (Ruling 2).

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "nothing loads an effect until Task 4")
)]
pub(crate) mod host;
#[cfg_attr(not(test), expect(dead_code, reason = "Task 4 loads effects"))]
pub(crate) mod sandbox;
