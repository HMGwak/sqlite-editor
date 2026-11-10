use rvariant::{Number, NumberTy, Variant, VariantTy};
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

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
            // Count rows safely
            let count_query = format!("SELECT COUNT(*) FROM \"{}\";", name.replace('"', "\"\""));
            let row_count: usize = conn
                .query_row(&count_query, [], |r| r.get::<_, i64>(0))
                .map(|cnt| cnt as usize)
                .unwrap_or(0);

            // Count columns from pragma
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
        // Fallback for NUMERIC, BLOB, or untyped SQLite columns
        VariantTy::Str
    }
}

/// Helper method to create a rich sample database for testing Phase 1
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
