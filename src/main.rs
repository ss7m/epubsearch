use argh::FromArgs;
use percent_encoding::percent_decode;
use regex::{Matches, Regex};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use xml::attribute::OwnedAttribute;
use xml::reader::{EventReader, XmlEvent};
use zip::read::{ZipArchive, ZipFile};
use zip::result::ZipError;

// Error struct to collect all possible error types
#[derive(Debug)]
enum EpubError {
    Zip(ZipError),
    IO(std::io::Error),
    Epub(String),
}

type Result<T> = std::result::Result<T, EpubError>;

// Look for an attribute and return it's value if found among
// the attribute list of an xml tag
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

// If event is a start element event with name element_name
// return a list of its attributes.
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

// return true if event is an end element event with name element_name
fn is_end_element(event: &XmlEvent, element_name: &str) -> bool {
    match event {
        XmlEvent::EndElement { name, .. } => name.local_name == element_name,
        _ => false,
    }
}

// Find the name of the content file of an epub file
fn get_content_file_name(epub: &mut ZipArchive<File>) -> Result<String> {
    let container = epub
        .by_name("META-INF/container.xml")
        .map_err(EpubError::Zip)?;

    for e in EventReader::new(BufReader::new(container)) {
        match e {
            Ok(event) => {
                if let Some(attributes) = is_start_element(&event, "rootfile") {
                    return get_attribute(&attributes, "full-path")
                        .ok_or_else(|| EpubError::Epub("Missing Attribute".to_string()));
                }
            }
            Err(_) => return Err(EpubError::Epub("Invalid Xml".to_string())),
        }
    }

    Err(EpubError::Epub(
        "Malformed META-INF/containter.xml file".to_string(),
    ))
}

// find the name of the toc file and a list of the xhtml documents in the spine
fn get_spine_documents(epub: &mut ZipArchive<File>) -> Result<(String, Vec<String>)> {
    // oebps is the folder containing the content_file, necessary since
    // hrefs in the content file are relative to the content file
    let (content_file, oebps) = {
        let content_file_name = get_content_file_name(epub)?;

        let content_file = epub.by_name(&content_file_name).map_err(EpubError::Zip)?;

        let mut path = PathBuf::from(content_file_name);
        path.pop();
        let oebps = match path.to_str() {
            Some("") => String::new(),
            Some(s) => s.to_string() + "/",
            None => return Err(EpubError::Epub("non utf8 file name".to_string())),
        };

        (content_file, oebps)
    };

    let mut content_parser = EventReader::new(content_file);

    // iterate to the start of the manifest section
    while content_parser
        .next()
        .ok()
        .and_then(|e| is_start_element(&e, "manifest"))
        .is_none()
    {}

    // collect the ids for all the xhtml documents
    let mut content_ids = HashMap::new();
    loop {
        let event = match content_parser.next() {
            Ok(event) => event,
            Err(_) => return Err(EpubError::Epub("Malformed Xml".to_string())),
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

    // iterate to the start of the spine, and find the id for the toc file
    let toc_id = loop {
        let event = match content_parser.next() {
            Ok(event) => event,
            Err(_) => return Err(EpubError::Epub("Malformed Epub".to_string())),
        };

        if let Some(attrs) = is_start_element(&event, "spine") {
            match get_attribute(&attrs, "toc") {
                Some(toc_id) => break toc_id,
                None => return Err(EpubError::Epub("Malformed Epub".to_string())),
            }
        }
    };

    let toc = match content_ids.get(&toc_id) {
        Some(toc) => format!("{}{}", oebps, toc),
        None => return Err(EpubError::Epub("Malformed Epub".to_string())),
    };

    // collect the spine documents
    let mut spine = Vec::new();
    loop {
        let event = match content_parser.next() {
            Ok(event) => event,
            Err(_) => return Err(EpubError::Epub("Malformed Epub".to_string())),
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

// iterator over the text in the paragraph of an xhtml file
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

fn print_paragraph(stdout: &mut StandardStream, paragraph: &str, matches: Matches) -> usize {
    let paragraph = &paragraph[..];
    let mut previous_end = 0;
    let mut num_matches = 0;

    for m in matches {
        num_matches += 1;
        let start = m.start();
        let end = m.end();

        stdout.set_color(ColorSpec::new().set_fg(None)).unwrap();
        write!(stdout, "{}", &paragraph[previous_end..start]).unwrap();
        stdout
            .set_color(ColorSpec::new().set_fg(Some(Color::Blue)))
            .unwrap();
        write!(stdout, "{}", &paragraph[start..end]).unwrap();

        previous_end = end;
    }
    stdout.set_color(ColorSpec::new().set_fg(None)).unwrap();
    writeln!(stdout, "{}", &paragraph[previous_end..]).unwrap();

    num_matches
}

fn print_error(stderr: &mut StandardStream, message: String) {
    stderr
        .set_color(ColorSpec::new().set_fg(Some(Color::Red)))
        .unwrap();
    write!(stderr, "Error").unwrap();
    stderr.set_color(ColorSpec::new().set_fg(None)).unwrap();
    write!(stderr, ": {}", message).unwrap();
}

#[derive(FromArgs, Debug)]
/// Search an epub for a regular expression
struct EpubArgs {
    #[argh(switch, short = 'c')]
    /// print the number of matching paragraphs
    count: bool,

    #[argh(switch, short = 'q')]
    /// produce no output
    quiet: bool,

    #[argh(switch, short = 'i')]
    /// do case insensitive search
    ignore_case: bool,

    #[argh(switch, short = 'w')]
    /// find matches surrounded by word boundaries
    word_regexp: bool,

    #[argh(option, default = "String::from(\"auto\")")]
    /// whether to print results in color.
    /// options: always, auto, never
    color: String,

    #[argh(positional)]
    /// regular Expression
    regex: String,

    #[argh(positional)]
    /// files to search
    file_names: Vec<String>,
}

fn main() {
    let args: EpubArgs = argh::from_env();

    let color_choice = match args.color.as_str() {
        "always" => ColorChoice::Always,
        "auto" => ColorChoice::Auto,
        "never" => ColorChoice::Never,
        s => {
            eprintln!("Invalid color choice: {}", s);
            return;
        }
    };

    let mut stdout = StandardStream::stdout(color_choice);
    let mut stderr = StandardStream::stderr(color_choice);

    let mut re_string = String::new();
    if args.ignore_case {
        re_string.push_str("(?i)");
    }
    re_string.push_str(&args.regex);
    if args.word_regexp {
        re_string = format!(r"\b{}\b", re_string);
    }
    let re = Regex::new(&re_string).unwrap();

    let mut total_matches = 0;

    for file_name in args.file_names {
        // open up the file as a zip archive
        let mut archive = match File::open(file_name.clone())
            .map_err(EpubError::IO)
            .and_then(|f| ZipArchive::new(f).map_err(EpubError::Zip))
        {
            Ok(archive) => archive,
            Err(e) => {
                print_error(
                    &mut stderr,
                    match e {
                        EpubError::IO(_) => format!("unable to open {}", file_name),
                        EpubError::Zip(_) => format!("{} may not be a zip archive", file_name),
                        _ => "you shouldn't see this message".to_string(),
                    },
                );
                continue;
            }
        };

        let (_toc, spine) = match get_spine_documents(&mut archive) {
            Ok(t) => t,
            Err(_) => {
                print_error(
                    &mut stderr,
                    format!("{} may not be an epub file", file_name),
                );
                continue;
            }
        };

        let mut num_matches = 0;
        for doc in spine {
            let file = archive.by_name(&doc).unwrap();
            for (idx, paragraph) in XhtmlTextIterator::new(file).enumerate() {
                if re.is_match(&paragraph) {
                    if args.quiet {
                        std::process::exit(0);
                    }
                    let matches = re.find_iter(&paragraph);
                    if !args.count {
                        stdout
                            .set_color(ColorSpec::new().set_fg(Some(Color::Green)))
                            .unwrap();
                        write!(&mut stdout, "{}({})", file_name, idx).unwrap();
                        stdout.set_color(ColorSpec::new().set_fg(None)).unwrap();
                        write!(&mut stdout, ": ").unwrap();
                        num_matches += print_paragraph(&mut stdout, &paragraph, matches);
                    } else {
                        num_matches += matches.count();
                    }
                }
            }
        }

        if args.count {
            println!(
                "\x1b[32m{}\x1b[0m: \x1b[34m{}\x1b[0m",
                file_name, num_matches
            );
        }
        total_matches += num_matches;
    }

    std::process::exit(if total_matches > 0 { 0 } else { 1 });
}
