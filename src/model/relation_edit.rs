use std::collections::{BTreeSet, VecDeque};

use crate::db::mutation::RelationMutationRequest;
use crate::db::mutation::RowVersion;
use crate::db::value::CellValue;
use crate::model::cell_editor::CellEditorBuffer;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EditableRowId(pub u64);

#[derive(Clone, Debug, Default, PartialEq)]
pub enum RelationGridMode {
    #[default]
    Browse,
    EditCell(Box<CellEditorState>),
    VisualLine {
        anchor: usize,
    },
    Busy,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellEditorState {
    pub row: usize,
    pub column: usize,
    pub input: CellEditorBuffer,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditableRowState {
    Clean,
    Updated { changed_columns: BTreeSet<usize> },
    InsertDraft,
    Inserted,
    Deleted,
    Conflict { message: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditableRow {
    pub id: EditableRowId,
    pub original: Vec<CellValue>,
    pub current: Vec<CellValue>,
    pub state: EditableRowState,
    pub supplied_columns: BTreeSet<usize>,
    pub version: Option<RowVersion>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelationMutationHistory {
    pub forward: RelationMutationRequest,
    pub inverse: RelationMutationRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingMutationHistory {
    Undo,
    Redo,
}

impl EditableRow {
    pub fn new(id: EditableRowId, values: Vec<CellValue>) -> Self {
        Self {
            id,
            original: values.clone(),
            current: values,
            state: EditableRowState::Clean,
            supplied_columns: BTreeSet::new(),
            version: None,
        }
    }

    pub fn update_cell(&mut self, column: usize, value: CellValue) -> bool {
        if matches!(self.state, EditableRowState::Deleted) {
            return false;
        }
        let Some(current) = self.current.get_mut(column) else {
            return false;
        };
        *current = value;
        if matches!(
            self.state,
            EditableRowState::InsertDraft | EditableRowState::Inserted
        ) {
            self.supplied_columns.insert(column);
            return true;
        }
        let changed_columns = self
            .current
            .iter()
            .enumerate()
            .filter_map(|(index, value)| (self.original.get(index) != Some(value)).then_some(index))
            .collect::<BTreeSet<_>>();
        self.state = if changed_columns.is_empty() {
            EditableRowState::Clean
        } else {
            EditableRowState::Updated { changed_columns }
        };
        true
    }

    pub fn restore_unprovided(&mut self, column: usize) -> bool {
        if !matches!(self.state, EditableRowState::InsertDraft) {
            return false;
        }
        if self.current.get(column).is_none() || !self.supplied_columns.remove(&column) {
            return false;
        }
        self.current[column] = CellValue::Null;
        true
    }

    pub fn mark_deleted(&mut self) -> bool {
        if matches!(self.state, EditableRowState::Deleted) {
            return false;
        }
        self.state = EditableRowState::Deleted;
        true
    }

    pub fn mark_inserted(&mut self, values: Vec<CellValue>, version: Option<RowVersion>) {
        self.current = values.clone();
        self.original = values;
        self.state = EditableRowState::Inserted;
        self.supplied_columns.clear();
        self.version = version;
    }

    pub fn mark_conflict(&mut self, message: impl Into<String>) {
        self.state = EditableRowState::Conflict {
            message: message.into(),
        };
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RelationEditSession {
    pub mode: RelationGridMode,
    pub rows: Vec<EditableRow>,
    pub yank: Option<Vec<Vec<CellValue>>>,
    pub undo_depth: usize,
    pub redo_depth: usize,
    next_row_id: u64,
    undo: Vec<Vec<EditableRow>>,
    redo: Vec<Vec<EditableRow>>,
    pub mutation_undo: Vec<RelationMutationHistory>,
    pub mutation_redo: Vec<RelationMutationHistory>,
    pub pending_mutation_history: Option<PendingMutationHistory>,
    pub pending_save: VecDeque<RelationMutationRequest>,
    pub save_after_metadata_load: bool,
}

impl RelationEditSession {
    pub fn from_rows(rows: Vec<Vec<CellValue>>) -> Self {
        Self::from_rows_with_versions(rows, None).expect("unversioned rows have matching metadata")
    }

    pub fn from_rows_with_versions(
        rows: Vec<Vec<CellValue>>,
        versions: Option<Vec<RowVersion>>,
    ) -> Result<Self, String> {
        if let Some(versions) = &versions
            && versions.len() != rows.len()
        {
            return Err(format!(
                "row version count {} does not match row count {}",
                versions.len(),
                rows.len()
            ));
        }
        let mut session = Self::default();
        session.rows = rows
            .into_iter()
            .enumerate()
            .map(|(index, values)| {
                let id = session.allocate_id();
                let mut row = EditableRow::new(id, values);
                row.version = versions
                    .as_ref()
                    .and_then(|values| values.get(index).copied());
                row
            })
            .collect();
        Ok(session)
    }

    pub fn allocate_id(&mut self) -> EditableRowId {
        self.next_row_id = self.next_row_id.saturating_add(1);
        EditableRowId(self.next_row_id)
    }

    pub fn visual_range(&self, cursor: usize) -> Option<(usize, usize)> {
        let RelationGridMode::VisualLine { anchor } = self.mode else {
            return None;
        };
        Some((anchor.min(cursor), anchor.max(cursor)))
    }

    pub fn insert_row(&mut self, position: usize, values: Vec<CellValue>) -> EditableRowId {
        self.record_change();
        let id = self.allocate_id();
        let position = position.min(self.rows.len());
        let mut row = EditableRow::new(id, values);
        row.state = EditableRowState::InsertDraft;
        self.rows.insert(position, row);
        id
    }

    pub fn yank_row(&mut self, row: usize) -> bool {
        let Some(row) = self.rows.get(row) else {
            return false;
        };
        self.yank = Some(vec![row.current.clone()]);
        true
    }

    pub fn yank_rows(&mut self, range: std::ops::RangeInclusive<usize>) -> bool {
        let rows = range
            .clone()
            .filter_map(|index| self.rows.get(index))
            .map(|row| row.current.clone())
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return false;
        }
        self.yank = Some(rows);
        true
    }

    pub fn update_cell(&mut self, row: usize, column: usize, value: CellValue) -> bool {
        if self.rows.get(row).is_none() {
            return false;
        }
        self.record_change();
        self.rows[row].update_cell(column, value)
    }

    pub fn restore_unprovided(&mut self, row: usize, column: usize) -> bool {
        let Some(editable_row) = self.rows.get(row) else {
            return false;
        };
        if !matches!(editable_row.state, EditableRowState::InsertDraft)
            || editable_row.current.get(column).is_none()
            || !editable_row.supplied_columns.contains(&column)
        {
            return false;
        }
        self.record_change();
        self.rows[row].restore_unprovided(column)
    }

    pub fn delete_rows(&mut self, range: std::ops::RangeInclusive<usize>) -> bool {
        let mut changed = false;
        self.record_change();
        for index in range.rev() {
            if let Some(row) = self.rows.get_mut(index) {
                if matches!(row.state, EditableRowState::InsertDraft) {
                    self.rows.remove(index);
                    changed = true;
                } else {
                    changed |= row.mark_deleted();
                }
            }
        }
        if !changed {
            self.undo.pop();
        }
        changed
    }

    pub fn paste_rows(&mut self, position: usize) -> bool {
        let Some(rows) = self.yank.clone() else {
            return false;
        };
        if rows.is_empty() {
            return false;
        }
        for (offset, values) in rows.into_iter().enumerate() {
            let id = self.insert_row(position.saturating_add(offset), values);
            if let Some(row) = self.rows.iter_mut().find(|row| row.id == id) {
                row.supplied_columns = (0..row.current.len()).collect();
            }
        }
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.rows.clone());
        self.rows = previous;
        self.sync_history_depth();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.rows.clone());
        self.rows = next;
        self.sync_history_depth();
        true
    }

    pub fn discard_changes(&mut self) {
        self.rows
            .retain(|row| !matches!(row.state, EditableRowState::InsertDraft));
        for row in &mut self.rows {
            row.current = row.original.clone();
            row.state = EditableRowState::Clean;
            row.supplied_columns.clear();
        }
        self.mode = RelationGridMode::Browse;
        self.undo.clear();
        self.redo.clear();
        self.pending_save.clear();
        self.save_after_metadata_load = false;
        self.sync_history_depth();
    }

    pub fn commit_changes(&mut self) {
        self.rows
            .retain(|row| !matches!(row.state, EditableRowState::Deleted));
        for row in &mut self.rows {
            row.original = row.current.clone();
            row.state = EditableRowState::Clean;
            row.supplied_columns.clear();
        }
        self.pending_save.clear();
        self.save_after_metadata_load = false;
        self.undo.clear();
        self.redo.clear();
        self.sync_history_depth();
    }

    pub fn sync_history_depth(&mut self) {
        self.undo_depth = self.undo.len();
        self.redo_depth = self.redo.len();
    }

    pub fn record_mutation(&mut self, history: RelationMutationHistory) {
        self.mutation_undo.push(history);
        self.mutation_redo.clear();
    }

    pub fn pending_mutation(
        &mut self,
        direction: PendingMutationHistory,
    ) -> Option<RelationMutationRequest> {
        let request = match direction {
            PendingMutationHistory::Undo => self.mutation_undo.last()?.inverse.clone(),
            PendingMutationHistory::Redo => self.mutation_redo.last()?.forward.clone(),
        };
        self.pending_mutation_history = Some(direction);
        Some(request)
    }

    pub fn complete_mutation(&mut self) -> bool {
        let Some(direction) = self.pending_mutation_history.take() else {
            return false;
        };
        match direction {
            PendingMutationHistory::Undo => {
                let Some(history) = self.mutation_undo.pop() else {
                    return false;
                };
                self.mutation_redo.push(history);
            }
            PendingMutationHistory::Redo => {
                let Some(history) = self.mutation_redo.pop() else {
                    return false;
                };
                self.mutation_undo.push(history);
            }
        }
        true
    }

    fn record_change(&mut self) {
        self.undo.push(self.rows.clone());
        self.redo.clear();
        self.sync_history_depth();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CellEditorState, EditableRow, EditableRowId, EditableRowState, PendingMutationHistory,
        RelationEditSession, RelationGridMode, RelationMutationHistory, RowVersion,
    };
    use crate::db::value::CellValue;
    use crate::model::cell_editor::CellEditorBuffer;

    fn row() -> EditableRow {
        EditableRow::new(
            EditableRowId(1),
            vec![CellValue::Integer(1), CellValue::Text("old".into())],
        )
    }

    #[test]
    fn versioned_rows_preserve_order_and_reject_mismatched_versions() {
        let rows = vec![vec![CellValue::Integer(1)], vec![CellValue::Integer(2)]];
        let versions = vec![RowVersion::PostgresXmin(11), RowVersion::PostgresXmin(22)];
        let session = RelationEditSession::from_rows_with_versions(rows, Some(versions)).unwrap();
        assert_eq!(session.rows[0].id, EditableRowId(1));
        assert_eq!(session.rows[1].id, EditableRowId(2));
        assert_eq!(session.rows[0].version, Some(RowVersion::PostgresXmin(11)));
        assert_eq!(session.rows[1].version, Some(RowVersion::PostgresXmin(22)));

        let error = RelationEditSession::from_rows_with_versions(
            vec![vec![CellValue::Integer(1)]],
            Some(Vec::new()),
        )
        .unwrap_err();
        assert!(error.contains("version count 0 does not match row count 1"));
    }

    #[test]
    fn row_versions_survive_history_discard_and_snapshot_clone() {
        let mut session = RelationEditSession::from_rows_with_versions(
            vec![vec![CellValue::Integer(1)]],
            Some(vec![RowVersion::PostgresXmin(7)]),
        )
        .unwrap();
        let snapshot = session.clone();
        session.update_cell(0, 0, CellValue::Integer(2));
        assert!(session.undo());
        assert_eq!(session.rows[0].version, snapshot.rows[0].version);
        assert!(session.redo());
        assert_eq!(session.rows[0].version, snapshot.rows[0].version);
        session.discard_changes();
        assert_eq!(session.rows[0].version, Some(RowVersion::PostgresXmin(7)));
        assert_eq!(snapshot.rows[0].version, Some(RowVersion::PostgresXmin(7)));

        let inserted = session.insert_row(1, vec![CellValue::Integer(3)]);
        let inserted_row = session.rows.iter().find(|row| row.id == inserted).unwrap();
        assert_eq!(inserted_row.version, None);
        assert!(session.yank_row(0));
        assert_eq!(session.yank, Some(vec![vec![CellValue::Integer(1)]]));
    }

    #[test]
    fn changing_a_cell_marks_only_changed_columns() {
        let mut row = row();
        assert!(row.update_cell(1, CellValue::Text("new".into())));
        assert_eq!(
            row.state,
            EditableRowState::Updated {
                changed_columns: [1].into_iter().collect()
            }
        );
        assert!(row.update_cell(1, CellValue::Text("old".into())));
        assert_eq!(row.state, EditableRowState::Clean);
    }

    #[test]
    fn visual_line_range_is_inclusive_and_direction_independent() {
        let mut session = RelationEditSession::from_rows(vec![vec![CellValue::Integer(1)]; 5]);
        session.mode = RelationGridMode::VisualLine { anchor: 3 };
        assert_eq!(session.visual_range(1), Some((1, 3)));
        assert_eq!(session.visual_range(4), Some((3, 4)));
    }

    #[test]
    fn yank_rows_captures_a_range_and_paste_restores_every_row() {
        let mut session = RelationEditSession::from_rows(vec![
            vec![CellValue::Integer(1)],
            vec![CellValue::Integer(2)],
            vec![CellValue::Integer(3)],
        ]);
        session.mode = RelationGridMode::VisualLine { anchor: 0 };
        assert!(session.yank_rows(0..=2));
        assert_eq!(
            session.yank,
            Some(vec![
                vec![CellValue::Integer(1)],
                vec![CellValue::Integer(2)],
                vec![CellValue::Integer(3)],
            ])
        );
        assert!(session.paste_rows(3));
        assert_eq!(session.rows.len(), 6);
        assert_eq!(session.rows[3].current, vec![CellValue::Integer(1)]);
        assert_eq!(session.rows[4].current, vec![CellValue::Integer(2)]);
        assert_eq!(session.rows[5].current, vec![CellValue::Integer(3)]);
        for row in &session.rows[3..] {
            assert!(matches!(row.state, EditableRowState::InsertDraft));
            assert_eq!(row.supplied_columns, [0].into_iter().collect());
        }

        assert!(!session.yank_rows(9..=12));
        assert!(session.yank_rows(2..=2));
        assert_eq!(session.yank, Some(vec![vec![CellValue::Integer(3)]]));
    }

    #[test]
    fn inserted_rows_receive_stable_ids_and_yank_is_structured() {
        let mut session = RelationEditSession::from_rows(vec![vec![CellValue::Integer(1)]]);
        let first = session.rows[0].id;
        let inserted = session.insert_row(0, vec![CellValue::Integer(2)]);
        assert_ne!(first, inserted);
        assert!(session.yank_row(0));
        assert_eq!(session.yank, Some(vec![vec![CellValue::Integer(2)]]));
        assert_ne!(session.rows[1].id, session.rows[0].id);
    }

    #[test]
    fn deleted_rows_cannot_be_updated_or_deleted_twice() {
        let mut row = row();
        assert!(row.mark_deleted());
        assert!(!row.mark_deleted());
        assert!(!row.update_cell(1, CellValue::Text("no".into())));
    }

    #[test]
    fn delete_marks_existing_rows_but_removes_uncommitted_inserts() {
        let mut session = RelationEditSession::from_rows(vec![vec![CellValue::Integer(1)]]);
        session.insert_row(1, vec![CellValue::Null]);

        assert!(session.delete_rows(0..=0));
        assert_eq!(session.rows[0].state, EditableRowState::Deleted);
        assert!(session.delete_rows(1..=1));
        assert_eq!(session.rows.len(), 1);
    }

    #[test]
    fn insert_draft_tracks_only_explicitly_edited_columns() {
        let mut session = RelationEditSession::default();
        session.insert_row(0, vec![CellValue::Null, CellValue::Null]);

        assert!(session.update_cell(0, 1, CellValue::Integer(7)));
        assert_eq!(session.rows[0].supplied_columns, [1].into_iter().collect());
    }

    #[test]
    fn insert_draft_tracks_explicit_false_and_null() {
        let mut session = RelationEditSession::default();
        session.insert_row(0, vec![CellValue::Null, CellValue::Null]);

        assert!(session.update_cell(0, 0, CellValue::Boolean(false)));
        assert!(session.update_cell(0, 1, CellValue::Null));
        assert_eq!(
            session.rows[0].supplied_columns,
            [0, 1].into_iter().collect()
        );
    }

    #[test]
    fn restore_unprovided_omits_only_the_selected_insert_column() {
        let mut session = RelationEditSession::default();
        session.insert_row(
            0,
            vec![
                CellValue::Integer(1),
                CellValue::Text("value".into()),
                CellValue::Null,
            ],
        );
        session.update_cell(0, 0, CellValue::Integer(7));
        session.update_cell(0, 1, CellValue::Text("changed".into()));
        let undo_depth = session.undo_depth;

        assert!(session.restore_unprovided(0, 0));
        assert_eq!(session.rows[0].current[0], CellValue::Null);
        assert_eq!(session.rows[0].supplied_columns, [1].into_iter().collect());
        assert_eq!(session.undo_depth, undo_depth + 1);

        assert!(session.undo());
        assert_eq!(session.rows[0].current[0], CellValue::Integer(7));
        assert_eq!(
            session.rows[0].supplied_columns,
            [0, 1].into_iter().collect()
        );
        assert!(session.redo());
        assert_eq!(session.rows[0].current[0], CellValue::Null);
        assert_eq!(session.rows[0].supplied_columns, [1].into_iter().collect());
    }

    #[test]
    fn restore_unprovided_rejects_existing_rows_and_invalid_columns_without_history() {
        let mut session = RelationEditSession::from_rows(vec![vec![CellValue::Null]]);
        let undo_depth = session.undo_depth;
        assert!(!session.restore_unprovided(0, 0));
        assert!(!session.restore_unprovided(0, 1));
        assert_eq!(session.undo_depth, undo_depth);

        session.insert_row(1, vec![CellValue::Null]);
        let undo_depth = session.undo_depth;
        assert!(!session.restore_unprovided(1, 0));
        assert_eq!(session.undo_depth, undo_depth);
        assert!(session.rows[1].supplied_columns.is_empty());
    }

    #[test]
    fn typed_mutation_history_moves_only_after_success() {
        let request = crate::db::mutation::RelationMutationRequest {
            tab_id: uuid::Uuid::nil(),
            tab_generation: 1,
            edit_generation: 1,
            row_id: EditableRowId(1),
            connection: crate::identity::ConnectionIdentity {
                profile_id: uuid::Uuid::nil(),
                generation: 1,
            },
            target: crate::model::execution_target::ExecutionTarget {
                profile_id: uuid::Uuid::nil(),
                database: "db".into(),
                schema: None,
            },
            relation: crate::db::catalog::CatalogId::new(
                uuid::Uuid::nil(),
                crate::db::catalog::CatalogKind::Table,
                ["db", "items"],
            ),
            relation_key: crate::model::relation::RelationKey {
                profile_id: uuid::Uuid::nil(),
                object_id: crate::db::catalog::CatalogId::new(
                    uuid::Uuid::nil(),
                    crate::db::catalog::CatalogKind::Table,
                    ["db", "items"],
                ),
            },
            scope: crate::profile::CatalogScope::for_profile(
                crate::profile::DatabaseKind::Sqlite,
                "db",
                None,
            ),
            metadata: crate::db::mutation::MetadataFingerprint {
                relation: "items".into(),
                columns: vec![("id".into(), "INTEGER".into(), false)],
                primary_key: vec!["id".into()],
            },
            operation: crate::db::mutation::RelationMutation::DeleteRows(Vec::new()),
        };
        let mut session = RelationEditSession::default();
        session.record_mutation(RelationMutationHistory {
            forward: request.clone(),
            inverse: request,
        });
        assert_eq!(session.mutation_undo.len(), 1);
        assert!(
            session
                .pending_mutation(PendingMutationHistory::Undo)
                .is_some()
        );
        assert_eq!(session.mutation_undo.len(), 1);
        assert!(session.complete_mutation());
        assert!(session.mutation_undo.is_empty());
        assert_eq!(session.mutation_redo.len(), 1);
    }

    #[test]
    fn cell_editor_state_can_hold_a_typed_boolean_without_changing_relation_mode() {
        let mut session = RelationEditSession::from_rows(vec![vec![CellValue::Boolean(true)]]);
        session.mode = RelationGridMode::EditCell(Box::new(CellEditorState {
            row: 0,
            column: 0,
            input: CellEditorBuffer {
                presence: crate::model::cell_editor::CellEditorPresence::Value,
                content: crate::model::cell_editor::CellEditorContent::Typed {
                    kind: crate::model::cell_editor::CellEditorKind::Boolean,
                    draft: crate::model::cell_editor::TypedDraft::Boolean(
                        crate::model::text_input::TextInput::from("true"),
                    ),
                },
                ..CellEditorBuffer::default()
            },
            error: None,
        }));
        assert!(matches!(session.mode, RelationGridMode::EditCell(_)));
    }
}
