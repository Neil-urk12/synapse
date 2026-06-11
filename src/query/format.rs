pub fn format_ascii_table(headers: &[String], rows: &[Vec<String>]) -> String {
    if headers.is_empty() {
        return String::new();
    }
    let mut col_widths = vec![0; headers.len()];
    for (i, h) in headers.iter().enumerate() {
        col_widths[i] = h.len();
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_widths.len() && cell.len() > col_widths[i] {
                col_widths[i] = cell.len();
            }
        }
    }
    let mut separator = String::new();
    for w in &col_widths {
        separator.push('+');
        separator.push_str(&"-".repeat(*w + 2));
    }
    separator.push_str("+\n");

    let mut out = String::new();
    out.push_str(&separator);
    out.push('|');
    for (i, h) in headers.iter().enumerate() {
        out.push_str(&format!(" {:<width$} |", h, width = col_widths[i]));
    }
    out.push('\n');
    out.push_str(&separator);

    for row in rows {
        out.push('|');
        for (i, cell) in row.iter().take(headers.len()).enumerate() {
            out.push_str(&format!(" {:<width$} |", cell, width = col_widths[i]));
        }
        if row.len() < headers.len() {
            for &width in col_widths.iter().take(headers.len()).skip(row.len()) {
                out.push_str(&format!(" {:<width$} |", "", width = width));
            }
        }
        out.push('\n');
    }
    out.push_str(separator.trim_end());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_table_rendering() {
        let headers = vec!["ID".to_string(), "NAME".to_string(), "VAL".to_string()];
        let rows = vec![
            vec!["1".to_string(), "Alice".to_string(), "10.5".to_string()],
            vec!["20".to_string(), "Bob".to_string(), "true".to_string()],
        ];
        let formatted = format_ascii_table(&headers, &rows);
        let expected = "+----+-------+------+\n\
                        | ID | NAME  | VAL  |\n\
                        +----+-------+------+\n\
                        | 1  | Alice | 10.5 |\n\
                        | 20 | Bob   | true |\n\
                        +----+-------+------+";
        assert_eq!(formatted.trim(), expected);
    }
}
