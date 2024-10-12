use crate::encodings::bytes_to_string;
use crate::encodings;
use crate::{
    Error, Object, Result, StringFormat,
};

/// Creates a text string.
/// If the input only contains ASCII characters, the string is encoded
/// in PDFDocEncoding, otherwise in UTF-16BE.
pub fn text_string(text: &str) -> Object {
    if text.is_ascii() {
        return Object::String(text.into(), StringFormat::Literal);
    }
    Object::String(encodings::encode_utf16_be(text), StringFormat::Hexadecimal)
}

/// Decodes a text string.
/// Depending on the BOM at the start of the string, a different encoding is chosen.
/// All encodings specified in PDF2.0 are supported (PDFDocEncoding, UTF-16BE,
/// and UTF-8).
pub fn decode_text_string(obj: &Object) -> Result<String> {
    let s = obj.as_str()?;
    if s.starts_with(b"\xFE\xFF") {
        // Detected UTF-16BE BOM
        String::from_utf16(
            &s[2..]
                .chunks(2)
                .map(|c| {
                    if c.len() == 1 {
                        u16::from_be_bytes([c[0], 0])
                    } else {
                        u16::from_be_bytes(c.try_into().unwrap())
                    }
                })
                .collect::<Vec<u16>>(),
        )
        .map_err(|_| Error::StringDecode)
    } else if s.starts_with(b"\xEF\xBB\xBF") {
        // Detected UTF-8 BOM
        String::from_utf8(s.to_vec()).map_err(|_| Error::StringDecode)
    } else {
        // If neither BOM is detected, PDFDocEncoding is used
        Ok(bytes_to_string(&encodings::PDF_DOC_ENCODING, s))
    }
}
