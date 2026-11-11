//! The names a finger's resize takes.

/// Every edge and corner a window is resized from by touch, by the name
/// `touch-action`'s `resize` and a touch event's `edges` use.
pub const RESIZE_EDGES: [&str; 8] = [
    "top",
    "bottom",
    "left",
    "right",
    "top_left",
    "top_right",
    "bottom_left",
    "bottom_right",
];
