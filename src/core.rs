use std::str;
use std::slice;
use std::fmt::Debug;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::collections::{VecDeque, BTreeMap};
use std::fs::{File, read_dir, read_to_string};
use std::io::{BufReader, Result as IoResult, Error as IoError, ErrorKind as IoErrorKind};

use rayon::prelude::*;
use tl::ParserOptions;
use hashbrown::HashMap;
use lopdf::{Document, Object};
use foldhash::fast::RandomState;
use xml::reader::{EventReader, XmlEvent};
use crate::snowball::{SnowballEnv, algorithms::english_stemmer::stem};

const SPLIT_CHARACTERS: &[char] = &[' ', ',', '.'];

const IGNORE: &[&str] = &["Length", "BBox", "FormType", "Matrix", "Type", "XObject", "Subtype", "Filter", "ColorSpace", "Width", "Height", "BitsPerComponent", "Length1", "Length2", "Length3", "PTEX.FileName", "PTEX.PageNumber", "PTEX.InfoDict", "FontDescriptor", "ExtGState", "MediaBox", "Annot",];

type Contents = Vec::<(PathBuf, String)>;
type DocFreq<'a> = HashMap<&'a str, usize>;
type TermFreq<'a> = HashMap<&'a str, usize>;
type Ranks<'a> = Vec::<(&'a PathBuf, f32)>;

pub struct DirRec {
    stack: VecDeque::<PathBuf>,
}

impl DirRec {
    pub fn new<P>(root: P) -> DirRec
    where
        P: Into::<PathBuf>
    {
        let mut stack = VecDeque::new();
        stack.push_back(root.into());
        DirRec {stack}
    }
}

impl Iterator for DirRec {
    type Item = PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(p) = self.stack.pop_front() {
            if p.is_file() { return Some(p) }

            match read_dir(&p) {
                Ok(es) => es.filter_map(Result::ok).for_each(|e| {
                    self.stack.push_back(e.path())
                }),
                Err(e) => eprintln!("ERROR: {e}")
            }
        } None
    }
}

macro_rules! am {
    ($($tt: tt) *) => { std::sync::Arc::new(std::sync::Mutex::new($($tt) *)) }
}

#[inline]
fn read_file<P>(file_path: P) -> IoResult::<BufReader<File>>
where
    P: AsRef::<Path> + Debug
{
    let file = File::open(&file_path).inspect_err(|err| {
        eprintln!("could not read {file_path:?}: {err}")
    })?;

    Ok(BufReader::new(file))
}

struct PdfText {
    text: BTreeMap<u32, Vec::<String>>,
    errors: Vec::<String>
}

fn filter_func(object_id: (u32, u16), object: &mut Object) -> Option::<((u32, u16), Object)> {
    if IGNORE.contains(&object.type_name().unwrap_or_default()) {
        return None;
    }

    if let Ok(d) = object.as_dict_mut() {
        d.remove(b"Producer");
        d.remove(b"ModDate");
        d.remove(b"Creator");
        d.remove(b"ProcSet");
        d.remove(b"Procset");
        d.remove(b"XObject");
        d.remove(b"MediaBox");
        d.remove(b"Annots");
        if d.is_empty() {
            return None;
        }
    }

    Some((object_id, object.to_owned()))
}

#[inline(always)]
fn load_pdf<P>(path: P) -> Result::<Document, IoError>
where
    P: AsRef::<Path>
{
    Document::load_filtered(path, filter_func).map_err(|e| IoError::new(IoErrorKind::Other, e.to_string()))
}

fn get_pdf_text(doc: &Document) -> Result::<PdfText, IoError> {
    let mut pdf_text = PdfText {
        text: BTreeMap::new(),
        errors: Vec::new(),
    };

    let pdf_text_am = am!(&mut pdf_text);

    doc.get_pages()
        .into_par_iter()
        .map(|(npage, page_id)| {
            let text = doc.extract_text(&[npage]).map_err(|e| {
                IoError::new(IoErrorKind::Other,
                             format!("could not to extract text from page {npage} id={page_id:?}: {e:}"))
            })?;

            Ok((npage,
                text.split('\n')
                .map(|s| s.to_lowercase())
                .collect()))

        }).for_each(|page: Result::<_, IoError>| {
            let mut pdf_text = unsafe { pdf_text_am.lock().unwrap_unchecked() };
            match page {
                Ok((npage, lines)) => { pdf_text.text.insert(npage, lines); },
                Err(e) => pdf_text.errors.push(e.to_string()),
            }
        });

    Ok(pdf_text)
}

trait ParseFn {
    fn parse<P>(file_path: P) -> IoResult::<String>
    where
        P: AsRef::<Path> + Debug;
}

struct Pdf;

impl ParseFn for Pdf {
    #[inline]
    fn parse<P>(file_path: P) -> IoResult::<String>
    where
        P: AsRef::<Path> + Debug
    {
        let doc = load_pdf(&file_path)?;
        if doc.is_encrypted() {
            return Err(IoError::new(IoErrorKind::InvalidData, "doc is encrypted"))
        }

        let text = get_pdf_text(&doc)?;
        if !text.errors.is_empty() {
            return Err(IoError::new(IoErrorKind::InvalidData, "could not parse document as pdf"))
        }

        let string = text.text.iter().map(|(_, text)| text.join(" ")).collect::<String>();
        Ok(string)
    }
}

struct Txt;

impl ParseFn for Txt {
    #[inline]
    fn parse<P>(file_path: P) -> IoResult::<String>
    where
        P: AsRef::<Path> + Debug
    {
        read_to_string(&file_path)
    }
}

struct Html;

impl ParseFn for Html {
    fn parse<P>(file_path: P) -> IoResult::<String>
    where
        P: AsRef::<Path> + Debug
    {
        let input = read_to_string(&file_path)?;
        let Ok(dom) = tl::parse(&input, ParserOptions::default()) else {
            return Err(IoError::new(IoErrorKind::InvalidData, "could not parse html"))
        };
        let parser = dom.parser();
        let string = dom.nodes().iter().map(|node| node.inner_text(parser)).collect();
        Ok(string)
    }
}

struct Xml;

impl ParseFn for Xml {
    fn parse<P>(file_path: P) -> IoResult::<String>
    where
        P: AsRef::<Path> + Debug
    {
        let file = read_file(&file_path)?;
        let parser = EventReader::new(file);

        let string = parser.into_iter().filter_map(|event| {
            match event {
                Ok(XmlEvent::Characters(text)) => Some(text),
                _ => None
            }
        }).collect();

        Ok(string)
    }
}

#[inline]
fn parse(file_path: &Path) -> IoResult::<String> {
    let ext = unsafe {
        file_path.extension()
            .unwrap_or_default()
            .to_str()
            .unwrap_unchecked()
    };

    match ext {
        "pdf" => Pdf::parse(file_path),
        "html" => Html::parse(file_path),
        "xml" | "xhtml" => Xml::parse(file_path),
        _ => Txt::parse(file_path),
    }
}

pub struct Doc<'a> {
    tf: TermFreq<'a>,
    count: usize
}

type Docs<'a> = HashMap::<&'a PathBuf, Doc<'a>>;

// trim and lowercase all the words but without copying
#[inline]
fn prepare_word<'a>(word: &'a str) -> Option::<&'a str> {
    let word = word.trim_matches(|c: char| !c.is_alphanumeric());
    if word.is_empty() || word.len() > 64 { return None }
    let lowered = unsafe {
        let bytes = slice::from_raw_parts_mut(word.as_ptr() as *mut _, word.len());

        bytes.iter_mut()
            .filter(|byte| **byte >= b'A' && **byte <= b'Z')
            .for_each(|byte| *byte += 32);

        str::from_utf8_unchecked(bytes)
    };

    let mut env = SnowballEnv::create(lowered);
    stem(&mut env);
    
    let ret = match env.get_current() {
        Cow::Borrowed(bw) => bw,
        Cow::Owned(ow) => Box::leak(ow.into_boxed_str())
    };

    Some(ret)
}

impl<'a> Doc<'a> {
    pub fn new(content: &'a str) -> Self {
        let (count, tf) = content.split(SPLIT_CHARACTERS).fold({
            (0, TermFreq::with_capacity_and_hasher(128, RandomState::default()))
        }, |(c, mut tf), word| {
            if let Some(word) = prepare_word(word) {
                *tf.entry(word).or_insert(0) += 1;
                (c + 1, tf)
            } else {
                (c, tf)
            }
        });

        Doc { tf, count }
    }
}

pub struct Model<'a> {
    pub docs: Docs<'a>,
    pub df: DocFreq<'a>
}

impl<'a> Model<'a> {
    pub fn new() -> Self {
        Model {
            docs: HashMap::with_capacity_and_hasher(32, RandomState::default()),
            df: HashMap::with_capacity_and_hasher(128, RandomState::default())
        }
    }

    pub fn search(&self, query: &str) -> Ranks {
        let tokens = query.split(SPLIT_CHARACTERS).collect::<Vec<_>>();

        let mut ranks = self.docs.par_iter().filter_map(|(path, doc)| {
            let rank = tokens.iter().map(|token| {
                Self::tf(token, doc) * self.idf(token)
            }).sum::<f32>();

            if !rank.is_nan() {
                Some((*path, rank))
            } else {
                None
            }
        }).collect::<Vec::<_>>();

        ranks.sort_unstable_by(|a, b| unsafe { b.1.partial_cmp(&a.1).unwrap_unchecked() });
        ranks
    }

    pub fn add_document(&mut self, file_path: &'a PathBuf, content: &'a str) {
        self.rm_document(&file_path);

        let doc = Doc::new(content);

        doc.tf.keys().for_each(|t| {
            if let Some(f) = self.df.get_mut(t) {
                *f += 1;
            } else {
                self.df.insert(t, 1);
            }
        });

        self.docs.insert(file_path, doc);
    }

    #[inline]
    pub fn add_contents(&mut self, contents: &'a Contents) {
        contents.iter().for_each(|(file_path, content)| {
            self.add_document(file_path, content);
        });
    }

    #[inline]
    fn rm_document(&mut self, file_path: &PathBuf) {
        if let Some(doc) = self.docs.remove(file_path) {
            doc.tf.keys().for_each(|t| {
                self.df.entry(t).and_modify(|f| *f -= 1);
            });
        }
    }

    #[inline(always)]
    fn tf(t: &str, doc: &Doc) -> f32 {
        *doc.tf.get(t).unwrap_or(&0) as f32 / doc.count as f32
    }

    #[inline(always)]
    fn idf(&self, term: &str) -> f32 {
        (self.docs.len() as f32 / *self.df.get(term).unwrap_or(&0) as f32).log10()
    }
}

#[inline]
pub fn dir_get_contents(dir_path: &str) -> Contents {
    let dir = DirRec::new(dir_path);
    dir.into_iter()
        .par_bridge()
        .filter_map(|e| parse(&e).ok().map(|r| (e, r)))
        .collect()
}
