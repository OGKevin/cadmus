//! HTML and XML parsers that produce an [`XmlTree`].
//!
//! Two parsers are provided, each suited to a different use-case:
//!
//! - [`XmlParser`] — a hand-rolled recursive-descent parser. Node offsets are
//!   exact byte positions of each token in the source string. Use this wherever
//!   reading positions need to be persisted to disk (EPUB spine chapters,
//!   standalone HTML files).
//!
//! - [`parse_html5`] — a thin wrapper around `html5ever`. Handles entities,
//!   void elements, and the full HTML5 error-recovery algorithm. Node offsets
//!   are **synthetic** (a monotonically increasing counter, not source
//!   positions). Use this for ephemeral rendering where offset precision is
//!   not required (e.g. the dictionary view).

use super::dom::{Attributes, NodeId, XmlTree, element, text, whitespace};
use fxhash::FxHashMap;
use html5ever::tendril::{Tendril, TendrilSink};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{Attribute, QualName};
use std::cell::{Ref, RefCell};

/// Extension trait that adds XML whitespace detection to [`char`].
pub trait XmlExt {
    /// Returns `true` for the four XML whitespace characters: space, tab,
    /// carriage return, and newline.
    fn is_xml_whitespace(&self) -> bool;
}

impl XmlExt for char {
    fn is_xml_whitespace(&self) -> bool {
        matches!(self, ' ' | '\t' | '\n' | '\r')
    }
}

/// Hand-rolled recursive-descent parser for XML and basic HTML documents.
///
/// Produces an [`XmlTree`] where every node's `offset` field is the exact byte
/// position of the opening `<` (elements) or first character (text nodes) in
/// `input`. This byte-accuracy is required when reading positions are
/// persisted across sessions.
///
/// The parser is intentionally lenient: unknown tags, processing instructions,
/// and CDATA sections are skipped silently. Self-closing tags (`<br/>`,
/// `<img/>`) are supported.
#[derive(Debug)]
pub struct XmlParser<'a> {
    /// The full source string being parsed.
    pub input: &'a str,
    /// Current byte offset into `input`.
    pub offset: usize,
}

impl<'a> XmlParser<'a> {
    /// Creates a new parser positioned at the start of `input`.
    pub fn new(input: &str) -> XmlParser<'_> {
        XmlParser { input, offset: 0 }
    }

    /// Returns `true` when the cursor has reached the end of the input.
    fn eof(&self) -> bool {
        self.offset >= self.input.len()
    }

    /// Returns the next character without advancing the cursor.
    fn next(&self) -> Option<char> {
        self.input[self.offset..].chars().next()
    }

    /// Returns `true` if the remaining input starts with `s`.
    fn starts_with(&self, s: &str) -> bool {
        self.input[self.offset..].starts_with(s)
    }

    /// Advances the cursor by exactly `n` Unicode scalar values.
    fn advance(&mut self, n: usize) {
        for c in self.input[self.offset..].chars().take(n) {
            self.offset += c.len_utf8();
        }
    }

    /// Advances the cursor as long as `test` returns `true` for the next char.
    fn advance_while<F>(&mut self, test: F)
    where
        F: FnMut(&char) -> bool,
    {
        for c in self.input[self.offset..].chars().take_while(test) {
            self.offset += c.len_utf8();
        }
    }

    /// Advances the cursor until `target` is found and consumes it.
    /// Does nothing if `target` is never found before EOF.
    fn advance_until(&mut self, target: &str) {
        while !self.eof() && !self.starts_with(target) {
            self.advance(1);
        }
        self.advance(target.chars().count());
    }

    /// Parses the attribute list of an open tag, stopping at `>` or `/`.
    ///
    /// Both single- and double-quoted attribute values are supported. The
    /// cursor is left immediately before the closing `>` or `/`.
    fn parse_attributes(&mut self) -> Attributes {
        let mut attrs = FxHashMap::default();
        while !self.eof() {
            self.advance_while(|&c| c.is_xml_whitespace());
            match self.next() {
                Some('>') | Some('/') | None => break,
                _ => {
                    let offset = self.offset;
                    self.advance_while(|&c| c != '=');
                    let key = self.input[offset..self.offset].to_string();
                    self.advance_while(|&c| c != '"' && c != '\'');
                    let quote = self.next().unwrap_or('"');
                    self.advance(1);
                    let offset = self.offset;
                    self.advance_while(|&c| c != quote);
                    let value = self.input[offset..self.offset].to_string();
                    attrs.insert(key, value);
                    self.advance(1);
                }
            }
        }
        attrs
    }

    /// Parses a single element (tag name + attributes + children) and appends
    /// it to `parent_id` in `tree`.
    ///
    /// The cursor must be positioned immediately after the opening `<` when
    /// this function is called. After returning the cursor is positioned after
    /// the element's closing tag.
    fn parse_element(&mut self, tree: &mut XmlTree, parent_id: NodeId) {
        let offset = self.offset;
        self.advance_while(|&c| c != '>' && c != '/' && !c.is_xml_whitespace());
        let name = &self.input[offset..self.offset];
        let attributes = self.parse_attributes();

        match self.next() {
            Some('/') => {
                self.advance(2);
                tree.get_mut(parent_id)
                    .append(element(name, offset - 1, attributes));
            }
            Some('>') => {
                self.advance(1);
                let id = tree
                    .get_mut(parent_id)
                    .append(element(name, offset - 1, attributes));
                self.parse_nodes(tree, id);
            }
            _ => (),
        }
    }

    /// Parses all child nodes of `parent_id` until a matching closing tag or
    /// EOF is reached.
    ///
    /// Handles text nodes, whitespace, elements, processing instructions
    /// (`<?…?>`), comments (`<!--…-->`), CDATA sections (`<![…]]>`), and
    /// DOCTYPE declarations.
    fn parse_nodes(&mut self, tree: &mut XmlTree, parent_id: NodeId) {
        while !self.eof() {
            let offset = self.offset;
            self.advance_while(|&c| c.is_xml_whitespace());

            match self.next() {
                Some('<') => {
                    if self.offset > offset {
                        tree.get_mut(parent_id)
                            .append(whitespace(&self.input[offset..self.offset], offset));
                    }
                    if self.starts_with("</") {
                        self.advance(2);
                        self.advance_while(|&c| c != '>');
                        self.advance(1);
                        break;
                    }
                    self.advance(1);
                    match self.next() {
                        Some('?') => {
                            self.advance(1);
                            self.advance_until("?>");
                        }
                        Some('!') => {
                            self.advance(1);
                            match self.next() {
                                Some('-') => {
                                    self.advance(2);
                                    self.advance_until("-->");
                                }
                                Some('[') => {
                                    self.advance(1);
                                    self.advance_until("]]>");
                                }
                                _ => {
                                    self.advance_while(|&c| c != '>');
                                    self.advance(1);
                                }
                            }
                        }
                        _ => self.parse_element(tree, parent_id),
                    }
                }
                Some(..) => {
                    self.advance_while(|&c| c != '<');
                    tree.get_mut(parent_id)
                        .append(text(&self.input[offset..self.offset], offset));
                }
                None => break,
            }
        }
    }

    /// Parses `self.input` and returns the resulting [`XmlTree`].
    ///
    /// Every node's `offset` is the byte position of its opening `<` or first
    /// text character within the original source string.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(len = self.input.len())))]
    pub fn parse(&mut self) -> XmlTree {
        let mut tree = XmlTree::new();
        self.parse_nodes(&mut tree, NodeId::from_index(0));
        tree
    }
}

/// [`TreeSink`] implementation that bridges html5ever's push-based API into
/// an [`XmlTree`].
///
/// Node offsets are assigned from a monotonically increasing counter rather
/// than from source byte positions, because html5ever's `TreeSink` callbacks
/// do not receive source positions. The counter advances by 1 per element and
/// by `text.len()` per text chunk, preserving the non-overlap invariant needed
/// by the layout engine's page-finding binary search.
struct Html5Sink {
    /// The tree being built. `RefCell` is required because multiple `TreeSink`
    /// methods need mutable access and Rust's borrow checker cannot see that
    /// html5ever calls them non-concurrently.
    tree: RefCell<XmlTree>,
    /// Maps each element `NodeId` to its fully-qualified name so that
    /// `elem_name` can return a borrowed reference as required by the trait.
    qual_names: RefCell<FxHashMap<NodeId, QualName>>,
    /// Maps `<template>` element `NodeId`s to their associated content root
    /// `NodeId`, as required by the HTML5 template element spec.
    template_contents: RefCell<FxHashMap<NodeId, NodeId>>,
    /// Synthetic position counter. Incremented for every node created so that
    /// all offsets are unique and ordered by document position.
    offset_counter: RefCell<usize>,
}

impl Html5Sink {
    /// Creates a new sink with an empty tree and a zeroed offset counter.
    fn new() -> Self {
        Html5Sink {
            tree: RefCell::new(XmlTree::new()),
            qual_names: RefCell::new(FxHashMap::default()),
            template_contents: RefCell::new(FxHashMap::default()),
            offset_counter: RefCell::new(0),
        }
    }

    /// Returns the current value of the offset counter without advancing it.
    fn next_offset(&self) -> usize {
        *self.offset_counter.borrow()
    }

    /// Advances the offset counter by `by`, clamped to a minimum of 1 to
    /// guarantee that every node receives a strictly larger offset than the
    /// previous one even for zero-length text runs.
    fn advance_offset(&self, by: usize) {
        *self.offset_counter.borrow_mut() += by.max(1);
    }

    /// Returns `true` when `text` contains only ASCII whitespace characters.
    fn is_whitespace_only(text: &str) -> bool {
        text.chars().all(|c| c.is_ascii_whitespace())
    }

    /// Converts an html5ever [`Attribute`] name to its string representation,
    /// prefixing with the namespace if one is present (e.g. `xml:lang`).
    fn attr_name(attr: &Attribute) -> String {
        match &attr.name.prefix {
            Some(prefix) => format!("{}:{}", prefix.as_ref(), attr.name.local.as_ref()),
            None => attr.name.local.as_ref().to_string(),
        }
    }

    /// Converts a `Vec<Attribute>` from html5ever into the [`Attributes`] map
    /// used by the DOM.
    fn build_attributes(attrs: Vec<Attribute>) -> Attributes {
        let mut attributes = Attributes::default();
        for attr in attrs {
            attributes.insert(Self::attr_name(&attr), attr.value.to_string());
        }
        attributes
    }
}

impl TreeSink for Html5Sink {
    type Handle = NodeId;
    type Output = XmlTree;
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Self::Output {
        self.tree.into_inner()
    }

    /// Silently ignores all parse errors. The dictionary content from
    /// reader-dict is often malformed HTML, and we rely on html5ever's
    /// error-recovery rather than failing on bad input.
    fn parse_error(&self, _msg: std::borrow::Cow<'static, str>) {}

    fn get_document(&self) -> Self::Handle {
        NodeId::from_index(0)
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        Ref::map(self.qual_names.borrow(), |names| {
            names.get(target).expect("elem_name called on unknown node")
        })
    }

    /// Creates a new element node, assigns it the next synthetic offset, and
    /// registers its qualified name for later `elem_name` lookups.
    ///
    /// For `<template>` elements an additional content-root node is created
    /// and stored in `template_contents`, as required by the spec.
    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> Self::Handle {
        let tag_name = name.local.as_ref();
        let offset = self.next_offset();
        self.advance_offset(1);
        let attributes = Self::build_attributes(attrs);
        let data = element(tag_name, offset, attributes);
        let id = self.tree.borrow_mut().push_node(data);
        self.qual_names.borrow_mut().insert(id, name.clone());

        if flags.template {
            let template_root_offset = self.next_offset();
            self.advance_offset(1);
            let template_root = element(
                "template-contents",
                template_root_offset,
                Attributes::default(),
            );
            let template_id = self.tree.borrow_mut().push_node(template_root);
            self.template_contents.borrow_mut().insert(id, template_id);
        }

        id
    }

    /// Maps an HTML comment to an empty whitespace node so it occupies a slot
    /// in the offset space without contributing visible content.
    fn create_comment(&self, _text: Tendril<html5ever::tendril::fmt::UTF8>) -> Self::Handle {
        let offset = self.next_offset();
        self.advance_offset(1);
        let data = whitespace("", offset);
        self.tree.borrow_mut().push_node(data)
    }

    /// Maps a processing instruction to an empty whitespace node so it
    /// occupies a slot in the offset space without contributing visible
    /// content.
    fn create_pi(
        &self,
        _target: Tendril<html5ever::tendril::fmt::UTF8>,
        _data: Tendril<html5ever::tendril::fmt::UTF8>,
    ) -> Self::Handle {
        let offset = self.next_offset();
        self.advance_offset(1);
        let data = whitespace("", offset);
        self.tree.borrow_mut().push_node(data)
    }

    /// Appends a child node or text run to `parent`.
    ///
    /// Text runs are coalesced into the preceding sibling text node when one
    /// exists, to match the behaviour of the hand-rolled parser and avoid
    /// producing redundant nodes for adjacent text chunks.
    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        match child {
            NodeOrText::AppendNode(node) => {
                self.tree.borrow_mut().attach_child(*parent, node);
            }
            NodeOrText::AppendText(t) => {
                let text_str = t.as_ref();
                let last_child_id = self
                    .tree
                    .borrow()
                    .get(*parent)
                    .last_child()
                    .filter(|n| n.tag_name().is_none() && !n.text().is_empty())
                    .map(|n| n.id);

                if let Some(last_id) = last_child_id {
                    self.advance_offset(text_str.len());
                    self.tree.borrow_mut().append_text_to(last_id, text_str);
                } else {
                    let offset = self.next_offset();
                    self.advance_offset(text_str.len());
                    let data = if Self::is_whitespace_only(text_str) {
                        whitespace(text_str, offset)
                    } else {
                        text(text_str, offset)
                    };
                    let node_id = self.tree.borrow_mut().push_node(data);
                    self.tree.borrow_mut().attach_child(*parent, node_id);
                }
            }
        }
    }

    /// Delegates to [`Self::append`] using `element` as the target parent.
    ///
    /// Called by html5ever during foster-parenting and similar error-recovery
    /// situations where the intended parent is determined by the element rather
    /// than its previous sibling.
    fn append_based_on_parent_node(
        &self,
        element: &Self::Handle,
        prev_element: &Self::Handle,
        child: NodeOrText<Self::Handle>,
    ) {
        let has_parent = self.tree.borrow().get(*element).parent().is_some();
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    /// Inserts a node or text run immediately before `sibling`.
    fn append_before_sibling(&self, sibling: &Self::Handle, new_node: NodeOrText<Self::Handle>) {
        match new_node {
            NodeOrText::AppendNode(node) => {
                self.tree.borrow_mut().insert_before(*sibling, node);
            }
            NodeOrText::AppendText(t) => {
                let text_str = t.as_ref();
                let offset = self.next_offset();
                self.advance_offset(text_str.len());
                let data = if Self::is_whitespace_only(text_str) {
                    whitespace(text_str, offset)
                } else {
                    text(text_str, offset)
                };
                let node_id = self.tree.borrow_mut().push_node(data);
                self.tree.borrow_mut().insert_before(*sibling, node_id);
            }
        }
    }

    /// DOCTYPE declarations are not represented in the tree.
    fn append_doctype_to_document(
        &self,
        _name: Tendril<html5ever::tendril::fmt::UTF8>,
        _public_id: Tendril<html5ever::tendril::fmt::UTF8>,
        _system_id: Tendril<html5ever::tendril::fmt::UTF8>,
    ) {
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        *self
            .template_contents
            .borrow()
            .get(target)
            .expect("template contents not registered")
    }

    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        x == y
    }

    /// Quirks mode is accepted but has no effect on the tree representation.
    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn add_attrs_if_missing(&self, target: &Self::Handle, attrs: Vec<Attribute>) {
        let mut tree = self.tree.borrow_mut();
        for attr in attrs {
            tree.add_attr_if_missing(*target, &Self::attr_name(&attr), &attr.value);
        }
    }

    fn remove_from_parent(&self, target: &Self::Handle) {
        self.tree.borrow_mut().detach(*target);
    }

    fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
        let children: Vec<NodeId> = self
            .tree
            .borrow()
            .get(*node)
            .children()
            .map(|c| c.id)
            .collect();
        for child in children {
            self.tree.borrow_mut().detach(child);
            self.tree.borrow_mut().attach_child(*new_parent, child);
        }
    }
}

/// Parses `input` as HTML using the html5ever spec-compliant parser and
/// returns the resulting [`XmlTree`].
///
/// Compared to [`XmlParser`] this handles the full range of HTML5 content
/// correctly:
///
/// - Named and numeric entities (`&amp;`, `&#160;`, …) are decoded.
/// - Void elements (`<br>`, `<img>`, `<input>`, …) are never given children.
/// - Implicitly-closed block tags (`<p>`, `<li>`, …) are auto-closed per spec.
/// - Unclosed tags at EOF are closed automatically.
///
/// **Offset semantics:** node offsets are synthetic (a monotonically
/// increasing counter) and are **not** byte positions in the source string.
/// This makes the tree unsuitable for persisting reading positions to disk.
/// Use [`XmlParser`] when byte-accurate offsets are required.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(input), fields(len = input.len())))]
pub fn parse_html5(input: &str) -> XmlTree {
    use html5ever::{ParseOpts, parse_document};

    let parser = parse_document(Html5Sink::new(), ParseOpts::default());
    let input_tendril: Tendril<html5ever::tendril::fmt::UTF8> = input.into();
    let mut tree = parser.one(input_tendril);
    tree.wrap_lost_inlines();
    tree
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_element() {
        let text = "<a/>";
        let xml = XmlParser::new(text).parse();
        let n = xml.root().first_child().unwrap();
        assert_eq!(n.offset(), 0);
        assert_eq!(n.tag_name(), Some("a"));
    }

    #[test]
    fn test_attributes() {
        let text = r#"<a b="c" d='e"'/>"#;
        let xml = XmlParser::new(text).parse();
        let n = xml.root().first_child().unwrap();
        assert_eq!(n.attribute("b"), Some("c"));
        assert_eq!(n.attribute("d"), Some("e\""));
    }

    #[test]
    fn test_text() {
        let text = "<a>bcd</a>";
        let xml = XmlParser::new(text).parse();
        let child = xml.root().first_child().unwrap().children().next();
        assert_eq!(child.map(|c| c.offset()), Some(3));
        assert_eq!(child.map(|c| c.text()), Some("bcd".to_string()));
    }

    #[test]
    fn test_inbetween_space() {
        let text = "<a><b>x</b> <c>y</c></a>";
        let xml = XmlParser::new(text).parse();
        let child = xml.root().first_child().unwrap().children().nth(1);
        assert_eq!(child.map(|c| c.text()), Some(" ".to_string()));
    }

    #[test]
    fn test_central_space() {
        let text = "<a><b> </b></a>";
        let xml = XmlParser::new(text).parse();
        assert_eq!(xml.root().text(), " ");
    }

    #[test]
    fn html5_void_element() {
        let text = "<br>";
        let xml = parse_html5(text);
        assert!(xml.root().find("br").is_some());
    }

    #[test]
    fn html5_entity_decoding() {
        let text = "<p>hello&amp;world</p>";
        let xml = parse_html5(text);
        let p = xml.root().find("p").unwrap();
        assert_eq!(p.text(), "hello&world");
    }

    #[test]
    fn html5_unclosed_p_tags() {
        let text = "<p>first<p>second";
        let xml = parse_html5(text);
        let count = xml
            .root()
            .descendants()
            .filter(|n| n.tag_name() == Some("p"))
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn html5_nested_ol_in_ol() {
        let text =
            r#"<ol><li>top</li><ol style="list-style-type:lower-alpha"><li>sub</li></ol></ol>"#;
        let xml = parse_html5(text);
        let inner_ol = xml
            .root()
            .descendants()
            .find(|n| n.tag_name() == Some("ol") && n.attribute("style").is_some());
        assert!(
            inner_ol.is_some(),
            "inner <ol> with style should exist in the tree"
        );
        assert_eq!(
            inner_ol.unwrap().attribute("style"),
            Some("list-style-type:lower-alpha")
        );
    }

    #[test]
    fn html5_comment_does_not_coalesce_following_text() {
        let text = "<p>Hello<!-- comment -->World</p>";
        let xml = parse_html5(text);

        let p = xml.root().find("p").expect("p should exist");
        let children: Vec<_> = p.children().collect();

        assert_eq!(
            children.len(),
            3,
            "p should have 3 children: text, comment placeholder, text"
        );

        let text_nodes: Vec<_> = children
            .iter()
            .filter(|n| !n.text().is_empty())
            .map(|n| n.text())
            .collect();

        assert!(
            text_nodes.contains(&"Hello".to_string()),
            "text 'Hello' should exist as separate node"
        );
        assert!(
            text_nodes.contains(&"World".to_string()),
            "text 'World' should exist as separate node, not coalesced into comment node"
        );

        let comment_node = children
            .iter()
            .find(|n| n.text().is_empty() && n.tag_name().is_none());
        assert!(
            comment_node.is_some(),
            "empty whitespace node (comment placeholder) should exist"
        );
    }

    #[test]
    fn html5_pi_does_not_coalesce_following_text() {
        let text = "<p>Hello<?target data?>World</p>";
        let xml = parse_html5(text);

        let p = xml.root().find("p").expect("p should exist");
        let children: Vec<_> = p.children().collect();

        assert_eq!(
            children.len(),
            3,
            "p should have 3 children: text, pi placeholder, text"
        );

        let text_nodes: Vec<_> = children
            .iter()
            .filter(|n| !n.text().is_empty())
            .map(|n| n.text())
            .collect();

        assert!(
            text_nodes.contains(&"Hello".to_string()),
            "text 'Hello' should exist as separate node"
        );
        assert!(
            text_nodes.contains(&"World".to_string()),
            "text 'World' should exist as separate node, not coalesced into pi node"
        );
    }

    #[test]
    fn html5_text_node_offsets_do_not_overlap() {
        let text = "<p><em>Cadmus</em> is a document reader for <em>Kobo</em>'s e-readers.</p>";
        let xml = parse_html5(text);

        let mut text_nodes: Vec<(usize, usize)> = xml
            .root()
            .descendants()
            .filter(|n| n.tag_name().is_none())
            .map(|n| (n.offset(), n.text().len()))
            .filter(|(_, len)| *len > 0)
            .collect();

        text_nodes.sort_by_key(|(offset, _)| *offset);

        for window in text_nodes.windows(2) {
            let (offset_a, len_a) = window[0];
            let (offset_b, _) = window[1];
            assert!(
                offset_a + len_a <= offset_b,
                "text node at offset {} with len {} overlaps next node at offset {}",
                offset_a,
                len_a,
                offset_b
            );
        }
    }
}
