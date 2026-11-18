use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use aho_corasick::AhoCorasick;
use arrow::array::{Array, AsArray, DictionaryArray, StringArray};
use arrow::datatypes::{DataType, Int16Type, Int32Type, Int64Type, Int8Type, UInt16Type, UInt32Type, UInt64Type, UInt8Type};
use arrow_cast::cast;
use bytes::Bytes;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use rayon::prelude::*;

/// A request to search a parquet file.
pub struct SearchRequest {
    pub path: Box<Path>,
    pub needle: String,
    pub cancelled: Arc<AtomicBool>,
}

/// Search results: sorted list of (global_row, column_index) pairs.
pub type SearchResult = Vec<(usize, usize)>;

/// Sentinel row value for header matches, must match `RootView::HEADER_ROW`.
const HEADER_ROW: usize = usize::MAX;

/// Batch size for reading row groups.
const BATCH_SIZE: usize = 64 * 1024;

/// Run parallel text search over a parquet file.
///
/// Returns matches as `(global_row, column_index)` sorted by (row, col).
/// Column indices refer to the full schema, not the projected subset.
pub fn parallel_search(req: &SearchRequest) -> Result<SearchResult, String> {
    if req.needle.is_empty() {
        return Ok(Vec::new());
    }

    let data = Bytes::from(std::fs::read(&req.path).map_err(|e| e.to_string())?);
    let meta = ArrowReaderMetadata::load(&data, ArrowReaderOptions::new())
        .map_err(|e| e.to_string())?;
    let schema = meta.schema().clone();

    // Build projection: string-like + numeric columns only.
    // Track (projected_index -> full_schema_index) mapping.
    let searchable: Vec<usize> = schema
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, f)| is_searchable(f.data_type()))
        .map(|(i, _)| i)
        .collect();

    if searchable.is_empty() {
        return Ok(search_headers(&schema, &req.needle));
    }

    let mask = ProjectionMask::roots(meta.parquet_schema(), searchable.clone());

    // Build case-insensitive searcher.
    let searcher = AhoCorasick::builder()
        .ascii_case_insensitive(true)
        .build([&req.needle])
        .map_err(|e| e.to_string())?;
    let searcher = Arc::new(searcher);

    let n_rg = meta.metadata().num_row_groups();

    // Compute row-group base offsets.
    let mut rg_bases = Vec::with_capacity(n_rg);
    let mut base = 0usize;
    for rg in 0..n_rg {
        rg_bases.push(base);
        base += meta.metadata().row_group(rg).num_rows() as usize;
    }

    let mut results = if n_rg <= 1 {
        // Fallback: single row group → read batches sequentially, match in parallel.
        search_single_row_group(
            &data,
            &meta,
            &mask,
            &searchable,
            &searcher,
            &req.cancelled,
            rg_bases.first().copied().unwrap_or(0),
        )?
    } else {
        // Standard: one rayon task per row group.
        (0..n_rg)
            .into_par_iter()
            .filter_map(|rg| {
                if req.cancelled.load(Ordering::Relaxed) {
                    return None;
                }
                match search_row_group(
                    &data,
                    &meta,
                    &mask,
                    &searchable,
                    &searcher,
                    rg,
                    rg_bases[rg],
                ) {
                    Ok(hits) if !hits.is_empty() => Some(hits),
                    _ => None,
                }
            })
            .flatten()
            .collect::<Vec<_>>()
    };

    // Add header matches.
    let mut header_matches = search_headers(&schema, &req.needle);
    header_matches.append(&mut results);
    results = header_matches;

    results.sort_unstable();
    Ok(results)
}

/// Check if a data type should be included in search.
fn is_searchable(dt: &DataType) -> bool {
    matches!(
        dt,
        DataType::Utf8
            | DataType::LargeUtf8
            | DataType::Dictionary(_, _)
    ) || is_castable_to_string(dt)
}

/// Search column header names.
fn search_headers(schema: &arrow::datatypes::SchemaRef, needle: &str) -> SearchResult {
    let needle_lower = needle.to_lowercase();
    schema
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, f)| f.name().to_lowercase().contains(&needle_lower))
        .map(|(col, _)| (HEADER_ROW, col))
        .collect()
}

/// Search a single row group: read batches, run match functions per column.
fn search_row_group(
    data: &Bytes,
    meta: &ArrowReaderMetadata,
    mask: &ProjectionMask,
    searchable: &[usize],
    searcher: &AhoCorasick,
    rg: usize,
    base_row: usize,
) -> Result<SearchResult, String> {
    let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(data.clone(), meta.clone())
        .with_row_groups(vec![rg])
        .with_projection(mask.clone())
        .with_batch_size(BATCH_SIZE)
        .build()
        .map_err(|e| e.to_string())?;

    let mut hits = Vec::new();
    let mut row_offset = base_row;

    for batch in reader {
        let batch = batch.map_err(|e| e.to_string())?;
        let n_rows = batch.num_rows();

        for (proj_idx, col) in batch.columns().iter().enumerate() {
            let full_col_idx = searchable[proj_idx];
            search_column(col, searcher, row_offset, full_col_idx, &mut hits);
        }
        row_offset += n_rows;
    }

    Ok(hits)
}

/// Fallback for single-row-group files: read batches sequentially,
/// then distribute SIMD matching across rayon tasks.
fn search_single_row_group(
    data: &Bytes,
    meta: &ArrowReaderMetadata,
    mask: &ProjectionMask,
    searchable: &[usize],
    searcher: &AhoCorasick,
    cancelled: &AtomicBool,
    base_row: usize,
) -> Result<SearchResult, String> {
    let rg = if meta.metadata().num_row_groups() == 0 {
        return Ok(Vec::new());
    } else {
        0
    };

    let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(data.clone(), meta.clone())
        .with_row_groups(vec![rg])
        .with_projection(mask.clone())
        .with_batch_size(BATCH_SIZE)
        .build()
        .map_err(|e| e.to_string())?;

    // Collect batches with their row offsets.
    let mut batches = Vec::new();
    let mut offset = base_row;
    for batch in reader {
        let batch = batch.map_err(|e| e.to_string())?;
        let n = batch.num_rows();
        batches.push((offset, batch));
        offset += n;
    }

    // Parallel matching over batches.
    let results: Vec<_> = batches
        .into_par_iter()
        .filter_map(|(row_offset, batch)| {
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            let mut hits = Vec::new();
            for (proj_idx, col) in batch.columns().iter().enumerate() {
                let full_col_idx = searchable[proj_idx];
                search_column(col, searcher, row_offset, full_col_idx, &mut hits);
            }
            if hits.is_empty() { None } else { Some(hits) }
        })
        .flatten()
        .collect();

    Ok(results)
}

/// Dispatch column matching based on data type.
fn search_column(
    col: &dyn Array,
    searcher: &AhoCorasick,
    row_offset: usize,
    full_col_idx: usize,
    hits: &mut Vec<(usize, usize)>,
) {
    match col.data_type() {
        DataType::Utf8 => {
            match_utf8(col.as_string::<i32>(), searcher, row_offset, full_col_idx, hits);
        }
        DataType::LargeUtf8 => {
            match_large_utf8(col.as_string::<i64>(), searcher, row_offset, full_col_idx, hits);
        }
        DataType::Dictionary(key_type, _) => {
            match key_type.as_ref() {
                DataType::Int8 => match_dictionary::<Int8Type>(col.as_dictionary::<Int8Type>(), searcher, row_offset, full_col_idx, hits),
                DataType::Int16 => match_dictionary::<Int16Type>(col.as_dictionary::<Int16Type>(), searcher, row_offset, full_col_idx, hits),
                DataType::Int32 => match_dictionary::<Int32Type>(col.as_dictionary::<Int32Type>(), searcher, row_offset, full_col_idx, hits),
                DataType::Int64 => match_dictionary::<Int64Type>(col.as_dictionary::<Int64Type>(), searcher, row_offset, full_col_idx, hits),
                DataType::UInt8 => match_dictionary::<UInt8Type>(col.as_dictionary::<UInt8Type>(), searcher, row_offset, full_col_idx, hits),
                DataType::UInt16 => match_dictionary::<UInt16Type>(col.as_dictionary::<UInt16Type>(), searcher, row_offset, full_col_idx, hits),
                DataType::UInt32 => match_dictionary::<UInt32Type>(col.as_dictionary::<UInt32Type>(), searcher, row_offset, full_col_idx, hits),
                DataType::UInt64 => match_dictionary::<UInt64Type>(col.as_dictionary::<UInt64Type>(), searcher, row_offset, full_col_idx, hits),
                _ => {}
            }
        }
        dt if is_castable_to_string(dt) => {
            match_castable(col, searcher, row_offset, full_col_idx, hits);
        }
        _ => {}
    }
}

/// Types that can be cast to Utf8 for text search: numerics, booleans, dates, times.
fn is_castable_to_string(dt: &DataType) -> bool {
    matches!(
        dt,
        DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Decimal128(_, _)
            | DataType::Decimal256(_, _)
            | DataType::Boolean
            | DataType::Date32
            | DataType::Date64
            | DataType::Time32(_)
            | DataType::Time64(_)
            | DataType::Timestamp(_, _)
            | DataType::Duration(_)
    )
}

// -- Matching functions --

/// SIMD scan over a StringArray's contiguous value buffer using aho-corasick.
/// Maps hits back to row indices via the offsets array.
///
/// Re-slices the buffer after each match (like the memchr `Finder` pattern)
/// so that cross-boundary false positives are skipped and we don't duplicate
/// hits within the same cell.
fn match_utf8(
    arr: &StringArray,
    searcher: &AhoCorasick,
    row_offset: usize,
    col_idx: usize,
    hits: &mut Vec<(usize, usize)>,
) {
    if arr.is_empty() {
        return;
    }
    let offsets = arr.value_offsets();
    let values = arr.values().as_slice();
    let needle_len = searcher.min_pattern_len();
    let end = offsets[arr.len()] as usize;
    let mut start = offsets[0] as usize;

    while let Some(mat) = searcher.find(&values[start..end]) {
        let pos = start + mat.start();
        // Find which row this position falls in.
        let row = offsets.partition_point(|&o| (o as usize) <= pos) - 1;
        if row >= arr.len() {
            break;
        }
        let row_end = offsets[row + 1] as usize;
        if pos + needle_len <= row_end && arr.is_valid(row) {
            hits.push((row_offset + row, col_idx));
        }
        // Advance past this row whether the match was valid or not.
        start = row_end;
    }
}

/// Same as match_utf8 but for LargeUtf8 (i64 offsets).
fn match_large_utf8(
    arr: &arrow::array::GenericStringArray<i64>,
    searcher: &AhoCorasick,
    row_offset: usize,
    col_idx: usize,
    hits: &mut Vec<(usize, usize)>,
) {
    if arr.is_empty() {
        return;
    }
    let offsets = arr.value_offsets();
    let values = arr.values().as_slice();
    let needle_len = searcher.min_pattern_len();
    let end = offsets[arr.len()] as usize;
    let mut start = offsets[0] as usize;

    while let Some(mat) = searcher.find(&values[start..end]) {
        let pos = start + mat.start();
        let row = offsets.partition_point(|&o| (o as usize) <= pos) - 1;
        if row >= arr.len() {
            break;
        }
        let row_end = offsets[row + 1] as usize;
        if pos + needle_len <= row_end && arr.is_valid(row) {
            hits.push((row_offset + row, col_idx));
        }
        start = row_end;
    }
}

/// Search dictionary-encoded columns efficiently: search unique values first,
/// then map matching keys back to rows.
fn match_dictionary<K: arrow::datatypes::ArrowDictionaryKeyType>(
    arr: &DictionaryArray<K>,
    searcher: &AhoCorasick,
    row_offset: usize,
    col_idx: usize,
    hits: &mut Vec<(usize, usize)>,
) where
    K::Native: TryInto<usize>,
{
    let values = arr.values();

    // Try to get a StringArray from the dictionary values.
    let string_values = match values.data_type() {
        DataType::Utf8 => values.as_any().downcast_ref::<StringArray>(),
        _ => {
            // If dictionary values aren't strings, try casting to Utf8.
            if let Ok(casted) = cast(values.as_ref(), &DataType::Utf8) {
                // We need to search the casted values, but can't hold a reference.
                // Fall back to per-row search for non-string dictionaries.
                let casted_str = casted.as_any().downcast_ref::<StringArray>().unwrap();
                let matching_indices: std::collections::HashSet<usize> = (0..casted_str.len())
                    .filter(|&i| {
                        casted_str.is_valid(i)
                            && searcher.is_match(casted_str.value(i).as_bytes())
                    })
                    .collect();

                if matching_indices.is_empty() {
                    return;
                }

                let keys = arr.keys();
                for row in 0..arr.len() {
                    if arr.is_valid(row) {
                        if let Ok(key) = keys.value(row).try_into() {
                            if matching_indices.contains(&key) {
                                hits.push((row_offset + row, col_idx));
                            }
                        }
                    }
                }
                return;
            }
            return;
        }
    };

    let Some(string_values) = string_values else {
        return;
    };

    // Find which dictionary indices match.
    let matching_indices: std::collections::HashSet<usize> = (0..string_values.len())
        .filter(|&i| {
            string_values.is_valid(i)
                && searcher.is_match(string_values.value(i).as_bytes())
        })
        .collect();

    if matching_indices.is_empty() {
        return;
    }

    // Scan keys to find rows.
    let keys = arr.keys();
    for row in 0..arr.len() {
        if arr.is_valid(row) {
            if let Ok(key) = keys.value(row).try_into() {
                if matching_indices.contains(&key) {
                    hits.push((row_offset + row, col_idx));
                }
            }
        }
    }
}

/// Cast non-string columns (numerics, booleans, dates, times) to Utf8,
/// then apply the same SIMD string scan.
fn match_castable(
    col: &dyn Array,
    searcher: &AhoCorasick,
    row_offset: usize,
    col_idx: usize,
    hits: &mut Vec<(usize, usize)>,
) {
    let Ok(casted) = cast(col, &DataType::Utf8) else {
        return;
    };
    let string_arr = casted.as_any().downcast_ref::<StringArray>().unwrap();
    match_utf8(string_arr, searcher, row_offset, col_idx, hits);
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, RecordBatch, StringArray, Float64Array};
    use arrow::datatypes::{Field, Schema};
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;
    use std::sync::Arc;

    struct TempFile(std::path::PathBuf);
    impl Drop for TempFile {
        fn drop(&mut self) {
            std::fs::remove_file(&self.0).ok();
        }
    }

    fn write_test_parquet(tag: &str, batch: &RecordBatch, max_rg_rows: usize) -> TempFile {
        let path = std::env::temp_dir().join(format!(
            "parqueter-search-test-{tag}-{}.parquet",
            std::process::id()
        ));
        let props = WriterProperties::builder()
            .set_max_row_group_row_count(Some(max_rg_rows))
            .build();
        let mut writer =
            ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), batch.schema(), Some(props))
                .unwrap();
        writer.write(batch).unwrap();
        writer.close().unwrap();
        TempFile(path)
    }

    fn make_request(path: &Path, needle: &str) -> SearchRequest {
        SearchRequest {
            path: path.into(),
            needle: needle.to_string(),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn test_batch(rows: usize) -> RecordBatch {
        let ids = Int64Array::from_iter_values(0..rows as i64);
        let names = StringArray::from_iter(
            (0..rows).map(|i| Some(format!("name_{i}"))),
        );
        let prices = Float64Array::from_iter_values(
            (0..rows).map(|i| i as f64 * 3.14),
        );
        RecordBatch::try_from_iter([
            ("id", Arc::new(ids) as _),
            ("name", Arc::new(names) as _),
            ("price", Arc::new(prices) as _),
        ])
        .unwrap()
    }

    #[test]
    fn match_utf8_basic() {
        let arr = StringArray::from(vec!["hello world", "foo", "hello again", "bar"]);
        let searcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(["hello"])
            .unwrap();
        let mut hits = Vec::new();
        match_utf8(&arr, &searcher, 0, 0, &mut hits);
        assert_eq!(hits, vec![(0, 0), (2, 0)]);
    }

    #[test]
    fn match_utf8_at_boundaries() {
        let arr = StringArray::from(vec!["abc", "def", "ghi"]);
        let searcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(["abc"])
            .unwrap();
        let mut hits = Vec::new();
        match_utf8(&arr, &searcher, 10, 5, &mut hits);
        assert_eq!(hits, vec![(10, 5)]);
    }

    #[test]
    fn match_utf8_cross_boundary_rejected() {
        // "cd" spans the boundary between "abc" and "def"
        let arr = StringArray::from(vec!["abc", "def"]);
        let searcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(["cd"])
            .unwrap();
        let mut hits = Vec::new();
        match_utf8(&arr, &searcher, 0, 0, &mut hits);
        assert!(hits.is_empty());
    }

    #[test]
    fn match_utf8_empty_strings() {
        let arr = StringArray::from(vec!["", "hello", "", ""]);
        let searcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(["hello"])
            .unwrap();
        let mut hits = Vec::new();
        match_utf8(&arr, &searcher, 0, 0, &mut hits);
        assert_eq!(hits, vec![(1, 0)]);
    }

    #[test]
    fn match_utf8_null_values() {
        let arr = StringArray::from(vec![Some("hello"), None, Some("hello")]);
        let searcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(["hello"])
            .unwrap();
        let mut hits = Vec::new();
        match_utf8(&arr, &searcher, 0, 0, &mut hits);
        assert_eq!(hits, vec![(0, 0), (2, 0)]);
    }

    #[test]
    fn numeric_cast_search() {
        let arr: Arc<dyn Array> = Arc::new(Int64Array::from(vec![123, 456, 1234, 789]));
        let searcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(["23"])
            .unwrap();
        let mut hits = Vec::new();
        match_castable(arr.as_ref(), &searcher, 0, 0, &mut hits);
        // "23" matches in "123" and "1234"
        assert_eq!(hits, vec![(0, 0), (2, 0)]);
    }

    #[test]
    fn float_search() {
        let arr: Arc<dyn Array> = Arc::new(Float64Array::from(vec![std::f64::consts::PI, std::f64::consts::E, 1.0]));
        let searcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(["3.14"])
            .unwrap();
        let mut hits = Vec::new();
        match_castable(arr.as_ref(), &searcher, 0, 0, &mut hits);
        assert_eq!(hits, vec![(0, 0)]);
    }

    #[test]
    fn boolean_search() {
        let arr: Arc<dyn Array> = Arc::new(arrow::array::BooleanArray::from(vec![true, false, true, false]));
        let searcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(["true"])
            .unwrap();
        let mut hits = Vec::new();
        match_castable(arr.as_ref(), &searcher, 0, 0, &mut hits);
        assert_eq!(hits, vec![(0, 0), (2, 0)]);

        let mut hits = Vec::new();
        let searcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(["false"])
            .unwrap();
        match_castable(arr.as_ref(), &searcher, 0, 0, &mut hits);
        assert_eq!(hits, vec![(1, 0), (3, 0)]);
    }

    #[test]
    fn case_insensitive_search() {
        let arr = StringArray::from(vec!["Hello World", "HELLO", "goodbye"]);
        let searcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(["hello"])
            .unwrap();
        let mut hits = Vec::new();
        match_utf8(&arr, &searcher, 0, 0, &mut hits);
        assert_eq!(hits, vec![(0, 0), (1, 0)]);
    }

    #[test]
    fn cancellation_stops_search() {
        let batch = test_batch(10000);
        let file = write_test_parquet("cancel", &batch, 1000);
        let req = SearchRequest {
            path: file.0.as_path().into(),
            needle: "name".to_string(),
            cancelled: Arc::new(AtomicBool::new(true)), // pre-cancelled
        };
        let results = parallel_search(&req).unwrap();
        // With cancellation set, results should be empty or very few (just headers)
        // since rayon tasks check the token before processing.
        assert!(results.len() <= 1); // at most header match for "name" column
    }

    #[test]
    fn multi_row_group_search() {
        let batch = test_batch(5000);
        // 500 rows per row group → 10 row groups
        let file = write_test_parquet("multi-rg", &batch, 500);
        let req = make_request(&file.0, "name_42");
        let results = parallel_search(&req).unwrap();
        // "name_42" matches rows: 42, 420-429, 4200-4209 (and column "name" header)
        assert!(results.iter().any(|&(r, _)| r == 42));
        assert!(results.iter().any(|&(r, _)| r == 420));
    }

    #[test]
    fn single_row_group_fallback() {
        let batch = test_batch(5000);
        // One giant row group
        let file = write_test_parquet("single-rg", &batch, 100000);
        let req = make_request(&file.0, "name_42");
        let results = parallel_search(&req).unwrap();
        assert!(results.iter().any(|&(r, _)| r == 42));
        assert!(results.iter().any(|&(r, _)| r == 420));
    }

    #[test]
    fn header_search() {
        let batch = test_batch(10);
        let file = write_test_parquet("header", &batch, 100);
        let req = make_request(&file.0, "price");
        let results = parallel_search(&req).unwrap();
        assert!(results.iter().any(|&(r, c)| r == HEADER_ROW && c == 2));
    }

    #[test]
    fn results_are_sorted() {
        let batch = test_batch(5000);
        let file = write_test_parquet("sorted", &batch, 500);
        let req = make_request(&file.0, "name_1");
        let results = parallel_search(&req).unwrap();
        assert!(!results.is_empty());
        for w in results.windows(2) {
            assert!(w[0] <= w[1], "results not sorted: {:?} > {:?}", w[0], w[1]);
        }
    }

    #[test]
    fn end_to_end_matches_brute_force() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("text", DataType::Utf8, false),
            Field::new("num", DataType::Int64, false),
        ]));
        let texts = StringArray::from(vec!["apple", "banana", "cherry", "apple pie", "grape"]);
        let nums = Int64Array::from(vec![100, 200, 300, 400, 500]);
        let batch = RecordBatch::try_new(schema, vec![Arc::new(texts), Arc::new(nums)]).unwrap();
        let file = write_test_parquet("e2e", &batch, 100);

        let req = make_request(&file.0, "apple");
        let results = parallel_search(&req).unwrap();
        // Should match row 0 ("apple") and row 3 ("apple pie") in col 0
        let data_matches: Vec<_> = results.iter().filter(|&&(r, _)| r != HEADER_ROW).collect();
        assert_eq!(data_matches.len(), 2);
        assert!(data_matches.contains(&&(0, 0)));
        assert!(data_matches.contains(&&(3, 0)));

        // "00" should match in num column: 100, 200, 300, 400, 500
        let req2 = make_request(&file.0, "00");
        let results2 = parallel_search(&req2).unwrap();
        let num_matches: Vec<_> = results2.iter().filter(|&&(r, c)| r != HEADER_ROW && c == 1).collect();
        assert_eq!(num_matches.len(), 5);
    }

    /// Manual probe against a real file:
    /// PARQUETER_SEARCH_FILE=/path/file.parquet cargo test --release search::tests::probe_real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe_real() {
        let path = std::env::var("PARQUETER_SEARCH_FILE").expect("set PARQUETER_SEARCH_FILE");
        let req = make_request(Path::new(&path), "a");
        let t = std::time::Instant::now();
        let results = parallel_search(&req).unwrap();
        eprintln!("Search for 'a': {} matches in {:?}", results.len(), t.elapsed());
    }
}
