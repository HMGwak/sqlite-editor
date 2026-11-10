use rvariant::{Number, Variant};
use sqlite_editor::db::{create_sample_database, DbService};
use sqlite_editor::model::TrackedTableBackend;
use std::fs;
use tabular_core::backend::{TableBackend, VisualRowIdx};
use tabular_core::ColumnUid;

fn setup_temp_db(filename: &str) -> std::path::PathBuf {
    let mut temp_path = std::env::temp_dir();
    temp_path.push(format!("sqlite_editor_test_p24_{}_{}.db", filename, std::process::id()));
    let _ = fs::remove_file(&temp_path);
    create_sample_database(&temp_path).expect("Failed to create sample db");
    temp_path
}

#[test]
fn test_p24_01_row_deletion_and_revert() {
    let db_path = setup_temp_db("01");
    let mut db = DbService::new();
    db.open(&db_path).unwrap();

    let (schema, rows) = db.load_table_data("customers", None).unwrap();
    let mut backend = TrackedTableBackend::from_db_table(schema, rows);

    let row1_uid = backend.row_uid(VisualRowIdx(1)).unwrap();
    assert!(!backend.tracker.is_row_deleted(row1_uid));
    assert!(!backend.is_row_skipped(row1_uid));

    // Mark row 1 for deletion
    backend.mark_row_deleted(row1_uid);
    assert!(backend.tracker.is_row_deleted(row1_uid));
    assert!(backend.is_row_skipped(row1_uid));
    assert!(backend.tracker.is_dirty());

    // Revert all
    backend.revert_all();
    assert!(!backend.tracker.is_row_deleted(row1_uid));
    assert!(!backend.is_row_skipped(row1_uid));
    assert!(!backend.tracker.is_dirty());

    let _ = fs::remove_file(db_path);
}

#[test]
fn test_p24_02_actual_sqlite_save_transactions_and_backup() {
    let db_path = setup_temp_db("02");
    let mut db = DbService::new();
    db.open(&db_path).unwrap();

    let (schema, rows) = db.load_table_data("customers", None).unwrap();
    let mut backend = TrackedTableBackend::from_db_table(schema.clone(), rows);

    // 1. UPDATE: Modify row 0 email
    let row0_uid = backend.row_uid(VisualRowIdx(0)).unwrap();
    let col2_uid = ColumnUid(2); // email
    backend.set((row0_uid, col2_uid).into(), Variant::Str("chulsoo_new@company.com".to_string()));

    // 2. DELETE: Delete row 2 (박민수)
    let row2_uid = backend.row_uid(VisualRowIdx(2)).unwrap();
    backend.mark_row_deleted(row2_uid);

    // 3. INSERT: Add a brand new customer
    let new_row_uid = backend.create_row(vec![
        (ColumnUid(0), Variant::Number(Number::I64(999))), // id
        (ColumnUid(1), Variant::Str("새고객".to_string())), // name
        (ColumnUid(2), Variant::Str("newbie@example.com".to_string())), // email
        (ColumnUid(3), Variant::Number(Number::F64(rvariant::F64(50000.0)))), // balance
        (ColumnUid(4), Variant::Bool(true)), // is_active
        (ColumnUid(5), Variant::Str("신규 가입".to_string())), // memo
    ]).unwrap();
    assert!(backend.tracker.is_row_new(new_row_uid));

    // Execute Save to SQLite
    let summary = db.save_changes("customers", &schema, &backend.tracker, &backend)
        .expect("Save to SQLite should succeed");

    println!("Save Summary: {summary:?}");
    assert_eq!(summary.updated_count, 1, "1개 행이 UPDATE 되어야 합니다.");
    assert_eq!(summary.deleted_count, 1, "1개 행이 DELETE 되어야 합니다.");
    assert_eq!(summary.inserted_count, 1, "1개 행이 INSERT 되어야 합니다.");

    // Verify Backup File was created
    assert!(summary.backup_path.is_some(), "백업 파일이 생성되어야 합니다.");
    let backup_path = summary.backup_path.unwrap();
    assert!(backup_path.exists(), "백업 파일이 디스크에 실존해야 합니다.");
    println!("Verified backup created at: {:?}", backup_path);

    // Verify mutations in SQLite database by reading freshly from DB
    let conn = db.conn.as_ref().unwrap();

    // Check UPDATE
    let updated_email: String = conn.query_row(
        "SELECT email FROM customers WHERE id = 1;",
        [],
        |r| r.get(0)
    ).unwrap();
    assert_eq!(updated_email, "chulsoo_new@company.com", "DB 파일의 이메일이 업데이트되어야 합니다.");

    // Check DELETE
    let deleted_exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM customers WHERE id = 3;", // 박민수 id = 3
        [],
        |r| r.get(0)
    ).unwrap();
    assert!(!deleted_exists, "삭제된 고객(id 3)은 DB에 존재하지 않아야 합니다.");

    // Check INSERT
    let new_name: String = conn.query_row(
        "SELECT name FROM customers WHERE id = 999;",
        [],
        |r| r.get(0)
    ).unwrap();
    assert_eq!(new_name, "새고객", "신규 고객이 DB에 삽입되어야 합니다.");

    // Cleanup
    let _ = fs::remove_file(db_path);
    let _ = fs::remove_file(backup_path);
}

#[test]
fn test_p24_03_transaction_rollback_on_failure() {
    let db_path = setup_temp_db("03");
    let mut db = DbService::new();
    db.open(&db_path).unwrap();

    let (schema, rows) = db.load_table_data("customers", None).unwrap();
    let mut backend = TrackedTableBackend::from_db_table(schema.clone(), rows);

    // 1. Valid update
    let row0_uid = backend.row_uid(VisualRowIdx(0)).unwrap();
    let col2_uid = ColumnUid(2); // email
    backend.set((row0_uid, col2_uid).into(), Variant::Str("valid_update@test.com".to_string()));

    // 2. INVALID insertion: Duplicate email (UNIQUE constraint violation)
    // 'younghee@example.com' already exists for customer 2
    let _ = backend.create_row(vec![
        (ColumnUid(0), Variant::Number(Number::I64(999))),
        (ColumnUid(1), Variant::Str("중복이메일고객".to_string())),
        (ColumnUid(2), Variant::Str("younghee@example.com".to_string())), // DUPLICATE!
    ]).unwrap();

    // Attempt save - should FAIL and ROLLBACK!
    let result = db.save_changes("customers", &schema, &backend.tracker, &backend);
    assert!(result.is_err(), "UNIQUE 제약조건 위반으로 저장이 실패해야 합니다.");
    println!("Expected error captured: {:?}", result.unwrap_err());

    // Verify Rollback: row 0 email should STILL BE original in DB!
    let conn = db.conn.as_ref().unwrap();
    let email_in_db: String = conn.query_row(
        "SELECT email FROM customers WHERE id = 1;",
        [],
        |r| r.get(0)
    ).unwrap();
    assert_eq!(
        email_in_db, "chulsoo@example.com",
        "롤백으로 인해 유효한 업데이트도 원복되어 원래 이메일이어야 합니다."
    );

    let _ = fs::remove_file(db_path);
}

#[test]
fn test_p24_04_compact_database_vacuum() {
    let db_path = setup_temp_db("04");
    let mut db = DbService::new();
    db.open(&db_path).unwrap();

    // Insert dummy table with 500 rows then drop/delete to create free pages
    {
        let conn = db.conn.as_ref().unwrap();
        conn.execute_batch(
            "CREATE TABLE temp_bloat (id INTEGER PRIMARY KEY, big_text TEXT);
             BEGIN TRANSACTION;
            "
        ).unwrap();
        for i in 0..500 {
            conn.execute("INSERT INTO temp_bloat VALUES (?1, zeroblob(4096));", [i]).unwrap();
        }
        conn.execute_batch("COMMIT; PRAGMA wal_checkpoint(TRUNCATE); DROP TABLE temp_bloat; PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    }

    // Call compact_database (VACUUM)
    let (before_bytes, after_bytes) = db.compact_database().unwrap();
    println!("Database compaction (VACUUM): {before_bytes} bytes -> {after_bytes} bytes");

    assert!(before_bytes > after_bytes, "VACUUM 후 파일 용량이 줄어들어야 합니다.");
    assert!(after_bytes > 0, "유효한 DB 파일 크기가 유지되어야 합니다.");

    let _ = fs::remove_file(db_path);
}

#[test]
fn test_p24_05_journal_mode_switching_wal_and_delete() {
    let db_path = setup_temp_db("05");
    let mut db = DbService::new();
    db.open(&db_path).unwrap();

    // 1. Initial sample DB has WAL mode
    let initial_mode = db.get_journal_mode().unwrap();
    assert_eq!(initial_mode, "WAL", "샘플 DB는 초기에 WAL 모드여야 합니다.");

    // Perform an insert
    {
        let conn = db.conn.as_ref().unwrap();
        conn.execute("INSERT INTO customers (name, email) VALUES (?1, ?2);", ["테스트고객", "test_journal@example.com"]).unwrap();
    }

    // 2. Switch to DELETE mode (Rollback Journal, single file)
    let set_result = db.set_journal_mode("DELETE").unwrap();
    assert_eq!(set_result, "DELETE", "set_journal_mode(\"DELETE\") 반환값은 DELETE여야 합니다.");
    assert_eq!(db.get_journal_mode().unwrap(), "DELETE", "현재 모드가 DELETE로 조회되어야 합니다.");

    // Verify -wal and -shm files are removed / cleaned up
    let file_str = db_path.to_string_lossy();
    let wal_file = std::path::PathBuf::from(format!("{file_str}-wal"));
    let shm_file = std::path::PathBuf::from(format!("{file_str}-shm"));
    assert!(!wal_file.exists(), "DELETE 모드 전환 후 .db-wal 파일이 정리되어 존재하지 않아야 합니다.");
    assert!(!shm_file.exists(), "DELETE 모드 전환 후 .db-shm 파일이 정리되어 존재하지 않아야 합니다.");

    // 3. Verify data persists with zero data loss
    {
        let conn = db.conn.as_ref().unwrap();
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM customers WHERE email = 'test_journal@example.com';",
            [],
            |r| r.get(0)
        ).unwrap();
        assert!(exists, "저널 모드 전환 후에도 데이터가 유실 없이 안전하게 보존되어야 합니다.");
    }

    // 4. Switch back to WAL mode
    let set_wal_result = db.set_journal_mode("WAL").unwrap();
    assert_eq!(set_wal_result, "WAL", "set_journal_mode(\"WAL\") 반환값은 WAL이어야 합니다.");
    assert_eq!(db.get_journal_mode().unwrap(), "WAL", "현재 모드가 WAL로 조회되어야 합니다.");

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_file(wal_file);
    let _ = fs::remove_file(shm_file);
}
