//! The window manager's stores (#473), as the compositor reads them when it
//! asks: the files are the compositor's, in its state directory.

use std::cell::RefCell;

use crate::files::StoreFile;

/// The store files read for the window manager since it was last looked at,
/// and what a check would say of them.
#[derive(Debug, Default)]
pub(crate) struct StoreReads {
    /// A check changes nothing of the user's: a file that cannot be used is
    /// said, and left where it is.
    checking: bool,
    /// Each store read, by name, as the JSON text saved: already saved, so
    /// not written back unchanged.
    read: RefCell<Vec<(String, String)>>,
    /// What a check says of the store files it left as they were.
    notes: RefCell<Vec<String>>,
}

impl StoreReads {
    /// For `solium --check`.
    pub(crate) fn checking() -> Self {
        Self {
            checking: true,
            ..Self::default()
        }
    }

    /// Store `name`'s saved document as JSON text, read at most `max` bytes
    /// of its file (`files.store_max`), or `None`. One that cannot be used
    /// is moved aside to `<name>.json.broken` so the next save does not
    /// overwrite it unseen; under a check it is only said.
    /// `check::tests::a_check_leaves_a_broken_store_file_as_it_was`.
    pub(crate) fn read(&self, name: &str, max: u64) -> Option<String> {
        if !crate::files::store_name_ok(name) {
            return None;
        }
        match crate::files::read_store(name, max) {
            StoreFile::Missing => {
                tracing::info!(
                    store = name,
                    "no store file yet; starting from the defaults"
                );
                None
            }
            StoreFile::Found(loaded) => {
                let text = loaded.render();
                self.read.borrow_mut().push((name.to_owned(), text.clone()));
                Some(text)
            }
            StoreFile::Broken { path, why } => {
                tracing::warn!(store = name, path = %path.display(), why, "the store file cannot be used; starting from the defaults");
                if self.checking {
                    self.notes.borrow_mut().push(format!(
                        "store `{name}`: {} is {why}; a session would start from the \
                         defaults and move it to {name}.json.broken",
                        path.display()
                    ));
                } else {
                    crate::files::set_aside(name, &path);
                }
                None
            }
        }
    }

    /// The stores read since the last call, as the commands that tell the
    /// compositor's writer they are saved already.
    pub(crate) fn take_read(&self) -> Vec<crate::script::Command> {
        self.read
            .take()
            .into_iter()
            .map(|(name, text)| crate::script::Command::StoreLoaded { name, text })
            .collect()
    }

    /// What a check says of the store files it read.
    pub(crate) fn notes(&self) -> Vec<String> {
        self.notes.borrow().clone()
    }
}
