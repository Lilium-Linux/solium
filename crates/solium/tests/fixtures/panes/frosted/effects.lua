-- The `frosted` fixture's rules: each applies to this style's panes only.
-- Read in an effect's sandbox (math, table and string; no `sol`).
return {
    { match = "*", part = "region:titlebar", slot = "behind", effect = "tint" },
}
