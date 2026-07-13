//! HTML parser and associated utilities that produce an [`XmlTree`].
//!
//! **Production code** should always use [`parse_html5`] to parse HTML content.
//! It handles entities, void elements, and the full HTML5 error-recovery
//! algorithm. Node offsets are byte-accurate source positions supplied by the
//! `source-positions` feature on the `html5ever` crate.
//!
//! [`XmlParser`] is only compiled in **test builds** (`#[cfg(test)]`). It is
//! retained exclusively for parity tests that compare its output against
//! `parse_html5` to validate byte-offset equivalence. Do not use it in
//! production code.

use super::dom::{Attributes, NodeId, XmlTree, element, text, whitespace};
use fxhash::FxHashMap;
use html5ever::tendril::{Tendril, TendrilSink};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{Attribute, QualName};
use std::cell::{Cell, Ref, RefCell};

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
/// **Only available in test builds.** Production code uses [`parse_html5`] for
/// HTML content and `roxmltree` (via `epub::opf`) for XML metadata files.
/// This parser is retained solely for parity tests that compare its output
/// against `parse_html5` to validate byte-offset equivalence.
///
/// Produces an [`XmlTree`] where every node's `offset` field is the exact byte
/// position of the opening `<` (elements) or first character (text nodes) in
/// `input`.
#[cfg(test)]
#[derive(Debug)]
pub struct XmlParser<'a> {
    /// The full source string being parsed.
    pub input: &'a str,
    /// Current byte offset into `input`.
    pub offset: usize,
}

#[cfg(test)]
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
/// Node offsets are byte-accurate source positions supplied by
/// [`TreeSink::set_current_byte`], which html5ever calls before every tree
/// mutation when the `source-positions` feature is enabled on the `html5ever`
/// crate. This makes offsets stable and comparable to those produced by
/// [`XmlParser`], enabling html5ever-parsed documents to be used wherever
/// persisted byte offsets are required (EPUB spine, bookmarks, annotations).
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
    /// Byte offset of the most recent token in the source string, forwarded
    /// from the tokenizer via [`TreeSink::set_current_byte`].
    current_byte: Cell<usize>,
}

impl Html5Sink {
    /// Creates a new sink with an empty tree and a zeroed byte offset.
    fn new() -> Self {
        Html5Sink {
            tree: RefCell::new(XmlTree::new()),
            qual_names: RefCell::new(FxHashMap::default()),
            template_contents: RefCell::new(FxHashMap::default()),
            current_byte: Cell::new(0),
        }
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

    /// Creates text or whitespace node data for a text callback.
    fn text_data(text_str: &str, offset: usize) -> super::dom::NodeData {
        if Self::is_whitespace_only(text_str) {
            return whitespace(text_str, offset);
        }

        text(text_str, offset)
    }

    /// Appends text while preserving source offset gaps around entities.
    fn append_text_segment(&self, parent: &NodeId, text_str: &str, offset: usize) {
        let last_child_id = self
            .tree
            .borrow()
            .get(*parent)
            .last_child()
            .filter(|n| n.tag_name().is_none() && !n.text().is_empty())
            .map(|n| n.id);

        if let Some(last_id) = last_child_id {
            let expected_offset = {
                let tree = self.tree.borrow();
                let last = tree.get(last_id);
                last.offset() + last.text().len()
            };

            if offset == expected_offset {
                self.tree.borrow_mut().append_text_to(last_id, text_str);
                return;
            }
        }

        let node_id = self
            .tree
            .borrow_mut()
            .push_node(Self::text_data(text_str, offset));
        self.tree.borrow_mut().attach_child(*parent, node_id);
    }
}

impl TreeSink for Html5Sink {
    type Handle = NodeId;
    type Output = XmlTree;
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Self::Output {
        self.tree.into_inner()
    }

    fn set_current_byte(&self, byte_offset: u64) {
        self.current_byte.set(byte_offset as usize);
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

    /// Creates a new element node, assigns it the current source byte offset,
    /// and registers its qualified name for later `elem_name` lookups.
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
        let offset = self.current_byte.get();
        let attributes = Self::build_attributes(attrs);
        let data = element(tag_name, offset, attributes);
        let id = self.tree.borrow_mut().push_node(data);
        self.qual_names.borrow_mut().insert(id, name.clone());

        if flags.template {
            let template_root = element(
                "template-contents",
                self.current_byte.get(),
                Attributes::default(),
            );
            let template_id = self.tree.borrow_mut().push_node(template_root);
            self.template_contents.borrow_mut().insert(id, template_id);
        }

        id
    }

    /// Maps an HTML comment to an empty whitespace node at the current source
    /// byte offset without contributing visible content.
    fn create_comment(&self, _text: Tendril<html5ever::tendril::fmt::UTF8>) -> Self::Handle {
        let data = whitespace("", self.current_byte.get());
        self.tree.borrow_mut().push_node(data)
    }

    /// Maps a processing instruction to an empty whitespace node at the
    /// current source byte offset without contributing visible content.
    fn create_pi(
        &self,
        _target: Tendril<html5ever::tendril::fmt::UTF8>,
        _data: Tendril<html5ever::tendril::fmt::UTF8>,
    ) -> Self::Handle {
        let data = whitespace("", self.current_byte.get());
        self.tree.borrow_mut().push_node(data)
    }

    /// Appends a child node or text run to `parent`.
    ///
    /// Text runs are coalesced into the preceding sibling text node when one
    /// exists, to match the behaviour of the hand-rolled parser and avoid
    /// producing redundant nodes for adjacent text chunks. When coalescing,
    /// the first node's byte offset is preserved — it marks where the text
    /// content started in the source.
    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        match child {
            NodeOrText::AppendNode(node) => {
                self.tree.borrow_mut().attach_child(*parent, node);
            }
            NodeOrText::AppendText(t) => {
                let text_str = t.as_ref();
                self.append_text_segment(parent, text_str, self.current_byte.get());
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
                let node_id = self
                    .tree
                    .borrow_mut()
                    .push_node(Self::text_data(text_str, self.current_byte.get()));
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
/// **Offset semantics:** node offsets are byte-accurate source positions
/// supplied by the `source-positions` feature of the `html5ever` crate.
/// Offsets for nodes that exist in both parsers' output are identical to those
/// produced by [`XmlParser`].
///
/// The caller is responsible for calling [`XmlTree::wrap_lost_inlines`] on the
/// returned tree if inline wrapping is required (it is for EPUB spine chapters
/// and standalone HTML files, but not for all use cases).
#[cfg_attr(feature = "tracing", tracing::instrument(skip(input), fields(len = input.len())))]
pub fn parse_html5(input: &str) -> XmlTree {
    use html5ever::{ParseOpts, parse_document};

    let parser = parse_document(Html5Sink::new(), ParseOpts::default());
    let input_tendril: Tendril<html5ever::tendril::fmt::UTF8> = input.into();
    parser.one(input_tendril)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_element_has_correct_tag_name() {
        let tree = parse_html5("<p></p>");
        assert!(tree.root().find("p").is_some());
        assert_eq!(tree.root().find("p").unwrap().tag_name(), Some("p"));
    }

    #[test]
    fn simple_element_has_correct_offset() {
        let tree = parse_html5("<p></p>");
        let p = tree.root().find("p").unwrap();
        assert_eq!(p.offset(), 0);
    }

    #[test]
    fn attributes_double_and_single_quoted() {
        let text = r#"<a b="c" d='e"'></a>"#;
        let tree = parse_html5(text);
        let a = tree.root().find("a").unwrap();
        assert_eq!(a.attribute("b"), Some("c"));
        assert_eq!(a.attribute("d"), Some("e\""));
    }

    #[test]
    fn text_node_content_and_offset() {
        let text = "<a>bcd</a>";
        let tree = parse_html5(text);
        let a = tree.root().find("a").unwrap();
        let child = a.children().find(|n| !n.text().is_empty());
        assert_eq!(child.map(|c| c.text()), Some("bcd".to_string()));
        assert_eq!(child.map(|c| c.offset()), Some(3));
    }

    #[test]
    fn whitespace_text_node_preserved_between_inline_siblings() {
        let text = "<p><b>x</b> <i>y</i></p>";
        let tree = parse_html5(text);
        let p = tree.root().find("p").unwrap();
        let space_node = p.children().find(|n| n.text() == " ");
        assert!(
            space_node.is_some(),
            "whitespace text node should be preserved between inline siblings"
        );
    }

    #[test]
    fn whitespace_text_inside_element_accessible_via_tree_text() {
        let text = "<p><span> </span></p>";
        let tree = parse_html5(text);
        let span = tree.root().find("span").unwrap();
        assert_eq!(span.text(), " ");
    }

    #[test]
    fn html5_void_element() {
        let text = "<br>";
        let xml = parse_html5(text);
        assert!(xml.root().find("br").is_some());
    }

    /// XHTML EPUB content frequently uses self-closing syntax on RCDATA/RAWTEXT
    /// elements (`<title/>`, `<style/>`). Without the `xhtml-self-closing`
    /// feature, html5ever treats `<title>` as opening a RCDATA region that
    /// swallows the rest of the document, leaving the `<body>` empty. With the
    /// feature, the self-closing flag is honoured and the body parses normally.
    #[test]
    fn html5_self_closing_title_does_not_swallow_body() {
        let text = concat!(
            "<html><head><title/></head>",
            "<body><p>visible</p></body></html>",
        );
        let tree = parse_html5(text);
        let body = tree.root().find("body").expect("body should exist");
        let p = body
            .descendants()
            .find(|n| n.tag_name() == Some("p"))
            .expect("body paragraph should not be swallowed by <title/>");
        assert_eq!(p.text(), "visible");
    }

    /// A self-closing `<style/>` must likewise not swallow following content.
    #[test]
    fn html5_self_closing_style_does_not_swallow_body() {
        let text = concat!(
            "<html><head><style/></head>",
            "<body><p>visible</p></body></html>",
        );
        let tree = parse_html5(text);
        let p = tree
            .root()
            .find("body")
            .and_then(|b| b.descendants().find(|n| n.tag_name() == Some("p")))
            .expect("body paragraph should not be swallowed by <style/>");
        assert_eq!(p.text(), "visible");
    }

    /// A normally-closed `<title>…</title>` still captures its text content as
    /// an RCDATA region, unaffected by the self-closing handling.
    #[test]
    fn html5_normal_title_still_parses_as_rcdata() {
        let text = "<html><head><title>My Book</title></head><body></body></html>";
        let tree = parse_html5(text);
        let title = tree.root().find("title").expect("title should exist");
        assert_eq!(title.text(), "My Book");
    }

    #[test]
    fn html5_entity_decoding() {
        let text = "<p>hello&amp;world</p>";
        let xml = parse_html5(text);
        let p = xml.root().find("p").unwrap();
        assert_eq!(p.text(), "hello&world");
    }

    #[test]
    fn html5_entity_text_offsets_preserve_source_positions() {
        let text = "<p>a&amp;b</p>";
        let xml = parse_html5(text);
        let p = xml.root().find("p").unwrap();
        let text_nodes: Vec<_> = p
            .children()
            .filter(|n| n.tag_name().is_none() && !n.text().is_empty())
            .map(|n| (n.text(), n.offset()))
            .collect();

        assert_eq!(
            text_nodes,
            vec![("a&".to_string(), 3), ("b".to_string(), 9),]
        );
    }

    #[test]
    fn html5_text_after_entity_can_match_entity_name() {
        let text = "<p>a&amp;a</p>";
        let xml = parse_html5(text);
        let p = xml.root().find("p").unwrap();
        let text_nodes: Vec<_> = p
            .children()
            .filter(|n| n.tag_name().is_none() && !n.text().is_empty())
            .map(|n| (n.text(), n.offset()))
            .collect();

        assert_eq!(
            text_nodes,
            vec![("a&".to_string(), 3), ("a".to_string(), 9),]
        );
    }

    #[test]
    fn html5_adjacent_text_without_source_gap_is_coalesced() {
        let text = "<p>abc</p>";
        let xml = parse_html5(text);
        let p = xml.root().find("p").unwrap();
        let text_nodes: Vec<_> = p
            .children()
            .filter(|n| n.tag_name().is_none() && !n.text().is_empty())
            .map(|n| (n.text(), n.offset()))
            .collect();

        assert_eq!(text_nodes, vec![("abc".to_string(), 3)]);
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

    /// Collect `(tag_name, offset)` for every element in the tree, skipping
    /// implicit wrapper tags that html5ever inserts but XmlParser does not
    /// (`html`, `head`, `body`, `anonymous`).
    fn element_offsets(tree: &XmlTree) -> Vec<(String, usize)> {
        tree.root()
            .descendants()
            .filter_map(|n| {
                n.tag_name().and_then(|name| {
                    if matches!(name, "html" | "head" | "body" | "anonymous") {
                        None
                    } else {
                        Some((name.to_string(), n.offset()))
                    }
                })
            })
            .collect()
    }

    /// Collect `(parent_tag, first_text_offset, concatenated_text)` for every
    /// element that has at least one text-node child. The offset is that of
    /// the first text child; the text is all text children concatenated. This
    /// lets us compare text content and positions without being sensitive to
    /// how each parser splits text runs.
    fn text_first_offsets(tree: &XmlTree) -> Vec<(String, usize, String)> {
        tree.root()
            .descendants()
            .filter_map(|n| {
                let tag = n.tag_name()?;
                if matches!(tag, "html" | "head" | "body" | "anonymous") {
                    return None;
                }
                let text_children: Vec<_> = n
                    .children()
                    .filter(|c| c.tag_name().is_none() && !c.text().is_empty())
                    .collect();
                if text_children.is_empty() {
                    return None;
                }
                let first_offset = text_children[0].offset();
                let full_text: String = text_children.iter().map(|c| c.text()).collect();
                Some((tag.to_string(), first_offset, full_text))
            })
            .collect()
    }

    #[test]
    fn parity_simple_element() {
        let src = "<p>hello</p>";
        let xml = element_offsets(&XmlParser::new(src).parse());
        let h5 = element_offsets(&parse_html5(src));
        assert_eq!(xml, h5, "element offsets differ for {:?}", src);
    }

    #[test]
    fn parity_nested_elements() {
        let src = "<div><p>text</p></div>";
        let xml = element_offsets(&XmlParser::new(src).parse());
        let h5 = element_offsets(&parse_html5(src));
        assert_eq!(xml, h5, "element offsets differ for {:?}", src);
    }

    #[test]
    fn parity_adjacent_text_and_element() {
        let src = "<p><em>A</em> B</p>";
        let xml = element_offsets(&XmlParser::new(src).parse());
        let h5 = element_offsets(&parse_html5(src));
        assert_eq!(xml, h5, "element offsets differ for {:?}", src);
    }

    #[test]
    fn parity_text_first_offset_simple() {
        let src = "<p>hello</p>";
        let xml = text_first_offsets(&XmlParser::new(src).parse());
        let h5 = text_first_offsets(&parse_html5(src));
        assert_eq!(xml, h5, "text offsets differ for {:?}", src);
    }

    #[test]
    fn parity_text_first_offset_nested() {
        let src = "<p><em>A</em> B</p>";
        let xml = text_first_offsets(&XmlParser::new(src).parse());
        let h5 = text_first_offsets(&parse_html5(src));
        assert_eq!(xml, h5, "text offsets differ for {:?}", src);
    }

    #[test]
    fn parity_multibyte_utf8() {
        // 'é' encodes as 2 bytes (0xC3 0xA9); offset of "café" must be the
        // byte position of 'c', not the char index.
        let src = "<p>café</p>";
        let xml = text_first_offsets(&XmlParser::new(src).parse());
        let h5 = text_first_offsets(&parse_html5(src));
        assert_eq!(xml, h5, "text offsets differ for {:?}", src);
    }

    #[test]
    fn parity_sequential_siblings() {
        let src = "<h1>Title</h1><p>Body</p>";
        let xml = element_offsets(&XmlParser::new(src).parse());
        let h5 = element_offsets(&parse_html5(src));
        assert_eq!(xml, h5, "element offsets differ for {:?}", src);
    }

    #[test]
    fn parity_sequential_siblings_text() {
        let src = "<h1>Title</h1><p>Body</p>";
        let xml = text_first_offsets(&XmlParser::new(src).parse());
        let h5 = text_first_offsets(&parse_html5(src));
        assert_eq!(xml, h5, "text offsets differ for {:?}", src);
    }

    #[test]
    fn parity_deep_offset_accumulation() {
        let src = "<article><section><div><p>deep text</p></div></section></article>";
        let xml_el = element_offsets(&XmlParser::new(src).parse());
        let h5_el = element_offsets(&parse_html5(src));
        assert_eq!(xml_el, h5_el, "element offsets differ for {:?}", src);

        let xml_tx = text_first_offsets(&XmlParser::new(src).parse());
        let h5_tx = text_first_offsets(&parse_html5(src));
        assert_eq!(xml_tx, h5_tx, "text offsets differ for {:?}", src);
    }
}
