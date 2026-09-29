//! The one page a summarized source renders to: what its mirror holds,
//! counted and broken down, with the dates it spans. The file-tree and
//! photo-library sources render this way (fsindex, media, lightroom,
//! apple_photos), because a file or a photo is not a document anyone
//! reads on its own. A provider reads its raw store into a [`Summary`];
//! this crate writes the page and its one grid row.

pub mod dates;
pub mod page;
pub mod read;
pub mod table;

pub use page::{render_page, Profile, Summary};
pub use table::{Breakdown, Order, Tally};
