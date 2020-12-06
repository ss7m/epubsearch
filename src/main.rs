use argh::FromArgs;
use percent_encoding::percent_decode;
use regex::{Captures, Regex};
use std::collections::HashMap;
use std::fs::*;
use std::io::BufReader;
use std::path::PathBuf;
use xml::attribute::OwnedAttribute;
use xml::reader::{EventReader, XmlEvent};
use zip::read::*;
use zip::result::ZipError;

#[derive(Debug)]
enum EpubError {
    ZipError(ZipError),
    IoError(std::io::Error),
    EpubError(String),
}

type Result<T> = std::result::Result<T, EpubError>;

fn get_attribute(attributes: &[OwnedAttribute], name: &str) -> Option<String> {
    for attr in attributes {
        if attr.name.local_name == name {
            return Some(
                percent_decode(attr.value.as_bytes())
                    .decode_utf8_lossy()
                    .into_owned(),
            );
        }
    }
    None
}

fn is_start_element(event: &XmlEvent, element_name: &str) -> Option<Vec<OwnedAttribute>> {
    match event {
        XmlEvent::StartElement {
            name, attributes, ..
        } => {
            if name.local_name == element_name {
                Some(attributes.to_vec())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_end_element(event: &XmlEvent, element_name: &str) -> bool {
    match event {
        XmlEvent::EndElement { name, .. } => name.local_name == element_name,
        _ => false,
    }
}

fn get_content_file_name(epub: &mut ZipArchive<File>) -> Result<String> {
    let container = epub
        .by_name("META-INF/container.xml")
        .map_err(EpubError::ZipError)?;

    for e in EventReader::new(BufReader::new(container)) {
        match e {
            Ok(event) => {
                if let Some(attributes) = is_start_element(&event, "rootfile") {
                    return get_attribute(&attributes, "full-path")
                        .ok_or_else(|| EpubError::EpubError("Missing Attribute".to_string()));
                }
            }
            Err(_) => return Err(EpubError::EpubError("Invalid Xml".to_string())),
        }
    }

    Err(EpubError::EpubError(
        "Malformed META-INF/containter.xml file".to_string(),
    ))
}

fn get_spine_documents(epub: &mut ZipArchive<File>) -> Result<(String, Vec<String>)> {
    let (content_file, oebps) = {
        let content_file_name = get_content_file_name(epub)?;

        let content_file = epub
            .by_name(&content_file_name)
            .map_err(EpubError::ZipError)?;

        let mut path = PathBuf::from(content_file_name);
        path.pop();
        let oebps = match path.to_str() {
            Some("") => String::new(),
            Some(s) => s.to_string() + "/",
            None => return Err(EpubError::EpubError("non utf8 file name".to_string())),
        };

        (content_file, oebps)
    };

    let mut content_parser = EventReader::new(content_file);

    while content_parser
        .next()
        .ok()
        .and_then(|e| is_start_element(&e, "manifest"))
        .is_none()
    {}

    let mut content_ids = HashMap::new();
    loop {
        let event = match content_parser.next() {
            Ok(event) => event,
            Err(_) => return Err(EpubError::EpubError("Malformed Xml".to_string())),
        };

        if is_end_element(&event, "manifest") {
            break;
        } else if let Some(attrs) = is_start_element(&event, "item") {
            let media_type = get_attribute(&attrs, "media-type");
            let id = get_attribute(&attrs, "id");
            let href = get_attribute(&attrs, "href");

            if let (Some(media_type), Some(id), Some(href)) = (media_type, id, href) {
                if media_type == "application/xhtml+xml" || media_type == "application/x-dtbncx+xml"
                {
                    content_ids.insert(id, href);
                }
            }
        }
    }

    let toc_id = loop {
        let event = match content_parser.next() {
            Ok(event) => event,
            Err(_) => return Err(EpubError::EpubError("Malformed Epub".to_string())),
        };

        if let Some(attrs) = is_start_element(&event, "spine") {
            match get_attribute(&attrs, "toc") {
                Some(toc_id) => break toc_id,
                None => return Err(EpubError::EpubError("Malformed Epub".to_string())),
            }
        }
    };

    let toc = match content_ids.get(&toc_id) {
        Some(toc) => format!("{}{}", oebps, toc),
        None => return Err(EpubError::EpubError("Malformed Epub".to_string())),
    };

    let mut spine = Vec::new();
    loop {
        let event = match content_parser.next() {
            Ok(event) => event,
            Err(_) => return Err(EpubError::EpubError("Malformed Epub".to_string())),
        };

        if is_end_element(&event, "spine") {
            break;
        } else if let Some(attrs) = is_start_element(&event, "itemref") {
            let idref = get_attribute(&attrs, "idref");
            if let Some(href) = idref.and_then(|id| content_ids.get(&id)) {
                spine.push(format!("{}{}", oebps, href));
            }
        }
    }

    Ok((toc, spine))
}

// TODO: parse the toc to report what chapter the match is found in
//struct NavMap {
//    points: Vec<NavPoint>,
//}
//
//struct NavPoint {
//    label: String,
//    content_src: String,
//    points: Vec<NavPoint>,
//}

struct XhtmlTextIterator<'a> {
    event_reader: EventReader<ZipFile<'a>>,
}

impl<'a> XhtmlTextIterator<'a> {
    fn new(file: ZipFile<'a>) -> Self {
        XhtmlTextIterator {
            event_reader: EventReader::new(file),
        }
    }
}

impl<'a> Iterator for XhtmlTextIterator<'a> {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        loop {
            let event = match self.event_reader.next() {
                Ok(event) => event,
                Err(_) => return None,
            };

            if is_start_element(&event, "p").is_some() {
                break;
            } else if let XmlEvent::EndDocument = event {
                return None;
            }
        }

        let mut text = String::new();
        loop {
            let event = match self.event_reader.next() {
                Ok(event) => event,
                Err(_) => return None,
            };

            if is_end_element(&event, "p") {
                break;
            } else if let XmlEvent::Characters(s) = event {
                text += &s;
            }
        }

        Some(text)
    }
}

fn colorize_matches(captures: &Captures) -> String {
    captures
        .get(0)
        .map_or("".to_string(), |m| format!("\x1b[34m{}\x1b[0m", m.as_str()))
}

fn print_error() {
    print!("\x1b[31mError\x1b[0m: ");
}

#[derive(FromArgs, Debug)]
/// Search an epub for a regular expression
struct EpubArgs {
    #[argh(switch, short = 'c')]
    /// print the number of matches found
    count: bool,

    #[argh(switch, short = 'q')]
    /// produce no output
    quiet: bool,

    #[argh(switch, short = 'i')]
    /// do case insensitive search
    ignore_case: bool,

    #[argh(positional)]
    /// regular Expression
    regex: String,

    #[argh(positional)]
    /// files to search
    file_names: Vec<String>,
}

fn main() {
    let args: EpubArgs = argh::from_env();

    let mut re_string = String::new();
    if args.ignore_case {
        re_string.push_str("(?i)");
    }
    re_string.push_str(&args.regex);
    let re = Regex::new(&re_string).unwrap();

    let mut total_matches = 0;

    for file_name in args.file_names {
        let mut archive = match File::open(file_name.clone())
            .map_err(EpubError::IoError)
            .and_then(|f| ZipArchive::new(f).map_err(EpubError::ZipError))
        {
            Ok(archive) => archive,
            Err(e) => {
                print_error();
                match e {
                    EpubError::IoError(_) => println!("unable to open {}", file_name),
                    EpubError::ZipError(_) => println!("{} is not a zip archive", file_name),
                    _ => println!("you shouldn't see this message"),
                }
                continue;
            }
        };

        let (_toc, spine) = match get_spine_documents(&mut archive) {
            Ok(t) => t,
            Err(_) => {
                print_error();
                println!("{} may not be an epub file", file_name);
                continue;
            }
        };

        let mut num_matches = 0;
        for doc in spine {
            let file = archive.by_name(&doc).unwrap();
            for (idx, paragraph) in XhtmlTextIterator::new(file).enumerate() {
                let paragraph = paragraph.as_str();
                if re.is_match(paragraph) {
                    num_matches += 1;
                    if !args.count && !args.quiet {
                        println!(
                            "\x1b[32m{}({})\x1b[0m: {}",
                            file_name,
                            idx + 1,
                            re.replace_all(paragraph, colorize_matches)
                        );
                    }
                }
            }
        }

        if args.count && !args.quiet {
            println!(
                "\x1b[32m{}\x1b[0m: \x1b[34m{}\x1b[0m",
                file_name, num_matches
            );
        }
        total_matches += num_matches;
    }

    std::process::exit(total_matches);
}
