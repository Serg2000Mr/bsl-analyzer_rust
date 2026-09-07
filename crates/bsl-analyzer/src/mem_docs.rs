use std::sync::Arc;

use anyhow::Result;
use line_index::LineIndex;
use lsp_types::{TextDocumentContentChangeEvent, Url};
use parking_lot::RwLock;
use rustc_hash::FxHashMap;

use crate::lsp::PositionEncoding;

type DocumentMap = FxHashMap<Url, Arc<FrozenDocument>>;

#[derive(Debug, Clone, Default)]
pub struct MemDocs {
    docs: Arc<RwLock<Arc<DocumentMap>>>,
}

#[derive(Debug, Clone)]
pub struct FrozenDocument {
    text: String,
    version: i32,
    line_index: LineIndex,
}

impl FrozenDocument {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn version(&self) -> i32 {
        self.version
    }

    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }
}

#[derive(Debug, Clone, Default)]
pub struct FrozenMemDocs {
    docs: Arc<DocumentMap>,
}

impl FrozenMemDocs {
    pub fn get(&self, uri: &Url) -> Option<&FrozenDocument> {
        self.docs.get(uri).map(Arc::as_ref)
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}

impl MemDocs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, uri: Url, text: String, version: i32) {
        let line_index = LineIndex::new(&text);
        let data = Arc::new(FrozenDocument { text, version, line_index });
        Arc::make_mut(&mut self.docs.write()).insert(uri, data);
    }

    pub fn update(&mut self, uri: &Url, changes: Vec<TextDocumentContentChangeEvent>) {
        if let Err(err) = self.update_with_encoding(uri, changes, PositionEncoding::Utf16) {
            tracing::error!(%uri, error = %err, "failed to apply document changes");
        }
    }

    pub fn update_with_encoding(
        &mut self,
        uri: &Url,
        changes: Vec<TextDocumentContentChangeEvent>,
        encoding: PositionEncoding,
    ) -> Result<()> {
        let mut docs = self.docs.write();

        if let Some(data) = Arc::make_mut(&mut docs).get_mut(uri) {
            // Requests already dispatched must keep their text, version and line index.
            // The map clone shares every other document instead of copying its contents.
            let data = Arc::make_mut(data);
            for change in changes {
                if let Some(range) = change.range {
                    let start = lsp_position_to_offset(
                        &data.line_index,
                        &data.text,
                        range.start,
                        encoding,
                    )?;
                    let end =
                        lsp_position_to_offset(&data.line_index, &data.text, range.end, encoding)?;

                    data.text.replace_range(start..end, &change.text);
                } else {
                    data.text = change.text;
                }

                data.line_index = LineIndex::new(&data.text);
            }
            data.version += 1;
        } else {
            tracing::warn!("Attempted to update non-existent document: {}", uri);
        }

        Ok(())
    }

    pub fn remove(&mut self, uri: &Url) {
        Arc::make_mut(&mut self.docs.write()).remove(uri);
    }

    pub fn get(&self, uri: &Url) -> Option<String> {
        self.docs.read().get(uri).map(|data| data.text.clone())
    }

    pub fn get_version(&self, uri: &Url) -> Option<i32> {
        self.docs.read().get(uri).map(|data| data.version)
    }

    pub fn get_line_index(&self, uri: &Url) -> Option<LineIndex> {
        self.docs.read().get(uri).map(|data| data.line_index.clone())
    }

    pub fn contains(&self, uri: &Url) -> bool {
        self.docs.read().contains_key(uri)
    }

    pub fn len(&self) -> usize {
        self.docs.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.read().is_empty()
    }

    pub fn uris(&self) -> Vec<Url> {
        self.docs.read().keys().cloned().collect()
    }

    /// Share the immutable map; mutations detach it only while a snapshot is alive.
    pub fn freeze(&self) -> FrozenMemDocs {
        FrozenMemDocs { docs: Arc::clone(&self.docs.read()) }
    }
}

/// Byte offset of an LSP position inside the document text.
///
/// The bounds and the character boundary are proven by
/// [`crate::lsp::offset_with_encoding`] — the same conversion the request
/// handlers use, so an edit range and a cursor position can never disagree
/// about what a column means.
fn lsp_position_to_offset(
    line_index: &LineIndex,
    text: &str,
    position: lsp_types::Position,
    encoding: PositionEncoding,
) -> Result<usize> {
    crate::lsp::offset_with_encoding(line_index, text, position, encoding).map(usize::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use line_index::{LineCol, TextSize};

    #[test]
    fn test_insert_and_get() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///test.bsl").unwrap();
        let text = "Процедура Тест()\nКонецПроцедуры".to_string();

        mem_docs.insert(uri.clone(), text.clone(), 1);

        assert_eq!(mem_docs.get(&uri), Some(text));
        assert_eq!(mem_docs.get_version(&uri), Some(1));
        assert!(mem_docs.contains(&uri));
    }

    #[test]
    fn test_update_full() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///test.bsl").unwrap();

        mem_docs.insert(uri.clone(), "old text".to_string(), 1);

        let changes = vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "new text".to_string(),
        }];

        mem_docs.update(&uri, changes);

        assert_eq!(mem_docs.get(&uri), Some("new text".to_string()));
        assert_eq!(mem_docs.get_version(&uri), Some(2));
    }

    #[test]
    fn test_update_incremental() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///test.bsl").unwrap();

        mem_docs.insert(uri.clone(), "hello world".to_string(), 1);

        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range {
                start: lsp_types::Position { line: 0, character: 6 },
                end: lsp_types::Position { line: 0, character: 11 },
            }),
            range_length: Some(5),
            text: "rust".to_string(),
        }];

        mem_docs.update(&uri, changes);

        assert_eq!(mem_docs.get(&uri), Some("hello rust".to_string()));
        assert_eq!(mem_docs.get_version(&uri), Some(2));
    }

    #[test]
    fn test_remove() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///test.bsl").unwrap();

        mem_docs.insert(uri.clone(), "test".to_string(), 1);
        assert!(mem_docs.contains(&uri));

        mem_docs.remove(&uri);
        assert!(!mem_docs.contains(&uri));
        assert_eq!(mem_docs.get(&uri), None);
    }

    #[test]
    fn test_line_index() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///test.bsl").unwrap();
        let text = "line1\nline2\nline3".to_string();

        mem_docs.insert(uri.clone(), text, 1);

        let line_index = mem_docs.get_line_index(&uri).unwrap();

        let pos = line_index.line_col(TextSize::from(6));
        assert_eq!(pos, LineCol { line: 1, col: 0 });
    }

    #[test]
    fn test_update_incremental_cyrillic() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///test.bsl").unwrap();

        mem_docs.insert(uri.clone(), "Процедура Тест()\nКонецПроцедуры".to_string(), 1);

        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range {
                start: lsp_types::Position { line: 0, character: 10 },
                end: lsp_types::Position { line: 0, character: 10 },
            }),
            range_length: Some(0),
            text: "Новая".to_string(),
        }];

        mem_docs.update(&uri, changes);

        assert_eq!(mem_docs.get(&uri), Some("Процедура НоваяТест()\nКонецПроцедуры".to_string()));
        assert_eq!(mem_docs.get_version(&uri), Some(2));
    }

    #[test]
    fn test_update_incremental_replace_cyrillic() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///test.bsl").unwrap();

        mem_docs.insert(uri.clone(), "Функция Старый()\nКонецФункции".to_string(), 1);

        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range {
                start: lsp_types::Position { line: 0, character: 8 },
                end: lsp_types::Position { line: 0, character: 14 },
            }),
            range_length: Some(6),
            text: "Новый".to_string(),
        }];

        mem_docs.update(&uri, changes);

        assert_eq!(mem_docs.get(&uri), Some("Функция Новый()\nКонецФункции".to_string()));
    }

    #[test]
    fn test_update_incremental_cyrillic_utf8_encoding() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///test.bsl").unwrap();

        mem_docs.insert(uri.clone(), "Процедура Тест()\nКонецПроцедуры".to_string(), 1);

        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range {
                start: lsp_types::Position { line: 0, character: 19 },
                end: lsp_types::Position { line: 0, character: 19 },
            }),
            range_length: Some(0),
            text: "Новая".to_string(),
        }];

        mem_docs.update_with_encoding(&uri, changes, PositionEncoding::Utf8).unwrap();

        assert_eq!(mem_docs.get(&uri), Some("Процедура НоваяТест()\nКонецПроцедуры".to_string()));
        assert_eq!(mem_docs.get_version(&uri), Some(2));
    }

    #[test]
    fn test_update_incremental_utf8_rejects_non_char_boundary() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///test.bsl").unwrap();

        mem_docs.insert(uri.clone(), "Функция Старый()\nКонецФункции".to_string(), 1);

        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range {
                start: lsp_types::Position { line: 0, character: 1 },
                end: lsp_types::Position { line: 0, character: 1 },
            }),
            range_length: Some(0),
            text: "X".to_string(),
        }];

        let result = mem_docs.update_with_encoding(&uri, changes, PositionEncoding::Utf8);

        assert!(result.is_err());
        assert_eq!(mem_docs.get(&uri), Some("Функция Старый()\nКонецФункции".to_string()));
        assert_eq!(mem_docs.get_version(&uri), Some(1));
    }

    #[test]
    fn freeze_is_independent_of_mutation() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///test.bsl").unwrap();
        mem_docs.insert(uri.clone(), "original".to_string(), 1);

        let frozen = mem_docs.freeze();

        let changes = vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "mutated".to_string(),
        }];
        mem_docs.update(&uri, changes);

        assert_eq!(frozen.get(&uri).map(|d| d.text()), Some("original"));
        assert_eq!(frozen.get(&uri).map(|d| d.version()), Some(1));
        assert_eq!(mem_docs.get(&uri), Some("mutated".to_string()));
        assert_eq!(mem_docs.get_version(&uri), Some(2));

        mem_docs.remove(&uri);
        assert!(frozen.get(&uri).is_some(), "frozen view must survive removal");
    }

    #[test]
    fn test_update_incremental_multiline_cyrillic() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///test.bsl").unwrap();

        mem_docs.insert(uri.clone(), "Процедура\nТест()\nКонецПроцедуры".to_string(), 1);

        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range {
                start: lsp_types::Position { line: 0, character: 9 },
                end: lsp_types::Position { line: 1, character: 0 },
            }),
            range_length: Some(1),
            text: " ".to_string(),
        }];

        mem_docs.update(&uri, changes);

        assert_eq!(mem_docs.get(&uri), Some("Процедура Тест()\nКонецПроцедуры".to_string()));
    }

    #[test]
    fn repeated_freezes_share_the_map_and_document_storage() {
        let mut mem_docs = MemDocs::new();
        let uri = Url::parse("file:///large.bsl").unwrap();
        mem_docs.insert(uri.clone(), "Процедура Тест()\nКонецПроцедуры\n".repeat(1024), 1);

        let first = mem_docs.freeze();
        let second = mem_docs.freeze();

        assert!(Arc::ptr_eq(&first.docs, &second.docs));
        assert!(Arc::ptr_eq(&first.docs[&uri], &second.docs[&uri]));
        assert_eq!(first.get(&uri).unwrap().text().as_ptr(), second.get(&uri).unwrap().text().as_ptr());
    }

    #[test]
    fn editing_detaches_only_the_changed_document() {
        let mut mem_docs = MemDocs::new();
        let edited = Url::parse("file:///edited.bsl").unwrap();
        let untouched = Url::parse("file:///untouched.bsl").unwrap();
        mem_docs.insert(edited.clone(), "old\ntext".to_owned(), 1);
        mem_docs.insert(untouched.clone(), "unchanged".to_owned(), 7);
        let before = mem_docs.freeze();

        mem_docs.update(
            &edited,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "new text".to_owned(),
            }],
        );
        let after = mem_docs.freeze();

        assert!(!Arc::ptr_eq(&before.docs, &after.docs));
        assert!(!Arc::ptr_eq(&before.docs[&edited], &after.docs[&edited]));
        assert!(Arc::ptr_eq(&before.docs[&untouched], &after.docs[&untouched]));
        assert_eq!(before.get(&edited).unwrap().text(), "old\ntext");
        assert_eq!(before.get(&edited).unwrap().version(), 1);
        assert_eq!(after.get(&edited).unwrap().text(), "new text");
        assert_eq!(after.get(&edited).unwrap().version(), 2);
        let offset = TextSize::of("old\n");
        assert_eq!(before.get(&edited).unwrap().line_index().line_col(offset).line, 1);
        assert_eq!(after.get(&edited).unwrap().line_index().line_col(offset).line, 0);
    }

    #[test]
    fn insert_and_remove_preserve_earlier_snapshots() {
        let mut mem_docs = MemDocs::new();
        let first_uri = Url::parse("file:///first.bsl").unwrap();
        let second_uri = Url::parse("file:///second.bsl").unwrap();
        mem_docs.insert(first_uri.clone(), "first".to_owned(), 1);
        let first = mem_docs.freeze();

        mem_docs.insert(second_uri.clone(), "second".to_owned(), 2);
        let both = mem_docs.freeze();
        mem_docs.remove(&first_uri);
        let last = mem_docs.freeze();

        assert_eq!(first.len(), 1);
        assert!(first.get(&second_uri).is_none());
        assert_eq!(both.len(), 2);
        assert!(last.get(&first_uri).is_none());
        assert_eq!(last.len(), 1);
        assert!(Arc::ptr_eq(&first.docs[&first_uri], &both.docs[&first_uri]));
        assert!(Arc::ptr_eq(&both.docs[&second_uri], &last.docs[&second_uri]));
    }

    #[test]
    fn cloned_live_handles_see_changes_but_frozen_handles_do_not() {
        let mut live = MemDocs::new();
        let uri = Url::parse("file:///shared.bsl").unwrap();
        live.insert(uri.clone(), "original".to_owned(), 1);
        let mut other_live = live.clone();
        let frozen = live.freeze();

        other_live.insert(uri.clone(), "replacement".to_owned(), 2);

        assert_eq!(live.get(&uri).as_deref(), Some("replacement"));
        assert_eq!(live.get_version(&uri), Some(2));
        assert_eq!(frozen.get(&uri).unwrap().text(), "original");
        assert_eq!(frozen.get(&uri).unwrap().version(), 1);
        assert!(Arc::ptr_eq(&live.freeze().docs, &other_live.freeze().docs));
    }
}
