//! Reads FileZilla 3 `sitemanager.xml` into an intermediate tree.
//!
//! Numeric codes come from FileZilla's `ServerProtocol` / `LogonType` enums (stable by
//! contract: "never change existing values"). `RemoteDir` uses `CServerPath::GetSafePath`:
//! `<type+1> <prefix-len> [<prefix> ](<len> <segment> )*`.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use quick_xml::XmlVersion;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::ImportError;

/// A password as read from the file; never printed by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Pw(String);

impl Pw {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Pw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pw(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FzPassword {
    None,
    /// `encoding="base64"` (or no encoding attribute), decoded.
    Plain(Pw),
    /// Protected by a FileZilla master password (`encoding="crypt"`) or otherwise unreadable.
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FzProtocol {
    /// Code 0: FTP, with explicit TLS when the server offers it.
    Ftp,
    Sftp,
    /// Code 3: implicit FTPS.
    FtpsImplicit,
    /// Code 4: explicit FTPS (AUTH TLS).
    FtpsExplicit,
    /// Code 6: plain FTP, TLS forbidden.
    InsecureFtp,
    Unsupported(i32),
}

impl FzProtocol {
    fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Ftp,
            1 => Self::Sftp,
            3 => Self::FtpsImplicit,
            4 => Self::FtpsExplicit,
            6 => Self::InsecureFtp,
            other => Self::Unsupported(other),
        }
    }
}

/// Human name of a protocol code filecargo does not support, for the import report.
pub(crate) fn unsupported_protocol_name(code: i32) -> String {
    let name = match code {
        2 => "HTTP",
        5 => "HTTPS",
        7 => "S3",
        8 => "Storj",
        9 => "WebDAV",
        10 => "Azure Files",
        11 => "Azure Blob Storage",
        12 => "OpenStack Swift",
        13 => "Google Cloud Storage",
        14 => "Google Drive",
        15 => "Dropbox",
        16 => "OneDrive",
        17 => "Backblaze B2",
        18 => "Box",
        19 => "WebDAV (insecure)",
        20 => "Rackspace",
        21 => "Storj (grant)",
        22 => "S3 (SSO)",
        23 => "Google Cloud Storage (service account)",
        24 => "Cloudflare R2",
        _ => return format!("protocol #{code}"),
    };
    name.to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FzLogon {
    Anonymous,
    Normal,
    Ask,
    Interactive,
    /// FTP "account" logon: treated like a normal password logon.
    Account,
    Key,
    Unsupported(i32),
}

impl FzLogon {
    fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Anonymous,
            1 => Self::Normal,
            2 => Self::Ask,
            3 => Self::Interactive,
            4 => Self::Account,
            5 => Self::Key,
            other => Self::Unsupported(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FzSite {
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub protocol: FzProtocol,
    pub logon: FzLogon,
    pub user: String,
    pub password: FzPassword,
    pub keyfile: Option<String>,
    /// `Some(true)` for `MODE_ACTIVE`, `Some(false)` for `MODE_PASSIVE`, `None` for the default.
    pub active_mode: Option<bool>,
    pub remote_dir: Option<String>,
    pub local_dir: Option<String>,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FzNode {
    Folder { name: String, children: Vec<FzNode> },
    Site(Box<FzSite>),
}

/// Parses the contents of a `sitemanager.xml`. `path` is only used in error messages.
pub(crate) fn parse(path: &Path, xml: &str) -> Result<Vec<FzNode>, ImportError> {
    let root = parse_dom(path, xml)?;
    let servers = root.child("Servers").ok_or_else(|| ImportError::Format {
        path: path.to_path_buf(),
        msg: "no <Servers> element; not a FileZilla site manager file".into(),
    })?;
    Ok(servers.children.iter().filter_map(read_node).collect())
}

fn read_node(el: &Elem) -> Option<FzNode> {
    match el.name.as_str() {
        "Folder" => {
            let name = el
                .text_of("Name")
                .unwrap_or_else(|| el.own_text().trim().to_owned());
            Some(FzNode::Folder {
                name,
                children: el.children.iter().filter_map(read_node).collect(),
            })
        }
        "Server" => Some(FzNode::Site(Box::new(read_site(el)))),
        _ => None,
    }
}

fn read_site(el: &Elem) -> FzSite {
    let code = |tag: &str| el.text_of(tag).and_then(|t| t.parse::<i32>().ok());
    let host = el.text_of("Host").unwrap_or_default();
    FzSite {
        name: el.text_of("Name").unwrap_or_else(|| host.clone()),
        port: el
            .text_of("Port")
            .and_then(|p| p.parse::<u16>().ok())
            .filter(|p| *p != 0),
        protocol: FzProtocol::from_code(code("Protocol").unwrap_or(0)),
        logon: FzLogon::from_code(code("Logontype").unwrap_or(1)),
        user: el.text_of("User").unwrap_or_default(),
        password: read_password(el),
        keyfile: el.text_of("Keyfile"),
        active_mode: match el.text_of("PasvMode").as_deref() {
            Some("MODE_ACTIVE") => Some(true),
            Some("MODE_PASSIVE") => Some(false),
            _ => None,
        },
        remote_dir: el.text_of("RemoteDir").and_then(|d| decode_remote_dir(&d)),
        local_dir: el.text_of("LocalDir"),
        notes: el.text_of("Comments").unwrap_or_default(),
        host,
    }
}

fn read_password(el: &Elem) -> FzPassword {
    let Some(pass) = el.child("Pass") else {
        return FzPassword::None;
    };
    let raw = pass.own_text();
    let raw = raw.trim();
    if raw.is_empty() {
        return FzPassword::None;
    }
    match pass.attr("encoding") {
        Some("base64") => STANDARD
            .decode(raw)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map_or(FzPassword::Unreadable, |s| FzPassword::Plain(Pw(s))),
        None | Some("") | Some("none") => FzPassword::Plain(Pw(raw.to_owned())),
        Some(_) => FzPassword::Unreadable,
    }
}

/// Decodes FileZilla's "safe path" into a `/`-separated path. `None` for an empty or
/// malformed value.
pub(crate) fn decode_remote_dir(encoded: &str) -> Option<String> {
    let mut cur = Cursor {
        chars: encoded.trim().chars().collect(),
        pos: 0,
    };
    cur.number()?; // server type + 1
    let prefix_len = cur.number()?;
    let prefix = if prefix_len > 0 {
        cur.take(prefix_len)?
    } else {
        String::new()
    };
    let mut segments = Vec::new();
    while let Some(len) = cur.number() {
        segments.push(cur.take(len)?);
    }
    if cur.pos < cur.chars.len() {
        return None;
    }
    Some(format!("{prefix}/{}", segments.join("/")))
}

struct Cursor {
    chars: Vec<char>,
    pos: usize,
}

impl Cursor {
    /// Reads a decimal token and the single space after it.
    fn number(&mut self) -> Option<usize> {
        let start = self.pos;
        while self.chars.get(self.pos).is_some_and(char::is_ascii_digit) {
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        let n = self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .parse()
            .ok()?;
        self.skip_space();
        Some(n)
    }

    /// Reads exactly `len` characters and the single space after them.
    fn take(&mut self, len: usize) -> Option<String> {
        let end = self.pos.checked_add(len)?;
        let s: String = self.chars.get(self.pos..end)?.iter().collect();
        self.pos = end;
        self.skip_space();
        Some(s)
    }

    fn skip_space(&mut self) {
        if self.chars.get(self.pos) == Some(&' ') {
            self.pos += 1;
        }
    }
}

/// Minimal element tree: just enough for the site manager format.
#[derive(Debug, Default)]
struct Elem {
    name: String,
    attrs: Vec<(String, String)>,
    /// Text directly inside this element (not inside children), concatenated.
    text: String,
    children: Vec<Elem>,
}

impl Elem {
    fn child(&self, name: &str) -> Option<&Elem> {
        self.children.iter().find(|c| c.name == name)
    }

    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn own_text(&self) -> &str {
        &self.text
    }

    /// Trimmed text of a child element; `None` when missing or empty.
    fn text_of(&self, name: &str) -> Option<String> {
        let t = self.child(name)?.text.trim();
        (!t.is_empty()).then(|| t.to_owned())
    }
}

fn parse_dom(path: &Path, xml: &str) -> Result<Elem, ImportError> {
    let line_at = |pos: u64| {
        let end = usize::try_from(pos).unwrap_or(usize::MAX).min(xml.len());
        xml.as_bytes()[..end]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
            + 1
    };
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<Elem> = vec![Elem::default()];
    let parse_err = |reader: &Reader<&[u8]>, msg: String| ImportError::Parse {
        path: path.to_path_buf(),
        line: line_at(reader.buffer_position()),
        msg,
    };

    loop {
        let event = reader
            .read_event()
            .map_err(|e| parse_err(&reader, e.to_string()))?;
        match event {
            Event::Start(e) => stack.push(new_elem(&e).map_err(|m| parse_err(&reader, m))?),
            Event::Empty(e) => {
                let el = new_elem(&e).map_err(|m| parse_err(&reader, m))?;
                top(&mut stack).children.push(el);
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    return Err(parse_err(&reader, "unexpected closing tag".into()));
                }
                if let Some(done) = stack.pop() {
                    top(&mut stack).children.push(done);
                }
            }
            Event::Text(t) => top(&mut stack)
                .text
                .push_str(&t.xml_content(XmlVersion::Implicit1_0)),
            Event::CData(c) => top(&mut stack)
                .text
                .push_str(&c.xml_content(XmlVersion::Implicit1_0)),
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref() {
                    Ok(Some(ch)) => ch.to_string(),
                    Ok(None) => match r.as_ref() {
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "amp" => "&".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        other => {
                            return Err(parse_err(&reader, format!("unknown entity &{other};")));
                        }
                    },
                    Err(e) => return Err(parse_err(&reader, e.to_string())),
                };
                top(&mut stack).text.push_str(&resolved);
            }
            Event::Eof => break,
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) | Event::DocType(_) => {}
        }
    }

    if stack.len() != 1 {
        return Err(parse_err(&reader, "unclosed element at end of file".into()));
    }
    let mut document = stack.remove(0);
    if document.children.is_empty() {
        return Err(parse_err(&reader, "no root element".into()));
    }
    Ok(document.children.remove(0))
}

fn top(stack: &mut [Elem]) -> &mut Elem {
    // The stack always holds the synthetic document element.
    stack
        .last_mut()
        .unwrap_or_else(|| unreachable!("element stack is never empty"))
}

fn new_elem(e: &quick_xml::events::BytesStart<'_>) -> Result<Elem, String> {
    let mut attrs = Vec::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|err| err.to_string())?;
        let value = attr
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|err| err.to_string())?;
        attrs.push((attr.key.as_ref().to_owned(), value.into_owned()));
    }
    Ok(Elem {
        name: e.name().as_ref().to_owned(),
        attrs,
        ..Elem::default()
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/filezilla/sitemanager.xml");

    fn site(name: &str, host: &str) -> FzSite {
        FzSite {
            name: name.into(),
            host: host.into(),
            port: None,
            protocol: FzProtocol::Ftp,
            logon: FzLogon::Normal,
            user: String::new(),
            password: FzPassword::None,
            keyfile: None,
            active_mode: None,
            remote_dir: None,
            local_dir: None,
            notes: String::new(),
        }
    }

    fn parse_fixture() -> Vec<FzNode> {
        parse(&PathBuf::from("sitemanager.xml"), FIXTURE).unwrap()
    }

    #[test]
    fn fixture_parses_to_the_expected_tree() {
        let expected = vec![
            FzNode::Folder {
                name: "Work".into(),
                children: vec![
                    FzNode::Site(Box::new(FzSite {
                        port: Some(2222),
                        protocol: FzProtocol::Sftp,
                        logon: FzLogon::Key,
                        user: "deploy".into(),
                        keyfile: Some("/home/me/.ssh/id_ed25519".into()),
                        local_dir: Some("/home/me/projects".into()),
                        remote_dir: Some("/var/www".into()),
                        notes: "production & staging gateway".into(),
                        ..site("prod-web", "web1.example.com")
                    })),
                    FzNode::Folder {
                        name: "Internal".into(),
                        children: vec![FzNode::Site(Box::new(FzSite {
                            port: Some(21),
                            protocol: FzProtocol::FtpsExplicit,
                            user: "alice".into(),
                            password: FzPassword::Plain(Pw("fixture-password".into())),
                            active_mode: Some(false),
                            ..site("intranet-ftps", "files.example.com")
                        }))],
                    },
                ],
            },
            FzNode::Site(Box::new(FzSite {
                port: Some(21),
                logon: FzLogon::Anonymous,
                user: "anonymous".into(),
                ..site("public-mirror", "ftp.example.org")
            })),
            FzNode::Site(Box::new(FzSite {
                port: Some(990),
                protocol: FzProtocol::FtpsImplicit,
                user: "bob".into(),
                password: FzPassword::Unreadable,
                ..site("implicit-ftps", "secure.example.net")
            })),
            FzNode::Site(Box::new(FzSite {
                protocol: FzProtocol::Unsupported(7),
                user: "AKIAFAKEFAKEFAKE".into(),
                ..site("backup-bucket", "s3.example.com")
            })),
            FzNode::Site(Box::new(FzSite {
                port: Some(21),
                protocol: FzProtocol::InsecureFtp,
                user: "carol".into(),
                password: FzPassword::Plain(Pw("legacy-pw".into())),
                active_mode: Some(true),
                remote_dir: Some("/home/user".into()),
                ..site("legacy-ftp", "legacy.example.com")
            })),
            FzNode::Site(Box::new(FzSite {
                protocol: FzProtocol::Sftp,
                logon: FzLogon::Ask,
                user: "dave".into(),
                ..site("ask-me", "ask.example.com")
            })),
        ];
        assert_eq!(parse_fixture(), expected);
    }

    #[test]
    fn passwords_are_redacted_in_debug_output() {
        let text = format!("{:?}", parse_fixture());
        assert!(!text.contains("fixture-password"), "{text}");
        assert!(!text.contains("legacy-pw"), "{text}");
    }

    #[test]
    fn remote_dir_decoding() {
        let d = decode_remote_dir;
        assert_eq!(d("1 0 4 home 4 user").as_deref(), Some("/home/user"));
        assert_eq!(d("1 0").as_deref(), Some("/"));
        assert_eq!(d("1 0 7 my docs 3 a b").as_deref(), Some("/my docs/a b"));
        assert_eq!(d("3 2 C: 5 Users").as_deref(), Some("C:/Users"));
        assert_eq!(d(""), None);
        assert_eq!(d("1 0 9 short"), None, "length beyond the data");
        assert_eq!(d("garbage"), None);
    }

    #[test]
    fn missing_servers_element_is_a_format_error() {
        let err = parse(Path::new("x.xml"), "<FileZilla3></FileZilla3>").unwrap_err();
        assert!(matches!(err, ImportError::Format { .. }), "{err}");
    }

    #[test]
    fn malformed_xml_names_the_file_and_line() {
        let xml = "<FileZilla3>\n<Servers>\n<Server></Folder>\n</Servers></FileZilla3>";
        let err = parse(Path::new("sm.xml"), xml).unwrap_err();
        match err {
            ImportError::Parse { path, line, .. } => {
                assert_eq!(path, PathBuf::from("sm.xml"));
                assert_eq!(line, 3);
            }
            other => panic!("expected parse error, got {other}"),
        }
    }

    #[test]
    fn unclosed_and_empty_documents_are_rejected() {
        assert!(parse(Path::new("x"), "<FileZilla3><Servers>").is_err());
        assert!(parse(Path::new("x"), "").is_err());
    }

    #[test]
    fn folder_name_may_be_a_name_child() {
        let xml =
            "<FileZilla3><Servers><Folder><Name>Archive</Name></Folder></Servers></FileZilla3>";
        let nodes = parse(Path::new("x"), xml).unwrap();
        assert_eq!(
            nodes,
            vec![FzNode::Folder {
                name: "Archive".into(),
                children: vec![]
            }]
        );
    }

    #[test]
    fn unsupported_protocols_have_readable_names() {
        assert_eq!(unsupported_protocol_name(7), "S3");
        assert_eq!(unsupported_protocol_name(99), "protocol #99");
    }
}
