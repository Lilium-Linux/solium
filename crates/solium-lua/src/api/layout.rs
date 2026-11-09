//! `sol.layout`: the layout engines.

use mlua::{Lua, Table};

use crate::*;

/// Add this area's functions to `sol`.
pub(crate) fn register(lua: &Lua, sol: &Table) -> mlua::Result<()> {
    // The standard arrangements, from the same crate the preview calls. A
    // script picks which and when — and may compute its own rectangles instead,
    // which is why these are offered rather than imposed.
    sol.set("layout", layouts(lua)?)?;

    Ok(())
}
