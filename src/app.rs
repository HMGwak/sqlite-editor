use crate::clipboard_util::{
    apply_paste_append, apply_paste_overwrite, export_to_tsv, generate_sample_excel_tsv, parse_tsv,
};
use crate::db::{create_sample_database, DbService, TableSummary};
use crate::model::TrackedTableBackend;
use arboard::Clipboard;
use egui::{CentralPanel, Color32, Id, Key, Panel, RichText, ScrollArea};
use egui_tabular::table_view::TableViewConfig;
use egui_tabular::{TableBackend, TableView};
use rvariant::{Variant, VariantTy};
use std::path::PathBuf;
use std::time::Instant;
use tabular_core::backend::VisualRowIdx;

pub struct SqliteEditorApp {
    pub db_service: DbService,
    pub table_summaries: Vec<TableSummary>,
    pub selected_table: Option<String>,

    pub table: TrackedTableBackend,
    pub table_view: TableView,
    pub table_config: TableViewConfig,
    pub status_message: String,
    pub last_action_time: Option<Instant>,
    pub benchmark_result: Option<String>,

    pub target_row_idx: usize,
    pub pending_table_switch: Option<String>,
    pub show_dirty_switch_dialog: bool,
}

impl SqliteEditorApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self {
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
            status_message: "SQLite DB 파일을 열거나, 상단의 '샘플 DB 로드'를 클릭하세요.".to_string(),
            last_action_time: None,
            benchmark_result: None,
            target_row_idx: 0,
            pending_table_switch: None,
            show_dirty_switch_dialog: false,
        };

        // Automatically load initial sample database for immediate exploration
        app.load_sample_database();
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
            self.status_message = format!("❌ 샘플 데이터베이스 생성 실패: {e}");
            return;
        }

        self.load_database(temp_path);
        self.status_message = "✅ 이커머스 샘플 SQLite 데이터베이스가 로드되었습니다 (3개 테이블).".to_string();
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
                        self.status_message = format!("❌ 테이블 목록 조회 실패: {e}");
                    }
                }
            }
            Err(e) => {
                self.status_message = format!("❌ DB 파일 열기 실패: {e}");
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
                    "테이블 '{}' 로드 완료 ({}행 × {}열, 기본키: {})",
                    table_name, row_cnt, col_cnt, pk_str
                );
                self.last_action_time = Some(Instant::now());
            }
            Err(e) => {
                self.status_message = format!("❌ 테이블 '{}' 로드 실패: {e}", table_name);
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
            self.status_message = "❌ 선택된 테이블이 없습니다.".to_string();
            return;
        };
        let Some(schema) = &self.table.schema else {
            self.status_message = "❌ 테이블 스키마 정보가 없습니다.".to_string();
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
                    "💾 DB 저장 완료! [추가: {}행, 수정: {}행, 삭제: {}행]{}",
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
                self.status_message = format!("❌ DB 저장 실패 (전체 롤백됨): {e}");
            }
        }
        self.last_action_time = Some(Instant::now());
    }

    pub fn delete_row_by_index(&mut self, row_idx: usize) {
        if let Some(row_uid) = self.table.row_uid(VisualRowIdx(row_idx)) {
            if self.table.tracker.is_row_deleted(row_uid) {
                self.table.unmark_row_deleted(row_uid);
                self.status_message = format!("행 #{} 삭제가 취소(복원)되었습니다.", row_idx);
            } else {
                self.table.mark_row_deleted(row_uid);
                self.status_message = format!("행 #{}이(가) 삭제 대기 목록에 추가되었습니다.", row_idx);
            }
        }
    }

    fn copy_sample_to_clipboard(&mut self) {
        let tsv = generate_sample_excel_tsv(20);
        if let Ok(mut cb) = Clipboard::new() {
            if cb.set_text(tsv).is_ok() {
                self.status_message = "✅ 엑셀 샘플 20행 × 5열 데이터가 클립보드에 복사되었습니다!".to_string();
                self.last_action_time = Some(Instant::now());
                return;
            }
        }
        self.status_message = "❌ 클립보드 접근에 실패했습니다.".to_string();
    }

    fn paste_from_clipboard_overwrite(&mut self) {
        if let Ok(mut cb) = Clipboard::new() {
            if let Ok(text) = cb.get_text() {
                let parsed = parse_tsv(&text);
                if parsed.is_empty() {
                    self.status_message = "⚠️ 클립보드에 유효한 텍스트/테이블 데이터가 없습니다.".to_string();
                    return;
                }
                let modified = apply_paste_overwrite(&mut self.table, 0, 0, &parsed);
                self.status_message = format!(
                    "📥 클립보드에서 {}개 행을 덮어썼습니다 (수정된 셀: {}개).",
                    parsed.len(),
                    modified
                );
                self.last_action_time = Some(Instant::now());
                return;
            }
        }
        self.status_message = "❌ 클립보드 읽기에 실패했습니다.".to_string();
    }

    fn paste_from_clipboard_append(&mut self) {
        if let Ok(mut cb) = Clipboard::new() {
            if let Ok(text) = cb.get_text() {
                let parsed = parse_tsv(&text);
                if parsed.is_empty() {
                    self.status_message = "⚠️ 클립보드에 유효한 데이터가 없습니다.".to_string();
                    return;
                }
                let count = apply_paste_append(&mut self.table, &parsed);
                self.status_message = format!("➕ 클립보드에서 {}개의 신규 행을 추가했습니다.", count);
                self.last_action_time = Some(Instant::now());
                return;
            }
        }
        self.status_message = "❌ 클립보드 읽기에 실패했습니다.".to_string();
    }

    fn copy_table_to_clipboard(&mut self) {
        let tsv = export_to_tsv(&self.table, None, None);
        if let Ok(mut cb) = Clipboard::new() {
            if cb.set_text(tsv).is_ok() {
                self.status_message = format!(
                    "📤 전체 테이블({}행)이 클립보드(TSV/Excel)로 복사되었습니다.",
                    self.table.row_count()
                );
                self.last_action_time = Some(Instant::now());
                return;
            }
        }
        self.status_message = "❌ 클립보드 복사에 실패했습니다.".to_string();
    }

    fn run_1000_rows_benchmark(&mut self) {
        let start = Instant::now();
        let sample = generate_sample_excel_tsv(1000);
        let parsed = parse_tsv(&sample);
        let parse_dur = start.elapsed();

        let insert_start = Instant::now();
        let rows_added = apply_paste_append(&mut self.table, &parsed);
        let insert_dur = insert_start.elapsed();

        let total_dur = start.elapsed();
        let msg = format!(
            "🚀 1,000행 일괄 처리 완료! 파싱: {:?}, 행 추가({}행): {:?}, 총 소요: {:?}",
            parse_dur, rows_added, insert_dur, total_dur
        );
        self.benchmark_result = Some(msg.clone());
        self.status_message = msg;
        self.last_action_time = Some(Instant::now());
    }
}

impl eframe::App for SqliteEditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Global Keyboard shortcuts
        let ctrl_or_cmd = ctx.input(|i| i.modifiers.command || i.modifiers.ctrl);
        let shift = ctx.input(|i| i.modifiers.shift);

        // Ctrl+S / Cmd+S: Save to DB!
        if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::S)) {
            if self.table.tracker.is_dirty() {
                self.save_to_sqlite();
            }
        }

        if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::V)) {
            if shift {
                self.paste_from_clipboard_append();
            } else {
                self.paste_from_clipboard_overwrite();
            }
        }

        // Top Control Panel
        Panel::top("top_toolbar").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.heading(RichText::new("📊 Rust SQLite Editor").strong());
                ui.separator();

                // DB connection buttons
                if ui
                    .button(RichText::new("📂 파일 열기 (.db)").strong().color(Color32::from_rgb(0, 160, 220)))
                    .on_hover_text("로컬 SQLite 데이터베이스 파일(.db, .sqlite)을 엽니다.")
                    .clicked()
                {
                    self.open_db_file();
                }

                if ui
                    .button("⚡ 샘플 DB 로드")
                    .on_hover_text("다양한 테이블과 자료형이 포함된 샘플 DB를 즉시 생성하여 로드합니다.")
                    .clicked()
                {
                    self.load_sample_database();
                }

                if ui
                    .button("🔄 새로고침")
                    .on_hover_text("현재 데이터베이스 및 테이블을 다시 읽어옵니다.")
                    .clicked()
                {
                    self.refresh_current();
                }

                ui.separator();

                // Editing & Excel actions
                if ui
                    .button("📋 샘플 엑셀 복사")
                    .on_hover_text("20행 5열의 엑셀 호환 TSV 데이터를 클립보드에 복사합니다.")
                    .clicked()
                {
                    self.copy_sample_to_clipboard();
                }

                if ui
                    .button(RichText::new("📥 덮어쓰기 붙여넣기 (Ctrl+V)").strong())
                    .clicked()
                {
                    self.paste_from_clipboard_overwrite();
                }

                if ui
                    .button("➕ 신규 행 추가 붙여넣기 (Ctrl+Shift+V)")
                    .clicked()
                {
                    self.paste_from_clipboard_append();
                }

                if ui
                    .button("📤 전체 복사 (Ctrl+C)")
                    .clicked()
                {
                    self.copy_table_to_clipboard();
                }

                ui.separator();

                // Row operations
                if ui.button("➕ 행 추가").clicked() {
                    self.table.create_row([]);
                    self.status_message = "새 행이 추가되었습니다.".to_string();
                }

                let max_row = self.table.row_count().saturating_sub(1);
                ui.add(
                    egui::DragValue::new(&mut self.target_row_idx)
                        .range(0..=max_row)
                        .prefix("행 #"),
                );

                if ui
                    .button(RichText::new("🗑 삭제/복원").color(Color32::from_rgb(220, 100, 100)))
                    .on_hover_text("지정한 행을 삭제 상태로 전환하거나 복원합니다 (좌측 행 번호 우클릭으로도 가능).")
                    .clicked()
                {
                    let row_idx = self.target_row_idx;
                    self.delete_row_by_index(row_idx);
                }

                let is_dirty = self.table.tracker.is_dirty();

                if ui
                    .add_enabled(is_dirty, egui::Button::new(RichText::new("🔄 되돌리기").color(Color32::from_rgb(200, 80, 80))))
                    .on_hover_text("모든 셀 수정 및 행 추가/삭제 내역을 원복합니다.")
                    .clicked()
                {
                    self.table.revert_all();
                    self.status_message = "모든 변경사항이 원래대로 되돌려졌습니다.".to_string();
                }

                // Real Safe SQLite Transaction Save Button!
                if ui
                    .add_enabled(
                        is_dirty,
                        egui::Button::new(RichText::new("💾 DB에 저장 (Ctrl+S)").strong().color(Color32::from_rgb(40, 160, 60))),
                    )
                    .on_hover_text("SQLite 트랜잭션을 통해 변경사항을 실제 .db 파일에 안전하게 영구 저장합니다. 저장 전 자동 백업(.bak)이 생성됩니다.")
                    .clicked()
                {
                    self.save_to_sqlite();
                }

                ui.separator();

                if ui
                    .button(RichText::new("⚡ 1,000행 벤치마크").italics())
                    .clicked()
                {
                    self.run_1000_rows_benchmark();
                }
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if let Some(fname) = self.db_service.file_name() {
                    ui.label(RichText::new(format!("📁 DB: {fname}")).strong().color(Color32::LIGHT_GREEN));
                    ui.separator();
                }
                if let Some(tname) = &self.selected_table {
                    ui.label(RichText::new(format!("📋 테이블: {tname}")).strong().color(Color32::from_rgb(255, 215, 0)));
                    ui.separator();
                }
                ui.label(RichText::new(&self.status_message).small().color(Color32::LIGHT_GRAY));
            });
            ui.add_space(4.0);
        });

        // Left Sidebar: Tables List
        Panel::left("tables_sidebar")
            .resizable(true)
            .default_size(210.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.strong("📋 테이블 목록");
                    ui.label(format!("({})", self.table_summaries.len()));
                });
                ui.separator();

                if self.table_summaries.is_empty() {
                    ui.label(RichText::new("열린 데이터베이스가 없거나\n테이블이 없습니다.").weak());
                } else {
                    ScrollArea::vertical().show(ui, |ui| {
                        let current_selected = self.selected_table.clone();
                        let mut clicked_table = None;
                        for table in &self.table_summaries {
                            let is_selected = current_selected.as_deref() == Some(&table.name);
                            let text = format!("📄 {}  ({}행)", table.name, table.row_count);

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

        // Bottom Info & Dirty Tracking Panel
        Panel::bottom("bottom_info_panel")
            .resizable(true)
            .default_size(150.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.strong("📌 변경사항 추적 상태:");

                    let dirty_count = self.table.tracker.dirty_cells_count();
                    let new_rows_count = self.table.tracker.new_rows.len();
                    let deleted_rows_count = self.table.tracker.deleted_rows.len();
                    let total_rows = self.table.row_count();

                    if dirty_count == 0 && new_rows_count == 0 && deleted_rows_count == 0 {
                        ui.colored_label(Color32::GREEN, "● Clean (저장 완료 / 변경사항 없음)");
                    } else {
                        ui.colored_label(
                            Color32::from_rgb(255, 140, 0),
                            format!(
                                "● 미저장 변경: [수정: {}개, 추가: {}개, 삭제: {}개]",
                                dirty_count, new_rows_count, deleted_rows_count
                            ),
                        );
                    }

                    ui.separator();
                    ui.label(format!("현재 표시: {}행 × {}열", total_rows, self.table.column_names.len()));

                    if let Some(schema) = &self.table.schema {
                        ui.separator();
                        let pks = if schema.primary_keys.is_empty() {
                            "None".to_string()
                        } else {
                            schema.primary_keys.join(", ")
                        };
                        ui.label(format!("기본키(PK): 🔑 {}", pks));
                    }

                    if let Some(bench) = &self.benchmark_result {
                        ui.separator();
                        ui.colored_label(Color32::GOLD, bench);
                    }
                });

                ui.separator();

                // Detailed dirty items log
                ScrollArea::vertical().max_height(85.0).show(ui, |ui| {
                    if !self.table.tracker.is_dirty() {
                        ui.label(RichText::new("현재 수정된 셀이나 추가/삭제된 행이 없습니다. 테이블 셀을 더블클릭하거나 복사·붙여넣기, 행 삭제를 실행해보세요.").weak());
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            for (coord, change) in &self.table.tracker.dirty_cells {
                                let col_name = self.table.col_name(coord.col_uid);
                                let row_num = coord.row_uid.0;
                                ui.label(RichText::new(format!(
                                    "[수정] 행 {}, 열 '{}': \"{}\" ➔ \"{}\"",
                                    row_num, col_name, change.old_value, change.new_value
                                )).color(Color32::from_rgb(255, 193, 7)));
                            }
                            for row_uid in &self.table.tracker.new_rows {
                                ui.label(RichText::new(format!(
                                    "[신규 행] 행 ID {} (추가됨)",
                                    row_uid.0
                                )).color(Color32::from_rgb(76, 175, 80)));
                            }
                            for row_uid in &self.table.tracker.deleted_rows {
                                ui.label(RichText::new(format!(
                                    "[삭제 예정] 행 ID {} (제거됨)",
                                    row_uid.0
                                )).color(Color32::from_rgb(244, 67, 54)));
                            }
                        });
                    }
                });
            });

        // Unsaved changes confirmation dialog if switching tables
        if self.show_dirty_switch_dialog {
            let mut switch_now = false;
            let mut cancel = false;
            let pending_name = self.pending_table_switch.clone().unwrap_or_default();

            egui::Window::new("⚠️ 저장되지 않은 변경사항")
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
