use crate::Document;

#[derive(Debug, Clone)]
pub struct IncrementalDocument {
    /// The raw data for the files read from input.
    bytes_documents: Vec<u8>,

    /// The combined result of `bytes_documents`.
    /// Do not edit this document as it will not be saved.
    prev_documents: Document,

    /// A new document appended to the previously loaded file.
    pub new_document: Document,
}

impl IncrementalDocument {
    /// Create new PDF document.
    pub fn new() -> Self {
        Self {
            bytes_documents: Vec::new(),
            prev_documents: Document::new(),
            new_document: Document::new(),
        }
    }

    /// Create new `IncrementalDocument` from the bytes and document.
    ///
    /// The function expects the bytes and previous document to match.
    /// If they do not match exactly this might result in broken PDFs.
    pub fn create_from(prev_bytes: Vec<u8>, prev_documents: Document) -> Self {
        Self {
            bytes_documents: prev_bytes,
            new_document: Document::new_from_prev(&prev_documents),
            prev_documents,
        }
    }

    /// Get the structure of the previous documents (all prev incremental updates combined.)
    pub fn get_prev_documents(&self) -> &Document {
        &self.prev_documents
    }

    /// Get the bytes of the previous documents.
    pub fn get_prev_documents_bytes(&self) -> &[u8] {
        &self.bytes_documents
    }
}

impl Default for IncrementalDocument {
    fn default() -> Self {
        Self::new()
    }
}
