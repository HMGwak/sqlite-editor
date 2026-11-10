use crate::model::TrackedTableBackend;
use rvariant::{Number, Variant, VariantTy};
use tabular_core::backend::{TableBackend, VisualColIdx, VisualRowIdx};
use tabular_core::CellCoord;

/// Generate a sample 20 rows × 5 columns dataset simulating data copied from Excel.
/// Columns: ID \t Name \t Email \t Amount \t Active
pub fn generate_sample_excel_tsv(row_count: usize) -> String {
    let mut lines = Vec::new();
    for i in 1..=row_count {
        let id = 1000 + i;
        let name = match i % 5 {
            1 => format!("홍길동_{i}"),
            2 => format!("이순신_{i}"),
            3 => format!("강감찬_{i}"),
            4 => format!("김유신_{i}"),
            _ => format!("을지문덕_{i}"),
        };
        let email = format!("user{i}@example.com");
        let amount = format!("{:.2}", (i as f64) * 1250.5);
        let active = if i % 2 == 0 { "true" } else { "false" };

        lines.push(format!("{id}\t{name}\t{email}\t{amount}\t{active}"));
    }
    lines.join("\n")
}

/// Parse TSV text (handling Windows \r\n and Unix \n) into a 2D matrix of strings
pub fn parse_tsv(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .map(|line| {
            line.trim_end_matches('\r')
                .split('\t')
                .map(|col| col.trim().to_string())
                .collect()
        })
        .filter(|row: &Vec<String>| !row.is_empty() && (row.len() > 1 || !row[0].is_empty()))
        .collect()
}

/// Convert a string to a typed Variant based on target column type
pub fn parse_cell_value(text: &str, ty: &VariantTy) -> Variant {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Variant::Empty;
    }

    match ty {
        VariantTy::Bool => match trimmed.to_lowercase().as_str() {
            "true" | "1" | "y" | "yes" | "t" => Variant::Bool(true),
            "false" | "0" | "n" | "no" | "f" => Variant::Bool(false),
            _ => Variant::Str(trimmed.to_string()),
        },
        VariantTy::Number(_) => {
            if let Ok(i) = trimmed.parse::<i64>() {
                Variant::Number(Number::I64(i))
            } else if let Ok(f) = trimmed.parse::<f64>() {
                Variant::Number(Number::F64(rvariant::F64(f)))
            } else {
                Variant::Str(trimmed.to_string())
            }
        }
        _ => Variant::Str(trimmed.to_string()),
    }
}

/// Apply paste as overwrite, starting at the specified (start_row_idx, start_col_idx).
/// Returns the number of cells modified.
pub fn apply_paste_overwrite(
    table: &mut TrackedTableBackend,
    start_row_idx: usize,
    start_col_idx: usize,
    tsv_data: &[Vec<String>],
) -> usize {
    let mut modified_count = 0;

    for (r_offset, row) in tsv_data.iter().enumerate() {
        let row_idx = start_row_idx + r_offset;
        let row_uid = match table.row_uid(VisualRowIdx(row_idx)) {
            Some(uid) => uid,
            None => {
                // If there are no more existing rows, create new rows (expansion)
                match table.create_row([]) {
                    Some(uid) => uid,
                    None => break,
                }
            }
        };

        for (c_offset, cell_str) in row.iter().enumerate() {
            let col_idx = start_col_idx + c_offset;
            let col_uid = match table.col_uid(VisualColIdx(col_idx)) {
                Some(uid) => uid,
                None => break, // Don't create ad-hoc columns beyond schema
            };

            let col_ty = table.inner.column_ty(col_uid).unwrap_or(VariantTy::Str);
            let new_val = parse_cell_value(cell_str, &col_ty);
            let coord: CellCoord = (row_uid, col_uid).into();

            let current_val = table.get(coord).cloned().unwrap_or(Variant::Empty);
            if current_val != new_val {
                table.set(coord, new_val);
                modified_count += 1;
            }
        }
    }

    modified_count
}

/// Apply paste as append: creates brand new rows at the bottom for every row in the clipboard.
/// Returns the number of rows created.
pub fn apply_paste_append(
    table: &mut TrackedTableBackend,
    tsv_data: &[Vec<String>],
) -> usize {
    let mut rows_added = 0;

    for row in tsv_data {
        let mut row_values = Vec::new();

        for (col_idx, cell_str) in row.iter().enumerate() {
            if let Some(col_uid) = table.col_uid(VisualColIdx(col_idx)) {
                let col_ty = table.inner.column_ty(col_uid).unwrap_or(VariantTy::Str);
                let val = parse_cell_value(cell_str, &col_ty);
                row_values.push((col_uid, val));
            }
        }

        if let Some(_uid) = table.create_row(row_values) {
            rows_added += 1;
        }
    }

    table.update_metadata_indicators();
    rows_added
}

/// Export selected or all table rows as TSV for copying to Excel.
pub fn export_to_tsv(
    table: &TrackedTableBackend,
    row_range: Option<(usize, usize)>,
    col_range: Option<(usize, usize)>,
) -> String {
    let total_rows = table.row_count();
    let total_cols = table.column_names.len();

    let (start_r, end_r) = row_range.unwrap_or((0, total_rows.saturating_sub(1)));
    let (start_c, end_c) = col_range.unwrap_or((0, total_cols.saturating_sub(1)));

    let mut lines = Vec::new();
    for r in start_r..=end_r {
        if let Some(row_uid) = table.row_uid(VisualRowIdx(r)) {
            let mut cols = Vec::new();
            for c in start_c..=end_c {
                if let Some(col_uid) = table.col_uid(VisualColIdx(c)) {
                    let coord = (row_uid, col_uid).into();
                    let val_str = match table.get(coord) {
                        Some(Variant::Str(s)) => s.clone(),
                        Some(other) => other.to_string(),
                        None => String::new(),
                    };
                    cols.push(val_str);
                }
            }
            lines.push(cols.join("\t"));
        }
    }

    lines.join("\n")
}
