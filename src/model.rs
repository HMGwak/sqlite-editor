use std::collections::{HashMap, HashSet};
use tabular_core::backend::{
    BackendColumn, CellMetadata, OneShotFlags, PersistentFlags, Rgb, TableBackend, VisualColIdx,
    VisualRowIdx,
};
use tabular_core::{CellCoord, ColumnUid, RowUid};
use egui::{Color32, Id, Ui};
use egui_tabular::frontend::TableFrontend;
use egui_tabular::backends::variant::VariantBackend;
use rvariant::{Variant, VariantTy};
use crate::db::TableSchema;

/// Information about a modified cell
#[derive(Debug, Clone, PartialEq)]
pub struct CellChange {
    pub old_value: Variant,
    pub new_value: Variant,
}

/// Dirty tracker to monitor modifications before saving to SQLite
#[derive(Clone, Default)]
pub struct DirtyTracker {
    /// Baseline (initial or last committed) value of cells
    pub baseline_cells: HashMap<CellCoord, Variant>,
    /// Active changes: CellCoord -> (Old, New)
    pub dirty_cells: HashMap<CellCoord, CellChange>,
    /// Newly added rows (not yet committed to DB)
    pub new_rows: HashSet<RowUid>,
    /// Rows marked for deletion
    pub deleted_rows: HashSet<RowUid>,
}

impl DirtyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set baseline snapshot (e.g. after loading from DB or after successful save)
    pub fn set_baseline(&mut self, cells: &HashMap<CellCoord, Variant>, rows: &[RowUid]) {
        self.baseline_cells = cells.clone();
        self.dirty_cells.clear();
        self.new_rows.clear();
        self.deleted_rows.clear();
        let _ = rows;
    }

    /// Record a cell edit
    pub fn record_cell_change(&mut self, coord: CellCoord, new_val: Variant) {
        let baseline = self.baseline_cells.get(&coord).cloned().unwrap_or(Variant::Empty);

        if baseline == new_val {
            self.dirty_cells.remove(&coord);
        } else {
            self.dirty_cells.insert(
                coord,
                CellChange {
                    old_value: baseline,
                    new_value: new_val,
                },
            );
        }
    }

    pub fn record_new_row(&mut self, row_uid: RowUid) {
        self.new_rows.insert(row_uid);
    }

    pub fn record_delete_row(&mut self, row_uid: RowUid) {
        if self.new_rows.contains(&row_uid) {
            self.new_rows.remove(&row_uid);
        } else {
            self.deleted_rows.insert(row_uid);
        }
        self.dirty_cells.retain(|coord, _| coord.row_uid != row_uid);
    }

    pub fn is_dirty(&self) -> bool {
        !self.dirty_cells.is_empty() || !self.new_rows.is_empty() || !self.deleted_rows.is_empty()
    }

    pub fn dirty_cells_count(&self) -> usize {
        self.dirty_cells.len()
    }

    pub fn is_cell_dirty(&self, coord: &CellCoord) -> bool {
        self.dirty_cells.contains_key(coord)
    }

    pub fn is_row_new(&self, row_uid: RowUid) -> bool {
        self.new_rows.contains(&row_uid)
    }

    pub fn is_row_deleted(&self, row_uid: RowUid) -> bool {
        self.deleted_rows.contains(&row_uid)
    }

    pub fn clear(&mut self) {
        self.dirty_cells.clear();
        self.new_rows.clear();
        self.deleted_rows.clear();
    }
}

/// A wrapper around `VariantBackend` that integrates dirty tracking,
/// visual styling for dirty cells (colors and corner triangles),
/// and row management.
pub struct TrackedTableBackend {
    pub inner: VariantBackend,
    pub tracker: DirtyTracker,
    pub column_names: Vec<String>,
    pub schema: Option<TableSchema>,
    pub custom_columns: HashMap<ColumnUid, BackendColumn>,
    pub search_matches: HashSet<CellCoord>,
    pub current_search_focus: Option<CellCoord>,
}

impl TrackedTableBackend {
    pub fn new(columns: Vec<(&'static str, VariantTy, Option<Variant>)>) -> Self {
        let names: Vec<String> = columns.iter().map(|(n, _, _)| n.to_string()).collect();
        let mut custom_columns = HashMap::new();
        for (idx, (n, _, _)) in columns.iter().enumerate() {
            let col_uid = ColumnUid(idx as u32);
            custom_columns.insert(col_uid, BackendColumn {
                name: n.to_string(),
                synonyms: vec![],
                ty: String::new(),
                is_sortable: true,
                is_required: false,
                is_used: true,
                is_skipped: false,
            });
        }

        let inner = VariantBackend::new(columns);
        Self {
            inner,
            tracker: DirtyTracker::new(),
            column_names: names,
            schema: None,
            custom_columns,
            search_matches: HashSet::new(),
            current_search_focus: None,
        }
    }

    /// Construct backend directly from SQLite table schema and rows
    pub fn from_db_table(schema: TableSchema, rows: Vec<Vec<Variant>>) -> Self {
        let mut backend = Self {
            inner: VariantBackend::new(Vec::<(&str, VariantTy, Option<Variant>)>::new()),
            tracker: DirtyTracker::new(),
            column_names: Vec::new(),
            schema: None,
            custom_columns: HashMap::new(),
            search_matches: HashSet::new(),
            current_search_focus: None,
        };
        backend.load_db_table(schema, rows);
        backend
    }

    /// Load or reload data from a SQLite table
    pub fn load_db_table(&mut self, schema: TableSchema, rows: Vec<Vec<Variant>>) {
        self.inner.remove_all_columns();
        self.column_names.clear();
        self.custom_columns.clear();
        self.search_matches.clear();
        self.current_search_focus = None;

        for (idx, col) in schema.columns.iter().enumerate() {
            // Clean column header: [PK] name if primary key, otherwise just name
            let display_name = if col.is_pk {
                format!("[PK] {}", col.name)
            } else {
                col.name.clone()
            };

            // SQLite Data Type & Constraints (Line 2 below column name)
            let type_desc = if col.is_pk {
                if col.not_null {
                    format!("{} • PK • NOT NULL", col.data_type)
                } else {
                    format!("{} • PK", col.data_type)
                }
            } else if col.not_null {
                format!("{} • NOT NULL", col.data_type)
            } else {
                col.data_type.clone()
            };

            self.column_names.push(display_name.clone());
            let col_uid = ColumnUid(idx as u32);

            self.inner.insert_column(
                Some(col_uid),
                display_name.clone(),
                vec![col.name.clone()],
                col.variant_ty.clone(),
                None,
                col.not_null,
                true,
            );

            // BackendColumn with column name on Line 1, and exact SQLite schema on Line 2
            self.custom_columns.insert(
                col_uid,
                BackendColumn {
                    name: display_name,
                    synonyms: vec![col.data_type.clone()],
                    ty: type_desc,
                    is_sortable: true,
                    is_required: col.not_null,
                    is_used: true,
                    is_skipped: false,
                },
            );
        }

        for row_vals in rows {
            let row_entries: Vec<(ColumnUid, Variant)> = row_vals
                .into_iter()
                .enumerate()
                .map(|(col_idx, v)| (ColumnUid(col_idx as u32), v))
                .collect();
            self.inner.insert_row(row_entries);
        }

        self.schema = Some(schema);
        self.snapshot_as_baseline();
    }

    /// Snapshot current inner data as the clean baseline
    pub fn snapshot_as_baseline(&mut self) {
        let mut baseline = HashMap::new();
        for row_idx in 0..self.inner.row_count() {
            if let Some(row_uid) = self.inner.row_uid(VisualRowIdx(row_idx)) {
                for (col_idx, _) in self.column_names.iter().enumerate() {
                    let col_uid = ColumnUid(col_idx as u32);
                    let coord = (row_uid, col_uid).into();
                    if let Some(v) = self.inner.get(coord) {
                        baseline.insert(coord, v.clone());
                    }
                }
            }
        }
        let rows: Vec<RowUid> = (0..self.inner.row_count())
            .filter_map(|i| self.inner.row_uid(VisualRowIdx(i)))
            .collect();
        self.tracker.set_baseline(&baseline, &rows);
        self.update_metadata_indicators();
    }

    /// Revert all dirty changes back to baseline
    pub fn revert_all(&mut self) {
        // Revert cell values
        for (coord, change) in &self.tracker.dirty_cells {
            self.inner.set(*coord, change.old_value.clone());
        }
        // Restore deleted rows
        for row_uid in &self.tracker.deleted_rows {
            self.inner.skip_row(*row_uid, false);
        }
        self.tracker.dirty_cells.clear();
        self.tracker.new_rows.clear();
        self.tracker.deleted_rows.clear();
        self.update_metadata_indicators();
    }

    /// Mark a row as deleted (draws strike-through cross, adds to deleted_rows)
    pub fn mark_row_deleted(&mut self, row_uid: RowUid) {
        self.tracker.record_delete_row(row_uid);
        self.inner.skip_row(row_uid, true);
        self.update_metadata_indicators();
    }

    /// Unmark a deleted row
    pub fn unmark_row_deleted(&mut self, row_uid: RowUid) {
        self.tracker.deleted_rows.remove(&row_uid);
        self.inner.skip_row(row_uid, false);
        self.update_metadata_indicators();
    }

    /// Update cell visual metadata (amber background & red corner for modified cells)
    pub fn update_metadata_indicators(&mut self) {
        self.inner.clear_metadata();

        for coord in self.tracker.dirty_cells.keys() {
            let mut meta = CellMetadata::new()
                .color(Rgb {
                    r: 255,
                    g: 243,
                    b: 205, // Subtle warm amber
                })
                .corner(Rgb {
                    r: 230,
                    g: 81,
                    b: 0, // Deep orange indicator
                });

            if let Some(change) = self.tracker.dirty_cells.get(coord) {
                meta.tooltips.push(std::sync::Arc::new(format!(
                    "원래 값: {}",
                    change.old_value
                )));
            }

            self.inner.set_metadata(*coord, meta, true);
        }
    }

    /// Get column name for ColumnUid
    pub fn col_name(&self, col_uid: ColumnUid) -> &str {
        self.column_names
            .get(col_uid.0 as usize)
            .map(|s| s.as_str())
            .unwrap_or("Unknown")
    }
}

// Forward TableBackend implementation to inner, intercepting mutations to update tracker
impl TableBackend for TrackedTableBackend {
    fn reload(&mut self) {
        self.inner.reload();
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.tracker.clear();
        self.custom_columns.clear();
        self.search_matches.clear();
        self.current_search_focus = None;
    }

    fn sort_by_col(&mut self, col_uid: ColumnUid, ascending: bool) {
        self.inner.sort_by_column(col_uid, ascending);
    }

    fn is_clearable(&self) -> bool {
        self.inner.is_clearable()
    }

    fn commit_all(&mut self) {
        self.inner.commit_all();
    }

    fn commit_immediately(&mut self, enabled: bool) {
        self.inner.commit_immediately(enabled);
    }

    fn persistent_flags(&self) -> &PersistentFlags {
        self.inner.persistent_flags()
    }

    fn one_shot_flags(&self) -> &OneShotFlags {
        self.inner.one_shot_flags()
    }

    fn one_shot_flags_internal(&self) -> &OneShotFlags {
        self.inner.one_shot_flags_internal()
    }

    fn one_shot_flags_archive(&mut self) {
        self.inner.one_shot_flags_archive();
    }

    fn one_shot_flags_internal_mut(&mut self) -> &mut OneShotFlags {
        self.inner.one_shot_flags_internal_mut()
    }

    fn poll(&mut self) {
        self.inner.poll();
    }

    fn available_columns(&self) -> impl Iterator<Item = ColumnUid> {
        self.inner.available_columns()
    }

    fn used_columns(&self) -> impl Iterator<Item = ColumnUid> {
        self.inner.used_columns()
    }

    fn column_info(&self, col_uid: ColumnUid) -> Option<&BackendColumn> {
        self.custom_columns.get(&col_uid).or_else(|| self.inner.column_info(col_uid))
    }

    fn col_uid(&self, col_idx: VisualColIdx) -> Option<ColumnUid> {
        self.inner.col_uid(col_idx)
    }

    fn use_column(&mut self, col_uid: ColumnUid, is_used: bool) {
        self.inner.use_column(col_uid, is_used);
    }

    fn row_count(&self) -> usize {
        self.inner.row_count()
    }

    fn row_uid(&self, row_idx: VisualRowIdx) -> Option<RowUid> {
        self.inner.row_uid(row_idx)
    }

    fn rows(&self) -> impl Iterator<Item = RowUid> {
        self.inner.rows()
    }

    fn un_skipped_rows(&self) -> impl Iterator<Item = RowUid> {
        self.inner.un_skipped_rows()
    }

    fn get(&self, coord: CellCoord) -> Option<&Variant> {
        self.inner.get(coord)
    }

    fn get_as_str(&self, coord: CellCoord) -> Option<&str> {
        self.inner.get_as_str(coord)
    }

    fn get_as_string(&self, coord: CellCoord) -> Option<String> {
        self.inner.get_as_string(coord)
    }

    fn set(&mut self, coord: CellCoord, variant: Variant) {
        self.tracker.record_cell_change(coord, variant.clone());
        self.inner.set(coord, variant);
        self.update_metadata_indicators();
    }

    fn commit_cell_edit(&mut self, coord: CellCoord) {
        self.inner.commit_cell_edit(coord);
        if let Some(val) = self.inner.get(coord) {
            self.tracker.record_cell_change(coord, val.clone());
            self.update_metadata_indicators();
        }
    }

    fn create_row(
        &mut self,
        values: impl IntoIterator<Item = (ColumnUid, Variant)>,
    ) -> Option<RowUid> {
        let r = self.inner.create_row(values);
        if let Some(row_uid) = r {
            self.tracker.record_new_row(row_uid);
        }
        r
    }

    fn create_column(&mut self) -> Option<ColumnUid> {
        self.inner.create_column()
    }

    fn on_highlight_cell(&mut self, coord: CellCoord) {
        self.inner.on_highlight_cell(coord);
    }

    fn column_mapping_choices(&self) -> &[String] {
        self.inner.column_mapping_choices()
    }

    fn skip_row(&mut self, row_uid: RowUid, skipped: bool) {
        if skipped {
            self.tracker.record_delete_row(row_uid);
        } else {
            self.tracker.deleted_rows.remove(&row_uid);
        }
        self.inner.skip_row(row_uid, skipped);
    }

    fn un_skip_all_rows(&mut self) {
        self.tracker.deleted_rows.clear();
        self.inner.un_skip_all_rows();
    }

    fn is_row_skipped(&self, row_uid: RowUid) -> bool {
        self.inner.is_row_skipped(row_uid)
    }

    fn skip_col(&mut self, col_uid: ColumnUid, skipped: bool) {
        self.inner.skip_col(col_uid, skipped);
    }

    fn un_skip_all_columns(&mut self) {
        self.inner.un_skip_all_columns();
    }

    fn is_col_skipped(&self, col_uid: ColumnUid) -> bool {
        self.inner.is_col_skipped(col_uid)
    }

    fn set_metadata(&mut self, coord: CellCoord, meta: CellMetadata, merge: bool) {
        self.inner.set_metadata(coord, meta, merge);
    }

    fn metadata(&self, coord: CellCoord) -> Option<&CellMetadata> {
        self.inner.metadata(coord)
    }
}

// Forward TableFrontend to inner with Excel-style gridlines and cell styling
impl TableFrontend for TrackedTableBackend {
    fn show_cell_view(&mut self, coord: CellCoord, ui: &mut Ui, id: Id) {
        let r = ui.max_rect();

        // Cell padding so text doesn't touch the left border
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            self.inner.show_cell_view(coord, ui, id);
        });

        // Draw Excel-style cell borders (horizontal and vertical gridlines) on top of cell view
        let border_color = Color32::from_rgb(222, 226, 230);
        let stroke = egui::Stroke::new(1.0, border_color);
        let painter = ui.painter();

        // Bottom border
        painter.hline(r.x_range(), r.max.y, stroke);
        // Right border
        painter.vline(r.max.x, r.y_range(), stroke);
    }

    fn show_cell_editor(
        &mut self,
        coord: CellCoord,
        ui: &mut Ui,
        id: Id,
    ) -> Option<egui::Response> {
        self.inner.show_cell_editor(coord, ui, id)
    }

    fn cancel_edit(&mut self) {
        self.inner.cancel_edit();
    }

    fn cell_color(&self, coord: CellCoord) -> Option<Color32> {
        if self.current_search_focus == Some(coord) {
            Some(Color32::from_rgb(255, 213, 79)) // Vibrant amber for focused search match
        } else if self.search_matches.contains(&coord) {
            Some(Color32::from_rgb(255, 249, 196)) // Soft yellow for all search matches
        } else if self.tracker.is_row_deleted(coord.row_uid) {
            Some(Color32::from_rgb(255, 230, 230)) // Soft light red for deleted row
        } else if self.tracker.is_cell_dirty(&coord) {
            Some(Color32::from_rgb(255, 243, 205)) // Warm amber for modified cell
        } else if self.tracker.is_row_new(coord.row_uid) {
            Some(Color32::from_rgb(235, 248, 238)) // Soft light green for newly added row
        } else {
            self.inner.cell_color(coord)
        }
    }

    fn cell_corner(&self, coord: CellCoord) -> Option<Color32> {
        if self.tracker.is_cell_dirty(&coord) {
            Some(Color32::from_rgb(220, 53, 69))
        } else {
            self.inner.cell_corner(coord)
        }
    }

    fn cell_tooltips(&self, coord: CellCoord) -> Vec<&str> {
        self.inner.cell_tooltips(coord)
    }
}
