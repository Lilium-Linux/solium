//! Workspaces, as the models read them.

use std::collections::BTreeMap;

/// What a script declared: for each window, by script id, the workspaces it
/// is on.
/// `state::tests::real_client::reflow_on_close::hosted::a_window_row_carries_where_it_lives_and_its_focus`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Declared {
    pub(crate) windows: BTreeMap<u64, Vec<String>>,
}
