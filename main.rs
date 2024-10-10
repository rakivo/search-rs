use std::str;
use std::env;
use std::slice;
use std::fmt::Debug;
#[cfg(feature = "dbg")]
use std::time::Instant;
use std::process::ExitCode;
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

#[inline]
fn read_file<P>(file_path: P) -> IoResult::<BufReader<File>>
where
    P: AsRef::<Path> + Debug
{
    let file = File::open(&file_path).map_err(|err| {
        eprintln!("could not read {file_path:?}: {err}"); err
    })?;

    Ok(BufReader::new(file))
}

struct PdfText {
    // key is page number
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
    use std::sync::{Arc, Mutex};

    let mut pdf_text = PdfText {
        text: BTreeMap::new(),
        errors: Vec::new(),
    };

    let pdf_text_am = Arc::new(Mutex::new(&mut pdf_text));

    doc.get_pages()
        .into_par_iter()
        .map(|(npage, page_id)| {
            let text = doc.extract_text(&[npage]).map_err(|e| {
                IoError::new(IoErrorKind::Other,
                             format!("could not to extract text from page {npage} id={page_id:?}: {e:}"))
            })?;

            Ok((npage,
                text.split('\n')
                .map(|s| prepare_word(s).to_string())
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
            let err = IoError::new(IoErrorKind::InvalidData, "doc is encrypted");
            return Err(err)
        }

        let text = get_pdf_text(&doc)?;
        if !text.errors.is_empty() {
            let err = IoError::new(IoErrorKind::InvalidData, "could not parse document as pdf");
            return Err(err)
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
            let err = IoError::new(IoErrorKind::InvalidData, "could not parse html");
            return Err(err)
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
    count: usize,
}

type Docs<'a> = HashMap::<&'a PathBuf, Doc<'a>>;

// trim and lowercase all the words but without copying
#[inline]
fn prepare_word<'a>(word: &'a str) -> &'a str {
    let word = word.trim_matches(|c: char| !c.is_alphanumeric());
    unsafe {
        let bytes = slice::from_raw_parts_mut(word.as_ptr() as *mut _, word.len());

        bytes.iter_mut()
            .filter(|byte| **byte >= b'A' && **byte <= b'Z')
            .for_each(|byte| *byte += 32);

        str::from_utf8_unchecked(bytes)
    }
}

impl<'a> Doc<'a> {
    pub fn new(content: &'a str) -> Self {
        let (count, tf) = content.split(SPLIT_CHARACTERS).fold({
            (0, TermFreq::with_capacity_and_hasher(128, RandomState::default()))
        }, |(c, mut tf), word| {
            let word = prepare_word(word);
            if !word.is_empty() {
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
                Self::tf(token, doc)*self.idf(token)
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

        let (count, tf) = content.split(SPLIT_CHARACTERS)
            .filter(|s| s.chars().all(|c| c.is_alphanumeric()))
            .fold({
                (0, TermFreq::with_capacity_and_hasher(128, RandomState::default()))
            }, |(c, mut tf), t| {
                if let Some(f) = tf.get_mut(t) {
                    *f += 1;
                } else {
                    tf.insert(t, 1);
                } (c + 1, tf)
            });

        tf.keys().for_each(|t| {
            if let Some(f) = self.df.get_mut(t) {
                *f += 1;
            } else {
                self.df.insert(t, 1);
            }
        });

        self.docs.insert(file_path, Doc {count, tf});
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

fn main() -> ExitCode {
    let args = env::args().collect::<Vec::<_>>();
    if args.len() < 3 {
        eprintln!("usage: {program} <directory to search in> <term to search with>", program = args[0]);
        return ExitCode::FAILURE
    }

    #[cfg(feature = "dbg")]
    let start = Instant::now();

    let ref dir_path = args[1];
    let term = args[2..].join(" ").to_lowercase();

    let dir = DirRec::new(dir_path);
    let contents = dir.into_iter()
        .par_bridge()
        .filter_map(|e| parse(&e).ok().map(|r| (e, r)))
        .collect::<Contents>();

    let mut model = Model::new();
    model.add_contents(&contents);
    let _results = model.search(&term);

    #[cfg(feature = "dbg")] {
        let end = start.elapsed().as_millis();
        println!("indexing took: {end} millis");
    }

    return ExitCode::SUCCESS
}
