use rvariant::{Number, Variant, VariantTy};
use sqlite_editor::db::{create_sample_database, DbService};
use sqlite_editor::model::TrackedTableBackend;
use std::fs;
use tabular_core::backend::{TableBackend, VisualColIdx, VisualRowIdx};

fn setup_temp_db(filename: &str) -> std::path::PathBuf {
    let mut temp_path = std::env::temp_dir();
    temp_path.push(format!("sqlite_editor_test_{}_{}.db", filename, std::process::id()));
    let _ = fs::remove_file(&temp_path);
    create_sample_database(&temp_path).expect("Failed to create sample db");
    temp_path
}

#[test]
fn test_phase1_01_open_db_and_list_tables() {
    let db_path = setup_temp_db("01");
    let mut db = DbService::new();
    assert!(!db.is_open());

    db.open(&db_path).expect("Failed to open database");
    assert!(db.is_open());
    assert!(db.file_name().unwrap().contains("sqlite_editor_test_01"));

    let tables = db.list_tables().expect("Failed to list tables");
    assert_eq!(tables.len(), 3, "customers, orders, products 3개 테이블이 존재해야 합니다.");

    // Check table names
    let names: Vec<String> = tables.iter().map(|t| t.name.clone()).collect();
    assert!(names.contains(&"customers".to_string()));
    assert!(names.contains(&"products".to_string()));
    assert!(names.contains(&"orders".to_string()));

    // Check row counts
    let cust_summary = tables.iter().find(|t| t.name == "customers").unwrap();
    assert_eq!(cust_summary.row_count, 5);
    assert_eq!(cust_summary.column_count, 6);

    let prod_summary = tables.iter().find(|t| t.name == "products").unwrap();
    assert_eq!(prod_summary.row_count, 5);
    assert_eq!(prod_summary.column_count, 6);

    let order_summary = tables.iter().find(|t| t.name == "orders").unwrap();
    assert_eq!(order_summary.row_count, 4);
    assert_eq!(order_summary.column_count, 5);

    let _ = fs::remove_file(db_path);
}

#[test]
fn test_phase1_02_schema_inspection_and_pk_detection() {
    let db_path = setup_temp_db("02");
    let mut db = DbService::new();
    db.open(&db_path).unwrap();

    // 1. customers table schema
    let cust_schema = db.get_table_schema("customers").unwrap();
    assert_eq!(cust_schema.table_name, "customers");
    assert_eq!(cust_schema.primary_keys, vec!["id"]);
    assert_eq!(cust_schema.pk_indices(), vec![0]);

    let id_col = &cust_schema.columns[0];
    assert_eq!(id_col.name, "id");
    assert!(id_col.is_pk);
    assert!(matches!(id_col.variant_ty, VariantTy::Number(_)));

    let email_col = cust_schema.columns.iter().find(|c| c.name == "email").unwrap();
    assert!(matches!(email_col.variant_ty, VariantTy::Str));
    assert!(email_col.not_null);

    let balance_col = cust_schema.columns.iter().find(|c| c.name == "balance").unwrap();
    assert!(matches!(balance_col.variant_ty, VariantTy::Number(_)));

    let is_active_col = cust_schema.columns.iter().find(|c| c.name == "is_active").unwrap();
    assert!(matches!(is_active_col.variant_ty, VariantTy::Bool));

    // 2. products table schema (TEXT PK)
    let prod_schema = db.get_table_schema("products").unwrap();
    assert_eq!(prod_schema.primary_keys, vec!["sku"]);
    assert!(prod_schema.columns[0].is_pk);
    assert_eq!(prod_schema.columns[0].name, "sku");

    let _ = fs::remove_file(db_path);
}

#[test]
fn test_phase1_03_load_table_data_and_type_conversions() {
    let db_path = setup_temp_db("03");
    let mut db = DbService::new();
    db.open(&db_path).unwrap();

    let (schema, rows) = db.load_table_data("customers", None).unwrap();
    assert_eq!(schema.table_name, "customers");
    assert_eq!(rows.len(), 5);

    // Row 0: ('김철수', 'chulsoo@example.com', 125000.50, 1, 'VIP 고객')
    let row0 = &rows[0];
    assert_eq!(row0[0], Variant::Number(Number::I64(1)));
    assert_eq!(row0[1], Variant::Str("김철수".to_string()));
    assert_eq!(row0[2], Variant::Str("chulsoo@example.com".to_string()));
    match &row0[3] {
        Variant::Number(Number::F64(f)) => {
            assert!((f.0 - 125000.50).abs() < 0.001);
        }
        other => panic!("Expected F64 for balance, got {other:?}"),
    }
    assert_eq!(row0[4], Variant::Number(Number::I64(1))); // Stored as integer 1 in sqlite

    let _ = fs::remove_file(db_path);
}

#[test]
fn test_phase1_04_grid_binding_and_table_switching() {
    let db_path = setup_temp_db("04");
    let mut db = DbService::new();
    db.open(&db_path).unwrap();

    // 1. Load customers table
    let (cust_schema, cust_rows) = db.load_table_data("customers", None).unwrap();
    let mut backend = TrackedTableBackend::from_db_table(cust_schema, cust_rows);

    assert_eq!(backend.row_count(), 5);
    assert_eq!(backend.column_names.len(), 6);
    assert!(backend.column_names[0].contains("[PK] id"));
    assert!(!backend.tracker.is_dirty(), "로드 직후에는 Clean 상태여야 합니다.");

    // Verify cell access
    let row0_uid = backend.row_uid(VisualRowIdx(0)).unwrap();
    let col1_uid = backend.col_uid(VisualColIdx(1)).unwrap();
    let name_val = backend.get((row0_uid, col1_uid).into()).unwrap();
    assert_eq!(name_val, &Variant::Str("김철수".to_string()));

    // 2. Switch to products table
    let (prod_schema, prod_rows) = db.load_table_data("products", None).unwrap();
    backend.load_db_table(prod_schema, prod_rows);

    assert_eq!(backend.row_count(), 5);
    assert_eq!(backend.column_names.len(), 6);
    assert!(backend.column_names[0].contains("[PK] sku"));
    assert!(!backend.tracker.is_dirty());

    let prod_row0_uid = backend.row_uid(VisualRowIdx(0)).unwrap();
    let prod_col0_uid = backend.col_uid(VisualColIdx(0)).unwrap();
    let sku_val = backend.get((prod_row0_uid, prod_col0_uid).into()).unwrap();
    assert_eq!(sku_val, &Variant::Str("SKU-1001".to_string()));

    // 3. Switch to orders table
    let (order_schema, order_rows) = db.load_table_data("orders", None).unwrap();
    backend.load_db_table(order_schema, order_rows);

    assert_eq!(backend.row_count(), 4);
    assert_eq!(backend.column_names.len(), 5);
    assert!(backend.column_names[0].contains("[PK] order_id"));
    assert!(!backend.tracker.is_dirty());

    let _ = fs::remove_file(db_path);
}
