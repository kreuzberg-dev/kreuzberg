//! Span-aware flattening of table grids to `Vec<Vec<String>>`.
//!
//! Several extractors (HTML, DOCX, PPTX, email) receive tables as a stream of
//! cells carrying `row_span` / `col_span`. Flattening them by trusting a naive
//! per-row column index shifts every cell under a `rowspan` left into the
//! spanning column, misaligning the data against its headers
//! (xberg-io/xberg#1223). This helper places cells on a grid that reserves the
//! columns still covered by a rowspan started in an earlier row, so merged-cell
//! tables keep their column alignment across every format.

/// A table cell with its span, in document order within its row.
pub(crate) struct SpanCell {
    pub content: String,
    pub row_span: u32,
    pub col_span: u32,
}

impl SpanCell {
    pub(crate) fn new(content: impl Into<String>, row_span: u32, col_span: u32) -> Self {
        Self {
            content: content.into(),
            row_span,
            col_span,
        }
    }
}

/// Resolve each span-carrying cell's `(row, col)` on an occupancy grid that
/// reserves the columns still covered by a rowspan from an earlier row, and
/// return the total column count.
///
/// Cells are visited row-major, in document order within each row; `place` is
/// called once per cell with its row index, resolved column, and the cell
/// itself. This is the single home of the merged-cell placement algorithm —
/// both the flattening helpers here and the `DocumentStructure` table grid
/// builder route through it so the geometry can never drift between them
/// (xberg-io/xberg#1223).
pub(crate) fn resolve_span_grid<C>(
    rows: &[Vec<C>],
    col_span: impl Fn(&C) -> u32,
    row_span: impl Fn(&C) -> u32,
    mut place: impl FnMut(u32, u32, &C),
) -> u32 {
    let mut occupied_until: Vec<u32> = Vec::new();
    for (row_idx, row) in rows.iter().enumerate() {
        let row_idx = row_idx as u32;
        let mut col = 0usize;
        for cell in row {
            while col < occupied_until.len() && occupied_until[col] > row_idx {
                col += 1;
            }
            let end_row = row_idx + row_span(cell).max(1);
            let span = col_span(cell).max(1) as usize;
            for c in col..col + span {
                if c >= occupied_until.len() {
                    occupied_until.resize(c + 1, 0);
                }
                occupied_until[c] = end_row;
            }
            place(row_idx, col as u32, cell);
            col += span;
        }
    }
    occupied_until.len() as u32
}

/// Flatten rows of span-carrying cells into a dense `Vec<Vec<String>>`.
///
/// The origin cell of a span holds the value; the columns/rows it covers are
/// left empty. The output is rectangular: every row is padded to the widest
/// column reached.
pub(crate) fn flatten_spanned_rows(rows: &[Vec<SpanCell>]) -> Vec<Vec<String>> {
    let mut placed: Vec<(u32, u32, String)> = Vec::new();
    let num_cols = resolve_span_grid(
        rows,
        |c| c.col_span,
        |c| c.row_span,
        |row_idx, col, cell| placed.push((row_idx, col, cell.content.clone())),
    ) as usize;

    let num_rows = rows.len();
    let mut grid = vec![vec![String::new(); num_cols]; num_rows];
    for (r, c, content) in placed {
        if (r as usize) < num_rows && (c as usize) < num_cols {
            grid[r as usize][c as usize] = content;
        }
    }
    grid
}

/// Flatten a stream of span-carrying cells whose per-row order is known but
/// whose column index is naive (does not reserve rowspan columns) — the shape
/// `html_to_markdown_rs` grids arrive in. Groups by row, then re-derives true
/// positions with [`flatten_spanned_rows`].
///
/// `cells` yields `(row, row_span, col_span, content)` in document order.
pub(crate) fn flatten_positioned_cells(
    num_rows: usize,
    cells: impl Iterator<Item = (u32, u32, u32, String)>,
) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<SpanCell>> = (0..num_rows.max(1)).map(|_| Vec::new()).collect();
    for (row, row_span, col_span, content) in cells {
        let r = row as usize;
        // A cell whose reported row index falls outside the caller's declared
        // `num_rows` used to be clamped onto the last row, scrambling it in as
        // extra columns under the wrong header (xberg-io/xberg#234). Grow the
        // grid instead so the cell keeps its own row.
        if r >= rows.len() {
            rows.resize_with(r + 1, Vec::new);
        }
        rows[r].push(SpanCell::new(content, row_span, col_span));
    }
    flatten_spanned_rows(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rowspan_reserves_column() {
        let rows = vec![
            vec![
                SpanCell::new("Alpha", 2, 1),
                SpanCell::new("Alice", 1, 1),
                SpanCell::new("10", 1, 1),
            ],
            vec![SpanCell::new("Bob", 1, 1), SpanCell::new("20", 1, 1)],
        ];
        let grid = flatten_spanned_rows(&rows);
        assert_eq!(grid[0], vec!["Alpha", "Alice", "10"]);
        assert_eq!(grid[1], vec!["", "Bob", "20"]);
    }

    #[test]
    fn colspan_widens_grid() {
        let rows = vec![
            vec![SpanCell::new("Fuse", 1, 2), SpanCell::new("Circuit", 1, 1)],
            vec![
                SpanCell::new("101", 1, 1),
                SpanCell::new("40A", 1, 1),
                SpanCell::new("Blower", 1, 1),
            ],
        ];
        let grid = flatten_spanned_rows(&rows);
        assert_eq!(grid[0], vec!["Fuse", "", "Circuit"]);
        assert_eq!(grid[1], vec!["101", "40A", "Blower"]);
    }

    #[test]
    fn overflow_row_grows_grid_instead_of_clamping() {
        // num_rows under-reports the true row count (2 instead of 3); the
        // second cell's row index (2) is out of bounds for that declared size.
        let cells = vec![
            (0u32, 1u32, 1u32, "Header".to_string()),
            (2u32, 1u32, 1u32, "Orphan".to_string()),
        ];
        let grid = flatten_positioned_cells(1, cells.into_iter());
        assert_eq!(grid.len(), 3, "overflow row must grow the grid, not clamp into row 0");
        assert_eq!(grid[0], vec!["Header"]);
        assert_eq!(grid[1], vec![""]);
        assert_eq!(grid[2], vec!["Orphan"]);
    }

    #[test]
    fn no_spans_is_identity() {
        let rows = vec![
            vec![SpanCell::new("a", 1, 1), SpanCell::new("b", 1, 1)],
            vec![SpanCell::new("c", 1, 1), SpanCell::new("d", 1, 1)],
        ];
        let grid = flatten_spanned_rows(&rows);
        assert_eq!(grid, vec![vec!["a", "b"], vec!["c", "d"]]);
    }
}
