use log::warn;

use crate::{
    content::{Content, Operation},
    document::Document,
    encodings::Encoding,
    error::XrefError,
    parser::ParserInput,
    xref::{Xref, XrefEntry, XrefType},
    Error, Result,
};
use crate::{parser, Dictionary, Object, ObjectId, Stream};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};

impl Content<Vec<Operation>> {
    /// Decode content operations.
    pub fn decode(data: &[u8]) -> Result<Self> {
        parser::content(ParserInput::new_extra(data, "content operations")).ok_or(Error::ContentDecode)
    }
}

impl Stream {
    /// Decode content after decoding all stream filters.
    pub fn decode_content(&self) -> Result<Content<Vec<Operation>>> {
        Content::decode(&self.content)
    }
}

impl Document {
    /// Get decoded page content;
    pub fn get_and_decode_page_content(&self, page_id: ObjectId) -> Result<Content<Vec<Operation>>> {
        let content_data = self.get_page_content(page_id)?;
        Content::decode(&content_data)
    }
    pub fn extract_text(&self, page_numbers: &[u32]) -> Result<String> {
        fn collect_text(text: &mut String, encoding: &Encoding, operands: &[Object]) -> Result<()> {
            for operand in operands.iter() {
                match *operand {
                    Object::String(ref bytes, _) => {
                        text.push_str(&Document::decode_text(encoding, bytes)?);
                    }
                    Object::Array(ref arr) => {
                        collect_text(text, encoding, arr)?;
                        text.push(' ');
                    }
                    Object::Integer(i) => {
                        if i < -100 {
                            text.push(' ');
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        let mut text = String::new();
        let pages = self.get_pages();
        for page_number in page_numbers {
            let page_id = *pages.get(page_number).ok_or(Error::PageNumberNotFound(*page_number))?;
            let fonts = self.get_page_fonts(page_id)?;
            let encodings: BTreeMap<Vec<u8>, Encoding> = fonts
                .into_iter()
                .map(|(name, font)| font.get_font_encoding(self).map(|it| (name, it)))
                .collect::<Result<BTreeMap<Vec<u8>, Encoding>>>()?;
            let content_data = self.get_page_content(page_id)?;
            let content = Content::decode(&content_data)?;
            let mut current_encoding = None;
            for operation in &content.operations {
                match operation.operator.as_ref() {
                    "Tf" => {
                        let current_font = operation
                            .operands
                            .first()
                            .ok_or_else(|| Error::Syntax("missing font operand".to_string()))?
                            .as_name()?;
                        current_encoding = encodings.get(current_font);
                    }
                    "Tj" | "TJ" => match current_encoding {
                        Some(encoding) => collect_text(&mut text, encoding, &operation.operands)?,
                        None => warn!("Could not decode extracted text"),
                    },
                    "ET" => {
                        if !text.ends_with('\n') {
                            text.push('\n')
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(text)
    }
}

/// Decode CrossReferenceStream
pub fn decode_xref_stream(mut stream: Stream) -> Result<(Xref, Dictionary)> {
    if stream.is_compressed() {
        stream.decompress()?;
    }
    let mut dict = stream.dict;
    let mut reader = Cursor::new(stream.content);
    let size = dict
        .get(b"Size")
        .and_then(Object::as_i64)
        .map_err(|_| Error::Xref(XrefError::Parse))?;
    let mut xref = Xref::new(size as u32, XrefType::CrossReferenceStream);
    {
        let section_indice = dict
            .get(b"Index")
            .and_then(parse_integer_array)
            .unwrap_or_else(|_| vec![0, size]);
        let field_widths = dict
            .get(b"W")
            .and_then(parse_integer_array)
            .map_err(|_| Error::Xref(XrefError::Parse))?;

        if field_widths.len() < 3
            || field_widths[0].is_negative()
            || field_widths[1].is_negative()
            || field_widths[2].is_negative()
        {
            return Err(Error::Xref(XrefError::Parse));
        }

        let mut bytes1 = vec![0_u8; field_widths[0] as usize];
        let mut bytes2 = vec![0_u8; field_widths[1] as usize];
        let mut bytes3 = vec![0_u8; field_widths[2] as usize];

        for i in 0..section_indice.len() / 2 {
            let start = section_indice[2 * i];
            let count = section_indice[2 * i + 1];

            for j in 0..count {
                let entry_type = if !bytes1.is_empty() {
                    read_big_endian_integer(&mut reader, bytes1.as_mut_slice())?
                } else {
                    1
                };
                match entry_type {
                    0 => {
                        // free object
                        read_big_endian_integer(&mut reader, bytes2.as_mut_slice())?;
                        read_big_endian_integer(&mut reader, bytes3.as_mut_slice())?;
                    }
                    1 => {
                        // normal object
                        let offset = read_big_endian_integer(&mut reader, bytes2.as_mut_slice())?;
                        let generation = if !bytes3.is_empty() {
                            read_big_endian_integer(&mut reader, bytes3.as_mut_slice())?
                        } else {
                            0
                        } as u16;
                        xref.insert((start + j) as u32, XrefEntry::Normal { offset, generation });
                    }
                    2 => {
                        // compressed object
                        let container = read_big_endian_integer(&mut reader, bytes2.as_mut_slice())?;
                        let index = read_big_endian_integer(&mut reader, bytes3.as_mut_slice())? as u16;
                        xref.insert((start + j) as u32, XrefEntry::Compressed { container, index });
                    }
                    _ => {}
                }
            }
        }
    }
    dict.remove(b"Length");
    dict.remove(b"W");
    dict.remove(b"Index");
    Ok((xref, dict))
}

fn read_big_endian_integer(reader: &mut Cursor<Vec<u8>>, buffer: &mut [u8]) -> Result<u32> {
    reader.read_exact(buffer)?;
    let mut value = 0;
    for &mut byte in buffer {
        value = (value << 8) + u32::from(byte);
    }
    Ok(value)
}

fn parse_integer_array(array: &Object) -> Result<Vec<i64>> {
    let array = array.as_array()?;
    let mut out = Vec::with_capacity(array.len());

    for n in array {
        out.push(n.as_i64()?);
    }

    Ok(out)
}
