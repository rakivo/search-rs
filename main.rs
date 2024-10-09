use std::fmt::Debug;
use std::io::BufReader;
use std::fs::{File, read_dir};
use std::path::{Path, PathBuf};
use std::collections::VecDeque;

use rayon::prelude::*;
use hashbrown::HashMap;
use foldhash::fast::RandomState;
use xml::reader::{EventReader, XmlEvent};

type IoResult<T> = std::io::Result::<T>;

type Index<'a> = HashMap::<&'a str, usize>;
type Indexes<'a> = HashMap::<&'a PathBuf, Index<'a>>;
type TfIdf<'a> = HashMap::<&'a str, f32>;
type TfIdfs<'a> = HashMap::<&'a PathBuf, TfIdf<'a>>;
type Rank<'a> = Vec::<(&'a str, f32)>;
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

fn parse_xml<P>(file_path: P) -> IoResult::<String>
where
    P: AsRef::<Path> + Debug
{
    let file = File::open(&file_path).map_err(|err| {
        eprintln!("could not read {file_path:?}: {err}"); err
    })?;

    let file = BufReader::new(file);
    let parser = EventReader::new(file);

    let string = parser.into_iter().filter_map(|event| {
        match event {
            Ok(XmlEvent::Characters(text)) => Some(text),
            _ => None
        }
    }).collect();

    Ok(string)
}

#[inline]
fn index_content<'a>(content: &'a str) -> Index<'a> {
    content.split_whitespace()
        .filter(|s| s.chars().all(|c| c.is_alphabetic()))
        .fold(Index::with_hasher(RandomState::default()),
              |mut map, word| {
                  *map.entry(word.trim()).or_insert(0) += 1;
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

#[inline]
fn print_ranks(ranks: &Ranks) {
    for (path, ranks) in ranks {
        println!("{path:?}:");
        for (term, tf_idf) in ranks {
            println!("    {term} => {tf_idf}");
        }
    }
}

fn main() -> std::io::Result::<()> {
    let dir_path = "docs.gl";
    let dir = DirRec::new(dir_path);
    let strings = dir.into_iter()
        .par_bridge()
        .filter_map(|e| parse_xml(&e).map(|r| (e, r)).ok())
        .collect::<Vec::<_>>();

    let indexes = strings.par_iter()
        .map(|(file_path, string)| (file_path, index_content(string)))
        .collect::<Indexes>();

    let tfidfs = indexes.par_iter().map(|(path, index)| {
        let tf_idf = index.par_iter().map(|(term, _)| {
            let tf = tf(term, index);
            let idf = idf(term, &indexes);
            (*term, tf * idf)
        }).collect::<TfIdf>();

        (*path, tf_idf)
    }).collect::<TfIdfs>();

    let ranks = tfidfs.par_iter().map(|(path, tf_idf)| {
        let mut stats = tf_idf.iter().collect::<Vec::<_>>();
        stats.par_sort_unstable_by(|(_, a), (_, b)| unsafe {
            b.partial_cmp(a).unwrap_unchecked()
        });

        let top10 = stats.into_iter().take(10).map(|(s, f)| (*s, *f)).collect();
        (*path, top10)
    }).collect::<Ranks>();

    // print_ranks(&ranks);

    Ok(())
}
