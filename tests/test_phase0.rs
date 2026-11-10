use rvariant::{Number, Variant, VariantTy};
use sqlite_editor::clipboard_util::{
    apply_paste_append, apply_paste_overwrite, export_to_tsv, generate_sample_excel_tsv, parse_tsv,
};
use sqlite_editor::model::TrackedTableBackend;
use std::time::Instant;
use tabular_core::backend::{TableBackend, VisualColIdx, VisualRowIdx};
use tabular_core::ColumnUid;

fn create_test_table() -> TrackedTableBackend {
    let columns = vec![
        ("id", VariantTy::Number(rvariant::NumberTy::I64), None),
        ("name", VariantTy::Str, Some(Variant::Str("".into()))),
        ("email", VariantTy::Str, Some(Variant::Str("".into()))),
        ("amount", VariantTy::Number(rvariant::NumberTy::F64), None),
        ("active", VariantTy::Bool, Some(Variant::Bool(true))),
    ];

    let mut table = TrackedTableBackend::new(columns);

    for i in 1..=20 {
        let id = 1000 + i;
        let name = format!("사용자_{i}");
        let email = format!("user{i}@test.com");
        let amount = (i as f64) * 100.0;
        let active = i % 2 == 0;

        table.inner.insert_row(vec![
            (ColumnUid(0), Variant::Number(Number::I64(id))),
            (ColumnUid(1), Variant::Str(name)),
            (ColumnUid(2), Variant::Str(email)),
            (
                ColumnUid(3),
                Variant::Number(Number::F64(rvariant::F64(amount))),
            ),
            (ColumnUid(4), Variant::Bool(active)),
        ]);
    }

    table.snapshot_as_baseline();
    table
}

#[test]
fn test_01_initial_table_and_baseline() {
    let table = create_test_table();

    assert_eq!(table.row_count(), 20, "초기 행 수는 20개여야 합니다.");
    assert_eq!(table.column_names.len(), 5, "열 수는 5개여야 합니다.");
    assert!(
        !table.tracker.is_dirty(),
        "초기 로드 시 Dirty 상태가 아니어야 합니다."
    );
    assert_eq!(table.tracker.dirty_cells_count(), 0);
    assert_eq!(table.tracker.new_rows.len(), 0);
}

#[test]
fn test_02_excel_tsv_generation_and_parsing() {
    let tsv = generate_sample_excel_tsv(20);
    let rows = parse_tsv(&tsv);

    assert_eq!(rows.len(), 20, "20개 행이 파싱되어야 합니다.");
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row.len(), 5, "각 행은 5개 열로 구분되어야 합니다.");
        let expected_id = (1001 + i).to_string();
        assert_eq!(row[0], expected_id);
    }
}

#[test]
fn test_03_paste_overwrite_and_dirty_tracking() {
    let mut table = create_test_table();

    // Generate 20 rows x 5 cols new Excel data
    let new_tsv = generate_sample_excel_tsv(20);
    let new_rows = parse_tsv(&new_tsv);

    // Apply paste overwrite starting at (0, 0)
    let modified = apply_paste_overwrite(&mut table, 0, 0, &new_rows);
    println!("Modified cells count: {modified}");

    assert!(
        table.tracker.is_dirty(),
        "붙여넣기 후 Dirty 상태여야 합니다."
    );
    assert!(
        table.tracker.dirty_cells_count() > 0,
        "수정된 셀이 추적되어야 합니다."
    );

    // Verify cell values changed
    let row0_uid = table.row_uid(VisualRowIdx(0)).unwrap();
    let col2_uid = table.col_uid(VisualColIdx(2)).unwrap(); // email
    let cell_coord = (row0_uid, col2_uid).into();

    let new_email = table.get(cell_coord).unwrap();
    assert_eq!(
        new_email,
        &Variant::Str("user1@example.com".to_string()),
        "이메일이 'user1@example.com'으로 변경되어야 합니다."
    );

    // Check dirty record contains old value and new value
    let change = table.tracker.dirty_cells.get(&cell_coord).unwrap();
    assert_eq!(change.old_value, Variant::Str("user1@test.com".to_string()));
    assert_eq!(
        change.new_value,
        Variant::Str("user1@example.com".to_string())
    );

    // Test revert
    table.revert_all();
    assert!(
        !table.tracker.is_dirty(),
        "되돌리기(Revert) 후 Dirty 상태가 해제되어야 합니다."
    );
    assert_eq!(table.tracker.dirty_cells_count(), 0);

    let reverted_email = table.get(cell_coord).unwrap();
    assert_eq!(
        reverted_email,
        &Variant::Str("user1@test.com".to_string()),
        "되돌리기 후 원래 이메일로 복원되어야 합니다."
    );
}

#[test]
fn test_04_paste_append_as_new_rows() {
    let mut table = create_test_table();
    let initial_count = table.row_count();

    let new_tsv = generate_sample_excel_tsv(20);
    let new_rows = parse_tsv(&new_tsv);

    let added = apply_paste_append(&mut table, &new_rows);
    assert_eq!(added, 20, "20개 행이 신규로 추가되어야 합니다.");
    assert_eq!(
        table.row_count(),
        initial_count + 20,
        "총 행 수는 40개여야 합니다."
    );

    assert_eq!(
        table.tracker.new_rows.len(),
        20,
        "추적기에 20개의 신규 행 ID가 기록되어야 합니다."
    );
    assert!(
        table.tracker.is_dirty(),
        "신규 행 추가 시 Dirty 상태여야 합니다."
    );
}

#[test]
fn test_05_copy_to_excel_tsv_export() {
    let table = create_test_table();

    let exported_tsv = export_to_tsv(&table, None, None);
    let lines: Vec<&str> = exported_tsv.lines().collect();

    assert_eq!(lines.len(), 20, "내보낸 TSV는 20줄이어야 합니다.");

    let first_line_cols: Vec<&str> = lines[0].split('\t').collect();
    assert_eq!(first_line_cols.len(), 5, "내보낸 각 줄은 5열 탭 구분이어야 합니다.");
    assert_eq!(first_line_cols[0], "1001");
    assert_eq!(first_line_cols[1], "사용자_1");
    assert_eq!(first_line_cols[2], "user1@test.com");
}

#[test]
fn test_06_1000_rows_batch_paste_performance() {
    let mut table = create_test_table();

    let start = Instant::now();
    let tsv_1000 = generate_sample_excel_tsv(1000);
    let parsed = parse_tsv(&tsv_1000);
    assert_eq!(parsed.len(), 1000);

    let insert_start = Instant::now();
    let added = apply_paste_append(&mut table, &parsed);
    let insert_dur = insert_start.elapsed();
    let total_dur = start.elapsed();

    assert_eq!(added, 1000);
    assert_eq!(table.row_count(), 1020);
    println!("1,000 rows insert duration: {insert_dur:?}, total: {total_dur:?}");

    // Performance assertion: 1,000 rows batch paste should take less than 1 second
    assert!(
        total_dur.as_millis() < 1000,
        "1,000행 일괄 추가가 1초 이내에 완료되어야 합니다. (실제: {:?})",
        total_dur
    );
}
