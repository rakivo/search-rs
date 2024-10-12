use crate::*;
use crate::{Dictionary, Stream};

#[derive(Debug, Clone)]
pub struct PdfImage<'a> {
    pub id: ObjectId,
    pub width: i64,
    pub height: i64,
    pub color_space: Option<String>,
    pub filters: Option<Vec<String>>,
    pub bits_per_component: Option<i64>,
    /// Image Data
    pub content: &'a [u8],
    /// Origin Stream Dictionary
    pub origin_dict: &'a Dictionary,
}

pub fn form(boundingbox: Vec<f32>, matrix: Vec<f32>, content: Vec<u8>) -> Stream {
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"XObject".to_vec()));
    dict.set("Subtype", Object::Name(b"Form".to_vec()));
    dict.set(
        "BBox",
        Object::Array(boundingbox.into_iter().map(Object::Real).collect()),
    );
    dict.set("Matrix", Object::Array(matrix.into_iter().map(Object::Real).collect()));
    let mut xobject = Stream::new(dict, content);
    // Ignore any compression error.
    let _ = xobject.compress();
    xobject
}

