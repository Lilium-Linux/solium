//! The settings sections a window manager configures, each read from a
//! [`Doc`](crate::script::Doc): one file per section, the same parser
//! whatever language the document was written in.

pub(crate) mod client_sizes;
pub(crate) mod files;
pub(crate) mod focus;
pub(crate) mod frames;
pub(crate) mod idle;
pub(crate) mod input;
pub(crate) mod layer_shell;
pub(crate) mod lock;
pub(crate) mod read;
pub(crate) mod reload;
pub(crate) mod run;
pub(crate) mod session;
pub(crate) mod toplevels;
pub(crate) mod x11;
