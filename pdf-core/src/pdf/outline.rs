//! Document outline (bookmarks): read `/Root /Outlines` as a nested tree and
//! rewrite it wholesale from a client-supplied tree.
//!
//! Writing replaces the whole outline rather than patching single items. A
//! bookmark's position is expressed by three sibling pointers plus a parent
//! (`/Prev`, `/Next`, `/First`, `/Last`, `/Parent`) and an ancestor-visible
//! `/Count`; an incremental "move this item under that one" would have to
//! repair all of them on both the old and the new path. The panel edits a
//! tree and saves a tree, so rebuild is both simpler and always consistent.
//!
//! Reading has to cope with files we did not write: destinations may be an
//! explicit array, a named destination in `/Root /Dests`, or a name-tree
//! entry under `/Root /Names /Dests`, and may hang off `/Dest` or off a
//! `/GoTo` action in `/A`. Anything unresolvable comes back with
//! `page: None` instead of being dropped — a bookmark the viewer shows but
//! cannot follow is still one the user needs to see and fix.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use lopdf::{dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};

use super::docutil::{
    catalog_id, decode_text_string, page_ids, pdf_text_string, resolve_array, resolve_dict,
    save_atomic,
};
use super::protect;

/// Depth cap for both reading and writing. Real outlines are a handful of
/// levels; anything deeper is a malformed or hostile file.
const MAX_DEPTH: usize = 32;
/// Total item cap for a written tree, to bound the work a single request can
/// ask for.
const MAX_NODES: usize = 5000;

#[derive(Serialize)]
pub struct OutlineItem {
    pub title: String,
    /// 0-based target page, or `None` when the destination could not be
    /// resolved (remote/embedded target, or a dangling named destination).
    pub page: Option<u16>,
    /// True when the item is stored expanded (`/Count` > 0).
    pub open: bool,
    pub children: Vec<OutlineItem>,
}

#[derive(Deserialize)]
pub struct NewOutlineItem {
    pub title: String,
    pub page: u16,
    #[serde(default)]
    pub open: bool,
    #[serde(default)]
    pub children: Vec<NewOutlineItem>,
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

pub fn list(path: &Path) -> anyhow::Result<Vec<OutlineItem>> {
    let doc = Document::load(path)?;
    list_in(&doc)
}

fn list_in(doc: &Document) -> anyhow::Result<Vec<OutlineItem>> {
    let catalog = catalog_id(doc)?;
    let Some(outlines) = doc
        .get_dictionary(catalog)
        .ok()
        .and_then(|d| d.get(b"Outlines").ok().cloned())
    else {
        return Ok(Vec::new());
    };
    let Some(root) = resolve_dict(doc, &outlines) else {
        return Ok(Vec::new());
    };
    let Ok(first) = root.get(b"First").and_then(|o| o.as_reference()) else {
        return Ok(Vec::new());
    };

    let index_of = page_index_map(doc);
    let mut seen = HashSet::new();
    Ok(read_siblings(doc, first, &index_of, &mut seen, 0))
}

/// Walk a sibling chain via `/Next`, recursing into `/First`. `seen` guards
/// against `/Next` or `/First` cycles in a malformed file.
fn read_siblings(
    doc: &Document,
    first: ObjectId,
    index_of: &HashMap<ObjectId, u16>,
    seen: &mut HashSet<ObjectId>,
    depth: usize,
) -> Vec<OutlineItem> {
    let mut out = Vec::new();
    if depth >= MAX_DEPTH {
        return out;
    }
    let mut current = Some(first);
    while let Some(id) = current {
        if !seen.insert(id) {
            break;
        }
        let Ok(dict) = doc.get_dictionary(id) else {
            break;
        };
        let title = dict
            .get(b"Title")
            .ok()
            .and_then(|o| decode_text_string(doc, o))
            .unwrap_or_default();
        let count = dict.get(b"Count").ok().and_then(|o| o.as_i64().ok());
        let children = match dict.get(b"First").and_then(|o| o.as_reference()) {
            Ok(child) => read_siblings(doc, child, index_of, seen, depth + 1),
            Err(_) => Vec::new(),
        };
        out.push(OutlineItem {
            title,
            page: item_page(doc, dict, index_of),
            open: count.is_some_and(|c| c > 0),
            children,
        });
        current = dict.get(b"Next").ok().and_then(|o| o.as_reference().ok());
    }
    out
}

/// Resolve an outline item's target page from `/Dest` or from a `/GoTo`
/// action in `/A`.
fn item_page(
    doc: &Document,
    dict: &lopdf::Dictionary,
    index_of: &HashMap<ObjectId, u16>,
) -> Option<u16> {
    if let Ok(dest) = dict.get(b"Dest") {
        if let Some(page) = dest_page(doc, dest, index_of) {
            return Some(page);
        }
    }
    let action = resolve_dict(doc, dict.get(b"A").ok()?)?;
    let is_goto = action
        .get(b"S")
        .and_then(|o| o.as_name())
        .map(|n| n == b"GoTo")
        .unwrap_or(false);
    if !is_goto {
        return None;
    }
    dest_page(doc, action.get(b"D").ok()?, index_of)
}

/// A destination is either an explicit `[page /XYZ …]` array or a name that
/// has to be looked up in the document's destination tables.
pub(crate) fn dest_page(
    doc: &Document,
    dest: &Object,
    index_of: &HashMap<ObjectId, u16>,
) -> Option<u16> {
    let dest = match dest {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        other => other,
    };
    match dest {
        Object::Array(_) => {
            let arr = resolve_array(doc, dest)?;
            let page_ref = arr.first()?.as_reference().ok()?;
            index_of.get(&page_ref).copied()
        }
        Object::Name(name) => {
            let target = lookup_named_dest(doc, name)?;
            dest_page(doc, &target, index_of)
        }
        Object::String(bytes, _) => {
            let target = lookup_named_dest(doc, bytes)?;
            dest_page(doc, &target, index_of)
        }
        _ => None,
    }
}

/// Look `key` up in `/Root /Dests` (PDF 1.1 dictionary) and then in the
/// `/Root /Names /Dests` name tree. The value may be the destination array
/// itself or a dictionary wrapping it in `/D`.
fn lookup_named_dest(doc: &Document, key: &[u8]) -> Option<Object> {
    let catalog = doc.get_dictionary(catalog_id(doc).ok()?).ok()?;

    let unwrap_d = |obj: Object| -> Object {
        match resolve_dict(doc, &obj) {
            Some(d) => d.get(b"D").ok().cloned().unwrap_or(obj),
            None => obj,
        }
    };

    if let Some(dests) = catalog.get(b"Dests").ok().and_then(|o| resolve_dict(doc, o)) {
        if let Ok(hit) = dests.get(key) {
            return Some(unwrap_d(hit.clone()));
        }
    }

    let names = catalog.get(b"Names").ok().and_then(|o| resolve_dict(doc, o))?;
    let tree = names.get(b"Dests").ok().and_then(|o| resolve_dict(doc, o))?;
    search_name_tree(doc, &tree, key, 0).map(unwrap_d)
}

/// Depth-first search of a PDF name tree (PDF 32000-1 §7.9.6). Keys are
/// sorted, but the ranges in `/Limits` are only advisory here — a linear
/// scan over a bookmark-sized tree is not worth the extra failure mode.
fn search_name_tree(
    doc: &Document,
    node: &lopdf::Dictionary,
    key: &[u8],
    depth: usize,
) -> Option<Object> {
    if depth >= MAX_DEPTH {
        return None;
    }
    if let Some(names) = node.get(b"Names").ok().and_then(|o| resolve_array(doc, o)) {
        for pair in names.chunks_exact(2) {
            let matches = match &pair[0] {
                Object::String(bytes, _) => bytes.as_slice() == key,
                _ => false,
            };
            if matches {
                return Some(pair[1].clone());
            }
        }
    }
    let kids = node.get(b"Kids").ok().and_then(|o| resolve_array(doc, o))?;
    for kid in kids {
        let kid_dict = resolve_dict(doc, &kid)?;
        if let Some(hit) = search_name_tree(doc, &kid_dict, key, depth + 1) {
            return Some(hit);
        }
    }
    None
}

fn page_index_map(doc: &Document) -> HashMap<ObjectId, u16> {
    page_ids(doc)
        .into_iter()
        .enumerate()
        .map(|(i, id)| (id, i as u16))
        .collect()
}

// ---------------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------------

/// Replace the document's outline with `items`. An empty list drops the
/// outline entirely.
pub fn set(path: &Path, items: &[NewOutlineItem]) -> anyhow::Result<()> {
    protect::assert_editable(path)?;
    let mut doc = Document::load(path)?;

    validate(items)?;
    let pages = page_ids(&doc);
    check_pages(items, pages.len())?;

    let catalog = catalog_id(&doc)?;
    remove_existing(&mut doc, catalog)?;

    if items.is_empty() {
        let _ = doc.get_dictionary_mut(catalog)?.remove(b"Outlines");
        return save_atomic(&mut doc, path);
    }

    // The root has to exist before the items so they can point /Parent at it.
    let root_id = doc.add_object(dictionary! { "Type" => "Outlines" });
    let built = build_siblings(&mut doc, root_id, items, &pages);

    let root = doc.get_dictionary_mut(root_id)?;
    if let Some((first, last)) = built.ends {
        root.set("First", Object::Reference(first));
        root.set("Last", Object::Reference(last));
    }
    root.set("Count", built.visible as i64);
    doc.get_dictionary_mut(catalog)?
        .set("Outlines", Object::Reference(root_id));

    save_atomic(&mut doc, path)
}

struct Built {
    ends: Option<(ObjectId, ObjectId)>,
    /// Items this level contributes to an ancestor's `/Count`: every sibling,
    /// plus the visible descendants of the ones stored open.
    visible: usize,
}

fn build_siblings(
    doc: &mut Document,
    parent: ObjectId,
    items: &[NewOutlineItem],
    pages: &[ObjectId],
) -> Built {
    // Reserve every id up front so /Prev and /Next can be filled in one pass.
    let ids: Vec<ObjectId> = items.iter().map(|_| doc.new_object_id()).collect();
    let mut visible = ids.len();

    for (i, item) in items.iter().enumerate() {
        let id = ids[i];
        let child = build_siblings(doc, id, &item.children, pages);
        let has_children = child.ends.is_some();
        if item.open && has_children {
            visible += child.visible;
        }

        let mut dict = dictionary! {
            "Title" => pdf_text_string(&item.title),
            "Parent" => Object::Reference(parent),
            "Dest" => Object::Array(vec![
                Object::Reference(pages[item.page as usize]),
                "XYZ".into(),
                Object::Null,
                Object::Null,
                Object::Null,
            ]),
        };
        if i > 0 {
            dict.set("Prev", Object::Reference(ids[i - 1]));
        }
        if i + 1 < ids.len() {
            dict.set("Next", Object::Reference(ids[i + 1]));
        }
        if let Some((first, last)) = child.ends {
            dict.set("First", Object::Reference(first));
            dict.set("Last", Object::Reference(last));
            // Sign carries the open/closed state; magnitude is the number of
            // rows the item would add to the panel when expanded.
            let count = child.visible as i64;
            dict.set("Count", if item.open { count } else { -count });
        }
        doc.objects.insert(id, Object::Dictionary(dict));
    }

    Built {
        ends: match (ids.first(), ids.last()) {
            (Some(first), Some(last)) => Some((*first, *last)),
            _ => None,
        },
        visible,
    }
}

/// Drop the objects of the current outline so a rewrite does not leave the
/// old items behind as unreferenced bloat.
fn remove_existing(doc: &mut Document, catalog: ObjectId) -> anyhow::Result<()> {
    let Some(outlines) = doc
        .get_dictionary(catalog)
        .ok()
        .and_then(|d| d.get(b"Outlines").ok().cloned())
    else {
        return Ok(());
    };
    let mut doomed = HashSet::new();
    if let Object::Reference(id) = outlines {
        doomed.insert(id);
    }
    if let Some(root) = resolve_dict(doc, &outlines) {
        if let Ok(first) = root.get(b"First").and_then(|o| o.as_reference()) {
            collect_items(doc, first, &mut doomed, 0);
        }
    }
    for id in doomed {
        doc.objects.remove(&id);
    }
    Ok(())
}

fn collect_items(doc: &Document, first: ObjectId, out: &mut HashSet<ObjectId>, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    let mut current = Some(first);
    while let Some(id) = current {
        if !out.insert(id) {
            break; // cycle
        }
        let Ok(dict) = doc.get_dictionary(id) else {
            break;
        };
        if let Ok(child) = dict.get(b"First").and_then(|o| o.as_reference()) {
            collect_items(doc, child, out, depth + 1);
        }
        current = dict.get(b"Next").ok().and_then(|o| o.as_reference().ok());
    }
}

fn validate(items: &[NewOutlineItem]) -> anyhow::Result<()> {
    let mut total = 0usize;
    walk(items, 0, &mut total)?;
    return Ok(());

    fn walk(items: &[NewOutlineItem], depth: usize, total: &mut usize) -> anyhow::Result<()> {
        if depth >= MAX_DEPTH {
            anyhow::bail!("bookmark tree is nested deeper than {MAX_DEPTH} levels");
        }
        for item in items {
            *total += 1;
            if *total > MAX_NODES {
                anyhow::bail!("bookmark tree has more than {MAX_NODES} items");
            }
            if item.title.trim().is_empty() {
                anyhow::bail!("bookmark title must not be empty");
            }
            walk(&item.children, depth + 1, total)?;
        }
        Ok(())
    }
}

fn check_pages(items: &[NewOutlineItem], page_count: usize) -> anyhow::Result<()> {
    for item in items {
        if item.page as usize >= page_count {
            anyhow::bail!("bookmark page {} out of range", item.page);
        }
        check_pages(&item.children, page_count)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::docutil::test_support::temp_pdf;

    fn node(title: &str, page: u16, open: bool, children: Vec<NewOutlineItem>) -> NewOutlineItem {
        NewOutlineItem {
            title: title.into(),
            page,
            open,
            children,
        }
    }

    #[test]
    fn round_trips_a_nested_tree() {
        let path = temp_pdf(3);
        let items = vec![
            node("第一章", 0, true, vec![node("1.1 前言", 1, false, vec![])]),
            node("第二章", 2, false, vec![]),
        ];
        set(&path, &items).unwrap();

        let got = list(&path).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].title, "第一章");
        assert_eq!(got[0].page, Some(0));
        assert!(got[0].open);
        assert_eq!(got[0].children.len(), 1);
        assert_eq!(got[0].children[0].title, "1.1 前言");
        assert_eq!(got[0].children[0].page, Some(1));
        assert_eq!(got[1].title, "第二章");
        assert_eq!(got[1].page, Some(2));
        std::fs::remove_file(&path).ok();
    }

    /// `/Count` drives the viewer's expand state and its magnitude counts
    /// visible descendants, so an open parent and a closed one differ by more
    /// than a sign.
    #[test]
    fn count_encodes_open_state_and_visible_rows() {
        let path = temp_pdf(2);
        let items = vec![
            node(
                "open",
                0,
                true,
                vec![node("kid", 1, false, vec![node("grandkid", 1, false, vec![])])],
            ),
            node("closed", 1, false, vec![node("hidden", 1, false, vec![])]),
        ];
        set(&path, &items).unwrap();

        let doc = Document::load(&path).unwrap();
        let catalog = catalog_id(&doc).unwrap();
        let root_ref = doc
            .get_dictionary(catalog)
            .unwrap()
            .get(b"Outlines")
            .unwrap()
            .clone();
        let root = resolve_dict(&doc, &root_ref).unwrap();
        // 2 top-level rows + the one row "open" reveals.
        assert_eq!(root.get(b"Count").unwrap().as_i64().unwrap(), 3);

        let first = root.get(b"First").unwrap().as_reference().unwrap();
        let open_item = doc.get_dictionary(first).unwrap();
        assert_eq!(open_item.get(b"Count").unwrap().as_i64().unwrap(), 1);
        let next = open_item.get(b"Next").unwrap().as_reference().unwrap();
        let closed_item = doc.get_dictionary(next).unwrap();
        assert_eq!(closed_item.get(b"Count").unwrap().as_i64().unwrap(), -1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn setting_an_empty_tree_drops_the_outline() {
        let path = temp_pdf(1);
        set(&path, &[node("gone", 0, false, vec![])]).unwrap();
        assert_eq!(list(&path).unwrap().len(), 1);

        set(&path, &[]).unwrap();
        assert!(list(&path).unwrap().is_empty());
        let doc = Document::load(&path).unwrap();
        let catalog = catalog_id(&doc).unwrap();
        assert!(doc.get_dictionary(catalog).unwrap().get(b"Outlines").is_err());
        std::fs::remove_file(&path).ok();
    }

    /// A rewrite must not leave the previous items in the file as orphans.
    #[test]
    fn rewrite_removes_the_old_items() {
        let path = temp_pdf(1);
        set(&path, &[node("old", 0, true, vec![node("old kid", 0, false, vec![])])]).unwrap();
        let before = Document::load(&path).unwrap().objects.len();

        set(&path, &[node("new", 0, false, vec![])]).unwrap();
        let after = Document::load(&path).unwrap().objects.len();
        assert!(
            after < before,
            "expected the two old items to be dropped (before={before}, after={after})"
        );
        assert_eq!(list(&path).unwrap().len(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_out_of_range_page_and_empty_title() {
        let path = temp_pdf(1);
        let err = set(&path, &[node("x", 5, false, vec![])]).unwrap_err().to_string();
        assert!(err.contains("out of range"), "{err}");
        let err = set(&path, &[node("   ", 0, false, vec![])]).unwrap_err().to_string();
        assert!(err.contains("bookmark title"), "{err}");
        std::fs::remove_file(&path).ok();
    }

    /// Files from other tools point `/Dest` at a name and keep the real
    /// destination in the `/Names /Dests` tree.
    #[test]
    fn resolves_a_named_destination_through_the_name_tree() {
        let path = temp_pdf(2);
        let mut doc = Document::load(&path).unwrap();
        let pages = page_ids(&doc);
        let catalog = catalog_id(&doc).unwrap();

        let dest = Object::Array(vec![Object::Reference(pages[1]), "Fit".into()]);
        let leaf = doc.add_object(dictionary! {
            "Names" => vec![Object::string_literal("ch2"), dest],
        });
        let tree = doc.add_object(dictionary! { "Kids" => vec![Object::Reference(leaf)] });
        let names = doc.add_object(dictionary! { "Dests" => Object::Reference(tree) });
        let item = doc.add_object(dictionary! {
            "Title" => Object::string_literal("Chapter 2"),
            "Dest" => Object::String(b"ch2".to_vec(), lopdf::StringFormat::Literal),
        });
        let outlines = doc.add_object(dictionary! {
            "Type" => "Outlines",
            "First" => Object::Reference(item),
            "Last" => Object::Reference(item),
            "Count" => 1,
        });
        let root = doc.get_dictionary_mut(catalog).unwrap();
        root.set("Names", Object::Reference(names));
        root.set("Outlines", Object::Reference(outlines));
        doc.save(&path).unwrap();

        let got = list(&path).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].page, Some(1));
        std::fs::remove_file(&path).ok();
    }

    /// A destination we cannot follow must still surface as a bookmark.
    #[test]
    fn keeps_items_whose_destination_is_unresolvable() {
        let path = temp_pdf(1);
        let mut doc = Document::load(&path).unwrap();
        let catalog = catalog_id(&doc).unwrap();
        let item = doc.add_object(dictionary! {
            "Title" => Object::string_literal("dangling"),
            "Dest" => Object::String(b"nope".to_vec(), lopdf::StringFormat::Literal),
        });
        let outlines = doc.add_object(dictionary! {
            "Type" => "Outlines",
            "First" => Object::Reference(item),
            "Last" => Object::Reference(item),
        });
        doc.get_dictionary_mut(catalog)
            .unwrap()
            .set("Outlines", Object::Reference(outlines));
        doc.save(&path).unwrap();

        let got = list(&path).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "dangling");
        assert_eq!(got[0].page, None);
        std::fs::remove_file(&path).ok();
    }

    /// A `/Next` cycle in a hostile file must not hang the reader.
    #[test]
    fn survives_a_cyclic_sibling_chain() {
        let path = temp_pdf(1);
        let mut doc = Document::load(&path).unwrap();
        let catalog = catalog_id(&doc).unwrap();
        let a = doc.new_object_id();
        let b = doc.new_object_id();
        doc.objects.insert(
            a,
            Object::Dictionary(dictionary! {
                "Title" => Object::string_literal("a"),
                "Next" => Object::Reference(b),
            }),
        );
        doc.objects.insert(
            b,
            Object::Dictionary(dictionary! {
                "Title" => Object::string_literal("b"),
                "Next" => Object::Reference(a),
            }),
        );
        let outlines = doc.add_object(dictionary! {
            "Type" => "Outlines",
            "First" => Object::Reference(a),
        });
        doc.get_dictionary_mut(catalog)
            .unwrap()
            .set("Outlines", Object::Reference(outlines));
        doc.save(&path).unwrap();

        assert_eq!(list(&path).unwrap().len(), 2);
        std::fs::remove_file(&path).ok();
    }
}
