//! XML as upstream reads builds: `xml.ParseXML` of upstream's
//! `runtime/lua/xml.lua`, which keeps elements, their attributes and text,
//! and drops comments and processing instructions.

use crate::pat::{self, gsub, gsub_fn, is_match};
use std::collections::HashMap;

/// A child of an element: an element or text
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Element(Element),
    Text(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Element {
    pub elem: String,
    pub attrib: HashMap<String, String>,
    /// Written as `<a/>`, or closed without children
    pub empty: bool,
    pub children: Vec<Node>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrib.get(name).map(String::as_str)
    }

    /// The child elements
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|c| match c {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// The first child element named `name`
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.elem == name)
    }

    /// The first text child
    pub fn text(&self) -> Option<&str> {
        self.children.iter().find_map(|c| match c {
            Node::Text(t) => Some(t.as_str()),
            Node::Element(_) => None,
        })
    }
}

/// `decodeContent`: the five entities; anything else between `&` and `;`
/// is dropped
fn decode(content: &str) -> String {
    gsub_fn(content, "&(.-);", |c| {
        Some(
            match c[0].as_str() {
                "lt" => "<",
                "gt" => ">",
                "amp" => "&",
                "apos" => "'",
                "quot" => "\"",
                _ => "",
            }
            .to_string(),
        )
    })
}

fn attribs(s: &str) -> HashMap<String, String> {
    pat::gmatch(s, "(%w+)=([\"'])(.-)%2")
        .into_iter()
        .map(|c| (c[0].clone(), decode(&c[2])))
        .collect()
}

/// `xml.ParseXML(str)`: the document as the children of a nameless root.
pub fn parse(text: &str) -> Result<Element, String> {
    let s = gsub(text, "<!%-%-.-%-%->", "");
    let mut stack = vec![Element::default()];
    let mut i = 1;
    loop {
        // A CDATA section
        if let Some(c) = pat::captures_at(&s, "^%s*([^<]-)%s*<!%[CDATA%[(.-)%]%]>()", i) {
            let top = stack.last_mut().unwrap();
            if is_match(&c[0], "%S") {
                top.children.push(Node::Text(decode(&c[0])));
            }
            if is_match(&c[1], "%S") {
                top.children.push(Node::Text(c[1].clone()));
            }
            i = c[2].parse().map_err(|_| "position")?;
            continue;
        }
        let Some(c) = pat::captures_at(&s, "^%s*([^<]-)%s*<([%?/]?)([%w:]+)(.-)([%?/]?)>()", i)
        else {
            break;
        };
        let (content, cs, elem, attrib, ce) = (&c[0], &c[1], &c[2], &c[3], &c[4]);
        if is_match(content, "%S") {
            stack
                .last_mut()
                .unwrap()
                .children
                .push(Node::Text(decode(content)));
        }
        if cs == "?" || ce == "?" {
            // A processing instruction
        } else if ce == "/" {
            stack
                .last_mut()
                .unwrap()
                .children
                .push(Node::Element(Element {
                    elem: elem.clone(),
                    attrib: attribs(attrib),
                    empty: true,
                    children: vec![],
                }));
        } else if cs.is_empty() {
            stack.push(Element {
                elem: elem.clone(),
                attrib: attribs(attrib),
                empty: false,
                children: vec![],
            });
        } else {
            if stack.len() == 1 {
                return Err(format!("nothing to close with '{elem}'"));
            }
            let mut closed = stack.pop().unwrap();
            if closed.elem != *elem {
                return Err(format!("trying to close <{}> with '{elem}'", closed.elem));
            }
            if closed.children.is_empty() {
                closed.empty = true;
            }
            stack
                .last_mut()
                .unwrap()
                .children
                .push(Node::Element(closed));
        }
        i = c[5].parse().map_err(|_| "position")?;
    }
    if stack.len() > 1 {
        return Err(format!("unclosed element '{}'", stack.last().unwrap().elem));
    }
    let rest = pat::captures_at(&s, "^%s*(.-)%s*$", i)
        .map(|c| c[0].clone())
        .unwrap_or_default();
    let mut root = stack.pop().unwrap();
    if is_match(&rest, "%S") {
        root.children.push(Node::Text(decode(&rest)));
    }
    Ok(root)
}
