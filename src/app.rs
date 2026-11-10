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
    pub show_help_modal: bool,
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

    // 3. Put Korean font at the top priority of proportional and monospace font families
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

impl SqliteEditorApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Setup Korean fonts for crisp CJK glyph rendering
        setup_korean_fonts(&cc.egui_ctx);

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
            status_message: "준비 완료".to_string(),
            last_action_time: None,
            benchmark_result: None,
            target_row_idx: 0,
            pending_table_switch: None,
            show_dirty_switch_dialog: false,
            show_help_modal: false,
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

    fn paste_from_clipboard_overwrite(&mut self) {
        if let Ok(mut cb) = Clipboard::new() {
            if let Ok(text) = cb.get_text() {
                let parsed = parse_tsv(&text);
                if parsed.is_empty() {
                    self.status_message = "클립보드에 유효한 테이블 데이터가 없습니다.".to_string();
                    return;
                }
                let modified = apply_paste_overwrite(&mut self.table, 0, 0, &parsed);
                self.status_message = format!(
                    "클립보드에서 {}개 행 덮어쓰기 완료 (수정 셀: {}개)",
                    parsed.len(),
                    modified
                );
                self.last_action_time = Some(Instant::now());
                return;
            }
        }
        self.status_message = "클립보드 읽기에 실패했습니다.".to_string();
    }

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
            "1,000행 일괄 처리: 파싱 {:?}, 행 추가({}행) {:?}, 총 {:?}",
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

        // Keyboard Shortcuts
        let ctrl_or_cmd = ctx.input(|i| i.modifiers.command || i.modifiers.ctrl);
        let shift = ctx.input(|i| i.modifiers.shift);

        // F1: Help Dialog Toggle
        if ctx.input(|i| i.key_pressed(Key::F1)) {
            self.show_help_modal = !self.show_help_modal;
        }

        // Ctrl+S / Cmd+S: Save to SQLite
        if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::S)) {
            if self.table.tracker.is_dirty() {
                self.save_to_sqlite();
            }
        }

        // Ctrl+V: Paste Overwrite / Ctrl+Shift+V: Paste Append
        if ctrl_or_cmd && ctx.input(|i| i.key_pressed(Key::V)) {
            if shift {
                self.paste_from_clipboard_append();
            } else {
                self.paste_from_clipboard_overwrite();
            }
        }

        // Top Control Panel (Clean, uncluttered, professional)
        Panel::top("top_toolbar").show(ui, |ui| {
            ui.add_space(5.0);
            ui.horizontal_wrapped(|ui| {
                ui.strong(RichText::new("SQLite Editor").size(15.0));
                ui.separator();

                // Group 1: Database File
                if ui.button("열기").on_hover_text("로컬 SQLite 파일(.db) 선택").clicked() {
                    self.open_db_file();
                }
                if ui.button("샘플 DB").on_hover_text("이커머스 샘플 DB 생성 및 열기").clicked() {
                    self.load_sample_database();
                }
                if ui.button("새로고침").on_hover_text("현재 테이블 다시 읽기").clicked() {
                    self.refresh_current();
                }

                ui.separator();

                // Group 2: Clipboard
                if ui.button("복사").on_hover_text("전체 테이블을 엑셀(TSV)로 복사 (Ctrl+C)").clicked() {
                    self.copy_table_to_clipboard();
                }
                if ui.button("붙여넣기").on_hover_text("클립보드 내용으로 기존 셀 덮어쓰기 (Ctrl+V)").clicked() {
                    self.paste_from_clipboard_overwrite();
                }
                if ui.button("신규행 붙여넣기").on_hover_text("클립보드 데이터를 새 행으로 추가 (Ctrl+Shift+V)").clicked() {
                    self.paste_from_clipboard_append();
                }

                ui.separator();

                // Group 3: Row Operations
                if ui.button("행 추가").on_hover_text("테이블 끝에 새 빈 행 추가").clicked() {
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
                    .button(RichText::new("행 삭제/복원").color(Color32::from_rgb(215, 60, 60)))
                    .on_hover_text("지정한 행을 삭제/복원 (좌측 행 번호 우클릭으로도 가능)")
                    .clicked()
                {
                    let row_idx = self.target_row_idx;
                    self.delete_row_by_index(row_idx);
                }

                ui.separator();

                // Group 4: Save & Revert
                let is_dirty = self.table.tracker.is_dirty();

                if ui
                    .add_enabled(is_dirty, egui::Button::new(RichText::new("되돌리기").color(Color32::from_rgb(180, 70, 70))))
                    .on_hover_text("모든 수정 내역을 원래대로 되돌림")
                    .clicked()
                {
                    self.table.revert_all();
                    self.status_message = "모든 변경사항이 원복되었습니다.".to_string();
                }

                if ui
                    .add_enabled(
                        is_dirty,
                        egui::Button::new(RichText::new("저장").strong().color(Color32::from_rgb(34, 139, 34))),
                    )
                    .on_hover_text("자동 백업 생성 후 SQLite에 안전하게 영구 저장 (Ctrl+S)")
                    .clicked()
                {
                    self.save_to_sqlite();
                }

                ui.separator();

                // Group 5: Help & Utilities
                if ui.button("도움말 (F1)").on_hover_text("단축키 및 상세 가이드").clicked() {
                    self.show_help_modal = true;
                }

                if ui.button("1,000행 벤치").on_hover_text("1,000행 일괄 처리 성능 측정").clicked() {
                    self.run_1000_rows_benchmark();
                }
            });

            ui.add_space(3.0);

            // Clean single-line info strip under toolbar
            ui.horizontal(|ui| {
                if let Some(fname) = self.db_service.file_name() {
                    ui.label(RichText::new(format!("DB: {fname}")).strong());
                    ui.separator();
                }
                if let Some(tname) = &self.selected_table {
                    ui.label(RichText::new(format!("테이블: {tname}")).strong().color(Color32::from_rgb(0, 102, 204)));
                    ui.separator();
                }
                ui.label(RichText::new(&self.status_message).small().color(Color32::DARK_GRAY));
            });
            ui.add_space(3.0);
        });

        // Left Sidebar: Tables List (Clean and neat)
        Panel::left("tables_sidebar")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.strong("테이블");
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
                            let text = format!("{} ({})", table.name, table.row_count);

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

        // Bottom Compact Status Bar (Replaces 160px height panel with compact 30px bar)
        Panel::bottom("bottom_info_panel")
            .resizable(false)
            .default_size(28.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let dirty_count = self.table.tracker.dirty_cells_count();
                    let new_rows_count = self.table.tracker.new_rows.len();
                    let deleted_rows_count = self.table.tracker.deleted_rows.len();
                    let total_rows = self.table.row_count();

                    if dirty_count == 0 && new_rows_count == 0 && deleted_rows_count == 0 {
                        ui.colored_label(Color32::from_rgb(46, 125, 50), "저장됨 (변경 없음)");
                    } else {
                        ui.colored_label(
                            Color32::from_rgb(230, 81, 0),
                            format!(
                                "미저장: 수정 {} / 추가 {} / 삭제 {}",
                                dirty_count, new_rows_count, deleted_rows_count
                            ),
                        );
                    }

                    ui.separator();
                    ui.label(format!("행: {} | 열: {}", total_rows, self.table.column_names.len()));

                    if let Some(schema) = &self.table.schema {
                        if !schema.primary_keys.is_empty() {
                            ui.separator();
                            ui.label(format!("기본키: {}", schema.primary_keys.join(", ")));
                        }
                    }

                    ui.separator();
                    ui.label(RichText::new("도움말 및 단축키: F1").weak());

                    if let Some(bench) = &self.benchmark_result {
                        ui.separator();
                        ui.colored_label(Color32::from_rgb(180, 100, 0), bench);
                    }
                });
            });

        // F1 Help Modal Dialog
        if self.show_help_modal {
            egui::Window::new("도움말 및 단축키 (F1)")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.set_width(460.0);
                    ui.heading("단축키 안내");
                    ui.add_space(4.0);
                    egui::Grid::new("help_shortcuts_grid")
                        .striped(true)
                        .num_columns(2)
                        .spacing([24.0, 8.0])
                        .show(ui, |ui| {
                            ui.strong("Ctrl + S");
                            ui.label("데이터베이스에 안전하게 영구 저장 (자동 백업 생성)");
                            ui.end_row();

                            ui.strong("Ctrl + V");
                            ui.label("클립보드 엑셀 데이터를 현재 셀부터 덮어쓰기");
                            ui.end_row();

                            ui.strong("Ctrl + Shift + V");
                            ui.label("클립보드 데이터를 새 행으로 일괄 추가");
                            ui.end_row();

                            ui.strong("Ctrl + C");
                            ui.label("전체 테이블을 엑셀(TSV) 형식으로 클립보드에 복사");
                            ui.end_row();

                            ui.strong("더블클릭 / Enter");
                            ui.label("선택한 셀 내용 직접 편집");
                            ui.end_row();

                            ui.strong("Tab / Shift + Tab");
                            ui.label("다음/이전 셀로 이동 및 편집 확정");
                            ui.end_row();

                            ui.strong("Esc");
                            ui.label("셀 편집 취소 또는 팝업 창 닫기");
                            ui.end_row();

                            ui.strong("행 번호 우클릭");
                            ui.label("해당 행 삭제/복원 ('Skip row' 체크 시 취소선 표시)");
                            ui.end_row();
                        });

                    ui.add_space(12.0);
                    ui.separator();
                    ui.heading("안전 저장 및 백업 원칙");
                    ui.add_space(4.0);
                    ui.label("• 셀을 수정하거나 행을 추가/삭제해도 즉시 파일에 쓰이지 않습니다.");
                    ui.label("• '저장(Ctrl+S)'을 누르면 원본 DB가 자동으로 백업(*.bak)된 후 반영됩니다.");
                    ui.label("• 저장 전 언제든 '되돌리기' 버튼으로 모든 변경사항을 원복할 수 있습니다.");

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
