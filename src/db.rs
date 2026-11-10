use crate::model::{DirtyTracker, TrackedTableBackend};
use rvariant::{Number, NumberTy, Variant, VariantTy};
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use tabular_core::backend::TableBackend;
use tabular_core::{ColumnUid, RowUid};

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnInfo {
    pub cid: i64,
    pub name: String,
    pub data_type: String,
    pub not_null: bool,
    pub default_value: Option<String>,
    pub is_pk: bool,
    pub variant_ty: VariantTy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableSchema {
    pub table_name: String,
    pub columns: Vec<ColumnInfo>,
    pub primary_keys: Vec<String>,
}

impl TableSchema {
    pub fn pk_indices(&self) -> Vec<usize> {
        self.columns
            .iter()
            .enumerate()
            .filter(|(_, col)| col.is_pk)
            .map(|(idx, _)| idx)
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct TableSummary {
    pub name: String,
    pub row_count: usize,
    pub column_count: usize,
}

#[derive(Debug, Clone)]
pub struct SaveSummary {
    pub inserted_count: usize,
    pub updated_count: usize,
    pub deleted_count: usize,
    pub backup_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum SaveError {
    NotOpen,
    Validation(String),
    Sqlite(rusqlite::Error),
    Backup(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::NotOpen => write!(f, "데이터베이스가 열려있지 않습니다."),
            SaveError::Validation(msg) => write!(f, "유효성 검사 실패: {msg}"),
            SaveError::Sqlite(e) => write!(f, "SQLite 오류: {e}"),
            SaveError::Backup(msg) => write!(f, "백업 생성 오류: {msg}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<rusqlite::Error> for SaveError {
    fn from(e: rusqlite::Error) -> Self {
        SaveError::Sqlite(e)
    }
}

pub struct DbService {
    pub db_path: Option<PathBuf>,
    pub conn: Option<Connection>,
}

impl DbService {
    pub fn new() -> Self {
        Self {
            db_path: None,
            conn: None,
        }
    }

    /// Open a database file at the given path
    pub fn open<P: AsRef<Path>>(&mut self, path: P) -> Result<(), rusqlite::Error> {
        let path_buf = path.as_ref().to_path_buf();
        let conn = Connection::open(&path_buf)?;

        // Enable WAL mode and foreign keys for safety and performance
        let _ = conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;");

        self.db_path = Some(path_buf);
        self.conn = Some(conn);
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.conn.is_some()
    }

    pub fn file_name(&self) -> Option<String> {
        self.db_path.as_ref().and_then(|p| {
            p.file_name()
                .map(|f| f.to_string_lossy().into_owned())
        })
    }

    /// Get list of user tables (excluding SQLite internal tables) with row counts
    pub fn list_tables(&self) -> Result<Vec<TableSummary>, rusqlite::Error> {
        let conn = match &self.conn {
            Some(c) => c,
            None => return Ok(Vec::new()),
        };

        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master 
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' 
             ORDER BY name ASC;",
        )?;

        let table_names: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        let mut summaries = Vec::new();
        for name in table_names {
            let count_query = format!("SELECT COUNT(*) FROM \"{}\";", name.replace('"', "\"\""));
            let row_count: usize = conn
                .query_row(&count_query, [], |r| r.get::<_, i64>(0))
                .map(|cnt| cnt as usize)
                .unwrap_or(0);

            let pragma_query = format!("PRAGMA table_info(\"{}\");", name.replace('"', "\"\""));
            let mut pragma_stmt = conn.prepare(&pragma_query)?;
            let col_count = pragma_stmt.query_map([], |_| Ok(()))?.count();

            summaries.push(TableSummary {
                name,
                row_count,
                column_count: col_count,
            });
        }

        Ok(summaries)
    }

    /// Get schema info for a specific table
    pub fn get_table_schema(&self, table_name: &str) -> Result<TableSchema, rusqlite::Error> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| rusqlite::Error::InvalidQuery)?;

        let pragma_query = format!("PRAGMA table_info(\"{}\");", table_name.replace('"', "\"\""));
        let mut stmt = conn.prepare(&pragma_query)?;

        let mut columns = Vec::new();
        let mut primary_keys = Vec::new();

        let rows = stmt.query_map([], |row| {
            let cid: i64 = row.get(0)?;
            let name: String = row.get(1)?;
            let data_type: String = row.get(2)?;
            let not_null: bool = row.get::<_, i64>(3)? != 0;
            let default_value: Option<String> = row.get(4)?;
            let pk_val: i64 = row.get(5)?;
            let is_pk = pk_val > 0;

            Ok((cid, name, data_type, not_null, default_value, is_pk))
        })?;

        for row in rows {
            let (cid, name, data_type, not_null, default_value, is_pk) = row?;
            let variant_ty = map_sqlite_type_to_variant(&data_type);

            if is_pk {
                primary_keys.push(name.clone());
            }

            columns.push(ColumnInfo {
                cid,
                name,
                data_type,
                not_null,
                default_value,
                is_pk,
                variant_ty,
            });
        }

        Ok(TableSchema {
            table_name: table_name.to_string(),
            columns,
            primary_keys,
        })
    }

    /// Load all rows from a table into 2D Variants
    pub fn load_table_data(
        &self,
        table_name: &str,
        limit: Option<usize>,
    ) -> Result<(TableSchema, Vec<Vec<Variant>>), rusqlite::Error> {
        let schema = self.get_table_schema(table_name)?;
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| rusqlite::Error::InvalidQuery)?;

        let query = if let Some(limit) = limit {
            format!(
                "SELECT * FROM \"{}\" LIMIT {};",
                table_name.replace('"', "\"\""),
                limit
            )
        } else {
            format!("SELECT * FROM \"{}\";", table_name.replace('"', "\"\""))
        };

        let mut stmt = conn.prepare(&query)?;
        let num_cols = schema.columns.len();

        let mut data = Vec::new();
        let mut rows = stmt.query([])?;

        while let Some(row) = rows.next()? {
            let mut row_vals = Vec::with_capacity(num_cols);
            for col_idx in 0..num_cols {
                let val_ref = row.get_ref(col_idx)?;
                let variant = match val_ref {
                    ValueRef::Null => Variant::Empty,
                    ValueRef::Integer(i) => Variant::Number(Number::I64(i)),
                    ValueRef::Real(f) => Variant::Number(Number::F64(rvariant::F64(f))),
                    ValueRef::Text(t) => {
                        Variant::Str(String::from_utf8_lossy(t).into_owned())
                    }
                    ValueRef::Blob(b) => {
                        Variant::Str(format!("<BLOB: {} bytes>", b.len()))
                    }
                };
                row_vals.push(variant);
            }
            data.push(row_vals);
        }

        Ok((schema, data))
    }

    /// Execute an arbitrary SQL query (e.g. SELECT with JOIN/GROUP BY) and return (columns, rows)
    pub fn execute_query(
        &self,
        sql: &str,
        max_rows: Option<usize>,
    ) -> Result<(Vec<String>, Vec<Vec<Variant>>), rusqlite::Error> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| rusqlite::Error::InvalidQuery)?;

        let mut stmt = conn.prepare(sql)?;
        let column_names: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let num_cols = column_names.len();

        let mut data = Vec::new();
        let mut rows = stmt.query([])?;
        let limit = max_rows.unwrap_or(10_000);

        while let Some(row) = rows.next()? {
            let mut row_vals = Vec::with_capacity(num_cols);
            for col_idx in 0..num_cols {
                let val_ref = row.get_ref(col_idx)?;
                let variant = match val_ref {
                    ValueRef::Null => Variant::Empty,
                    ValueRef::Integer(i) => Variant::Number(Number::I64(i)),
                    ValueRef::Real(f) => Variant::Number(Number::F64(rvariant::F64(f))),
                    ValueRef::Text(t) => {
                        Variant::Str(String::from_utf8_lossy(t).into_owned())
                    }
                    ValueRef::Blob(b) => {
                        Variant::Str(format!("<BLOB: {} bytes>", b.len()))
                    }
                };
                row_vals.push(variant);
            }
            data.push(row_vals);
            if data.len() >= limit {
                break;
            }
        }

        Ok((column_names, data))
    }

    /// Atomically save in-memory modifications (updates, inserts, deletes) into SQLite.
    /// Creates an online backup prior to mutating the database.
    pub fn save_changes(
        &mut self,
        table_name: &str,
        schema: &TableSchema,
        tracker: &DirtyTracker,
        backend: &TrackedTableBackend,
    ) -> Result<SaveSummary, SaveError> {
        let conn = self.conn.as_mut().ok_or(SaveError::NotOpen)?;

        // 1. Create Online Backup if writing to a disk file
        let mut backup_path_created = None;
        if let Some(db_path) = &self.db_path {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let bak_path = db_path.with_extension(format!("bak.{secs}.db"));

            // Use SQLite Online Backup API
            match Connection::open(&bak_path) {
                Ok(mut dst_conn) => {
                    match rusqlite::backup::Backup::new(conn, &mut dst_conn) {
                        Ok(backup) => {
                            if let Err(e) = backup.run_to_completion(
                                100,
                                std::time::Duration::from_millis(5),
                                None,
                            ) {
                                return Err(SaveError::Backup(format!("온라인 백업 실행 실패: {e}")));
                            }
                            backup_path_created = Some(bak_path);
                        }
                        Err(e) => {
                            return Err(SaveError::Backup(format!("온라인 백업 생성 실패: {e}")));
                        }
                    }
                }
                Err(e) => {
                    return Err(SaveError::Backup(format!("백업 연결 생성 실패: {e}")));
                }
            }
        }

        // 2. PK Validation
        let pk_indices = schema.pk_indices();
        let is_single_int_pk = !schema.columns.is_empty()
            && pk_indices.len() == 1
            && schema.columns[pk_indices[0]]
                .data_type
                .to_uppercase()
                .contains("INT");

        for row_uid in &tracker.new_rows {
            if tracker.deleted_rows.contains(row_uid) {
                continue;
            }
            for &pk_idx in &pk_indices {
                let col = &schema.columns[pk_idx];
                let col_uid = ColumnUid(pk_idx as u32);
                let val = backend.get((*row_uid, col_uid).into()).unwrap_or(&Variant::Empty);
                if matches!(val, Variant::Empty) && !is_single_int_pk {
                    return Err(SaveError::Validation(format!(
                        "신규 행 {}의 기본키 '{}' 값이 비어있습니다.",
                        row_uid.0, col.name
                    )));
                }
            }
        }

        // 3. Begin Transaction
        let tx = conn.transaction()?;

        let mut deleted_count = 0;
        let mut inserted_count = 0;
        let mut updated_count = 0;

        // --- A. Process Deletions ---
        for row_uid in &tracker.deleted_rows {
            if tracker.new_rows.contains(row_uid) {
                continue; // Added and deleted in memory before saving
            }

            let mut where_clauses = Vec::new();
            let mut params: Vec<rusqlite::types::Value> = Vec::new();

            if !pk_indices.is_empty() {
                for &idx in &pk_indices {
                    let col = &schema.columns[idx];
                    let col_uid = ColumnUid(idx as u32);
                    let coord = (*row_uid, col_uid).into();
                    let orig_val = tracker.baseline_cells.get(&coord).unwrap_or(&Variant::Empty);
                    where_clauses.push(format!("\"{}\" = ?", col.name.replace('"', "\"\"")));
                    params.push(variant_to_value(orig_val));
                }
            } else {
                for (idx, col) in schema.columns.iter().enumerate() {
                    let col_uid = ColumnUid(idx as u32);
                    let coord = (*row_uid, col_uid).into();
                    let orig_val = tracker.baseline_cells.get(&coord).unwrap_or(&Variant::Empty);
                    where_clauses.push(format!("\"{}\" = ?", col.name.replace('"', "\"\"")));
                    params.push(variant_to_value(orig_val));
                }
            }

            let del_sql = format!(
                "DELETE FROM \"{}\" WHERE {};",
                table_name.replace('"', "\"\""),
                where_clauses.join(" AND ")
            );
            let rows_affected = tx.execute(&del_sql, rusqlite::params_from_iter(params.iter()))?;
            if rows_affected > 0 {
                deleted_count += rows_affected;
            }
        }

        // --- B. Process Insertions ---
        for row_uid in &tracker.new_rows {
            if tracker.deleted_rows.contains(row_uid) {
                continue;
            }

            let mut col_names = Vec::new();
            let mut placeholders = Vec::new();
            let mut params = Vec::new();

            for (idx, col) in schema.columns.iter().enumerate() {
                let col_uid = ColumnUid(idx as u32);
                let val = backend.get((*row_uid, col_uid).into()).unwrap_or(&Variant::Empty);

                // Auto-increment integer PK omitted if left empty
                if is_single_int_pk && col.is_pk && matches!(val, Variant::Empty) {
                    continue;
                }

                col_names.push(format!("\"{}\"", col.name.replace('"', "\"\"")));
                placeholders.push("?");
                params.push(variant_to_value(val));
            }

            let ins_sql = format!(
                "INSERT INTO \"{}\" ({}) VALUES ({});",
                table_name.replace('"', "\"\""),
                col_names.join(", "),
                placeholders.join(", ")
            );
            tx.execute(&ins_sql, rusqlite::params_from_iter(params.iter()))?;
            inserted_count += 1;
        }

        // --- C. Process Updates ---
        let mut updates_by_row: std::collections::HashMap<RowUid, Vec<(usize, Variant)>> =
            std::collections::HashMap::new();
        for (coord, change) in &tracker.dirty_cells {
            if tracker.new_rows.contains(&coord.row_uid)
                || tracker.deleted_rows.contains(&coord.row_uid)
            {
                continue;
            }
            updates_by_row
                .entry(coord.row_uid)
                .or_default()
                .push((coord.col_uid.0 as usize, change.new_value.clone()));
        }

        for (row_uid, changes) in updates_by_row {
            let mut set_clauses = Vec::new();
            let mut params = Vec::new();

            for (col_idx, new_val) in changes {
                if let Some(col) = schema.columns.get(col_idx) {
                    set_clauses.push(format!("\"{}\" = ?", col.name.replace('"', "\"\"")));
                    params.push(variant_to_value(&new_val));
                }
            }

            let mut where_clauses = Vec::new();
            if !pk_indices.is_empty() {
                for &idx in &pk_indices {
                    let col = &schema.columns[idx];
                    let col_uid = ColumnUid(idx as u32);
                    let coord = (row_uid, col_uid).into();
                    let orig_val = tracker.baseline_cells.get(&coord).unwrap_or(&Variant::Empty);
                    where_clauses.push(format!("\"{}\" = ?", col.name.replace('"', "\"\"")));
                    params.push(variant_to_value(orig_val));
                }
            } else {
                for (idx, col) in schema.columns.iter().enumerate() {
                    let col_uid = ColumnUid(idx as u32);
                    let coord = (row_uid, col_uid).into();
                    let orig_val = tracker.baseline_cells.get(&coord).unwrap_or(&Variant::Empty);
                    where_clauses.push(format!("\"{}\" = ?", col.name.replace('"', "\"\"")));
                    params.push(variant_to_value(orig_val));
                }
            }

            let upd_sql = format!(
                "UPDATE \"{}\" SET {} WHERE {};",
                table_name.replace('"', "\"\""),
                set_clauses.join(", "),
                where_clauses.join(" AND ")
            );
            let rows_affected = tx.execute(&upd_sql, rusqlite::params_from_iter(params.iter()))?;
            if rows_affected > 0 {
                updated_count += rows_affected;
            }
        }

        // 4. Commit Transaction
        tx.commit()?;

        Ok(SaveSummary {
            inserted_count,
            updated_count,
            deleted_count,
            backup_path: backup_path_created,
        })
    }
}

/// Convert a Variant to a rusqlite Value for parameterized query execution
pub fn variant_to_value(v: &Variant) -> rusqlite::types::Value {
    match v {
        Variant::Empty => rusqlite::types::Value::Null,
        Variant::Bool(b) => rusqlite::types::Value::Integer(if *b { 1 } else { 0 }),
        Variant::Number(n) => match n {
            Number::I8(i) => rusqlite::types::Value::Integer(*i as i64),
            Number::I16(i) => rusqlite::types::Value::Integer(*i as i64),
            Number::I32(i) => rusqlite::types::Value::Integer(*i as i64),
            Number::I64(i) => rusqlite::types::Value::Integer(*i),
            Number::I128(i) => rusqlite::types::Value::Integer(*i as i64),
            Number::U8(u) => rusqlite::types::Value::Integer(*u as i64),
            Number::U16(u) => rusqlite::types::Value::Integer(*u as i64),
            Number::U32(u) => rusqlite::types::Value::Integer(*u as i64),
            Number::U64(u) => rusqlite::types::Value::Integer(*u as i64),
            Number::U128(u) => rusqlite::types::Value::Integer(*u as i64),
            Number::F32(f) => rusqlite::types::Value::Real(f.0 as f64),
            Number::F64(f) => rusqlite::types::Value::Real(f.0),
        },
        Variant::Str(s) => rusqlite::types::Value::Text(s.clone()),
        other => rusqlite::types::Value::Text(other.to_string()),
    }
}

/// Map SQLite type affinities (INTEGER, TEXT, REAL, NUMERIC, BLOB) to VariantTy
pub fn map_sqlite_type_to_variant(sqlite_type: &str) -> VariantTy {
    let upper = sqlite_type.trim().to_uppercase();
    if upper.contains("INT") {
        VariantTy::Number(NumberTy::I64)
    } else if upper.contains("CHAR") || upper.contains("CLOB") || upper.contains("TEXT") {
        VariantTy::Str
    } else if upper.contains("REAL") || upper.contains("FLOA") || upper.contains("DOUB") {
        VariantTy::Number(NumberTy::F64)
    } else if upper.contains("BOOL") {
        VariantTy::Bool
    } else {
        VariantTy::Str
    }
}

/// Helper method to create a rich sample database for testing
pub fn create_sample_database<P: AsRef<Path>>(path: P) -> Result<(), rusqlite::Error> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;

        -- 1. 고객 (customers) 테이블
        DROP TABLE IF EXISTS customers;
        CREATE TABLE customers (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            email TEXT UNIQUE NOT NULL,
            balance REAL DEFAULT 0.0,
            is_active BOOLEAN DEFAULT 1,
            memo TEXT
        );

        INSERT INTO customers (name, email, balance, is_active, memo) VALUES
            ('김철수', 'chulsoo@example.com', 125000.50, 1, 'VIP 고객'),
            ('이영희', 'younghee@example.com', 89000.00, 1, '주말 배송 선호'),
            ('박민수', 'minsoo@example.com', 4500.00, 0, '휴면 회원'),
            ('정수진', 'sujin@example.com', 230000.75, 1, '기업 구매 담당자'),
            ('최동훈', 'donghoon@example.com', 15000.00, 1, '첫 구매 완료');

        -- 2. 제품 (products) 테이블
        DROP TABLE IF EXISTS products;
        CREATE TABLE products (
            sku TEXT PRIMARY KEY,
            product_name TEXT NOT NULL,
            category TEXT NOT NULL,
            price REAL NOT NULL,
            stock_quantity INTEGER NOT NULL,
            discontinued BOOLEAN DEFAULT 0
        );

        INSERT INTO products (sku, product_name, category, price, stock_quantity, discontinued) VALUES
            ('SKU-1001', '기계식 키보드 (청축)', '전자기기', 89000.0, 45, 0),
            ('SKU-1002', '무선 인체공학 마우스', '전자기기', 55000.0, 120, 0),
            ('SKU-2001', '알루미늄 모니터 암', '가구/액세서리', 42000.0, 15, 0),
            ('SKU-3001', '초고속 C타입 충전기 65W', '액세서리', 29000.0, 200, 0),
            ('SKU-9001', '단종된 이전 세대 케이블', '액세서리', 5000.0, 0, 1);

        -- 3. 주문 내역 (orders) 테이블
        DROP TABLE IF EXISTS orders;
        CREATE TABLE orders (
            order_id INTEGER PRIMARY KEY AUTOINCREMENT,
            customer_id INTEGER NOT NULL,
            order_date TEXT NOT NULL,
            total_amount REAL NOT NULL,
            status TEXT NOT NULL,
            FOREIGN KEY (customer_id) REFERENCES customers(id)
        );

        INSERT INTO orders (customer_id, order_date, total_amount, status) VALUES
            (1, '2026-11-01', 144000.0, '배송완료'),
            (2, '2026-11-03', 55000.0, '결제완료'),
            (4, '2026-11-08', 295000.0, '배송중'),
            (1, '2026-11-10', 89000.0, '주문접수');
        "#,
    )?;

    Ok(())
}
