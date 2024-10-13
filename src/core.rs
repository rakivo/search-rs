use std::str;
use std::slice;
use std::fmt::Debug;
use std::borrow::Cow;
use std::sync::mpsc::Sender;
use std::path::{Path, PathBuf};
use std::collections::BTreeMap;
#[cfg(unix)] use std::os::unix::fs::MetadataExt;
use std::fs::{File, metadata, read_to_string};
use std::io::{BufReader, Result as IoResult, Error as IoError, ErrorKind as IoErrorKind};

use rayon::prelude::*;
use tl::ParserOptions;
use hashbrown::HashMap;
use lopdf::{Document, Object};
use foldhash::fast::RandomState;
use xml::reader::{EventReader, XmlEvent};

use crate::term::Signal;
use crate::dir_rec::DirRec;
use crate::snowball::{SnowballEnv, algorithms::english_stemmer::stem};

const GIG: u64 = 1024 * 1024 * 1024;

const SPLIT_CHARACTERS: &[char] = &[' ', ',', '.', ';'];

const IGNORE: &[&str] = &["Length", "BBox", "FormType", "Matrix", "Type", "XObject", "Subtype", "Filter", "ColorSpace", "Width", "Height", "BitsPerComponent", "Length1", "Length2", "Length3", "PTEX.FileName", "PTEX.PageNumber", "PTEX.InfoDict", "FontDescriptor", "ExtGState", "MediaBox", "Annot",];

type Contents = Vec::<(PathBuf, String)>;
type DocFreq<'a> = HashMap<&'a str, usize>;
type TermFreq<'a> = HashMap<&'a str, usize>;
type Ranks<'a> = Vec::<(&'a PathBuf, f32)>;

macro_rules! am {
    ($($tt: tt) *) => { std::sync::Arc::new(std::sync::Mutex::new($($tt) *)) }
}

#[inline]
fn read_file<P>(file_path: P) -> IoResult::<BufReader::<File>>
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
    Document::load_filtered(path, filter_func)
        .map_err(|e| IoError::new(IoErrorKind::Other, e.to_string()))
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
        }).for_each(|page: IoResult::<_>| {
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

        let string = text.text.iter()
            .map(|(_, text)| text.join(" "))
            .collect();

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
        read_to_string(file_path)
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

    let md = metadata(file_path)?;

    #[cfg(unix)]
    if md.mode() & 0o111 != 0 {
        return Err(IoError::new(IoErrorKind::Unsupported, "not parsing binary files"))
    }

    if md.len() >= GIG {
        return Err(IoError::new(IoErrorKind::InvalidData, "file is too big"))
    }

    match ext {
        "pdf" => Pdf::parse(file_path),
        "html" => Html::parse(file_path),
        "xml" | "xhtml" => Xml::parse(file_path),
          "txt"      | "css"     | "js"       | "json"    | "rs"       | "py"
        | "rb"       | "java"    | "c"        | "cpp"     | "go"       | "sh"
        | "md"       | "yaml"    | "ini"      | "sql"     | "csv"      | "log"
        | "makefile" | "bat"     | "php"      | "pl"      | "asm"      | "dockerfile"
        | "erb"      | "proto"   | "tf"       | "tfvars"  | "toml"     | "v"
        | "rspec"    | "ml"      | "dart"     | "lua"     | "coffee"   | "scss"
        | "less"     | "svg"     | "acl"      | "patch"   | "diff"     | "zsh"
        | "r"        | "groovy"  | "h"        | "hpp"     | "c++"      | "nasm"
        | "wxml"     | "wxs"     | "cfg"      | "zig"     | "env"      | "d"
        | "f90"      | "f"       | "jl"       | "cabal"   | "hs"       | "nim"
        | "sol"      | "swift"   | "mxml"     | "clj"     | "cljs"     | "lisp"
        | "el"       | "sml"     | "styl"     | "nut"     | "wsgi"     | "raku"
        | "q"        | "sage"    | "pike"     | "xqy"     | "slim"     | "hx"
        | "pmd"      | "gsql"    | "cs"       | "ts"      | "gitignore"| "in" => Txt::parse(file_path),
        _ => Err(IoError::new(IoErrorKind::InvalidData, "unknown extension"))
    }
}

pub struct Doc<'a> {
    tf: TermFreq<'a>,
    count: usize
}

pub type Docs<'a> = HashMap::<&'a PathBuf, Doc<'a>>;

#[inline]
unsafe fn str_to_lower<'a>(s: &'a str) -> &'a str {
    let bytes = slice::from_raw_parts_mut(s.as_ptr() as *mut _, s.len());

    bytes.iter_mut()
        .filter(|byte| **byte >= b'A' && **byte <= b'Z')
        .for_each(|byte| *byte += 32);

    str::from_utf8_unchecked(bytes)
}

#[inline]
pub fn string_to_str(string: String) -> &'static str {
    Box::leak(string.into_boxed_str())
}

// trim, stem and lowercase word avoiding copying
#[inline]
fn prepare_word<'a>(word: &'a str) -> Option::<&'a str> {
    let word = word.trim_matches(|c: char| !c.is_alphanumeric());
    if word.is_empty() || word.len() > 64 { return None }
    let word = unsafe { str_to_lower(word) };
    let mut env = SnowballEnv::create(word);
    stem(&mut env);
    let word = match env.get_current() {
        Cow::Owned(ow) => string_to_str(ow),
        Cow::Borrowed(bw) => bw,
    };
    Some(word)
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
    // how many already indexed
    count: usize,
    milestones_tx: Sender::<Signal>,
    milestones: Vec::<(usize, Signal)>,

    pub df: DocFreq<'a>,
    pub docs: Docs<'a>
}

impl<'a> Model<'a> {
    #[inline]
    fn calculate_milestones(docs_count: usize) -> Vec::<(usize, Signal)> {
        (5..=100).step_by(5).map(|count| ((docs_count * count) / 100, count as _)).collect()
    }

    pub fn new(milestones_tx: Sender::<Signal>, docs_count: usize) -> Self {
        Model {
            count: 0,
            milestones_tx,
            milestones: Self::calculate_milestones(docs_count),
            docs: Docs::with_capacity_and_hasher(docs_count, RandomState::default()),
            df: HashMap::with_capacity_and_hasher(docs_count * 128, RandomState::default())
        }
    }

    pub fn search(&self, query: &str) -> Ranks {
        let tokens = query.split(SPLIT_CHARACTERS)
            .filter_map(prepare_word)
            .collect::<Vec<_>>();

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

        ranks.par_sort_unstable_by(|a, b| unsafe { b.1.partial_cmp(&a.1).unwrap_unchecked() });
        ranks
    }

    fn print_progress(&self) {
        self.milestones.iter().for_each(|(count, percentage)| {
            if self.count.eq(count) {
                self.milestones_tx.send(*percentage).unwrap();
                return
            }
        })
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

        self.count += 1;
        self.print_progress();
        self.docs.insert(file_path, doc);
    }

    #[inline]
    pub fn add_contents(&mut self, contents: &'a Contents) {
        let have_big_files = contents.iter()
            .take(contents.len() / 2)
            .any(|content| content.1.len() >= GIG as _);

        if have_big_files {
            let zelf = am!(self);
            contents.par_iter().for_each(|(file_path, content)| {
                let mut zelf = unsafe { zelf.lock().unwrap_unchecked() };
                zelf.add_document(file_path, content);
            });
            zelf.lock().unwrap().milestones_tx.send(0).unwrap();
        } else {
            contents.iter().for_each(|(file_path, content)| {
                self.add_document(file_path, content);
            });
            self.milestones_tx.send(0).unwrap();
        }
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
        .filter_map(|e| {
            parse(&e).ok().map(|r| (e, r))
        }).collect()
}
