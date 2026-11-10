use crate::clipboard_util::{apply_paste_append, apply_paste_overwrite, export_to_tsv, parse_tsv};
use crate::db::{create_sample_database, DbService, TableSummary};
use crate::model::TrackedTableBackend;
use arboard::Clipboard;
use egui::{CentralPanel, Color32, Id, Key, Panel, RichText, ScrollArea};
use egui_tabular::table_view::TableViewConfig;
use egui_tabular::{TableBackend, TableView};
use rvariant::{Variant, VariantTy};
use std::path::PathBuf;
use std::time::Instant;
use tabular_core::backend::{VisualColIdx, VisualRowIdx};
use tabular_core::{CellCoord, ColumnUid};

pub struct QueryWindowState {
    pub is_open: bool,
    pub sql_text: String,
    pub result_table: Option<TrackedTableBackend>,
    pub result_view: TableView,
    pub result_config: TableViewConfig,
    pub status_message: Option<String>,
    pub is_error: bool,
    pub execution_dur: Option<std::time::Duration>,
}

impl Default for QueryWindowState {
    fn default() -> Self {
        Self {
            is_open: false,
            sql_text: "SELECT * FROM customers LIMIT 50;".to_string(),
            result_table: None,
            result_view: TableView::new(),
            result_config: TableViewConfig::default(),
            status_message: None,
            is_error: false,
            execution_dur: None,
        }
    }
}

pub struct SqliteEditorApp {
    pub db_service: DbService,
    pub table_summaries: Vec<TableSummary>,
    pub selected_table: Option<String>,

    pub table: TrackedTableBackend,
    pub table_view: TableView,
    pub table_config: TableViewConfig,
    pub status_message: String,
    pub last_action_time: Option<Instant>,

    pub target_row_idx: usize,
    pub pending_table_switch: Option<String>,
    pub show_dirty_switch_dialog: bool,
    pub show_help_modal: bool,

    // Find & Replace state
    pub show_search_bar: bool,
    pub search_replace_mode: bool,
    pub search_query: String,
    pub replace_query: String,
    pub search_matches: Vec<CellCoord>,
    pub search_match_idx: usize,
    pub search_case_sensitive: bool,

    // Dedicated Access-style SQL Query Window
    pub query_window: QueryWindowState,

    // Smart Paste / Win+V Clipboard History Support Modal
    pub show_smart_paste_modal: bool,
    pub smart_paste_text: String,

    // Database Lock / Read-Only Mode
    pub is_locked: bool,
}

pub fn setup_korean_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // 1. Try Windows Malgun Gothic from disk
    let mut malgun_loaded = false;
    let windows_font_paths = [
        "C:\\Windows\\Fonts\\malgun.ttf",
        "C:\\Windows\\Fonts\\malgunbd.ttf",
    ];
    for path in windows_font_paths {
        if let Ok(data) = std::fs::read(path) {
            fonts.font_data.insert(
                "malgun".to_string(),
                std::sync::Arc::new(egui::FontData::from_owned(data)),
            );
            malgun_loaded = true;
            break;
        }
    }

    // 2. Embedded NanumGothic as universal reliable fallback
    const NANUM_GOTHIC: &[u8] = include_bytes!("../assets/NanumGothic-Regular.ttf");
    fonts.font_data.insert(
        "nanum_gothic".to_string(),
        std::sync::Arc::new(egui::FontData::from_static(NANUM_GOTHIC)),
    );

    // 3. Add Phosphor Icons
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    // 4. Put Korean font at the top priority of proportional and monospace font families
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        if malgun_loaded {
            family.insert(0, "malgun".to_string());
        }
        family.insert(if malgun_loaded { 1 } else { 0 }, "nanum_gothic".to_string());
    }
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        if malgun_loaded {
            family.insert(0, "malgun".to_string());
        }
        family.insert(if malgun_loaded { 1 } else { 0 }, "nanum_gothic".to_string());
    }

    ctx.set_fonts(fonts);
}

fn case_insensitive_replace(text: &str, query: &str, replacement: &str) -> String {
    let lower_text = text.to_lowercase();
    let lower_query = query.to_lowercase();
    if lower_query.is_empty() {
        return text.to_string();
    }
    let mut result = String::new();
    let mut last_idx = 0;
    while let Some(idx) = lower_text[last_idx..].find(&lower_query) {
        let abs_idx = last_idx + idx;
        result.push_str(&text[last_idx..abs_idx]);
        result.push_str(replacement);
        last_idx = abs_idx + query.len();
    }
    result.push_str(&text[last_idx..]);
    result
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn ribbon_button(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    tooltip: &str,
    highlight_color: Option<Color32>,
    bg_override: Option<Color32>,
) -> egui::Response {
    let desired_size = egui::vec2(58.0, 46.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        let bg_fill = if let Some(bg) = bg_override {
            if response.hovered() {
                bg
            } else {
                bg.gamma_multiply(0.6)
            }
        } else if response.hovered() {
            ui.visuals().widgets.hovered.bg_fill
        } else {
            Color32::TRANSPARENT
        };

        let stroke = if let Some(c) = highlight_color {
            egui::Stroke::new(1.0, c)
        } else if response.hovered() {
            ui.visuals().widgets.hovered.bg_stroke
        } else {
            egui::Stroke::NONE
        };

        ui.painter().rect(rect, 4.0, bg_fill, stroke, egui::StrokeKind::Inside);

        let icon_color = highlight_color.unwrap_or_else(|| visuals.text_color());
        let icon_galley = ui.painter().layout_no_wrap(
            icon.to_string(),
            egui::FontId::proportional(20.0),
            icon_color,
        );
        let icon_pos = egui::pos2(
            rect.center().x - icon_galley.size().x / 2.0,
            rect.top() + 4.0,
        );
        ui.painter().galley(icon_pos, icon_galley, icon_color);

        let text_color = highlight_color.unwrap_or_else(|| visuals.text_color());
        let label_galley = ui.painter().layout_no_wrap(
            label.to_string(),
            egui::FontId::proportional(11.0),
            text_color,
        );
        let label_pos = egui::pos2(
            rect.center().x - label_galley.size().x / 2.0,
            rect.bottom() - label_galley.size().y - 3.0,
        );
        ui.painter().galley(label_pos, label_galley, text_color);
    }
    response.on_hover_text(tooltip)
}

impl SqliteEditorApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Setup Korean fonts for crisp CJK glyph rendering and Phosphor icons
        setup_korean_fonts(&cc.egui_ctx);

        let app = Self {
            db_service: DbService::new(),
            table_summaries: Vec::new(),
            selected_table: None,
            table: TrackedTableBackend::new(vec![
                ("id", VariantTy::Number(rvariant::NumberTy::I64), None),
                ("name", VariantTy::Str, Some(Variant::Str("".into()))),
                ("email", VariantTy::Str, Some(Variant::Str("".into()))),
                ("amount", VariantTy::Number(rvariant::NumberTy::F64), None),
                ("active", VariantTy::Bool, Some(Variant::Bool(true))),
            ]),
            table_view: TableView::new(),
            table_config: TableViewConfig::default(),
            status_message: "준비 완료".to_string(),
            last_action_time: None,
            target_row_idx: 0,
            pending_table_switch: None,
            show_dirty_switch_dialog: false,
            show_help_modal: false,
            show_search_bar: false,
            search_replace_mode: false,
            search_query: String::new(),
            replace_query: String::new(),
            search_matches: Vec::new(),
            search_match_idx: 0,
            search_case_sensitive: false,
            query_window: QueryWindowState::default(),
            show_smart_paste_modal: false,
            smart_paste_text: String::new(),
            is_locked: false,
        };

        app
    }

    pub fn open_db_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("SQLite Database", &["db", "sqlite", "sqlite3", "s3db"])
            .pick_file()
        {
            self.load_database(path);
        }
    }

    pub fn load_sample_database(&mut self) {
        let mut temp_path = std::env::temp_dir();
        temp_path.push("sqlite_editor_sample_ecommerce.db");

        if let Err(e) = create_sample_database(&temp_path) {
            self.status_message = format!("샘플 데이터베이스 생성 실패: {e}");
            return;
        }

        self.load_database(temp_path);
        self.status_message = "샘플 SQLite 데이터베이스가 로드되었습니다 (3개 테이블).".to_string();
    }

    pub fn load_database(&mut self, path: PathBuf) {
        match self.db_service.open(&path) {
            Ok(()) => {
                match self.db_service.list_tables() {
                    Ok(tables) => {
                        self.table_summaries = tables;
                        if let Some(first_table) = self.table_summaries.first().map(|t| t.name.clone()) {
                            self.load_table_data(&first_table, true);
                        } else {
                            self.selected_table = None;
                            self.status_message = format!(
                                "DB '{}' 열림 (테이블 없음)",
                                self.db_service.file_name().unwrap_or_default()
                            );
                        }
                    }
                    Err(e) => {
                        self.status_message = format!("테이블 목록 조회 실패: {e}");
                    }
                }
            }
            Err(e) => {
                self.status_message = format!("DB 파일 열기 실패: {e}");
            }
        }
    }

    pub fn load_table_data(&mut self, table_name: &str, force: bool) {
        if !force && self.table.tracker.is_dirty() {
            self.pending_table_switch = Some(table_name.to_string());
            self.show_dirty_switch_dialog = true;
            return;
        }

        match self.db_service.load_table_data(table_name, None) {
            Ok((schema, rows)) => {
                let row_cnt = rows.len();
                let col_cnt = schema.columns.len();
                let pk_str = if schema.primary_keys.is_empty() {
                    "없음".to_string()
                } else {
                    schema.primary_keys.join(", ")
                };

                self.table.load_db_table(schema, rows);
                self.selected_table = Some(table_name.to_string());
                self.target_row_idx = 0;
                self.status_message = format!(
                    "테이블 '{}' ({}행 × {}열, 기본키: {})",
                    table_name, row_cnt, col_cnt, pk_str
                );
                self.last_action_time = Some(Instant::now());
            }
            Err(e) => {
                self.status_message = format!("테이블 '{}' 로드 실패: {e}", table_name);
            }
        }
    }

    pub fn refresh_current(&mut self) {
        if let Ok(tables) = self.db_service.list_tables() {
            self.table_summaries = tables;
        }
        if let Some(current) = self.selected_table.clone() {
            self.load_table_data(&current, true);
        }
    }

    pub fn save_to_sqlite(&mut self) {
        let Some(selected_table) = &self.selected_table else {
            self.status_message = "선택된 테이블이 없습니다.".to_string();
            return;
        };
        let Some(schema) = &self.table.schema else {
            self.status_message = "테이블 스키마 정보가 없습니다.".to_string();
            return;
        };

        match self.db_service.save_changes(
            selected_table,
            schema,
            &self.table.tracker,
            &self.table,
        ) {
            Ok(summary) => {
                let bak_str = if let Some(bak) = summary.backup_path {
                    format!(" [자동 백업: {}]", bak.file_name().unwrap_or_default().to_string_lossy())
                } else {
                    String::new()
                };
                self.status_message = format!(
                    "저장 완료! (추가 {} / 수정 {} / 삭제 {}){}",
                    summary.inserted_count, summary.updated_count, summary.deleted_count, bak_str
                );
                // Snapshot clean baseline after successful commit
                self.table.snapshot_as_baseline();
                // Refresh table summaries to update row count badge
                if let Ok(tables) = self.db_service.list_tables() {
                    self.table_summaries = tables;
                }
            }
            Err(e) => {
                self.status_message = format!("저장 실패 (롤백됨): {e}");
            }
        }
        self.last_action_time = Some(Instant::now());
    }

    #[allow(dead_code)]
    pub fn delete_row_by_index(&mut self, row_idx: usize) {
        if let Some(row_uid) = self.table.row_uid(VisualRowIdx(row_idx)) {
            if self.table.tracker.is_row_deleted(row_uid) {
                self.table.unmark_row_deleted(row_uid);
                self.status_message = format!("행 #{} 삭제가 취소(복원)되었습니다.", row_idx);
            } else {
                self.table.mark_row_deleted(row_uid);
                self.status_message = format!("행 #{} 삭제 예정 (저장 시 DB 반영)", row_idx);
            }
        }
    }

    pub fn toggle_lock(&mut self) {
        self.is_locked = !self.is_locked;
        self.table.set_read_only(self.is_locked);
        if self.is_locked {
            self.status_message = "잠금 상태 활성화: 읽기 전용으로 보호됩니다 (편집 불가).".to_string();
        } else {
            self.status_message = "잠금 해제: 편집 및 수정이 가능합니다.".to_string();
        }
        self.last_action_time = Some(Instant::now());
    }

    pub fn compact_database(&mut self) {
        if self.is_locked {
            self.status_message = "잠금 상태입니다. 잠금을 해제한 후 압축을 실행하세요.".to_string();
            return;
        }
        if self.table.tracker.is_dirty() {
            self.save_to_sqlite();
        }
        match self.db_service.compact_database() {
            Ok((before, after)) => {
                let saved = before.saturating_sub(after);
                let pct = if before > 0 {
                    (saved as f64 / before as f64) * 100.0
                } else {
                    0.0
                };
                self.status_message = format!(
                    "데이터베이스 압축(VACUUM) 완료: {} -> {} ({:.1}% 절감)",
                    format_bytes(before),
                    format_bytes(after),
                    pct
                );
                self.last_action_time = Some(Instant::now());
            }
            Err(e) => {
                self.status_message = format!("데이터베이스 압축 실패: {e}");
            }
        }
    }

    pub fn copy_table_schema_to_clipboard(&mut self) {
        let Some(table_name) = &self.selected_table else {
            self.status_message = "선택된 테이블이 없습니다.".to_string();
            return;
        };
        let row_count = self.table.row_count();
        let col_count = self.table.column_names.len();
        let col_list = self.table.column_names.join(", ");

        let mut lines = Vec::new();
        lines.push(format!("### SQLite Table: `{table_name}`"));
        lines.push(format!("- Dimensions: {col_count} columns × {row_count} rows"));
        lines.push(format!("- Columns ({col_count}):"));

        let pk_names: Vec<String> = self.table.schema.as_ref()
            .map(|s| s.primary_keys.clone())
            .unwrap_or_default();

        for col_name in &self.table.column_names {
            let is_pk = pk_names.contains(col_name);
            let col_ty = self.table.schema.as_ref()
                .and_then(|s| s.columns.iter().find(|c| c.name == *col_name))
                .map(|c| c.data_type.clone())
                .unwrap_or_else(|| "TEXT".to_string());
            let not_null = self.table.schema.as_ref()
                .and_then(|s| s.columns.iter().find(|c| c.name == *col_name))
                .map(|c| c.not_null)
                .unwrap_or(false);

            let mut flags = Vec::new();
            if is_pk { flags.push("PRIMARY KEY"); }
            if not_null { flags.push("NOT NULL"); }

            let flag_str = if flags.is_empty() {
                String::new()
            } else {
                format!(" ({})", flags.join(", "))
            };

            lines.push(format!("  - `{col_name}`: {col_ty}{flag_str}"));
        }

        lines.push(format!("- Column List (Comma-separated):\n  {col_list}"));

        let markdown = lines.join("\n");
        if let Ok(mut cb) = Clipboard::new() {
            if cb.set_text(markdown).is_ok() {
                self.status_message = format!("테이블 '{}' 스키마 정보가 AI 프롬프트용으로 복사되었습니다.", table_name);
                self.last_action_time = Some(Instant::now());
                return;
            }
        }
        self.status_message = "클립보드 복사에 실패했습니다.".to_string();
    }

    fn paste_from_clipboard_overwrite(&mut self) {
        if self.is_locked {
            self.status_message = "잠금 상태(읽기 전용)이므로 붙여넣기를 할 수 없습니다. 상단 '잠금' 버튼을 클릭하여 해제하세요.".to_string();
            return;
        }
        let (start_row, start_col) = self.table_view.selected_cell().unwrap_or((0, 0));
        if let Ok(mut cb) = Clipboard::new() {
            if let Ok(text) = cb.get_text() {
                let parsed = parse_tsv(&text);
                if parsed.is_empty() {
                    self.status_message = "클립보드에 유효한 테이블 데이터가 없습니다.".to_string();
                    return;
                }
                let modified = apply_paste_overwrite(&mut self.table, start_row, start_col, &parsed);
                self.status_message = format!(
                    "클립보드에서 {}개 행 붙여넣기 완료 ({}행 {}열부터, 수정/추가 셀: {}개)",
                    parsed.len(),
                    start_row,
                    start_col,
                    modified
                );
                self.last_action_time = Some(Instant::now());
                return;
            }
        }
        self.status_message = "클립보드 읽기에 실패했습니다. 사내 보안(M365 DLP) 환경인 경우 상단 '스마트 붙여넣기' 버튼 또는 Win + V를 사용하세요.".to_string();
    }

    #[allow(dead_code)]
    fn paste_from_clipboard_append(&mut self) {
        if let Ok(mut cb) = Clipboard::new() {
            if let Ok(text) = cb.get_text() {
                let parsed = parse_tsv(&text);
                if parsed.is_empty() {
                    self.status_message = "클립보드에 유효한 데이터가 없습니다.".to_string();
                    return;
                }
                let count = apply_paste_append(&mut self.table, &parsed);
                self.status_message = format!("클립보드에서 {}개 신규 행 추가 완료", count);
                self.last_action_time = Some(Instant::now());
                return;
            }
        }
        self.status_message = "클립보드 읽기에 실패했습니다.".to_string();
    }

    fn copy_table_to_clipboard(&mut self) {
        let tsv = export_to_tsv(&self.table, None, None);
        if let Ok(mut cb) = Clipboard::new() {
            if cb.set_text(tsv).is_ok() {
                self.status_message = format!(
                    "테이블 전체({}행)가 클립보드로 복사되었습니다.",
                    self.table.row_count()
                );
                self.last_action_time = Some(Instant::now());
                return;
            }
        }
        self.status_message = "클립보드 복사에 실패했습니다.".to_string();
    }

    pub fn perform_search(&mut self) {
        self.table.search_matches.clear();
        self.search_matches.clear();
        self.search_match_idx = 0;

        if self.search_query.is_empty() {
            self.table.current_search_focus = None;
            return;
        }

        let query = if self.search_case_sensitive {
            self.search_query.clone()
        } else {
            self.search_query.to_lowercase()
        };

        let row_count = self.table.row_count();
        let col_count = self.table.column_names.len();

        for r_idx in 0..row_count {
            if let Some(row_uid) = self.table.row_uid(VisualRowIdx(r_idx)) {
                for c_idx in 0..col_count {
                    let col_uid = ColumnUid(c_idx as u32);
                    let coord = CellCoord { row_uid, col_uid };
                    if let Some(val) = self.table.inner.get(coord) {
                        let text = val.to_string();
                        let target = if self.search_case_sensitive {
                            text
                        } else {
                            text.to_lowercase()
                        };
                        if target.contains(&query) {
                            self.search_matches.push(coord);
                            self.table.search_matches.insert(coord);
                        }
                    }
                }
            }
        }

        if !self.search_matches.is_empty() {
            self.table.current_search_focus = Some(self.search_matches[0]);
        } else {
            self.table.current_search_focus = None;
        }
    }

    pub fn search_next(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_match_idx = (self.search_match_idx + 1) % self.search_matches.len();
        let coord = self.search_matches[self.search_match_idx];
        self.table.current_search_focus = Some(coord);
    }

    pub fn search_prev(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        if self.search_match_idx == 0 {
            self.search_match_idx = self.search_matches.len() - 1;
        } else {
            self.search_match_idx -= 1;
        }
        let coord = self.search_matches[self.search_match_idx];
        self.table.current_search_focus = Some(coord);
    }

    pub fn replace_current(&mut self) {
        if let Some(coord) = self.table.current_search_focus {
            if let Some(val) = self.table.inner.get(coord) {
                let text = val.to_string();
                let new_text = if self.search_case_sensitive {
                    text.replace(&self.search_query, &self.replace_query)
                } else {
                    case_insensitive_replace(&text, &self.search_query, &self.replace_query)
                };
                let new_variant = Variant::Str(new_text);
                self.table.inner.set(coord, new_variant.clone());
                self.table.tracker.record_cell_change(coord, new_variant);
                self.table.update_metadata_indicators();
                self.status_message = "1개 셀 내용 변경 완료".to_string();
            }
            self.perform_search();
        }
    }

    pub fn replace_all(&mut self) {
        let matches = self.search_matches.clone();
        if matches.is_empty() {
            self.status_message = "바꿀 일치 항목이 없습니다.".to_string();
            return;
        }
        let mut count = 0;
        for coord in matches {
            if let Some(val) = self.table.inner.get(coord) {
                let text = val.to_string();
                let new_text = if self.search_case_sensitive {
                    text.replace(&self.search_query, &self.replace_query)
                } else {
                    case_insensitive_replace(&text, &self.search_query, &self.replace_query)
                };
                let new_variant = Variant::Str(new_text);
                self.table.inner.set(coord, new_variant.clone());
                self.table.tracker.record_cell_change(coord, new_variant);
                count += 1;
            }
        }
        self.table.update_metadata_indicators();
        self.status_message = format!("총 {}개 셀 일괄 교체 완료", count);
        self.perform_search();
    }

    pub fn run_sql_query(&mut self) {
        let query = self.query_window.sql_text.trim();
        if query.is_empty() {
            self.query_window.status_message = Some("실행할 SQL 쿼리를 입력하세요.".to_string());
            self.query_window.is_error = true;
            return;
        }

        let start = Instant::now();
        match self.db_service.execute_query(query, Some(10_000)) {
            Ok((col_names, rows)) => {
                let dur = start.elapsed();
                let row_cnt = rows.len();
                let col_cnt = col_names.len();
                let table = TrackedTableBackend::from_query_result(col_names, rows);
                self.query_window.result_table = Some(table);
                self.query_window.is_error = false;
                self.query_window.execution_dur = Some(dur);
                self.query_window.status_message = Some(format!(
                    "{}행 × {}열 조회 완료 ({:.2?})",
                    row_cnt, col_cnt, dur
                ));
            }
            Err(e) => {
                self.query_window.is_error = true;
                self.query_window.status_message = Some(format!("SQLite 실행 오류: {e}"));
            }
        }
    }

    pub fn copy_query_results_to_clipboard(&mut self) {
        let Some(table) = &self.query_window.result_table else {
            self.query_window.status_message = Some("복사할 쿼리 결과가 없습니다.".to_string());
            self.query_window.is_error = true;
            return;
        };
        let tsv = export_to_tsv(table, None, None);
        if let Ok(mut cb) = Clipboard::new() {
            if cb.set_text(tsv).is_ok() {
                self.query_window.status_message = Some(format!(
                    "결과 전체({}행)가 클립보드로 복사되었습니다.",
                    table.row_count()
                ));
                self.query_window.is_error = false;
                return;
            }
        }
        self.query_window.status_message = Some("클립보드 접근에 실패했습니다.".to_string());
        self.query_window.is_error = true;
    }
}

impl eframe::App for SqliteEditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Keyboard Shortcuts
        let ctrl_or_cmd = ctx.input(|i| i.modifiers.command || i.modifiers.ctrl);
        let wants_keyboard = ctx.egui_wants_keyboard_input();

        // F1: Help Dialog Toggle
        if ctx.input(|i| i.key_pressed(Key::F1)) {
            self.show_help_modal = !self.show_help_modal;
        }

        // F2: SQL Query Window Toggle (Access style)
        if ctx.input(|i| i.key_pressed(Key::F2)) {
            self.query_window.is_open = !self.query_window.is_open;
        }

        // F5: Refresh Current Table
        if ctx.input(|i| i.key_pressed(Key::F5)) {
            self.refresh_current();
        }

        // Ctrl+F: Open Find Bar
        if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::F)) {
            self.show_search_bar = true;
            self.search_replace_mode = false;
        }

        // Ctrl+H: Open Find & Replace Bar
        if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::H)) {
            self.show_search_bar = true;
            self.search_replace_mode = true;
        }

        // Esc: Close search bar or help modal
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.show_search_bar {
                self.show_search_bar = false;
                self.table.search_matches.clear();
                self.table.current_search_focus = None;
            }
            if self.show_help_modal {
                self.show_help_modal = false;
            }
        }

        // Ctrl+S / Cmd+S: Save to SQLite
        if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::S)) {
            if self.table.tracker.is_dirty() {
                self.save_to_sqlite();
            }
        }

        // Ctrl+L or Ctrl+O: Open DB File (불러오기)
        if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::L) || i.key_pressed(Key::O)) {
            self.open_db_file();
        }

        // Detect OS-level Paste Events (e.g. from Win+V selection, WM_PASTE, etc.)
        let mut os_paste_text = None;
        ctx.input(|i| {
            for event in &i.raw.events {
                if let egui::Event::Paste(text) = event {
                    os_paste_text = Some(text.clone());
                }
            }
        });

        // Grid-level shortcuts (active when not directly editing cell text)
        if !wants_keyboard {
            // Handle OS-level Paste (including Win+V clipboard history selection!)
            if let Some(text) = os_paste_text {
                let (start_row, start_col) = self.table_view.selected_cell().unwrap_or((0, 0));
                let parsed = parse_tsv(&text);
                if !parsed.is_empty() {
                    let count = apply_paste_overwrite(&mut self.table, start_row, start_col, &parsed);
                    self.status_message = format!(
                        "클립보드(Win+V)에서 {}개 행 붙여넣기 완료 ({}행 {}열부터, {}개 셀 반영)",
                        parsed.len(),
                        start_row,
                        start_col,
                        count
                    );
                    self.last_action_time = Some(Instant::now());
                }
            }

            // Ctrl+C: Copy Table to Clipboard
            if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::C)) {
                self.copy_table_to_clipboard();
            }

            // Ctrl+V and Ctrl+Shift+V: Overwrite Paste (대체해서 붙여넣기)
            if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::V)) {
                self.paste_from_clipboard_overwrite();
            }

            // Ctrl+Z: Revert all changes
            if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::Z)) {
                if self.table.tracker.is_dirty() {
                    self.table.revert_all();
                    self.status_message = "모든 변경사항이 원복되었습니다 (Ctrl+Z).".to_string();
                }
            }

            // Ctrl+N or Insert: Add New Row
            if (ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::N))) || ctx.input(|i| i.key_pressed(Key::Insert)) {
                self.table.create_row([]);
                self.status_message = "새 행이 추가되었습니다 (Ctrl+N).".to_string();
            }

            // Delete: Clear selected cells, or if entire rows selected, mark rows deleted
            if ctx.input(|i| i.key_pressed(Key::Delete)) {
                if self.is_locked {
                    self.status_message = "잠금 상태(읽기 전용)입니다. 상단 '잠금' 버튼을 클릭하여 해제하세요.".to_string();
                } else if let Some((r_start, r_end, c_start, c_end)) = self.table_view.selected_range() {
                    let total_cols = self.table.column_names.len();
                    if c_start == 0 && c_end >= total_cols.saturating_sub(1) {
                        for r_idx in r_start..=r_end {
                            if let Some(row_uid) = self.table.row_uid(VisualRowIdx(r_idx)) {
                                self.table.mark_row_deleted(row_uid);
                            }
                        }
                        self.status_message = format!("선택한 {}개 행이 삭제 예정으로 표시되었습니다.", r_end - r_start + 1);
                    } else {
                        let mut cleared = 0;
                        for r_idx in r_start..=r_end {
                            let Some(row_uid) = self.table.row_uid(VisualRowIdx(r_idx)) else { continue };
                            for c_idx in c_start..=c_end {
                                let Some(col_uid) = self.table.col_uid(VisualColIdx(c_idx)) else { continue };
                                let coord: CellCoord = (row_uid, col_uid).into();
                                self.table.set(coord, Variant::Empty);
                                cleared += 1;
                            }
                        }
                        self.status_message = format!("선택한 {}개 셀의 내용이 지워졌습니다.", cleared);
                    }
                    self.last_action_time = Some(Instant::now());
                }
            }

            // Ctrl+D: Delete/restore selected row(s)
            if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::D)) {
                if self.is_locked {
                    self.status_message = "잠금 상태(읽기 전용)입니다. 상단 '잠금' 버튼을 클릭하여 해제하세요.".to_string();
                } else if let Some((r_start, r_end, _, _)) = self.table_view.selected_range() {
                    for r_idx in r_start..=r_end {
                        if let Some(row_uid) = self.table.row_uid(VisualRowIdx(r_idx)) {
                            if self.table.tracker.is_row_deleted(row_uid) {
                                self.table.unmark_row_deleted(row_uid);
                            } else {
                                self.table.mark_row_deleted(row_uid);
                            }
                        }
                    }
                    self.status_message = format!("선택한 행(#{}~#{}) 삭제/복원 토글 완료", r_start, r_end);
                    self.last_action_time = Some(Instant::now());
                }
            }
        }

        // Dynamic Window Title Bar update
        let window_title = match (&self.db_service.file_name(), &self.selected_table) {
            (Some(db), Some(tbl)) => {
                let cols = self.table.column_names.len();
                let rows = self.table.row_count();
                format!("{} : {} ({} cols × {} rows) — SQLite Editor", db, tbl, cols, rows)
            }
            (Some(db), None) => format!("{} — SQLite Editor", db),
            (None, _) => "SQLite Editor".to_string(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(window_title));

        // Top Ribbon Toolbar (Modern clean single-row Ribbon design)
        Panel::top("top_toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                // Open DB
                if ribbon_button(
                    ui,
                    egui_phosphor::regular::FOLDER_OPEN,
                    "열기",
                    "로컬 SQLite DB 파일 열기 (Ctrl+O, Ctrl+L)",
                    None,
                    None,
                )
                .clicked()
                {
                    self.open_db_file();
                }

                // Save
                let is_dirty = self.table.tracker.is_dirty();
                let save_color = if is_dirty {
                    Some(Color32::from_rgb(34, 139, 34))
                } else {
                    None
                };
                let save_bg = if is_dirty {
                    Some(Color32::from_rgb(240, 253, 244))
                } else {
                    None
                };
                if ribbon_button(
                    ui,
                    egui_phosphor::regular::FLOPPY_DISK,
                    "저장",
                    "데이터베이스에 안전하게 영구 저장 (Ctrl+S)",
                    save_color,
                    save_bg,
                )
                .clicked()
                {
                    self.save_to_sqlite();
                }

                // Refresh
                if ribbon_button(
                    ui,
                    egui_phosphor::regular::ARROWS_CLOCKWISE,
                    "새로고침",
                    "현재 테이블 데이터 다시 조회 (F5)",
                    None,
                    None,
                )
                .clicked()
                {
                    self.refresh_current();
                }

                // Compact (VACUUM)
                if ribbon_button(
                    ui,
                    egui_phosphor::regular::ARROWS_IN,
                    "압축",
                    "미사용 공간 회수 및 DB 파일 크기 최적화 압축 (SQLite VACUUM)",
                    None,
                    None,
                )
                .clicked()
                {
                    self.compact_database();
                }

                ui.separator();

                // Lock Toggle (Read-only protection)
                let (lock_icon, lock_label, lock_color, lock_bg) = if self.is_locked {
                    (
                        egui_phosphor::regular::LOCK_SIMPLE,
                        "잠금",
                        Some(Color32::from_rgb(217, 119, 6)),
                        Some(Color32::from_rgb(254, 243, 199)),
                    )
                } else {
                    (
                        egui_phosphor::regular::LOCK_SIMPLE_OPEN,
                        "편집",
                        None,
                        None,
                    )
                };
                let lock_tip = if self.is_locked {
                    "현재 읽기 전용으로 보호 중 (실수 수정 방지). 클릭 시 편집 모드로 전환"
                } else {
                    "현재 편집 가능 모드. 클릭 시 읽기 전용으로 잠금"
                };
                if ribbon_button(ui, lock_icon, lock_label, lock_tip, lock_color, lock_bg).clicked() {
                    self.toggle_lock();
                }

                ui.separator();

                // SQL Query (Access style)
                let query_active = self.query_window.is_open;
                let query_color = if query_active {
                    Some(Color32::from_rgb(2, 132, 199))
                } else {
                    None
                };
                if ribbon_button(
                    ui,
                    egui_phosphor::regular::TERMINAL_WINDOW,
                    "SQL 쿼리",
                    "별도 Access 스타일 SQL 쿼리 조회 창 열기 (F2)",
                    query_color,
                    None,
                )
                .clicked()
                {
                    self.query_window.is_open = !query_active;
                }

                ui.separator();

                // Schema Copy for AI Agent
                if ribbon_button(
                    ui,
                    egui_phosphor::regular::FILE_CODE,
                    "스키마 복사",
                    "현재 테이블의 컬럼명, 타입, 기본키 정보를 AI 프롬프트용 마크다운으로 클립보드 복사",
                    Some(Color32::from_rgb(37, 99, 235)),
                    None,
                )
                .clicked()
                {
                    self.copy_table_schema_to_clipboard();
                }

                // Right Utility Icon Buttons
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(egui_phosphor::regular::QUESTION).size(16.0),
                            )
                            .min_size(egui::vec2(32.0, 32.0)),
                        )
                        .on_hover_text("단축키 및 도움말 (F1)")
                        .clicked()
                    {
                        self.show_help_modal = true;
                    }

                    let search_active = self.show_search_bar;
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(egui_phosphor::regular::MAGNIFYING_GLASS)
                                    .size(16.0)
                                    .color(if search_active {
                                        Color32::from_rgb(2, 132, 199)
                                    } else {
                                        ui.visuals().text_color()
                                    }),
                            )
                            .min_size(egui::vec2(32.0, 32.0)),
                        )
                        .on_hover_text("데이터 검색 바 열기 / 닫기 (Ctrl+F)")
                        .clicked()
                    {
                        self.show_search_bar = !search_active;
                    }
                });
            });
            ui.add_space(2.0);
        });

        // Search & Replace Docked Panel (Appears when Ctrl+F or Ctrl+H is active)
        if self.show_search_bar {
            Panel::top("search_replace_panel").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(egui_phosphor::regular::MAGNIFYING_GLASS).size(14.0));
                    ui.strong("찾기:");
                    let search_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("검색할 텍스트 입력 (Enter: 다음, Shift+Enter: 이전)")
                            .desired_width(220.0),
                    );

                    if search_resp.changed() {
                        self.perform_search();
                    }

                    if search_resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        if ui.input(|i| i.modifiers.shift) {
                            self.search_prev();
                        } else {
                            self.search_next();
                        }
                    }

                    if ui
                        .button(egui_phosphor::regular::CARET_UP)
                        .on_hover_text("이전 일치 항목 (Shift+Enter)")
                        .clicked()
                    {
                        self.search_prev();
                    }
                    if ui
                        .button(egui_phosphor::regular::CARET_DOWN)
                        .on_hover_text("다음 일치 항목 (Enter)")
                        .clicked()
                    {
                        self.search_next();
                    }

                    let match_cnt = self.search_matches.len();
                    if self.search_query.is_empty() {
                        ui.label(RichText::new("단어를 입력하세요").weak());
                    } else if match_cnt == 0 {
                        ui.colored_label(Color32::from_rgb(215, 60, 60), "일치 항목 없음");
                    } else {
                        ui.colored_label(
                            Color32::from_rgb(0, 120, 215),
                            format!("{}/{}개 일치", self.search_match_idx + 1, match_cnt),
                        );
                    }

                    if ui.checkbox(&mut self.search_case_sensitive, "대소문자 구분").changed() {
                        self.perform_search();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(egui_phosphor::regular::X).on_hover_text("검색바 닫기 (Esc)").clicked() {
                            self.show_search_bar = false;
                            self.table.search_matches.clear();
                            self.table.current_search_focus = None;
                        }
                    });
                });

                if self.search_replace_mode {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(egui_phosphor::regular::ARROWS_CLOCKWISE).size(14.0));
                        ui.strong("바꾸기:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.replace_query)
                                .hint_text("대체할 텍스트 입력")
                                .desired_width(220.0),
                        );

                        if ui.button("바꾸기").on_hover_text("현재 선택된 일치 항목 1개 교체").clicked() {
                            self.replace_current();
                        }

                        if ui.button("모두 바꾸기").on_hover_text("모든 일치 항목 일괄 교체").clicked() {
                            self.replace_all();
                        }
                    });
                }
                ui.add_space(4.0);
            });
        }

        // Left Sidebar: Tables List (Clean and neat with Phosphor table icons)
        Panel::left("tables_sidebar")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.strong(format!("{} 테이블", egui_phosphor::regular::TABLE));
                    ui.label(format!("({})", self.table_summaries.len()));
                });
                ui.separator();

                if self.table_summaries.is_empty() {
                    ui.label(RichText::new("열린 테이블 없음").weak());
                } else {
                    ScrollArea::vertical().show(ui, |ui| {
                        let current_selected = self.selected_table.clone();
                        let mut clicked_table = None;
                        for table in &self.table_summaries {
                            let is_selected = current_selected.as_deref() == Some(&table.name);
                            let text = format!("{} {}", egui_phosphor::regular::TABLE, table.name);

                            let resp = ui.selectable_label(is_selected, text);
                            if resp.clicked() && !is_selected {
                                clicked_table = Some(table.name.clone());
                            }
                        }
                        if let Some(target) = clicked_table {
                            self.load_table_data(&target, false);
                        }
                    });
                }
            });

        // Bottom Compact Status Bar (Clean 24px bar)
        Panel::bottom("bottom_info_panel")
            .resizable(false)
            .default_size(24.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&self.status_message).small().color(Color32::DARK_GRAY));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.is_locked {
                            ui.colored_label(
                                Color32::from_rgb(217, 119, 6),
                                format!("{} 잠금 (읽기 전용)", egui_phosphor::regular::LOCK_SIMPLE),
                            );
                        } else {
                            ui.colored_label(
                                Color32::from_rgb(46, 125, 50),
                                format!("{} 편집 모드", egui_phosphor::regular::LOCK_SIMPLE_OPEN),
                            );
                        }
                    });
                });
            });

        // F1 Help Modal Dialog
        if self.show_help_modal {
            egui::Window::new("도움말 및 단축키 (F1)")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.set_width(540.0);
                    ui.heading("단축키 & 조작 안내");
                    ui.add_space(4.0);
                    egui::Grid::new("help_shortcuts_grid")
                        .striped(true)
                        .num_columns(2)
                        .spacing([24.0, 6.0])
                        .show(ui, |ui| {
                            ui.strong("Ctrl + S");
                            ui.label("데이터베이스에 안전하게 영구 저장 (자동 백업 생성)");
                            ui.end_row();

                            ui.strong("Ctrl + L / Ctrl + O");
                            ui.label("로컬 SQLite DB 파일 열기 (불러오기)");
                            ui.end_row();

                            ui.strong("F5");
                            ui.label("현재 테이블 새로고침 (DB 최신 데이터 재조회)");
                            ui.end_row();

                            ui.strong("Ctrl + F");
                            ui.label("찾기 바 열기 (데이터 검색 및 일치 항목 강조)");
                            ui.end_row();

                            ui.strong("Ctrl + H");
                            ui.label("찾기 및 바꾸기 바 열기 (단어 일괄 교체)");
                            ui.end_row();

                            ui.strong("열 헤더 클릭");
                            ui.label("해당 열 전체 선택 (우클릭: 오름/내림차순 정렬)");
                            ui.end_row();

                            ui.strong("행 번호 클릭");
                            ui.label("해당 행 전체 선택 (우클릭: 행 삭제/복원)");
                            ui.end_row();

                            ui.strong("# 코너 클릭");
                            ui.label("테이블 전체 셀 일괄 선택");
                            ui.end_row();

                            ui.strong("Ctrl + Z");
                            ui.label("저장 전 모든 수정/추가/삭제를 원래대로 원복");
                            ui.end_row();

                            ui.strong("Ctrl + N / Insert");
                            ui.label("테이블 끝에 새 빈 행 추가");
                            ui.end_row();

                            ui.strong("Delete");
                            ui.label("선택한 셀 내용 일괄 비우기 (전체 열 선택 시 행 삭제 표시)");
                            ui.end_row();

                            ui.strong("Ctrl + D");
                            ui.label("선택한 행 삭제 예정(취소선) 표시 또는 복원 토글");
                            ui.end_row();

                            ui.strong("Ctrl + C");
                            ui.label("전체 테이블을 엑셀(TSV) 형식으로 클립보드 복사");
                            ui.end_row();

                            ui.strong("Ctrl + V / Ctrl + Shift + V");
                            ui.label("클립보드 엑셀 데이터를 현재 선택된 셀부터 대체(덮어쓰기) 붙여넣기");
                            ui.end_row();

                            ui.strong("Win + V");
                            ui.label("Windows 클립보드 기록에서 항목 선택 주입 (사내 M365 보안 우회 지원)");
                            ui.end_row();

                            ui.strong("더블클릭 / Enter");
                            ui.label("선택한 셀 내용 직접 편집 시작");
                            ui.end_row();

                            ui.strong("Enter / Shift + Enter");
                            ui.label("편집 확정 후 아래 / 위 행의 셀로 이동");
                            ui.end_row();

                            ui.strong("Tab / Shift + Tab");
                            ui.label("편집 확정 후 다음 / 이전 열의 셀로 이동");
                            ui.end_row();

                            ui.strong("Esc");
                            ui.label("편집 취소 / 검색바 닫기 / 팝업 닫기");
                            ui.end_row();

                            ui.strong("F2");
                            ui.label("별도의 SQL 쿼리 조회 창 열기 / 닫기 (Access 스타일)");
                            ui.end_row();

                            ui.strong("F1");
                            ui.label("도움말 팝업 열기 / 닫기");
                            ui.end_row();
                        });

                    ui.add_space(12.0);
                    ui.separator();
                    ui.heading("안전 저장 및 백업 원칙");
                    ui.add_space(4.0);
                    ui.label("• 셀을 수정하거나 행을 추가/삭제해도 즉시 파일에 쓰이지 않습니다.");
                    ui.label("• '저장(Ctrl+S)'을 누르면 원본 DB가 자동으로 백업(*.bak)된 후 안전하게 반영됩니다.");
                    ui.label("• 저장 전 언제든 '되돌리기(Ctrl+Z)'로 모든 변경사항을 원복할 수 있습니다.");

                    ui.add_space(14.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::RIGHT), |ui| {
                        if ui.button("닫기 (Esc)").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            self.show_help_modal = false;
                        }
                    });
                });
        }

        // Unsaved changes confirmation dialog if switching tables
        if self.show_dirty_switch_dialog {
            let mut switch_now = false;
            let mut cancel = false;
            let pending_name = self.pending_table_switch.clone().unwrap_or_default();

            egui::Window::new("저장되지 않은 변경사항")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(format!(
                        "현재 테이블에 저장되지 않은 수정사항이 있습니다.\n'{}' 테이블로 이동하면 변경사항이 유실됩니다. 계속하시겠습니까?",
                        pending_name
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("계속 이동 (변경 취소)").clicked() {
                            switch_now = true;
                        }
                        if ui.button("취소").clicked() {
                            cancel = true;
                        }
                    });
                });

            if switch_now {
                self.show_dirty_switch_dialog = false;
                if let Some(target) = self.pending_table_switch.take() {
                    self.load_table_data(&target, true);
                }
            } else if cancel {
                self.show_dirty_switch_dialog = false;
                self.pending_table_switch = None;
            }
        }

        // Dedicated Access-style SQL Query Window
        let mut query_window_open = self.query_window.is_open;
        let mut trigger_run_query = false;
        let mut trigger_copy_query = false;
        let mut preset_sql = None;

        if query_window_open {
            let selected_table_clone = self.selected_table.clone();
            let qw = &mut self.query_window;

            egui::Window::new("SQL 쿼리 조회 (Access 스타일)")
                .open(&mut query_window_open)
                .default_size([820.0, 560.0])
                .min_size([520.0, 360.0])
                .show(ui.ctx(), |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.strong(format!("{} SQL 쿼리식 입력:", egui_phosphor::regular::CODE));
                        ui.label(RichText::new("(Ctrl + Enter 로 즉시 실행)").weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("샘플: 50개 행").clicked() {
                                if let Some(table) = &selected_table_clone {
                                    preset_sql = Some(format!("SELECT * FROM \"{table}\" LIMIT 50;"));
                                } else {
                                    preset_sql = Some("SELECT * FROM customers LIMIT 50;".to_string());
                                }
                            }
                        });
                    });

                    // Multi-line Monospace SQL text editor
                    let editor = egui::TextEdit::multiline(&mut qw.sql_text)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY);
                    ui.add(editor);

                    // Execute & Export buttons
                    ui.horizontal(|ui| {
                        let ctrl_enter = ui.input(|i| (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(Key::Enter));
                        if ui
                            .button(
                                RichText::new(format!("{} 쿼리 실행 (Ctrl+Enter)", egui_phosphor::regular::LIGHTNING))
                                    .strong()
                                    .color(Color32::from_rgb(0, 150, 0)),
                            )
                            .clicked()
                            || ctrl_enter
                        {
                            trigger_run_query = true;
                        }

                        if ui
                            .button(format!("{} 결과 엑셀 복사 (TSV)", egui_phosphor::regular::COPY))
                            .on_hover_text("현재 조회된 쿼리 결과 표 전체를 엑셀용 클립보드로 복사")
                            .clicked()
                        {
                            trigger_copy_query = true;
                        }

                        if let Some(status) = &qw.status_message {
                            ui.separator();
                            if qw.is_error {
                                ui.colored_label(Color32::from_rgb(220, 53, 69), status);
                            } else {
                                ui.colored_label(Color32::from_rgb(40, 167, 69), status);
                            }
                        }
                    });

                    ui.separator();

                    // Results Spreadsheet Grid
                    if let Some(table) = &mut qw.result_table {
                        let available_height = ui.available_height().max(180.0);
                        qw.result_view.show(
                            table,
                            &mut qw.result_config,
                            Some(available_height),
                            ui,
                            Id::new("access_query_results_grid"),
                        );
                    } else {
                        ui.centered_and_justified(|ui| {
                            ui.label("상단에 쿼리를 입력하고 [쿼리 실행] 버튼 또는 Ctrl+Enter 를 누르세요.");
                        });
                    }
                });
        }

        self.query_window.is_open = query_window_open;
        if let Some(sql) = preset_sql {
            self.query_window.sql_text = sql;
        }
        if trigger_run_query {
            self.run_sql_query();
        }
        if trigger_copy_query {
            self.copy_query_results_to_clipboard();
        }

        // Smart Paste Modal for M365 DLP / Win+V Clipboard History
        if self.show_smart_paste_modal {
            egui::Window::new("스마트 붙여넣기 (Win + V 지원)")
                .collapsible(false)
                .resizable(true)
                .default_size([560.0, 360.0])
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(RichText::new("사내 보안(M365 DLP) 환경 가이드:").strong());
                    ui.label("아래 텍스트 상자에 커서를 두고 Win + V 를 눌러 클립보드 기록에서 항목을 선택하면 보안 차단을 우회하여 주입됩니다.");
                    ui.add_space(6.0);

                    let edit = egui::TextEdit::multiline(&mut self.smart_paste_text)
                        .desired_rows(6)
                        .hint_text("여기에 Ctrl+V 또는 Win+V 로 엑셀 데이터를 붙여넣으세요...")
                        .desired_width(f32::INFINITY);
                    ui.add(edit);

                    ui.add_space(8.0);
                    let parsed = parse_tsv(&self.smart_paste_text);
                    if !parsed.is_empty() {
                        let rows = parsed.len();
                        let cols = parsed.get(0).map(|r| r.len()).unwrap_or(0);
                        ui.colored_label(
                            Color32::from_rgb(0, 120, 215),
                            format!("인식된 표 데이터: {}개 행 × {}개 열", rows, cols),
                        );

                        ui.horizontal(|ui| {
                            if ui
                                .button(
                                    RichText::new("선택 셀부터 대체 붙여넣기")
                                        .strong()
                                        .color(Color32::from_rgb(0, 150, 0)),
                                )
                                .clicked()
                            {
                                let (start_row, start_col) = self.table_view.selected_cell().unwrap_or((0, 0));
                                let count = apply_paste_overwrite(&mut self.table, start_row, start_col, &parsed);
                                self.status_message = format!("{}개 행 대체 붙여넣기 완료 ({}행 {}열부터, {}개 셀 수정/추가)", rows, start_row, start_col, count);
                                self.show_smart_paste_modal = false;
                                self.smart_paste_text.clear();
                            }
                            if ui
                                .button(
                                    RichText::new("새 행으로 일괄 추가")
                                        .strong()
                                        .color(Color32::from_rgb(0, 102, 204)),
                                )
                                .clicked()
                            {
                                let count = apply_paste_append(&mut self.table, &parsed);
                                self.status_message = format!("{}개 신규 행 추가 완료", count);
                                self.show_smart_paste_modal = false;
                                self.smart_paste_text.clear();
                            }
                            if ui.button("내용 지우기").clicked() {
                                self.smart_paste_text.clear();
                            }
                        });
                    } else {
                        ui.label(RichText::new("아직 인식된 엑셀 데이터가 없습니다.").weak());
                    }

                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::RIGHT), |ui| {
                        if ui.button("닫기 (Esc)").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            self.show_smart_paste_modal = false;
                        }
                    });
                });
        }

        // Central Table Grid Area
        CentralPanel::default().show(ui, |ui| {
            let available_size = ui.available_size();
            self.table_view.show(
                &mut self.table,
                &mut self.table_config,
                Some(available_size.y),
                ui,
                Id::new("main_data_grid"),
            );
        });
    }
}
