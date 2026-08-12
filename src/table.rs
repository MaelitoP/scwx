/// Renders rows as aligned columns separated by two spaces, with trailing
/// whitespace trimmed. Widths count chars, not bytes, so non-ASCII names
/// don't skew the columns.
pub(crate) fn columns<const N: usize>(rows: &[[String; N]]) -> Vec<String> {
    let mut widths = [0usize; N];
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }

    rows.iter()
        .map(|row| {
            let mut line = String::new();
            for (cell, width) in row.iter().zip(widths) {
                line.push_str(cell);
                for _ in cell.chars().count()..width + 2 {
                    line.push(' ');
                }
            }
            line.trim_end().to_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row2(a: &str, b: &str) -> [String; 2] {
        [a.to_owned(), b.to_owned()]
    }

    #[test]
    fn ragged_widths_align_and_trailing_cells_are_trimmed() {
        let lines = columns(&[row2("a", "x"), row2("longer", "y"), row2("b", "")]);
        assert_eq!(lines, ["a       x", "longer  y", "b"]);
    }

    #[test]
    fn widths_count_chars_not_bytes() {
        let lines = columns(&[row2("café", "x"), row2("name", "y")]);
        assert_eq!(lines, ["café  x", "name  y"]);
    }

    #[test]
    fn empty_input_renders_nothing() {
        assert!(columns::<2>(&[]).is_empty());
    }
}
