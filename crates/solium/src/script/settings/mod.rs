//! The settings sections a window manager configures, each read from a
//! [`Doc`](crate::script::Doc): one file per section, the same parser
//! whatever language the document was written in.

pub(crate) mod focus;
pub(crate) mod frames;
pub(crate) mod idle;
pub(crate) mod input;
pub(crate) mod lock;
pub(crate) mod read;
