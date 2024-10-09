#![feature(seek_stream_len)]

use std::fmt::Debug;
#[cfg(feature = "dbg")]
use std::time::Instant;
use std::fs::{read_dir, File};
use std::path::{Path, PathBuf};
use std::collections::VecDeque;
use std::io::{BufReader, Read, Result as IoResult, Seek};

use rayon::prelude::*;
use hashbrown::HashMap;
use foldhash::fast::RandomState;
use xml::reader::{EventReader, XmlEvent};

type Contents = Vec::<(PathBuf, String)>;
type TfIdf<'a> = HashMap::<&'a str, f32>;
type Index<'a> = HashMap::<&'a str, usize>;
type Indexes<'a> = HashMap::<&'a PathBuf, Index<'a>>;
type Rank<'a> = Vec::<(&'a str, f32)>;
type PathRanks<'a> = Vec::<(&'a PathBuf, f32)>;
type Ranks<'a> = HashMap::<&'a PathBuf, Rank<'a>>;

#[derive(Debug)]
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

trait ParseFn {
    fn parse<P>(file_path: P) -> IoResult::<String>
    where
        P: AsRef::<Path> + Debug;
}

struct Txt;

impl ParseFn for Txt {
    fn parse<P>(file_path: P) -> IoResult::<String>
    where
        P: AsRef::<Path> + Debug
    {
        let mut b = read_file(&file_path)?;
        let stream_len = b.stream_len().unwrap_or_default();
        let mut s = String::with_capacity(stream_len as _);
        b.read_to_string(&mut s)?;
        Ok(s)
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
        "xml" | "xhtml" => Xml::parse(file_path),
        _ => Txt::parse(file_path),
    }
}

#[inline]
fn index_content<'a>(content: &'a str) -> Index<'a> {
    content.split_whitespace()
        .filter(|s| s.chars().all(|c| c.is_alphabetic()))
        .fold(Index::with_capacity_and_hasher(128, RandomState::default()),
              |mut map, word| {
                  let word = word.trim_matches(|c: char| !c.is_alphanumeric());
                  *map.entry(word).or_insert(0) += 1;
                  map
              })
}

#[inline(always)]
fn tf(t: &str, d: &Index) -> f32 {
    *d.get(t).unwrap_or(&0) as f32 / d.par_iter().map(|(_, f)| f).sum::<usize>() as f32
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
            (*term, tf * idf)
        }).collect::<TfIdf>();

        (*path, tf_idf)
    }).map(|(path, tf_idf)| {
        let mut stats = tf_idf.iter().collect::<Vec::<_>>();
        stats.par_sort_unstable_by(|(_, a), (_, b)| unsafe {
            b.partial_cmp(a).unwrap_unchecked()
        });

        let top100 = stats.into_iter().take(100).map(|(s, f)| (*s, *f)).collect();
        (path, top100)
    }).collect()
}

fn rank_documents_by_term<'a>(terms: &str, ranks: &'a Ranks) -> PathRanks<'a> {
   let search_terms: Vec<String> = terms
        .split(&[' ', ':', ',', '.'])
        .map(|term| term.to_lowercase())
        .collect();

    let mut doc_ranks = ranks
        .par_iter()
        .map(|(path, rank)| {
            let rank = search_terms
                .iter()
                .map(|search_term| {
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

fn main() {
    #[cfg(feature = "dbg")]
    let start = Instant::now();

    let dir_path = "docs.gl";
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

    let path_ranks = rank_documents_by_term("linear interpolation", &ranks);
    print_path_ranks(&path_ranks);
}
