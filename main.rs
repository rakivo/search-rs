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

type Contents = Vec::<(PathBuf, String)>;
type TfIdf<'a> = HashMap::<&'a str, f32>;
type Index<'a> = HashMap::<&'a str, usize>;
type Indexes<'a> = HashMap::<&'a PathBuf, Index<'a>>;
type Rank<'a> = Vec::<(&'a str, f32)>;
type PathRanks<'a> = Vec::<(&'a PathBuf, f32)>;
type Ranks<'a> = HashMap::<&'a PathBuf, Rank<'a>>;

const SPLIT_CHARACTERS: &[char] = &[' ', ',', '.'];

const IGNORE: &[&str] = &[
    "Length",
    "BBox",
    "FormType",
    "Matrix",
    "Type",
    "XObject",
    "Subtype",
    "Filter",
    "ColorSpace",
    "Width",
    "Height",
    "BitsPerComponent",
    "Length1",
    "Length2",
    "Length3",
    "PTEX.FileName",
    "PTEX.PageNumber",
    "PTEX.InfoDict",
    "FontDescriptor",
    "ExtGState",
    "MediaBox",
    "Annot",
];

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
    text: BTreeMap<u32, Vec::<String>>, // Key is page number
    errors: Vec<String>,
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

fn load_pdf<P>(path: P) -> Result::<Document, IoError>
where
    P: AsRef::<Path>
{
    Document::load_filtered(path, filter_func).map_err(|e| IoError::new(IoErrorKind::Other, e.to_string()))
}

fn get_pdf_text(doc: &Document) -> Result::<PdfText, IoError> {
    let mut pdf_text: PdfText = PdfText {
        text: BTreeMap::new(),
        errors: Vec::new(),
    };

    let pages = doc.get_pages()
        .into_par_iter()
        .map(|(page_num, page_id)| {
            let text = doc.extract_text(&[page_num]).map_err(|e| {
                IoError::new(IoErrorKind::Other,
                             format!("could not to extract text from page {page_num} id={page_id:?}: {e:}"))
            })?;

            Ok((page_num,
                text.split('\n')
                    .map(|s| s.trim_end().to_string())
                    .collect::<Vec<String>>()))
        }).collect::<Vec::<Result::<(u32, Vec::<String>), IoError>>>();

    for page in pages {
        match page {
            Ok((page_num, lines)) => {
                pdf_text.text.insert(page_num, lines);
            }
            Err(e) => {
                pdf_text.errors.push(e.to_string());
            }
        }
    }
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
            let err = std::io::Error::new(std::io::ErrorKind::InvalidData, "could not parse html");
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
    let ext = file_path.extension()
        .unwrap_or_default()
        .to_str()
        .unwrap();

    match ext {
        "pdf" => Pdf::parse(file_path),
        "html" => Html::parse(file_path),
        "xml" | "xhtml" => Xml::parse(file_path),
        _ => Txt::parse(file_path),
    }
}

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

#[inline]
fn index_content<'a>(content: &'a str) -> Index<'a> {
    content.split(SPLIT_CHARACTERS)
        .filter(|s| s.chars().all(|c| c.is_alphabetic()))
        .fold(Index::with_capacity_and_hasher(128, RandomState::default()),
              |mut map, word| {
                  *map.entry(prepare_word(word)).or_insert(0) += 1;
                  map
              })
}

#[inline(always)]
fn tf(t: &str, d: &Index) -> f32 {
    *d.get(t).unwrap_or(&1) as f32 / d.par_iter().map(|(_, f)| f).sum::<usize>() as f32
}

#[inline(always)]
fn idf(t: &str, df: &Indexes) -> f32 {
    let n = df.len() as f32;
    let m = df.par_values().filter(|tf| tf.contains_key(t)).count() as f32;
    (n / m).log10()
}

fn compute_ranks<'a>(contents: &'a Contents) -> Ranks<'a> {
    let indexes = contents.par_iter()
        .map(|(file_path, string)| (file_path, index_content(string)))
        .collect::<Indexes>();
    
    indexes.par_iter().map(|(path, index)| {
        let tf_idf = index.par_iter().map(|(term, _)| {
            let tf = tf(term, index);
            let idf = idf(term, &indexes);
            (*term, tf*idf)
        }).collect::<TfIdf>();

        (*path, tf_idf)
    }).map(|(path, tf_idf)| {
        let mut stats = tf_idf.iter().collect::<Vec::<_>>();
        stats.par_sort_unstable_by(|(_, a), (_, b)| unsafe {
            b.partial_cmp(a).unwrap_unchecked()
        });

        let top = stats.into_iter().take(50).map(|(s, f)| (*s, *f)).collect();
        (path, top)
    }).collect()
}

fn rank_documents_by_term<'a>(terms: &str, ranks: &'a Ranks) -> PathRanks<'a> {
   let search_terms = terms.split(SPLIT_CHARACTERS)
        .map(|term| term.to_lowercase())
        .collect::<Vec::<_>>();

    let mut doc_ranks = ranks
        .into_par_iter()
        .map(|(path, rank)| {
            let rank = search_terms.iter().map(|search_term| {
                rank.iter()
                    .find(|(term, _)| term.eq(search_term))
                    .map_or(0.0, |(_, rank)| *rank)
            }).sum();

            (*path, rank)
        }).collect::<PathRanks>();

    doc_ranks.par_sort_unstable_by(|(_, a), (_, b)| unsafe {
        b.partial_cmp(a).unwrap_unchecked()
    });

    doc_ranks
}

#[inline]
#[allow(unused)]
fn print_ranks(ranks: &Ranks) {
    for (path, ranks) in ranks {
        println!("{path:?}:");
        for (term, tf_idf) in ranks {
            println!("    {term} => {tf_idf}");
        }
    }
}

#[inline]
#[allow(unused)]
fn print_path_ranks(ranks: &PathRanks) {
    for (path, rank) in ranks.iter().filter(|(_, rank)| *rank != 0.0).take(5) {
        println!("{path:?} => {rank}")
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
    let term = args[2].to_lowercase();

    let dir = DirRec::new(dir_path);
    let contents = dir.into_iter()
        .par_bridge()
        .filter_map(|e| parse(&e).ok().map(|r| (e, r)))
        .collect::<Contents>();

    let ranks = compute_ranks(&contents);

    // print_ranks(&ranks);

    #[cfg(feature = "dbg")] {
        let end = start.elapsed().as_millis();
        println!("indexing took: {end} millis");
    }

    let path_ranks = rank_documents_by_term(&term, &ranks);
    print_path_ranks(&path_ranks);

    return ExitCode::SUCCESS
}
