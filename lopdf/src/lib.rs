#[macro_use]
mod object;
pub use crate::object::{Dictionary, Object, ObjectId, Stream, StringFormat};

mod common_data_structures;
mod document;
mod incremental_document;
mod object_stream;
pub use object_stream::ObjectStream;
pub mod xref;
pub use crate::common_data_structures::{decode_text_string, text_string};
pub use crate::document::Document;
pub use crate::encodings::{encode_utf16_be, encode_utf8};
pub use crate::incremental_document::IncrementalDocument;

mod bookmarks;
pub use crate::bookmarks::Bookmark;
#[path = "nom_cmap_parser.rs"]
mod cmap_parser;
mod cmap_section;
pub mod content;
mod encodings;
pub use encodings::Encoding;
pub mod encryption;
mod error;
pub use error::XrefError;
pub mod filters;
#[path = "nom_parser.rs"]
mod parser;
mod parser_aux;
mod processor;
mod reader;
pub use reader::Reader;
mod rc4;
pub mod xobject;

pub use error::{Error, Result};
