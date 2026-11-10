use sqlite_editor::model::TrackedTableBackend;
use egui_tabular::frontend::TableFrontend;
use rvariant::{Variant, VariantTy};
use tabular_core::backend::TableBackend;
use tabular_core::ColumnUid;

#[test]
fn test_search_and_replace_functionality() {
    let mut backend = TrackedTableBackend::new(vec![
        ("id", VariantTy::Number(rvariant::NumberTy::I64), None),
        ("name", VariantTy::Str, Some(Variant::str(""))),
        ("city", VariantTy::Str, Some(Variant::str(""))),
    ]);

    backend.create_row([
        (ColumnUid(0), Variant::i64(1)),
        (ColumnUid(1), Variant::str("Alice Wonderland")),
        (ColumnUid(2), Variant::str("Seoul")),
    ]);
    backend.create_row([
        (ColumnUid(0), Variant::i64(2)),
        (ColumnUid(1), Variant::str("Bob Builder")),
        (ColumnUid(2), Variant::str("Tokyo")),
    ]);
    backend.create_row([
        (ColumnUid(0), Variant::i64(3)),
        (ColumnUid(1), Variant::str("Alice Cooper")),
        (ColumnUid(2), Variant::str("New York")),
    ]);

    assert_eq!(backend.row_count(), 3);

    // Search for "alice"
    let mut matches = Vec::new();
    for r_idx in 0..backend.row_count() {
        let row_uid = backend.row_uid(tabular_core::backend::VisualRowIdx(r_idx)).unwrap();
        for c_idx in 0..3 {
            let col_uid = ColumnUid(c_idx as u32);
            let coord = (row_uid, col_uid).into();
            if let Some(val) = backend.inner.get(coord) {
                if val.to_string().to_lowercase().contains("alice") {
                    matches.push(coord);
                    backend.search_matches.insert(coord);
                }
            }
        }
    }

    assert_eq!(matches.len(), 2, "Alice should be found in 2 rows");
    backend.current_search_focus = Some(matches[0]);

    // Check that cell_color highlights search matches
    let color_focus = backend.cell_color(matches[0]);
    let color_match = backend.cell_color(matches[1]);
    assert!(color_focus.is_some());
    assert!(color_match.is_some());
    assert_ne!(color_focus, color_match, "Focus should have distinct color from regular match");
}

#[test]
fn test_column_sorting() {
    let mut backend = TrackedTableBackend::new(vec![
        ("name", VariantTy::Str, None),
        ("score", VariantTy::Number(rvariant::NumberTy::I64), None),
    ]);

    backend.create_row([
        (ColumnUid(0), Variant::str("Charlie")),
        (ColumnUid(1), Variant::i64(50)),
    ]);
    backend.create_row([
        (ColumnUid(0), Variant::str("Alice")),
        (ColumnUid(1), Variant::i64(95)),
    ]);
    backend.create_row([
        (ColumnUid(0), Variant::str("Bob")),
        (ColumnUid(1), Variant::i64(80)),
    ]);

    // Initial order
    let r0 = backend.row_uid(tabular_core::backend::VisualRowIdx(0)).unwrap();
    assert_eq!(backend.inner.get((r0, ColumnUid(0)).into()).unwrap().to_string(), "Charlie");

    // Sort ascending by name (col 0)
    backend.sort_by_col(ColumnUid(0), true);
    let s0 = backend.row_uid(tabular_core::backend::VisualRowIdx(0)).unwrap();
    let s1 = backend.row_uid(tabular_core::backend::VisualRowIdx(1)).unwrap();
    let s2 = backend.row_uid(tabular_core::backend::VisualRowIdx(2)).unwrap();
    assert_eq!(backend.inner.get((s0, ColumnUid(0)).into()).unwrap().to_string(), "Alice");
    assert_eq!(backend.inner.get((s1, ColumnUid(0)).into()).unwrap().to_string(), "Bob");
    assert_eq!(backend.inner.get((s2, ColumnUid(0)).into()).unwrap().to_string(), "Charlie");

    // Sort descending by score (col 1)
    backend.sort_by_col(ColumnUid(1), false);
    let top = backend.row_uid(tabular_core::backend::VisualRowIdx(0)).unwrap();
    assert_eq!(backend.inner.get((top, ColumnUid(1)).into()).unwrap().to_string(), "95");
}
